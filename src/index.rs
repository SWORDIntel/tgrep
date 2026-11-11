use crate::build::default_state_dir;
use crate::store;

#[derive(Default, Debug)]
struct DiskUsage {
    active_trigram: u64,
    active_word: u64,
    legacy_hash: u64,
    other_segments: u64,
    auxiliary: u64,
    logical: u64,
    allocated: u64,
}

fn disk_usage(
    state_dir: &std::path::Path,
    manifest: Option<&store::Manifest>,
) -> std::io::Result<DiskUsage> {
    use std::collections::HashSet;
    use std::os::unix::fs::MetadataExt;
    let mut usage = DiskUsage::default();
    let mut active_tgs = HashSet::new();
    let mut active_qwi = HashSet::new();
    let mut active_thi = HashSet::new();
    if let Some(manifest) = manifest {
        for name in &manifest.segments {
            let path = state_dir.join("segments").join(name);
            active_qwi.insert(path.with_extension("qwi"));
            active_thi.insert(path.with_extension("thi"));
            active_tgs.insert(path);
        }
    }
    let mut directories = vec![state_dir.to_path_buf()];
    let mut allocated_inodes = HashSet::new();
    let add = |value: &mut u64, bytes: u64| -> std::io::Result<()> {
        *value = value
            .checked_add(bytes)
            .ok_or_else(|| std::io::Error::other("state size overflow"))?;
        Ok(())
    };
    while let Some(directory) = directories.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error)
                if directory == state_dir && error.kind() == std::io::ErrorKind::NotFound =>
            {
                continue
            }
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                directories.push(path);
            } else if metadata.is_file() {
                let bytes = metadata.len();
                add(&mut usage.logical, bytes)?;
                let category = if active_tgs.contains(&path) {
                    &mut usage.active_trigram
                } else if active_qwi.contains(&path) {
                    &mut usage.active_word
                } else if active_thi.contains(&path) {
                    &mut usage.legacy_hash
                } else if path.starts_with(state_dir.join("segments")) {
                    &mut usage.other_segments
                } else {
                    &mut usage.auxiliary
                };
                add(category, bytes)?;
                if allocated_inodes.insert((metadata.dev(), metadata.ino())) {
                    add(
                        &mut usage.allocated,
                        metadata.blocks().checked_mul(512).ok_or_else(|| {
                            std::io::Error::other("allocated state size overflow")
                        })?,
                    )?;
                }
            }
            // Symlinks and special files are not followed or opened.
        }
    }
    Ok(usage)
}

fn print_disk_usage(usage: &DiskUsage) {
    println!(
        "  disk usage:     {} bytes ({:.3} MB)",
        usage.logical,
        usage.logical as f64 / 1_000_000.0
    );
    println!(
        "  allocated:      {} bytes ({:.3} MB)",
        usage.allocated,
        usage.allocated as f64 / 1_000_000.0
    );
    println!("  regular files; symlinks excluded; hard-linked allocation counted once");
    for (label, bytes) in [
        ("active TGS", usage.active_trigram),
        ("active QWI", usage.active_word),
        ("active legacy THI", usage.legacy_hash),
        ("other segment files", usage.other_segments),
        ("metadata/cache/WAL", usage.auxiliary),
    ] {
        println!("    {label}: {bytes} bytes");
    }
}

pub fn run_index_command(cmd: IndexCommand) -> std::io::Result<()> {
    match cmd {
        IndexCommand::Build {
            roots,
            threads,
            max_memory,
            io_limit,
        } => {
            crate::build::start_build(&roots, threads, &max_memory, &io_limit);
        }
        IndexCommand::Status => {
            print_status(&default_state_dir())?;
        }
        IndexCommand::Recover => {
            let state_dir = default_state_dir();
            let recovered = crate::build::recover_committed_segments(&state_dir)?;
            if recovered.is_empty() {
                println!("tgrep: committed-segment scan found no committed WAL segments");
            } else {
                println!(
                    "tgrep: committed-segment scan retained {} segment(s):",
                    recovered.len()
                );
                for seg in &recovered {
                    println!("  {}", seg);
                }
            }
            println!("The index manifest is unchanged; run `tgrep index build <path>` to publish a new index.");
        }
        IndexCommand::Pause => {
            crate::build::pause()?;
        }
        IndexCommand::Resume => {
            crate::build::resume()?;
        }
        IndexCommand::Refresh => {
            crate::build::refresh()?;
        }
        IndexCommand::Compact => {
            crate::build::compact()?;
        }
        IndexCommand::Rebuild => {
            crate::build::rebuild()?;
        }
    }
    Ok(())
}

fn print_status(state_dir: &std::path::Path) -> std::io::Result<()> {
    // Legacy root IDs must use build metadata captured before the manifest read.
    // A builder binds old roots in the manifest before replacing build.json.
    let build_data = crate::build::read_build_metadata(state_dir);
    let manifest = match store::read_manifest(state_dir) {
        Ok(m) => m,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("tgrep: no index found at {}", state_dir.display());
            println!("  Run `tgrep index build <path>` to create one.");
            print_disk_usage(&disk_usage(state_dir, None)?);
            return Ok(());
        }
        Err(error) => return Err(error),
    };

    let build_state: Option<serde_json::Value> = match build_data {
        Ok(data) => Some(data),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };

    println!("tgrep index status");
    println!("  state dir:    {}", state_dir.display());
    println!("  generation:   {}", manifest.generation);
    println!("  segments:     {}", manifest.segments.len());
    match indexed_roots(&manifest, build_state.as_ref()) {
        Some(roots) => {
            println!(
                "  {}: {}",
                root_status_label(&manifest, &roots),
                roots.len()
            );
            for root in roots {
                println!("    {}", root);
            }
        }
        None => println!("  legacy root provenance: unavailable"),
    }

    // WAL status
    let wal_dir = state_dir.join("wal");
    match std::fs::read_dir(&wal_dir) {
        Ok(entries) => {
            let mut count = 0;
            for entry in entries {
                entry?;
                count += 1;
            }
            println!("  wal segments: {}", count);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    // Sum up segment stats
    let segments_dir = state_dir.join("segments");
    let mut total_docs = 0u64;
    let mut total_postings = 0u64;
    let mut segment_error = None;

    for seg_name in &manifest.segments {
        let seg_path = segments_dir.join(seg_name);
        match store::SegmentReader::open(&seg_path) {
            Ok(reader) => {
                total_docs += reader.doc_count() as u64;
                total_postings += reader.posting_count();
                let file_size = std::fs::metadata(&seg_path).map(|m| m.len()).unwrap_or(0);
                println!(
                    "    {} — {} docs, {} TGS bytes",
                    seg_name,
                    reader.doc_count(),
                    file_size,
                );
            }
            Err(e) => {
                println!("    {} — ERROR: {}", seg_name, e);
                segment_error = Some(e);
            }
        }
    }

    println!();
    println!("  total docs:     {}", total_docs);
    println!("  total postings: {}", total_postings);
    print_disk_usage(&disk_usage(state_dir, Some(&manifest))?);

    if let Some(bs) = build_state {
        println!();
        println!(
            "  build status:   {}",
            bs.get("status")
                .and_then(|value| value.as_str())
                .unwrap_or("?")
        );
        println!(
            "  build progress files: {}",
            bs.get("files_indexed")
                .and_then(|value| value.as_u64())
                .unwrap_or(0)
        );
        println!(
            "  build progress bytes: {} MB",
            bs.get("bytes_indexed")
                .and_then(|value| value.as_u64())
                .unwrap_or(0)
                / 1_000_000
        );
    }
    if let Some(error) = segment_error {
        return Err(error);
    }
    Ok(())
}

fn indexed_roots(
    manifest: &store::Manifest,
    build_state: Option<&serde_json::Value>,
) -> Option<Vec<String>> {
    if let Some(roots) = &manifest.roots {
        return Some(roots.clone());
    }
    build_state?
        .get("roots")?
        .as_array()?
        .iter()
        .map(|value| value.as_str().map(str::to_owned))
        .collect()
}

fn root_status_label(manifest: &store::Manifest, roots: &[String]) -> &'static str {
    if roots
        .iter()
        .any(|root| !std::path::Path::new(root).is_absolute())
    {
        "legacy root provenance (relative; original build directory unknown)"
    } else if manifest.roots.is_some() {
        "indexed roots"
    } else {
        "legacy root provenance (build metadata)"
    }
}

pub enum IndexCommand {
    Build {
        roots: Vec<String>,
        threads: usize,
        max_memory: String,
        io_limit: String,
    },
    Status,
    Recover,
    Pause,
    Resume,
    Refresh,
    Compact,
    Rebuild,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch() -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("tgrep-status-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn disk_usage_counts_sidecars_wal_cache_and_obsolete_segments() {
        let dir = scratch();
        std::fs::create_dir(dir.join("segments")).unwrap();
        std::fs::create_dir(dir.join("wal")).unwrap();
        std::fs::create_dir(dir.join("tmp")).unwrap();
        for (name, size) in [
            ("segments/seg_00000001.tgs", 3),
            ("segments/seg_00000001.qwi", 5),
            ("segments/seg_00000001.thi", 7),
            ("segments/seg_old.tgs", 11),
            ("wal/record.log", 13),
            ("file_list_cache.json", 17),
            ("tmp/staged", 19),
        ] {
            std::fs::write(dir.join(name), vec![b'x'; size]).unwrap();
        }
        let manifest: store::Manifest = serde_json::from_value(serde_json::json!({
            "generation":1,"segments":["seg_00000001.tgs"],"profile_hash":0
        }))
        .unwrap();
        let usage = disk_usage(&dir, Some(&manifest)).unwrap();
        assert_eq!(usage.active_trigram, 3);
        assert_eq!(usage.active_word, 5);
        assert_eq!(usage.legacy_hash, 7);
        assert_eq!(usage.other_segments, 11);
        assert_eq!(usage.auxiliary, 49);
        assert_eq!(usage.logical, 75);
        assert!(usage.allocated >= usage.logical);
        std::fs::hard_link(dir.join("segments/seg_00000001.tgs"), dir.join("hard-link")).unwrap();
        let linked = disk_usage(&dir, Some(&manifest)).unwrap();
        assert_eq!(linked.logical, usage.logical + 3);
        assert_eq!(linked.allocated, usage.allocated);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn disk_usage_does_not_follow_symlinked_files_or_directories() {
        let dir = scratch();
        let outside = scratch();
        std::fs::write(outside.join("large"), vec![0; 8192]).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("outside")).unwrap();
        std::os::unix::fs::symlink(outside.join("large"), dir.join("alias")).unwrap();
        let usage = disk_usage(&dir, None).unwrap();
        assert_eq!(usage.logical, 0);
        assert_eq!(usage.allocated, 0);
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn status_distinguishes_missing_corrupt_and_empty_manifest() {
        let dir = scratch();
        assert!(print_status(&dir).is_ok());
        std::fs::write(dir.join("manifest.json"), b"broken").unwrap();
        assert_eq!(
            print_status(&dir).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        std::fs::write(
            dir.join("manifest.json"),
            br#"{"generation":0,"segments":[],"profile_hash":0}"#,
        )
        .unwrap();
        assert!(print_status(&dir).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn status_reports_unreadable_segment() {
        let dir = scratch();
        std::fs::write(
            dir.join("manifest.json"),
            br#"{"generation":1,"segments":["missing.tgs"],"profile_hash":0}"#,
        )
        .unwrap();
        assert_eq!(
            print_status(&dir).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn status_reports_bad_wal_and_build_state() {
        let dir = scratch();
        std::fs::write(
            dir.join("manifest.json"),
            br#"{"generation":0,"segments":[],"profile_hash":0}"#,
        )
        .unwrap();
        std::fs::write(dir.join("wal"), b"not a directory").unwrap();
        assert!(print_status(&dir).is_err());
        std::fs::remove_file(dir.join("wal")).unwrap();
        std::fs::write(dir.join("build.json"), b"broken").unwrap();
        assert_eq!(
            print_status(&dir).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn status_prefers_bound_roots_and_preserves_legacy_fallback() {
        let state = serde_json::json!({"status":"running","roots":["/new-progress-root"]});
        let mut manifest: store::Manifest = serde_json::from_value(
            serde_json::json!({"generation":1,"segments":[],"profile_hash":0}),
        )
        .unwrap();
        assert_eq!(
            indexed_roots(&manifest, Some(&state)),
            Some(vec!["/new-progress-root".into()])
        );
        manifest.roots = Some(vec!["/old-indexed-root".into()]);
        assert_eq!(indexed_roots(&manifest, Some(&state)), manifest.roots);
        manifest.roots = Some(vec![]);
        assert_eq!(indexed_roots(&manifest, Some(&state)), Some(vec![]));
        manifest.roots = None;
        assert_eq!(indexed_roots(&manifest, None), None);
    }
    #[test]
    fn status_fifo_child() {
        let Some(path) = std::env::var_os("TGREP_STATUS_TEST_FIFO_DIR") else {
            return;
        };
        assert!(print_status(&PathBuf::from(path)).is_err());
    }

    #[test]
    fn status_build_fifo_is_rejected_without_writer() {
        use std::os::unix::ffi::OsStrExt;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let dir = scratch();
        let fifo = dir.join("build.json");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        std::fs::write(
            dir.join("manifest.json"),
            br#"{"generation":0,"segments":[],"profile_hash":0,"roots":["/bound-root"]}"#,
        )
        .unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "index::tests::status_fifo_child"])
            .env("TGREP_STATUS_TEST_FIFO_DIR", &dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                std::fs::remove_dir_all(&dir).unwrap();
                panic!("status blocked while opening build.json FIFO");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn status_relative_roots_remain_uncertified_provenance() {
        let mut manifest: store::Manifest = serde_json::from_value(
            serde_json::json!({"generation":1,"segments":[],"profile_hash":0}),
        )
        .unwrap();
        let relative = vec!["relative/../old-root/".to_string()];
        assert!(
            root_status_label(&manifest, &relative).contains("original build directory unknown")
        );
        manifest.roots = Some(relative.clone());
        assert!(
            root_status_label(&manifest, &relative).contains("original build directory unknown")
        );
        let absolute = vec!["/indexed-root".to_string()];
        manifest.roots = Some(absolute.clone());
        assert_eq!(root_status_label(&manifest, &absolute), "indexed roots");
        manifest.roots = None;
        assert_eq!(
            root_status_label(&manifest, &absolute),
            "legacy root provenance (build metadata)"
        );
    }
}
