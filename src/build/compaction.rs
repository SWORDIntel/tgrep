//! Merge immutable index snapshots without opening or restating source files.
//!
//! This preserves the indexed metadata exactly. A live query still walks the
//! roots and scans new/changed files, so compaction does not pretend to refresh
//! the index or upgrade legacy text-decoding provenance.

use super::{
    cleanup_unreferenced_segments, parse_resource_quantity, resolve_build_resources,
    wal_checkpoint, IoPacer, MemoryGuard, WriterLock,
};
use crate::native::{MmapWordIndex, Wal, WordIndexBuilder};
use crate::store::{self, DocRecord, Manifest, SegmentReader, SegmentWriter};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

struct Input {
    reader: SegmentReader,
    words: Option<MmapWordIndex>,
}

pub(super) fn compact_at(state: &Path, memory: &str, io_rate: &str) -> io::Result<()> {
    compact_with_publisher(state, memory, io_rate, store::write_manifest)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn compact_with_publisher(
    state: &Path,
    memory: &str,
    io_rate: &str,
    publish: impl FnOnce(&Path, &Manifest) -> io::Result<()>,
) -> io::Result<()> {
    compact_with_commit_and_publisher(
        state,
        memory,
        io_rate,
        |wal, transaction| wal.commit(transaction).map_err(io::Error::other),
        publish,
    )
}

fn compact_with_commit_and_publisher(
    state: &Path,
    memory: &str,
    io_rate: &str,
    commit: impl FnOnce(&Wal, u64) -> io::Result<()>,
    publish: impl FnOnce(&Path, &Manifest) -> io::Result<()>,
) -> io::Result<()> {
    let _lock = WriterLock::acquire(state)?;
    let manifest = store::read_manifest(state)?;
    let roots = manifest.roots.as_ref().ok_or_else(|| {
        invalid("compaction requires bound roots; rebuild the legacy index first")
    })?;
    if (!manifest.segments.is_empty() && roots.is_empty())
        || roots.iter().any(|root| !Path::new(root).is_absolute())
    {
        return Err(invalid(
            "compaction requires an unambiguous absolute root binding",
        ));
    }
    let segments_dir = state.join("segments");
    if manifest.segments.is_empty() {
        eprintln!("tgrep: compact: empty index; nothing to merge");
        return Ok(());
    }

    let (memory, io_rate) = resolve_build_resources(memory, io_rate, state)?;
    let guard =
        MemoryGuard::new(parse_resource_quantity(&memory, false).map_err(io::Error::other)?)?;
    let pacer = IoPacer::new(parse_resource_quantity(&io_rate, true).map_err(io::Error::other)?)?;
    let mut input_bytes = 0u64;
    let mut sidecars = Vec::with_capacity(manifest.segments.len());
    for name in &manifest.segments {
        let path = segments_dir.join(name);
        input_bytes = input_bytes.saturating_add(fs::metadata(&path)?.len());
        let words = path.with_extension("qwi");
        let present = match fs::symlink_metadata(&words) {
            Ok(metadata) => {
                input_bytes = input_bytes.saturating_add(metadata.len());
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        sidecars.push(present.then_some(words));
    }
    // The existing serializer materializes postings plus its complete output
    // image, and the native word builder uses a tree. Reserve conservatively
    // before constructing those copies; later resident checks are cooperative,
    // not an allocator-enforced hard limit. Refusal leaves the snapshot intact.
    let merge_reserve =
        input_bytes.saturating_mul(if manifest.segments.len() == 1 { 4 } else { 12 });
    guard.check(merge_reserve).map_err(|error| {
        io::Error::other(format!(
            "compaction cannot fit the snapshot in its memory budget: {error}"
        ))
    })?;

    let mut inputs = Vec::with_capacity(manifest.segments.len());
    let mut total_documents = 0u64;
    for (name, words) in manifest.segments.iter().zip(sidecars) {
        let reader = SegmentReader::open_for_search(&segments_dir.join(name))?;
        if !reader.supports_safe_pruning() {
            return Err(invalid(
                "compaction cannot upgrade legacy TGS provenance; rebuild first",
            ));
        }
        if format!("{}.tgs", reader.segment_name()) != *name {
            return Err(invalid("segment filename does not match its generation"));
        }
        if reader
            .iter_docs()
            .any(|doc| doc.root_id as usize >= roots.len())
        {
            return Err(invalid("segment document refers to an unbound root"));
        }
        total_documents = total_documents
            .checked_add(u64::from(reader.doc_count()))
            .ok_or_else(|| invalid("compacted document count overflow"))?;
        if total_documents > u64::from(u32::MAX) {
            return Err(invalid("compacted document count exceeds TGS capacity"));
        }
        let words = words
            .map(|path| {
                MmapWordIndex::load_for_segment(
                    &path,
                    reader.generation(),
                    reader.metadata_fingerprint(),
                )
                .map_err(|error| invalid(&format!("cannot compact {}: {error}", path.display())))
            })
            .transpose()?;
        inputs.push(Input { reader, words });
        guard.check(0)?;
        pacer.pace()?;
    }

    if inputs.len() == 1 {
        let input = &inputs[0];
        let mut lists = 0usize;
        input.reader.visit_postings(|_, _| {
            lists += 1;
            if lists.is_multiple_of(128) {
                guard.check(0)?;
            }
            Ok(())
        })?;
        if let Some(words) = &input.words {
            let ids: Vec<_> = input.reader.iter_docs().map(|doc| doc.local_id).collect();
            words.validate_documents(&ids, || guard.check(0).map(|_| ()))?;
        }
        eprintln!("tgrep: compact: one checked segment; nothing to merge");
        return Ok(());
    }
    let complete_words = inputs.iter().all(|input| input.words.is_some());
    let mut word_builder = WordIndexBuilder::create().map_err(io::Error::other)?;
    let mut writer = SegmentWriter::new(PathBuf::new(), 0);
    let mut postings: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for input in &inputs {
        let first_document = writer.doc_count();
        let mut old_ids = Vec::with_capacity(input.reader.doc_count() as usize);
        for doc in input.reader.iter_docs() {
            old_ids.push(doc.local_id);
            writer.add_doc_record(DocRecord {
                local_id: writer.doc_count(),
                root_id: doc.root_id,
                path: doc.path,
                byte_length: doc.byte_length,
                device: doc.device,
                inode: doc.inode,
                mtime_ns: doc.mtime_ns,
                ctime_ns: doc.ctime_ns,
            });
        }
        let mut lists = 0usize;
        input.reader.visit_postings(|gram, source_ids| {
            lists += 1;
            if lists.is_multiple_of(128) {
                guard.check((source_ids.len() as u64).saturating_mul(8))?;
            }
            let destination = postings.entry(gram).or_default();
            destination
                .try_reserve(source_ids.len())
                .map_err(io::Error::other)?;
            for source_id in source_ids {
                let position = old_ids.binary_search(source_id).map_err(|_| {
                    invalid("posting refers to an absent document during compaction")
                })?;
                // Each source's old IDs and every new range are strictly ordered.
                destination.push(first_document + position as u32);
            }
            Ok(())
        })?;
        if let Some(words) = &input.words {
            // Even when another segment has no sidecar, validate every present
            // one. Do not launder corrupt input into an apparently valid merge.
            word_builder.import_snapshot(words, &old_ids, first_document, || {
                guard.check(256 * 1024).map(|_| ())
            })?;
        }
        guard.check(0)?;
        pacer.pace()?;
    }
    writer.set_postings(postings);

    if !complete_words {
        eprintln!("tgrep: compact: an input word sidecar is missing; merged snapshot will use checked trigram/scan fallback for word queries");
    }
    // Allow image copies in addition to the currently resident in-memory merge.
    guard.check(input_bytes.saturating_mul(3))?;
    let mut last_generation = manifest.generation;
    for entry in fs::read_dir(&segments_dir)? {
        let name = entry?.file_name();
        if let Some(generation) = name
            .to_str()
            .and_then(|name| name.strip_prefix("seg_"))
            .and_then(|name| name.split('.').next())
            .and_then(|name| name.parse::<u64>().ok())
        {
            last_generation = last_generation.max(generation);
        }
    }
    let wal = Wal::create(&state.join("wal")).map_err(io::Error::other)?;
    let generation = last_generation
        .max(wal.current_lsn())
        .checked_add(1)
        .ok_or_else(|| io::Error::other("segment generation exhausted"))?;
    writer.generation = generation;
    let name = format!("seg_{generation:08}.tgs");
    let tgs = segments_dir.join(&name);
    let qwi = tgs.with_extension("qwi");
    if wal.begin(generation) == 0 {
        return Err(io::Error::other("compaction WAL begin failed"));
    }
    let staged = (|| -> io::Result<()> {
        let fingerprint = writer.flush(&segments_dir)?;
        if complete_words {
            word_builder
                .save_for_segment(&qwi, generation, fingerprint)
                .map_err(io::Error::other)?;
        }
        pacer.pace()?;
        if wal.log_segment(generation, &name, br#"{"operation":"compact"}"#) == 0 {
            return Err(io::Error::other("compaction WAL segment record failed"));
        }
        Ok(())
    })();
    if let Err(error) = staged {
        let _ = wal.abort(generation);
        // These new names have not been offered for manifest publication.
        for path in [&tgs, &qwi] {
            if let Err(cleanup) = fs::remove_file(path) {
                if cleanup.kind() != io::ErrorKind::NotFound {
                    eprintln!(
                        "tgrep: compact cleanup failed for {}: {cleanup}",
                        path.display()
                    );
                }
            }
        }
        return Err(error);
    }
    // A complete COMMIT may have reached storage even when its sync reports
    // failure. Once commit is attempted, retain both staged files so replay can
    // never find a committed segment name whose file we removed here.
    commit(&wal, generation).map_err(|error| {
        io::Error::new(error.kind(), format!(
            "compaction WAL commit failed; outcome uncertain, unpublished segment files retained: {error}"
        ))
    })?;
    let replacement = Manifest {
        generation,
        segments: vec![name],
        profile_hash: manifest.profile_hash,
        roots: manifest.roots,
    };
    publish(state, &replacement).map_err(|error| {
        io::Error::new(error.kind(), format!(
            "compaction manifest publication failed; replacement may already be visible, segment files retained: {error}"
        ))
    })?;
    // Readers already mapping an old segment retain that immutable inode after
    // unlink. Never delete either snapshot on an ambiguous publication error.
    wal_checkpoint(state);
    if let Err(error) = cleanup_unreferenced_segments(state) {
        eprintln!("tgrep: compaction already published; obsolete segment cleanup failed: {error}");
    }
    if let Err(error) = pacer.pace() {
        eprintln!("tgrep: compaction already published; I/O pacing failed: {error}");
    }
    eprintln!("tgrep: compact: merged {} segments into one; retained {total_documents} indexed document records without reading sources", inputs.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{ensure_state_dirs, tests::Scratch};
    use std::os::unix::fs::MetadataExt;

    const ROOT_GRAM: u32 = 0x616c70;

    struct Fixture {
        scratch: Scratch,
        state: PathBuf,
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let scratch = Scratch::new();
            let state = scratch.state();
            let root = scratch.0.join("corpus");
            ensure_state_dirs(&state).unwrap();
            fs::create_dir(&root).unwrap();
            for name in ["one.txt", "two.txt", "three.txt"] {
                fs::write(root.join(name), b"alpha beta\n").unwrap();
            }
            for (generation, documents) in [
                (11, vec![(2, "one.txt", "alpha"), (9, "two.txt", "beta")]),
                (18, vec![(7, "three.txt", "alpha")]),
            ] {
                let mut writer = SegmentWriter::new(PathBuf::new(), generation);
                let mut words = WordIndexBuilder::create().unwrap();
                for (id, name, token) in documents {
                    let metadata = fs::metadata(root.join(name)).unwrap();
                    writer.add_document(
                        DocRecord {
                            local_id: id,
                            root_id: 0,
                            path: name.as_bytes().to_vec(),
                            byte_length: metadata.len(),
                            device: metadata.dev(),
                            inode: metadata.ino(),
                            mtime_ns: metadata.mtime() * 1_000_000_000 + metadata.mtime_nsec(),
                            ctime_ns: metadata.ctime() * 1_000_000_000 + metadata.ctime_nsec(),
                        },
                        &[ROOT_GRAM, if token == "alpha" { 10 } else { 20 }],
                    );
                    words.add(token.as_bytes(), id).unwrap();
                }
                let fingerprint = writer.flush(&state.join("segments")).unwrap();
                words
                    .save_for_segment(
                        &state.join(format!("segments/seg_{generation:08}.qwi")),
                        generation,
                        fingerprint,
                    )
                    .unwrap();
            }
            store::publish_manifest_with_roots(
                &state,
                &["seg_00000011.tgs".into(), "seg_00000018.tgs".into()],
                18,
                &[root.to_str().unwrap().into()],
            )
            .unwrap();
            fs::write(
                state.join("build.json"),
                br#"{"status":"completed","build_id":"original"}"#,
            )
            .unwrap();
            Self {
                scratch,
                state,
                root,
            }
        }

        fn snapshot(&self) -> (Vec<u8>, BTreeMap<String, Vec<u8>>) {
            let manifest = fs::read(self.state.join("manifest.json")).unwrap();
            let parsed: Manifest = serde_json::from_slice(&manifest).unwrap();
            let mut files = BTreeMap::new();
            for name in parsed.segments {
                for extension in ["tgs", "qwi"] {
                    let path = self
                        .state
                        .join("segments")
                        .join(&name)
                        .with_extension(extension);
                    if path.exists() {
                        files.insert(
                            path.file_name().unwrap().to_str().unwrap().to_owned(),
                            fs::read(path).unwrap(),
                        );
                    }
                }
            }
            (manifest, files)
        }

        fn compact(&self) -> io::Result<()> {
            compact_at(&self.state, "512MiB", "1GiB/s")
        }
    }

    #[test]
    fn merges_sparse_ids_and_words_without_accessing_sources() {
        let fixture = Fixture::new();
        let old = store::read_manifest(&fixture.state).unwrap();
        let old_reader =
            SegmentReader::open(&fixture.state.join("segments").join(&old.segments[0])).unwrap();
        let old_metadata = old_reader.get_doc(2).unwrap();
        let build_metadata = fs::read(fixture.state.join("build.json")).unwrap();
        fs::remove_dir_all(&fixture.root).unwrap();
        fixture.compact().unwrap();
        let new = store::read_manifest(&fixture.state).unwrap();
        assert_eq!(new.roots, old.roots);
        assert_eq!(new.segments.len(), 1);
        assert!(new.generation > old.generation);
        let path = fixture.state.join("segments").join(&new.segments[0]);
        let reader = SegmentReader::open(&path).unwrap();
        assert_eq!(reader.doc_count(), 3);
        assert_eq!(reader.try_read_postings(ROOT_GRAM).unwrap(), [0, 1, 2]);
        assert_eq!(reader.try_read_postings(10).unwrap(), [0, 2]);
        assert_eq!(reader.try_read_postings(20).unwrap(), [1]);
        let document = reader.get_doc(0).unwrap();
        assert_eq!(document.path, old_metadata.path);
        assert_eq!(
            (
                document.root_id,
                document.byte_length,
                document.device,
                document.inode,
                document.mtime_ns,
                document.ctime_ns
            ),
            (
                old_metadata.root_id,
                old_metadata.byte_length,
                old_metadata.device,
                old_metadata.inode,
                old_metadata.mtime_ns,
                old_metadata.ctime_ns
            )
        );
        let words = MmapWordIndex::load_for_segment(
            &path.with_extension("qwi"),
            reader.generation(),
            reader.metadata_fingerprint(),
        )
        .unwrap();
        assert_eq!(words.search(b"alpha", 4).unwrap(), [0, 2]);
        assert_eq!(words.search(b"beta", 4).unwrap(), [1]);
        assert!(MmapWordIndex::load_for_segment(
            &path.with_extension("qwi"),
            old_reader.generation(),
            old_reader.metadata_fingerprint()
        )
        .is_err());
        for name in old.segments {
            assert!(!fixture.state.join("segments").join(name).exists());
        }
        assert_eq!(old_reader.get_doc(2).unwrap().path, b"one.txt");
        assert_eq!(
            fs::read(fixture.state.join("build.json")).unwrap(),
            build_metadata
        );
        assert!(!fixture.root.exists());
        assert!(fixture.scratch.0.exists());
    }

    #[test]
    fn missing_word_sidecar_omits_merged_sidecar_without_losing_trigrams() {
        let fixture = Fixture::new();
        fs::remove_file(fixture.state.join("segments/seg_00000011.qwi")).unwrap();
        fixture.compact().unwrap();
        let manifest = store::read_manifest(&fixture.state).unwrap();
        let path = fixture.state.join("segments").join(&manifest.segments[0]);
        assert!(!path.with_extension("qwi").exists());
        assert_eq!(
            SegmentReader::open(&path)
                .unwrap()
                .try_read_postings(ROOT_GRAM)
                .unwrap(),
            [0, 1, 2]
        );
    }

    #[test]
    fn corrupt_posting_or_word_payload_preserves_published_snapshot() {
        for extension in ["tgs", "qwi"] {
            let fixture = Fixture::new();
            let path = fixture
                .state
                .join(format!("segments/seg_00000018.{extension}"));
            let mut contents = fs::read(&path).unwrap();
            let offset = if extension == "tgs" {
                u64::from_le_bytes(contents[50..58].try_into().unwrap()) as usize + 12
            } else {
                contents.len() - 1
            };
            contents[offset] ^= 1;
            fs::write(&path, contents).unwrap();
            let before = fixture.snapshot();
            assert_eq!(
                fixture.compact().unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert_eq!(fixture.snapshot(), before);
            assert_eq!(fs::read_dir(fixture.state.join("wal")).unwrap().count(), 0);
        }
    }

    #[test]
    fn missing_sidecar_does_not_hide_corruption_in_another_sidecar() {
        let fixture = Fixture::new();
        fs::remove_file(fixture.state.join("segments/seg_00000011.qwi")).unwrap();
        let path = fixture.state.join("segments/seg_00000018.qwi");
        let mut contents = fs::read(&path).unwrap();
        let last = contents.len() - 1;
        contents[last] ^= 1;
        fs::write(path, contents).unwrap();
        let before = fixture.snapshot();
        assert!(fixture.compact().is_err());
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn budget_and_writer_lock_failures_leave_snapshot_untouched() {
        let fixture = Fixture::new();
        let before = fixture.snapshot();
        assert!(compact_at(&fixture.state, "1B", "1GiB/s")
            .unwrap_err()
            .to_string()
            .contains("memory budget"));
        assert_eq!(fixture.snapshot(), before);
        let _lock = WriterLock::acquire(&fixture.state).unwrap();
        assert_eq!(
            fixture.compact().unwrap_err().kind(),
            io::ErrorKind::ResourceBusy
        );
        assert_eq!(fixture.snapshot(), before);
        assert_eq!(fs::read_dir(fixture.state.join("wal")).unwrap().count(), 0);
    }

    #[test]
    fn ambiguous_root_binding_is_not_upgraded() {
        let fixture = Fixture::new();
        let mut manifest = store::read_manifest(&fixture.state).unwrap();
        manifest.roots = None;
        store::write_manifest(&fixture.state, &manifest).unwrap();
        let before = fixture.snapshot();
        assert!(fixture
            .compact()
            .unwrap_err()
            .to_string()
            .contains("bound roots"));
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn legacy_provenance_requires_rebuild() {
        let fixture = Fixture::new();
        let mut writer = SegmentWriter::new(PathBuf::new(), 11);
        writer.add_doc_record(DocRecord {
            local_id: 0,
            root_id: 0,
            path: b"legacy.txt".to_vec(),
            byte_length: 1,
            device: 1,
            inode: 1,
            mtime_ns: 1,
            ctime_ns: 1,
        });
        writer.flush(&fixture.state.join("segments")).unwrap();
        let path = fixture.state.join("segments/seg_00000011.tgs");
        let mut bytes = fs::read(&path).unwrap();
        bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
        let footer = u64::from_le_bytes(bytes[66..74].try_into().unwrap()) as usize;
        let checksum = bytes[..footer].chunks(8).fold(0u64, |checksum, chunk| {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            checksum ^ u64::from_le_bytes(word)
        });
        bytes[footer..footer + 8].copy_from_slice(&checksum.to_le_bytes());
        fs::write(&path, bytes).unwrap();
        assert!(!SegmentReader::open(&path).unwrap().supports_safe_pruning());
        let before = fixture.snapshot();
        assert!(fixture
            .compact()
            .unwrap_err()
            .to_string()
            .contains("legacy TGS provenance"));
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn publication_failure_preserves_old_files_and_ambiguous_success_retains_new_files() {
        for publish_before_error in [false, true] {
            let fixture = Fixture::new();
            let before = fixture.snapshot();
            let error =
                compact_with_publisher(&fixture.state, "512MiB", "1GiB/s", |state, manifest| {
                    if publish_before_error {
                        store::write_manifest(state, manifest)?;
                    }
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected manifest publication failure",
                    ))
                })
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert!(error.to_string().contains("may already be visible"));
            if !publish_before_error {
                assert_eq!(fixture.snapshot(), before);
            } else {
                let manifest = store::read_manifest(&fixture.state).unwrap();
                assert_eq!(manifest.segments.len(), 1);
                let path = fixture.state.join("segments").join(&manifest.segments[0]);
                let reader = SegmentReader::open(&path).unwrap();
                assert_eq!(reader.doc_count(), 3);
                assert!(MmapWordIndex::load_for_segment(
                    &path.with_extension("qwi"),
                    reader.generation(),
                    reader.metadata_fingerprint()
                )
                .is_ok());
            }
            for (name, contents) in before.1 {
                assert_eq!(
                    fs::read(fixture.state.join("segments").join(name)).unwrap(),
                    contents
                );
            }
        }
    }

    #[test]
    fn uncertain_commit_retains_staged_files_and_old_manifest() {
        for commit_before_error in [false, true] {
            let fixture = Fixture::new();
            let before = fixture.snapshot();
            let error = compact_with_commit_and_publisher(
                &fixture.state,
                "512MiB",
                "1GiB/s",
                |wal, transaction| {
                    if commit_before_error {
                        wal.commit(transaction).map_err(io::Error::other)?;
                    }
                    Err(io::Error::other("injected commit sync failure"))
                },
                |_, _| panic!("failed commit must not publish a manifest"),
            )
            .unwrap_err();
            assert!(error.to_string().contains("outcome uncertain"));
            assert_eq!(fixture.snapshot(), before);
            let old_names: std::collections::HashSet<_> = before.1.keys().cloned().collect();
            let staged: Vec<_> = fs::read_dir(fixture.state.join("segments"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| !old_names.contains(path.file_name().unwrap().to_str().unwrap()))
                .collect();
            assert_eq!(staged.len(), 2);
            let segment = staged
                .iter()
                .find(|path| path.extension().unwrap() == "tgs")
                .unwrap();
            let reader = SegmentReader::open(segment).unwrap();
            assert!(MmapWordIndex::load_for_segment(
                &segment.with_extension("qwi"),
                reader.generation(),
                reader.metadata_fingerprint()
            )
            .is_ok());
            let replay = Wal::create(&fixture.state.join("wal"))
                .unwrap()
                .replay()
                .unwrap();
            assert_eq!(replay.len(), usize::from(commit_before_error));
            if commit_before_error {
                assert_eq!(
                    replay[0].key,
                    segment.file_name().unwrap().to_str().unwrap().as_bytes()
                );
            }
        }
    }
}
