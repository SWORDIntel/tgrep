use std::collections::BTreeMap;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use crate::native;

// ── Constants ───────────────────────────────────────────────────────

pub const SEGMENT_MAGIC: u32 = 0x54475331; // "TGS1"
pub const FORMAT_VERSION: u16 = 1;
pub const FOOTER_MAGIC: u32 = 0x5447464E; // "TGFN" (tgrep footer)
pub const POSTING_BLOCK_SIZE: usize = 128;

/// Segments are immutable once committed; verify each file identity once per
/// process instead of checksumming on every open (a search opens hundreds).
type SegmentIdentity = (u64, u64, u64, i64, i64, i64, i64);
static CHECKSUM_VERIFIED: LazyLock<Mutex<std::collections::HashSet<SegmentIdentity>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

// ── Structs ─────────────────────────────────────────────────────────

pub struct SegmentHeader {
    pub magic: u32,
    pub version: u16,
    pub generation: u64,
    pub profile_hash: u64,
    pub doc_count: u32,
    // Section offsets (from start of file)
    pub file_table_offset: u64,
    pub path_index_offset: u64,
    pub dictionary_offset: u64,
    pub postings_offset: u64,
    pub block_index_offset: u64,
    pub footer_offset: u64,
}

pub struct SegmentWriter {
    pub path: PathBuf,
    pub generation: u64,
    pub docs: Vec<DocRecord>,
    /// gram -> sorted doc IDs (built during add_document)
    postings: BTreeMap<u32, Vec<u32>>,
    /// grams per doc (for dedup)
    doc_grams: Vec<Vec<u32>>,
}

#[derive(Clone)]
pub struct DocRecord {
    pub local_id: u32,
    pub root_id: u32,
    pub path: Vec<u8>,
    pub byte_length: u64,
    pub device: u64,
    pub inode: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
}

impl SegmentWriter {
    pub fn new(path: PathBuf, generation: u64) -> Self {
        Self {
            path,
            generation,
            docs: Vec::new(),
            postings: BTreeMap::new(),
            doc_grams: Vec::new(),
        }
    }

    /// Add a document with its trigrams. Trigrams should be deduplicated
    /// per-document (KEYSTONE handles this internally).
    pub fn add_document(&mut self, doc: DocRecord, grams: &[u32]) {
        let id = doc.local_id;
        self.docs.push(doc);
        // Use a HashSet for O(1) dedup instead of Vec::contains (O(n²))
        let mut seen = std::collections::HashSet::with_capacity(grams.len());
        let mut deduped: Vec<u32> = Vec::with_capacity(grams.len());
        for &g in grams {
            if seen.insert(g) {
                deduped.push(g);
            }
        }
        for &g in &deduped {
            self.postings.entry(g).or_default().push(id);
        }
        self.doc_grams.push(deduped);
    }

    /// Add a doc record without trigrams (for use with set_postings).
    pub fn add_doc_record(&mut self, doc: DocRecord) {
        self.docs.push(doc);
    }

    /// Set postings directly from KEYSTONE's posting visitor output.
    /// Each (gram, doc_ids) pair is inserted into the BTreeMap.
    /// doc_ids must already be sorted (KEYSTONE guarantees this).
    pub fn set_postings(&mut self, postings: BTreeMap<u32, Vec<u32>>) {
        self.postings = postings;
    }

    pub fn doc_count(&self) -> u32 {
        self.docs.len() as u32
    }

    pub fn unique_trigrams(&self) -> usize {
        self.postings.len()
    }

    pub fn total_postings(&self) -> usize {
        self.postings.values().map(|v| v.len()).sum()
    }

    /// Serialize the segment to disk using the crash-safe atomic write.
    /// Format (all little-endian):
    ///   [Header]      fixed-size, contains section offsets
    ///   [File table]  per doc: id, root_id, path_len, path, byte_len, dev, ino, mtime, ctime
    ///   [Path index]  sorted (root_id, path) -> file_table_index, binary searchable
    ///   [Dictionary]  per gram: gram(4) + doc_freq(4) + posting_offset(8)
    ///   [Postings]     per gram: blocks of POSTING_BLOCK_SIZE, delta-varint encoded
    ///   [Block index]  per block: max_doc_id(4) + encoded_len(4) + checksum(4)
    ///   [Footer]       metadata_checksum(8) + footer_magic(4) + doc_count(4)
    pub fn flush(&self, segments_dir: &Path) -> io::Result<()> {
        let segment_name = format!("seg_{:08}.tgs", self.generation);
        let target_path = segments_dir.join(&segment_name);

        // ── Compute section sizes ──────────────────────────────────
        let header_size = 4 + 2 + 8 + 8 + 4 + 8 * 6; // magic(4), version(2), gen(8), hash(8), doc_count(4), 6 offsets(48) = 74

        // File table
        let file_table_size: u64 = self
            .docs
            .iter()
            .map(|d| {
                4 + 4 + 4 + d.path.len() as u64 + 8 + 8 + 8 + 8 + 8 // id, root_id, path_len, path, byte_len, dev, ino, mtime, ctime
            })
            .sum();

        // Path index: sorted by (root_id, path), each entry: root_id(4) + path_len(4) + path(var) + file_index(4)
        let mut path_index_entries: Vec<(u32, &[u8], u32)> = self
            .docs
            .iter()
            .map(|d| (d.root_id, d.path.as_slice(), d.local_id))
            .collect();
        path_index_entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
        let path_index_size: u64 = path_index_entries
            .iter()
            .map(|(_rid, path, _)| 4 + 4 + path.len() as u64 + 4)
            .sum();

        // Dictionary: gram(4) + doc_freq(4) + posting_offset(8) per entry
        let dict_size: u64 = (self.postings.len() as u64) * (4 + 4 + 8);

        // Postings + block index: compute after encoding
        // We'll encode postings and block index together
        let (postings_data, block_index_data, posting_offsets) = self.encode_postings();

        // ── Compute offsets ────────────────────────────────────────
        let file_table_offset = header_size as u64;
        let path_index_offset = file_table_offset + file_table_size;
        let dictionary_offset = path_index_offset + path_index_size;
        let postings_offset = dictionary_offset + dict_size;
        let block_index_offset = postings_offset + postings_data.len() as u64;
        let footer_offset = block_index_offset + block_index_data.len() as u64;

        // Reserve the complete image once; the footer is 8 + 4 + 4 bytes.
        let image_size = footer_offset
            .checked_add(16)
            .and_then(|size| usize::try_from(size).ok())
            .ok_or_else(|| io::Error::other("segment image size overflow"))?;
        let mut buf = Vec::new();
        buf.try_reserve_exact(image_size)
            .map_err(io::Error::other)?;

        // ── Write header ───────────────────────────────────────────
        buf.extend_from_slice(&SEGMENT_MAGIC.to_le_bytes());
        buf.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        buf.extend_from_slice(&self.generation.to_le_bytes());
        buf.extend_from_slice(&0u64.to_le_bytes()); // profile_hash (TODO: compute)
        buf.extend_from_slice(&(self.docs.len() as u32).to_le_bytes());
        buf.extend_from_slice(&file_table_offset.to_le_bytes());
        buf.extend_from_slice(&path_index_offset.to_le_bytes());
        buf.extend_from_slice(&dictionary_offset.to_le_bytes());
        buf.extend_from_slice(&postings_offset.to_le_bytes());
        buf.extend_from_slice(&block_index_offset.to_le_bytes());
        buf.extend_from_slice(&footer_offset.to_le_bytes());

        // ── Write file table ───────────────────────────────────────
        for doc in &self.docs {
            buf.extend_from_slice(&doc.local_id.to_le_bytes());
            buf.extend_from_slice(&doc.root_id.to_le_bytes());
            buf.extend_from_slice(&(doc.path.len() as u32).to_le_bytes());
            buf.extend_from_slice(&doc.path);
            buf.extend_from_slice(&doc.byte_length.to_le_bytes());
            buf.extend_from_slice(&doc.device.to_le_bytes());
            buf.extend_from_slice(&doc.inode.to_le_bytes());
            buf.extend_from_slice(&doc.mtime_ns.to_le_bytes());
            buf.extend_from_slice(&doc.ctime_ns.to_le_bytes());
        }

        // ── Write path index ────────────────────────────────────────
        for (root_id, path, file_index) in &path_index_entries {
            buf.extend_from_slice(&root_id.to_le_bytes());
            buf.extend_from_slice(&(path.len() as u32).to_le_bytes());
            buf.extend_from_slice(path);
            buf.extend_from_slice(&file_index.to_le_bytes());
        }

        // ── Write dictionary ───────────────────────────────────────
        for ((gram, doc_ids), start_offset) in self.postings.iter().zip(posting_offsets) {
            buf.extend_from_slice(&gram.to_le_bytes());
            buf.extend_from_slice(&(doc_ids.len() as u32).to_le_bytes());
            buf.extend_from_slice(&start_offset.to_le_bytes());
        }

        // ── Write postings ────────────────────────────────────────
        buf.extend_from_slice(&postings_data);

        // ── Write block index ──────────────────────────────────────
        buf.extend_from_slice(&block_index_data);

        // ── Write footer ───────────────────────────────────────────
        // Simple checksum: XOR of all 8-byte chunks (not cryptographic, just integrity)
        let checksum = simple_checksum(&buf);
        buf.extend_from_slice(&checksum.to_le_bytes());
        buf.extend_from_slice(&FOOTER_MAGIC.to_le_bytes());
        buf.extend_from_slice(&(self.docs.len() as u32).to_le_bytes());
        if buf.len() != image_size {
            return Err(io::Error::other("segment image size mismatch"));
        }

        // ── Atomic write to disk ──────────────────────────────────
        native::atomic_write(&target_path, &buf)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        Ok(())
    }

    /// Encode all postings across all grams and their dictionary offsets.
    fn encode_postings(&self) -> (Vec<u8>, Vec<u8>, Vec<u64>) {
        let mut postings_buf = Vec::new();
        let mut block_index_buf = Vec::new();
        let mut posting_offsets = Vec::with_capacity(self.postings.len());

        for (_, doc_ids) in &self.postings {
            posting_offsets.push(postings_buf.len() as u64);
            let gram_data = self.encode_gram_postings(doc_ids);
            // Extract block headers from gram_data and append to block index
            // Block format: max_doc_id(4) + encoded_len(4) + checksum(4) + deltas
            let mut offset = 0;
            let block_data = &gram_data;
            while offset < block_data.len() {
                if offset + 12 > block_data.len() {
                    break;
                }
                let max_id = u32::from_le_bytes(block_data[offset..offset + 4].try_into().unwrap());
                let enc_len =
                    u32::from_le_bytes(block_data[offset + 4..offset + 8].try_into().unwrap());
                let block_checksum =
                    u32::from_le_bytes(block_data[offset + 8..offset + 12].try_into().unwrap());

                block_index_buf.extend_from_slice(&max_id.to_le_bytes());
                block_index_buf.extend_from_slice(&enc_len.to_le_bytes());
                block_index_buf.extend_from_slice(&block_checksum.to_le_bytes());

                offset += 12 + enc_len as usize;
            }
            postings_buf.extend_from_slice(block_data);
        }

        (postings_buf, block_index_buf, posting_offsets)
    }

    /// Encode a single gram's posting list into blocks.
    /// Format per block: max_doc_id(4) + encoded_len(4) + checksum(4) + varint deltas
    fn encode_gram_postings(&self, doc_ids: &[u32]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut i = 0;
        while i < doc_ids.len() {
            let block_end = (i + POSTING_BLOCK_SIZE).min(doc_ids.len());
            let block = &doc_ids[i..block_end];
            let max_id = block[block.len() - 1];

            // Delta-encode: first value absolute, rest as deltas
            let mut deltas: Vec<u32> = Vec::with_capacity(block.len());
            deltas.push(block[0]);
            for j in 1..block.len() {
                deltas.push(block[j] - block[j - 1]);
            }

            // Varint encode the deltas
            let mut encoded: Vec<u8> = Vec::new();
            for &d in &deltas {
                encode_varint_u32(d, &mut encoded);
            }

            // Checksum: simple XOR of delta values
            let checksum: u32 = deltas.iter().fold(0u32, |acc, &d| acc ^ d);

            // Block header
            buf.extend_from_slice(&max_id.to_le_bytes());
            buf.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
            buf.extend_from_slice(&checksum.to_le_bytes());
            buf.extend_from_slice(&encoded);

            i = block_end;
        }
        buf
    }
}

// ── SegmentReader ───────────────────────────────────────────────────

pub struct SegmentReader {
    data: memmap2::Mmap,
    header: ParsedHeader,
}

struct ParsedHeader {
    generation: u64,
    _profile_hash: u64,
    doc_count: u32,
    file_table_offset: u64,
    path_index_offset: u64,
    dictionary_offset: u64,
    postings_offset: u64,
    block_index_offset: u64,
    footer_offset: u64,
}

#[derive(Clone)]
pub struct ParsedDocRecord {
    pub local_id: u32,
    pub root_id: u32,
    pub path: Vec<u8>,
    pub byte_length: u64,
    pub device: u64,
    pub inode: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
}

impl SegmentReader {
    /// Map an immutable segment. Publishers must replace files atomically; in-place
    /// writes/truncation while a reader exists violate the mapping lifetime contract.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        let before = file.metadata()?;
        if !before.file_type().is_file() {
            return Err(invalid("segment is not a regular file"));
        }
        let data = unsafe { memmap2::Mmap::map(&file)? };
        let header = Self::parse_header(&data)?;
        let len = data.len();
        let offsets = [
            header.file_table_offset,
            header.path_index_offset,
            header.dictionary_offset,
            header.postings_offset,
            header.block_index_offset,
            header.footer_offset,
        ];
        let offsets: Vec<usize> = offsets
            .iter()
            .map(|&off| {
                usize::try_from(off)
                    .ok()
                    .filter(|&off| off <= len)
                    .ok_or_else(|| invalid("section offset"))
            })
            .collect::<io::Result<_>>()?;
        if offsets[0] != 74 || offsets.windows(2).any(|pair| pair[0] > pair[1]) {
            return Err(invalid("section order"));
        }
        let footer = offsets[5];
        if footer.checked_add(16) != Some(len)
            || (offsets[3] - offsets[2]) % 16 != 0
            || (offsets[5] - offsets[4]) % 12 != 0
            || header.doc_count as usize > (offsets[1] - offsets[0]) / 52
        {
            return Err(invalid("section size"));
        }
        let footer_magic = u32::from_le_bytes(data[footer + 8..footer + 12].try_into().unwrap());
        let footer_docs = u32::from_le_bytes(data[footer + 12..footer + 16].try_into().unwrap());
        if footer_magic != FOOTER_MAGIC || footer_docs != header.doc_count {
            return Err(invalid("footer"));
        }
        let stored = u64::from_le_bytes(data[footer..footer + 8].try_into().unwrap());
        let reader = Self { data, header };
        #[cfg(unix)]
        let key = segment_identity(&before);
        #[cfg(unix)]
        let verified = CHECKSUM_VERIFIED.lock().unwrap().contains(&key);
        #[cfg(not(unix))]
        let verified = false;
        if !verified {
            if simple_checksum(&reader.data[..footer]) != stored {
                return Err(invalid("segment checksum mismatch"));
            }
            reader.validate_tables()?;
        }
        #[cfg(unix)]
        {
            if segment_identity(&file.metadata()?) != key {
                return Err(invalid("segment changed while opening"));
            }
            CHECKSUM_VERIFIED.lock().unwrap().insert(key);
        }
        Ok(reader)
    }

    fn validate_tables(&self) -> io::Result<()> {
        let mut records = std::collections::HashMap::new();
        let mut record_offset = self.header.file_table_offset as usize;
        let mut previous_id = None;
        for doc in self.iter_docs() {
            if previous_id.is_some_and(|previous| doc.local_id <= previous) {
                return Err(invalid("document order"));
            }
            previous_id = Some(doc.local_id);
            let path_start = record_offset + 12;
            records.insert(doc.local_id, (doc.root_id, path_start, doc.path.len()));
            record_offset = path_start + doc.path.len() + 40;
        }
        if records.len() != self.header.doc_count as usize
            || record_offset != self.header.path_index_offset as usize
        {
            return Err(invalid("document table"));
        }
        let mut previous_path: Option<(u32, &[u8])> = None;
        let mut indexed = std::collections::HashSet::new();
        let mut offset = self.header.path_index_offset as usize;
        let end = self.header.dictionary_offset as usize;
        for _ in 0..self.header.doc_count {
            let header_end = offset
                .checked_add(8)
                .filter(|&value| value <= end)
                .ok_or_else(|| invalid("path index header"))?;
            let root = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
            let length =
                u32::from_le_bytes(self.data[offset + 4..header_end].try_into().unwrap()) as usize;
            let path_end = header_end
                .checked_add(length)
                .ok_or_else(|| invalid("path index length"))?;
            let record_end = path_end
                .checked_add(4)
                .filter(|&value| value <= end)
                .ok_or_else(|| invalid("path index bounds"))?;
            let path = &self.data[header_end..path_end];
            let id = u32::from_le_bytes(self.data[path_end..record_end].try_into().unwrap());
            let &(doc_root, doc_path, doc_length) = records
                .get(&id)
                .ok_or_else(|| invalid("path index document"))?;
            if doc_root != root
                || &self.data[doc_path..doc_path + doc_length] != path
                || !indexed.insert(id)
                || previous_path.is_some_and(|previous| previous > (root, path))
            {
                return Err(invalid("path index order or document"));
            }
            previous_path = Some((root, path));
            offset = record_end;
        }
        if offset != end {
            return Err(invalid("path index length"));
        }
        let start = self.header.dictionary_offset as usize;
        let end = self.header.postings_offset as usize;
        let mut previous_gram = None;
        let mut next_posting = 0u64;
        for offset in (start..end).step_by(16) {
            let gram = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
            let frequency =
                u32::from_le_bytes(self.data[offset + 4..offset + 8].try_into().unwrap());
            let posting =
                u64::from_le_bytes(self.data[offset + 8..offset + 16].try_into().unwrap());
            if previous_gram.is_some_and(|previous| gram <= previous) || posting != next_posting {
                return Err(invalid("dictionary order or posting offset"));
            }
            let (docs, next) = self.decode_postings_checked(posting, frequency)?;
            if docs.iter().any(|id| !records.contains_key(id)) {
                return Err(invalid("posting document"));
            }
            previous_gram = Some(gram);
            next_posting = next;
        }
        if next_posting != self.header.block_index_offset - self.header.postings_offset {
            return Err(invalid("posting section length"));
        }
        Ok(())
    }

    /// Return the segment file name (e.g. "seg_00000001.tgs") based on generation.
    pub fn segment_name(&self) -> String {
        format!("seg_{:08}", self.header.generation)
    }

    fn parse_header(data: &[u8]) -> io::Result<ParsedHeader> {
        if data.len() < 74 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "segment too short for header",
            ));
        }
        let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
        if magic != SEGMENT_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid segment magic",
            ));
        }
        let version = u16::from_le_bytes(data[4..6].try_into().unwrap());
        if version != FORMAT_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported version {}", version),
            ));
        }
        Ok(ParsedHeader {
            generation: u64::from_le_bytes(data[6..14].try_into().unwrap()),
            _profile_hash: u64::from_le_bytes(data[14..22].try_into().unwrap()),
            doc_count: u32::from_le_bytes(data[22..26].try_into().unwrap()),
            file_table_offset: u64::from_le_bytes(data[26..34].try_into().unwrap()),
            path_index_offset: u64::from_le_bytes(data[34..42].try_into().unwrap()),
            dictionary_offset: u64::from_le_bytes(data[42..50].try_into().unwrap()),
            postings_offset: u64::from_le_bytes(data[50..58].try_into().unwrap()),
            block_index_offset: u64::from_le_bytes(data[58..66].try_into().unwrap()),
            footer_offset: u64::from_le_bytes(data[66..74].try_into().unwrap()),
        })
    }

    pub fn generation(&self) -> u64 {
        self.header.generation
    }
    pub fn doc_count(&self) -> u32 {
        self.header.doc_count
    }

    /// Iterate over all document records in order. O(n) total.
    pub fn iter_docs(&self) -> impl Iterator<Item = ParsedDocRecord> + '_ {
        let mut offset = self.header.file_table_offset as usize;
        let mut remaining = self.header.doc_count;
        let data = &self.data[..self.header.path_index_offset as usize];
        std::iter::from_fn(move || {
            if remaining == 0 {
                return None;
            }
            let header_end = offset.checked_add(12)?;
            let record_header = data.get(offset..header_end)?;
            let id = u32::from_le_bytes(record_header[..4].try_into().unwrap());
            let root_id = u32::from_le_bytes(record_header[4..8].try_into().unwrap());
            let path_len = u32::from_le_bytes(record_header[8..12].try_into().unwrap()) as usize;
            let path_end = header_end.checked_add(path_len)?;
            let end = path_end.checked_add(40)?;
            let rest = data.get(path_end..end)?;
            let path = data.get(header_end..path_end)?.to_vec();
            let byte_length = u64::from_le_bytes(rest[0..8].try_into().unwrap());
            let device = u64::from_le_bytes(rest[8..16].try_into().unwrap());
            let inode = u64::from_le_bytes(rest[16..24].try_into().unwrap());
            let mtime_ns = i64::from_le_bytes(rest[24..32].try_into().unwrap());
            let ctime_ns = i64::from_le_bytes(rest[32..40].try_into().unwrap());
            offset = end;
            remaining -= 1;
            Some(ParsedDocRecord {
                local_id: id,
                root_id,
                path,
                byte_length,
                device,
                inode,
                mtime_ns,
                ctime_ns,
            })
        })
    }

    /// Read a document record by local_id.
    /// Uses sequential scan — for bulk access, use iter_docs().
    pub fn get_doc(&self, local_id: u32) -> Option<ParsedDocRecord> {
        self.iter_docs().find(|d| d.local_id == local_id)
    }

    /// Get doc records for a set of local_ids, efficient for bulk access.
    /// Returns results in the same order as the input IDs.
    pub fn get_docs_bulk(&self, local_ids: &[u32]) -> Vec<Option<ParsedDocRecord>> {
        if local_ids.is_empty() {
            return Vec::new();
        }
        let id_set: std::collections::HashSet<u32> = local_ids.iter().copied().collect();
        let max_id = local_ids.iter().copied().max().unwrap_or(0);
        let mut result_map: std::collections::HashMap<u32, ParsedDocRecord> =
            std::collections::HashMap::with_capacity(local_ids.len());

        for doc in self.iter_docs() {
            if doc.local_id > max_id {
                break;
            }
            if id_set.contains(&doc.local_id) {
                result_map.insert(doc.local_id, doc);
            }
        }

        local_ids
            .iter()
            .map(|id| result_map.get(id).cloned())
            .collect()
    }

    /// Look up a file by (root_id, path). Returns local_id if found.
    pub fn lookup_path(&self, root_id: u32, path: &[u8]) -> Option<u32> {
        // Binary search the path index
        let mut lo: i64 = 0;
        let mut hi: i64 = self.header.doc_count as i64 - 1;

        while lo <= hi {
            let mid = (lo + hi) / 2;
            let entry = self.read_path_index_entry(mid as u32)?;
            let cmp = entry
                .0
                .cmp(&root_id)
                .then_with(|| entry.1.as_slice().cmp(path));
            match cmp {
                std::cmp::Ordering::Equal => return Some(entry.2),
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid - 1,
            }
        }
        None
    }

    fn read_path_index_entry(&self, index: u32) -> Option<(u32, Vec<u8>, u32)> {
        if index >= self.header.doc_count {
            return None;
        }
        let data = &self.data[..self.header.dictionary_offset as usize];
        let mut offset = self.header.path_index_offset as usize;
        for current in 0..=index {
            let header_end = offset.checked_add(8)?;
            let header = data.get(offset..header_end)?;
            let root = u32::from_le_bytes(header[..4].try_into().unwrap());
            let length = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
            let path_end = header_end.checked_add(length)?;
            let end = path_end.checked_add(4)?;
            let id = u32::from_le_bytes(data.get(path_end..end)?.try_into().unwrap());
            if current == index {
                return Some((root, data.get(header_end..path_end)?.to_vec(), id));
            }
            offset = end;
        }
        None
    }

    /// Read the posting list for a trigram. Returns sorted doc IDs.
    pub fn read_postings(&self, gram: u32) -> Vec<u32> {
        // Binary search the dictionary for the gram
        let dict_start = self.header.dictionary_offset as usize;
        let dict_entry_size = 16; // gram(4) + doc_freq(4) + posting_offset(8)
        let _num_entries = self.header.doc_count; // upper bound, not exact
                                                  // Actually, we don't know the exact number of dictionary entries without
                                                  // computing (postings_offset - dictionary_offset) / entry_size.
        let dict_end = self.header.postings_offset as usize;
        let dict_len = dict_end.saturating_sub(dict_start);
        let actual_entries = dict_len / dict_entry_size;

        let mut lo: i64 = 0;
        let mut hi: i64 = actual_entries as i64 - 1;
        while lo <= hi {
            let mid = (lo + hi) / 2;
            let entry_offset = dict_start + (mid as usize) * dict_entry_size;
            let entry_gram = u32::from_le_bytes(
                self.data[entry_offset..entry_offset + 4]
                    .try_into()
                    .unwrap(),
            );
            match entry_gram.cmp(&gram) {
                std::cmp::Ordering::Equal => {
                    let doc_freq = u32::from_le_bytes(
                        self.data[entry_offset + 4..entry_offset + 8]
                            .try_into()
                            .unwrap(),
                    );
                    let posting_offset = u64::from_le_bytes(
                        self.data[entry_offset + 8..entry_offset + 16]
                            .try_into()
                            .unwrap(),
                    );
                    return self.decode_postings(posting_offset, doc_freq);
                }
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid - 1,
            }
        }
        Vec::new()
    }

    fn decode_postings(&self, posting_offset: u64, doc_freq: u32) -> Vec<u32> {
        self.decode_postings_checked(posting_offset, doc_freq)
            .map(|(docs, _)| docs)
            .unwrap_or_default()
    }

    fn decode_postings_checked(
        &self,
        posting_offset: u64,
        doc_freq: u32,
    ) -> io::Result<(Vec<u32>, u64)> {
        if doc_freq > self.header.doc_count {
            return Err(invalid("posting frequency"));
        }
        let base = self.header.postings_offset as usize;
        let end = self.header.block_index_offset as usize;
        let relative = usize::try_from(posting_offset).map_err(|_| invalid("posting offset"))?;
        let mut offset = base
            .checked_add(relative)
            .filter(|&value| value <= end)
            .ok_or_else(|| invalid("posting offset"))?;
        let mut result = Vec::with_capacity((doc_freq as usize).min(end - offset));
        let mut remaining = doc_freq as usize;
        while remaining > 0 {
            let header_end = offset
                .checked_add(12)
                .filter(|&value| value <= end)
                .ok_or_else(|| invalid("posting header"))?;
            let header = &self.data[offset..header_end];
            let max_id = u32::from_le_bytes(header[..4].try_into().unwrap());
            let encoded_len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
            let expected_checksum = u32::from_le_bytes(header[8..12].try_into().unwrap());
            let block_end = header_end
                .checked_add(encoded_len)
                .filter(|&value| value <= end)
                .ok_or_else(|| invalid("posting block length"))?;
            let encoded = &self.data[header_end..block_end];
            let count = remaining.min(POSTING_BLOCK_SIZE);
            let mut position = 0;
            let mut previous = 0u32;
            let mut checksum = 0u32;
            for item in 0..count {
                let (delta, consumed) = decode_varint_u32(&encoded[position..]);
                if consumed == 0 {
                    return Err(invalid("posting varint"));
                }
                position += consumed;
                checksum ^= delta;
                let id = if item == 0 {
                    delta
                } else {
                    previous
                        .checked_add(delta)
                        .ok_or_else(|| invalid("posting delta overflow"))?
                };
                if result.last().is_some_and(|&last| id <= last) {
                    return Err(invalid("posting order"));
                }
                previous = id;
                result.push(id);
            }
            if position != encoded.len() || checksum != expected_checksum || previous != max_id {
                return Err(invalid("posting checksum or block bounds"));
            }
            remaining -= count;
            offset = block_end;
        }
        Ok((result, (offset - base) as u64))
    }

    /// Count postings from the validated dictionary without materializing their IDs.
    pub fn posting_count(&self) -> u64 {
        let start = self.header.dictionary_offset as usize;
        let end = self.header.postings_offset as usize;
        self.data[start..end]
            .chunks_exact(16)
            .map(|entry| u32::from_le_bytes(entry[4..8].try_into().unwrap()) as u64)
            .sum()
    }

    /// Iterate all (gram, doc_ids) pairs in the segment.
    pub fn iter_postings(&self) -> Vec<(u32, Vec<u32>)> {
        let dict_start = self.header.dictionary_offset as usize;
        let dict_end = self.header.postings_offset as usize;
        let dict_entry_size = 16;
        let num_entries = dict_end.saturating_sub(dict_start) / dict_entry_size;

        let mut result = Vec::with_capacity(num_entries);
        for i in 0..num_entries {
            let entry_offset = dict_start + i * dict_entry_size;
            let gram = u32::from_le_bytes(
                self.data[entry_offset..entry_offset + 4]
                    .try_into()
                    .unwrap(),
            );
            let doc_freq = u32::from_le_bytes(
                self.data[entry_offset + 4..entry_offset + 8]
                    .try_into()
                    .unwrap(),
            );
            let posting_offset = u64::from_le_bytes(
                self.data[entry_offset + 8..entry_offset + 16]
                    .try_into()
                    .unwrap(),
            );
            let doc_ids = self.decode_postings(posting_offset, doc_freq);
            result.push((gram, doc_ids));
        }
        result
    }
}

// ── Manifest ────────────────────────────────────────────────────────

pub fn publish_manifest(state_dir: &Path, segments: &[String], generation: u64) -> io::Result<()> {
    write_manifest(
        state_dir,
        &Manifest {
            generation,
            segments: segments.to_vec(),
            profile_hash: 0, // TODO: compute from segment headers
            roots: None,
        },
    )
}

pub fn publish_manifest_with_roots(
    state_dir: &Path,
    segments: &[String],
    generation: u64,
    roots: &[String],
) -> io::Result<()> {
    write_manifest(
        state_dir,
        &Manifest {
            generation,
            segments: segments.to_vec(),
            profile_hash: 0, // TODO: compute from segment headers
            roots: Some(roots.to_vec()),
        },
    )
}

/// Atomically publish a complete snapshot, preserving caller-owned metadata.
/// Root IDs are meaningful only within this snapshot's exact ordered roots.
pub fn write_manifest(state_dir: &Path, manifest: &Manifest) -> io::Result<()> {
    validate_manifest(manifest)?;
    let json = serde_json::to_string_pretty(manifest)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let manifest_path = state_dir.join("manifest.json");
    native::atomic_write(&manifest_path, json.as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    Ok(())
}

pub fn read_manifest(state_dir: &Path) -> io::Result<Manifest> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(state_dir.join("manifest.json"))?;
    let before = file.metadata()?;
    if !before.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "manifest is not a regular file",
        ));
    }
    let mut data = Vec::new();
    (&mut file)
        .take(before.len().saturating_add(1))
        .read_to_end(&mut data)?;
    if data.len() as u64 != before.len()
        || segment_identity(&file.metadata()?) != segment_identity(&before)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "manifest changed while reading",
        ));
    }
    let manifest: Manifest = serde_json::from_slice(&data)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub fn load_manifest(state_dir: &Path) -> Option<Manifest> {
    read_manifest(state_dir).ok()
}

fn validate_manifest(manifest: &Manifest) -> io::Result<()> {
    for name in &manifest.segments {
        let mut components = Path::new(name).components();
        if name.contains('/')
            || name.contains('\0')
            || !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid manifest segment name",
            ));
        }
    }
    Ok(())
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    pub generation: u64,
    pub segments: Vec<String>,
    pub profile_hash: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots: Option<Vec<String>>,
}

// ── Varint encoding ─────────────────────────────────────────────────

fn encode_varint_u32(val: u32, buf: &mut Vec<u8>) {
    let mut v = val;
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn decode_varint_u32(data: &[u8]) -> (u32, usize) {
    let mut result: u32 = 0;
    let mut shift: u32 = 0;
    let mut consumed = 0;
    for &b in data {
        consumed += 1;
        if shift == 28 && b > 0x0f {
            return (0, 0);
        }
        result |= ((b & 0x7F) as u32) << shift;
        if b & 0x80 == 0 {
            return (result, consumed);
        }
        shift += 7;
        if shift >= 32 {
            return (0, 0); // overflow
        }
    }
    (0, 0) // incomplete
}

// ── Checksum ────────────────────────────────────────────────────────

#[cfg(unix)]
fn segment_identity(metadata: &std::fs::Metadata) -> SegmentIdentity {
    use std::os::unix::fs::MetadataExt;
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid segment: {what}"),
    )
}

fn simple_checksum(data: &[u8]) -> u64 {
    // chunks_exact lets the optimizer vectorize the XOR; the old per-8-byte
    // bounds-checked loop ran ~55 MB/s and stalled segment-heavy commands.
    let mut checksum: u64 = data
        .chunks_exact(8)
        .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
        .fold(0u64, |acc, c| acc ^ c);
    // Handle remaining bytes
    let i = data.len() - data.len() % 8;
    if i < data.len() {
        let mut last: [u8; 8] = [0; 8];
        last[..data.len() - i].copy_from_slice(&data[i..]);
        checksum ^= u64::from_le_bytes(last);
    }
    checksum
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_doc(id: u32, root_id: u32, path: &str, content_len: u64) -> DocRecord {
        DocRecord {
            local_id: id,
            root_id,
            path: path.as_bytes().to_vec(),
            byte_length: content_len,
            device: 2064,
            inode: 1000000 + id as u64,
            mtime_ns: 1700000000_000_000_000 + id as i64 * 1000,
            ctime_ns: 1700000000_000_000_000 + id as i64 * 2000,
        }
    }

    #[test]
    fn test_segment_round_trip() {
        let tmpdir = std::env::temp_dir().join("tgrep_segment_test");
        std::fs::create_dir_all(&tmpdir).unwrap();

        // Build a segment with 3 docs
        let seg_path = tmpdir.join("test_seg.tgs");
        let mut writer = SegmentWriter::new(seg_path.clone(), 42);

        // Doc 0: "hello world" — trigrams: hel, ell, llo, lo , o w, wo, or, rl, ld
        let grams0 = vec![
            0x68656C, 0x656C6C, 0x6C6C6F, 0x6C6F20, 0x6F2077, 0x20776F, 0x776F72, 0x6F726C,
            0x726C64,
        ];
        writer.add_document(make_test_doc(0, 0, "src/main.c", 11), &grams0);

        // Doc 1: "hello there" — trigrams: hel, ell, llo, lo , o t, th, her, ere
        let grams1 = vec![
            0x68656C, 0x656C6C, 0x6C6C6F, 0x6C6F20, 0x6F2074, 0x207468, 0x746865, 0x686572,
            0x657265,
        ];
        writer.add_document(make_test_doc(1, 0, "src/lib.c", 12), &grams1);

        // Doc 2: "world peace" — trigrams: wor, orl, rl, ld, d , p, pe, ea, ac, ce
        let grams2 = vec![
            0x776F72, 0x6F726C, 0x726C64, 0x6C6420, 0x642070, 0x207065, 0x706561, 0x656163,
            0x616365,
        ];
        writer.add_document(make_test_doc(2, 1, "include/world.h", 11), &grams2);

        assert_eq!(writer.doc_count(), 3);
        assert_eq!(writer.unique_trigrams(), 20); // some overlap between docs
        assert!(writer.total_postings() > 0);

        // Flush to disk
        writer.flush(&tmpdir).unwrap();

        // Verify file exists
        let seg_file = tmpdir.join("seg_00000042.tgs");
        assert!(seg_file.exists(), "segment file should exist");

        // Read it back
        let reader = SegmentReader::open(&seg_file).unwrap();
        assert_eq!(reader.generation(), 42);
        assert_eq!(reader.doc_count(), 3);

        // Verify docs
        let doc0 = reader.get_doc(0).unwrap();
        assert_eq!(doc0.local_id, 0);
        assert_eq!(doc0.root_id, 0);
        assert_eq!(doc0.path, b"src/main.c");
        assert_eq!(doc0.byte_length, 11);

        let doc2 = reader.get_doc(2).unwrap();
        assert_eq!(doc2.path, b"include/world.h");
        assert_eq!(doc2.root_id, 1);

        // Verify path lookup
        assert_eq!(reader.lookup_path(0, b"src/main.c"), Some(0));
        assert_eq!(reader.lookup_path(0, b"src/lib.c"), Some(1));
        assert_eq!(reader.lookup_path(1, b"include/world.h"), Some(2));
        assert_eq!(reader.lookup_path(0, b"nonexistent"), None);

        // Verify postings
        // "hel" (0x68656C) should be in docs 0 and 1
        let postings = reader.read_postings(0x68656C);
        assert_eq!(postings, vec![0, 1]);

        // "wor" (0x776F72) should be in docs 0 and 2 (both contain "world")
        let postings = reader.read_postings(0x776F72);
        assert_eq!(postings, vec![0, 2]);

        // Non-existent trigram
        let postings = reader.read_postings(0xFFFFFF);
        assert!(postings.is_empty());

        // Verify iter_postings
        let all_postings = reader.iter_postings();
        assert_eq!(all_postings.len(), 20);

        // Clean up
        std::fs::remove_file(&seg_file).ok();
        std::fs::remove_dir_all(&tmpdir).ok();
    }

    #[test]
    fn test_manifest_round_trip() {
        let tmpdir = std::env::temp_dir().join("tgrep_manifest_test");
        std::fs::create_dir_all(&tmpdir).unwrap();

        let segments = vec!["seg_00000001.tgs".into(), "seg_00000002.tgs".into()];
        publish_manifest(&tmpdir, &segments, 2).unwrap();

        let loaded = load_manifest(&tmpdir).unwrap();
        assert_eq!(loaded.generation, 2);
        assert_eq!(loaded.segments, segments);

        std::fs::remove_dir_all(&tmpdir).ok();
    }

    #[test]
    fn test_varint_round_trip() {
        let values = vec![
            0u32,
            1,
            127,
            128,
            255,
            256,
            16383,
            16384,
            65535,
            65536,
            1000000,
            u32::MAX,
        ];
        for val in values {
            let mut buf = Vec::new();
            encode_varint_u32(val, &mut buf);
            let (decoded, consumed) = decode_varint_u32(&buf);
            assert_eq!(decoded, val, "failed for value {}", val);
            assert_eq!(consumed, buf.len());
        }
    }

    #[test]
    fn test_large_segment() {
        let tmpdir = std::env::temp_dir().join("tgrep_large_segment_test");
        std::fs::create_dir_all(&tmpdir).unwrap();

        let seg_path = tmpdir.join("large_seg.tgs");
        let mut writer = SegmentWriter::new(seg_path, 1);

        // Add 1000 docs with some shared trigrams
        for i in 0..1000u32 {
            let path = format!("file_{:04}.c", i);
            let content_len = (i as u64) * 100 + 50;
            let doc = make_test_doc(i, i / 100, &path, content_len);
            // Each doc has trigrams from "common" + unique ones
            let mut grams = vec![0x636F6D, 0x6F6D6D, 0x6D6D6F]; // "com", "omm", "mmo"
            grams.push(i * 0x10000 + 0x100); // unique-ish trigram
            writer.add_document(doc, &grams);
        }

        assert_eq!(writer.doc_count(), 1000);
        writer.flush(&tmpdir).unwrap();

        let seg_file = tmpdir.join("seg_00000001.tgs");
        let reader = SegmentReader::open(&seg_file).unwrap();
        assert_eq!(reader.doc_count(), 1000);

        // "com" (0x636F6D) should be in all 1000 docs
        let postings = reader.read_postings(0x636F6D);
        assert_eq!(postings.len(), 1000);
        assert_eq!(postings[0], 0);
        assert_eq!(postings[999], 999);

        // Verify a few docs
        let doc500 = reader.get_doc(500).unwrap();
        assert_eq!(doc500.path, b"file_0500.c");
        assert_eq!(doc500.byte_length, 50050);

        // Path lookup
        assert_eq!(reader.lookup_path(5, b"file_0500.c"), Some(500));
        assert_eq!(reader.lookup_path(0, b"file_0000.c"), Some(0));

        std::fs::remove_file(&seg_file).ok();
        std::fs::remove_dir_all(&tmpdir).ok();
    }
    fn audit_directory() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::var_os("TGREP_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = root.join(format!(
            "store-audit-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn audit_segment(dir: &Path) -> PathBuf {
        let mut writer = SegmentWriter::new(dir.join("unused"), 1);
        writer.add_document(make_test_doc(0, 0, "a", 1), &(0..64).collect::<Vec<_>>());
        writer.flush(dir).unwrap();
        dir.join("seg_00000001.tgs")
    }

    fn audit_mutate(path: &Path, change: impl FnOnce(&mut Vec<u8>)) {
        let mut bytes = std::fs::read(path).unwrap();
        change(&mut bytes);
        let footer = u64::from_le_bytes(bytes[66..74].try_into().unwrap()) as usize;
        let checksum = simple_checksum(&bytes[..footer]);
        bytes[footer..footer + 8].copy_from_slice(&checksum.to_le_bytes());
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn audit_docs_stop_at_file_table_boundary() {
        let dir = audit_directory();
        let path = audit_segment(&dir);
        let reader = SegmentReader::open(&path).unwrap();
        assert_eq!(reader.iter_docs().count(), 1);
        drop(reader);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn audit_bulk_keeps_duplicate_requests() {
        let dir = audit_directory();
        let path = audit_segment(&dir);
        let reader = SegmentReader::open(&path).unwrap();
        let docs = reader.get_docs_bulk(&[0, 0]);
        assert!(docs.iter().all(Option::is_some));
        assert_eq!(docs[0].as_ref().unwrap().path, b"a");
        drop(reader);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn audit_rejects_offsets_outside_own_sections() {
        for field in [26usize, 34, 58] {
            let dir = audit_directory();
            let path = audit_segment(&dir);
            audit_mutate(&path, |bytes| {
                bytes[field..field + 8]
                    .copy_from_slice(&(if field == 58 { u64::MAX } else { 0 }).to_le_bytes())
            });
            assert!(SegmentReader::open(&path).is_err(), "offset field {field}");
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn audit_rejects_footer_doc_count_mismatch() {
        let dir = audit_directory();
        let path = audit_segment(&dir);
        audit_mutate(&path, |bytes| {
            let n = bytes.len();
            bytes[n - 4..].copy_from_slice(&2u32.to_le_bytes());
        });
        assert!(SegmentReader::open(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn audit_rejects_posting_offset_overflow() {
        let dir = audit_directory();
        let path = audit_segment(&dir);
        audit_mutate(&path, |bytes| {
            let dict = u64::from_le_bytes(bytes[42..50].try_into().unwrap()) as usize;
            bytes[dict + 8..dict + 16].copy_from_slice(&u64::MAX.to_le_bytes());
        });
        assert!(SegmentReader::open(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn audit_rejects_bad_block_checksum_and_truncated_delta() {
        for corrupt_checksum in [true, false] {
            let dir = audit_directory();
            let path = audit_segment(&dir);
            audit_mutate(&path, |bytes| {
                let post = u64::from_le_bytes(bytes[50..58].try_into().unwrap()) as usize;
                if corrupt_checksum {
                    bytes[post + 8..post + 12].copy_from_slice(&1u32.to_le_bytes());
                } else {
                    bytes[post + 12] = 0x80;
                }
            });
            assert!(
                SegmentReader::open(&path).is_err(),
                "block checksum corruption={corrupt_checksum}"
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn audit_cache_cannot_hide_same_mtime_in_place_corruption() {
        use std::io::{Seek, SeekFrom, Write};
        use std::os::unix::fs::MetadataExt;
        let dir = audit_directory();
        let path = audit_segment(&dir);
        drop(SegmentReader::open(&path).unwrap());
        let before = std::fs::metadata(&path).unwrap();
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(86)).unwrap();
        file.write_all(b"z").unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(before.modified().unwrap()))
            .unwrap();
        let after = file.metadata().unwrap();
        assert_eq!(before.mtime(), after.mtime());
        assert!(SegmentReader::open(&path).is_err());
        drop(file);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn audit_varint_overflow_is_invalid() {
        assert_eq!(decode_varint_u32(&[0xff, 0xff, 0xff, 0xff, 0x1f]), (0, 0));
        assert_eq!(
            decode_varint_u32(&[0xff, 0xff, 0xff, 0xff, 0x0f]),
            (u32::MAX, 5)
        );
    }

    #[test]
    fn audit_valid_multiple_blocks_and_empty_segment() {
        let dir = audit_directory();
        let mut writer = SegmentWriter::new(dir.join("unused"), 2);
        for id in 0..260 {
            writer.add_document(make_test_doc(id, 0, &format!("file{id}"), 1), &[0x123456]);
        }
        writer.flush(&dir).unwrap();
        let reader = SegmentReader::open(&dir.join("seg_00000002.tgs")).unwrap();
        assert_eq!(reader.read_postings(0x123456), (0..260).collect::<Vec<_>>());
        assert_eq!(reader.posting_count(), 260);
        assert_eq!(reader.iter_docs().count(), 260);
        drop(reader);
        let empty = SegmentWriter::new(dir.join("unused"), 3);
        empty.flush(&dir).unwrap();
        let reader = SegmentReader::open(&dir.join("seg_00000003.tgs")).unwrap();
        assert_eq!(reader.iter_docs().count(), 0);
        assert!(reader.iter_postings().is_empty());
        drop(reader);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn audit_manifest_errors_are_distinct_and_names_stay_local() {
        let dir = audit_directory();
        assert_eq!(
            read_manifest(&dir).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        std::fs::write(dir.join("manifest.json"), b"broken").unwrap();
        assert_eq!(
            read_manifest(&dir).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        for name in [
            "../outside.tgs",
            "/tmp/outside.tgs",
            "nested/file.tgs",
            "",
            ".",
        ] {
            let manifest = Manifest {
                generation: 1,
                segments: vec![name.into()],
                profile_hash: 0,
                roots: None,
            };
            std::fs::write(
                dir.join("manifest.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            assert!(read_manifest(&dir).is_err(), "{name}");
            assert!(publish_manifest(&dir, &[name.into()], 2).is_err());
        }
        publish_manifest(&dir, &["seg_00000001.tgs".into()], 1).unwrap();
        assert_eq!(read_manifest(&dir).unwrap().segments, ["seg_00000001.tgs"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn audit_atomic_replacement_keeps_old_mapping_and_rechecks_new_identity() {
        let dir = audit_directory();
        let path = audit_segment(&dir);
        let old = SegmentReader::open(&path).unwrap();
        let replacement = audit_directory();
        let mut writer = SegmentWriter::new(replacement.join("unused"), 1);
        writer.add_document(make_test_doc(0, 0, "replacement", 20), &[0xabcdef]);
        writer.flush(&replacement).unwrap();
        std::fs::rename(replacement.join("seg_00000001.tgs"), &path).unwrap();
        let new = SegmentReader::open(&path).unwrap();
        assert_eq!(old.get_doc(0).unwrap().path, b"a");
        assert_eq!(new.get_doc(0).unwrap().path, b"replacement");
        assert_eq!(old.read_postings(0), [0]);
        assert_eq!(new.read_postings(0xabcdef), [0]);
        drop(old);
        drop(new);
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(replacement).unwrap();
    }

    #[test]
    fn audit_mutated_valid_checksum_segments_do_not_panic() {
        let dir = audit_directory();
        let path = audit_segment(&dir);
        let original = std::fs::read(&path).unwrap();
        let footer = u64::from_le_bytes(original[66..74].try_into().unwrap()) as usize;
        let mut random = 0x12345678u32;
        for _ in 0..512 {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            let position = random as usize % footer;
            let mut bytes = original.clone();
            bytes[position] ^= 1 << (random % 8);
            let checksum = simple_checksum(&bytes[..footer]);
            bytes[footer..footer + 8].copy_from_slice(&checksum.to_le_bytes());
            std::fs::write(&path, bytes).unwrap();
            if let Ok(reader) = SegmentReader::open(&path) {
                let docs: Vec<_> = reader.iter_docs().collect();
                assert_eq!(docs.len(), reader.doc_count() as usize);
                let _ = reader.iter_postings();
                for doc in docs {
                    assert_eq!(
                        reader.lookup_path(doc.root_id, &doc.path),
                        Some(doc.local_id)
                    );
                }
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn audit_sparse_document_ids_preserve_bulk_order() {
        let dir = audit_directory();
        let mut writer = SegmentWriter::new(dir.join("unused"), 1);
        writer.add_document(make_test_doc(7, 0, "a", 1), &[0x123456]);
        writer.add_document(make_test_doc(400, 0, "z", 1), &[0x123456]);
        writer.flush(&dir).unwrap();
        let reader = SegmentReader::open(&dir.join("seg_00000001.tgs")).unwrap();
        assert_eq!(reader.read_postings(0x123456), [7, 400]);
        let docs = reader.get_docs_bulk(&[400, 7, 400]);
        assert_eq!(
            docs.iter()
                .map(|doc| doc.as_ref().unwrap().path.clone())
                .collect::<Vec<_>>(),
            [b"z".to_vec(), b"a".to_vec(), b"z".to_vec()]
        );
        drop(reader);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn audit_manifest_legacy_and_bound_roots_round_trip() {
        let dir = audit_directory();
        let segments = vec!["seg_00000001.tgs".to_string()];
        publish_manifest(&dir, &segments, 7).unwrap();
        assert!(read_manifest(&dir).unwrap().roots.is_none());
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        assert!(json.get("roots").is_none());
        let roots = vec!["/canonical/root".to_string(), "/second/root".to_string()];
        publish_manifest_with_roots(&dir, &segments, 8, &roots).unwrap();
        let loaded = read_manifest(&dir).unwrap();
        assert_eq!(loaded.roots.as_ref(), Some(&roots));
        assert_eq!(loaded.segments, segments);
        assert_eq!(loaded.generation, 8);
        let mut legacy = loaded;
        legacy.generation = 7;
        legacy.profile_hash = 99;
        legacy.roots = Some(vec!["relative/../exact-original-root/".into()]);
        write_manifest(&dir, &legacy).unwrap();
        let rebound = read_manifest(&dir).unwrap();
        assert_eq!(rebound.profile_hash, 99);
        assert_eq!(rebound.generation, 7);
        assert_eq!(rebound.segments, segments);
        assert_eq!(rebound.roots, legacy.roots);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn audit_nonregular_read_child() {
        let Some(path) = std::env::var_os("TGREP_STORE_TEST_FIFO_PATH") else {
            return;
        };
        let path = PathBuf::from(path);
        if std::env::var("TGREP_STORE_TEST_FIFO_KIND").as_deref() == Ok("manifest") {
            assert!(read_manifest(&path).is_err());
        } else {
            assert!(SegmentReader::open(&path).is_err());
        }
    }

    fn audit_fifo_read_rejected(kind: &str) {
        use std::os::unix::ffi::OsStrExt;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        {
            let dir = audit_directory();
            let fifo = dir.join(if kind == "manifest" {
                "manifest.json"
            } else {
                "fifo.tgs"
            });
            let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "store::tests::audit_nonregular_read_child"])
                .env(
                    "TGREP_STORE_TEST_FIFO_PATH",
                    if kind == "manifest" {
                        dir.as_os_str()
                    } else {
                        fifo.as_os_str()
                    },
                )
                .env("TGREP_STORE_TEST_FIFO_KIND", kind)
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
                    panic!("{kind} FIFO open waited for a writer");
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
    }

    #[test]
    fn audit_manifest_fifo_is_rejected_without_writer() {
        audit_fifo_read_rejected("manifest");
    }

    #[test]
    fn audit_segment_fifo_is_rejected_without_writer() {
        audit_fifo_read_rejected("segment");
    }

    #[test]
    fn audit_manifest_regular_snapshot_has_no_arbitrary_size_ceiling() {
        let dir = audit_directory();
        let mut data = vec![b' '; 256 * 1024];
        data.extend_from_slice(
            br#"{"generation":7,"segments":[],"profile_hash":99,"roots":["/root"]}"#,
        );
        std::fs::write(dir.join("manifest.json"), data).unwrap();
        let manifest = read_manifest(&dir).unwrap();
        assert_eq!(manifest.generation, 7);
        assert_eq!(manifest.roots, Some(vec!["/root".into()]));
        std::fs::remove_file(dir.join("manifest.json")).unwrap();
        std::fs::create_dir(dir.join("manifest.json")).unwrap();
        assert!(read_manifest(&dir).is_err());
        assert!(SegmentReader::open(&dir.join("manifest.json")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
