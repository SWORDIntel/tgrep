use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::native;

// ── Constants ───────────────────────────────────────────────────────

pub const SEGMENT_MAGIC: u32 = 0x54475331; // "TGS1"
pub const FORMAT_VERSION: u16 = 1;
pub const FOOTER_MAGIC: u32 = 0x5447464E; // "TGFN" (tgrep footer)
pub const POSTING_BLOCK_SIZE: usize = 128;

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
        let mut deduped: Vec<u32> = Vec::with_capacity(grams.len());
        for &g in grams {
            if !deduped.contains(&g) {
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

        // Serialize to a buffer first, then atomic write
        let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);

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
        let (postings_data, block_index_data) = self.encode_postings();

        // ── Compute offsets ────────────────────────────────────────
        let file_table_offset = header_size as u64;
        let path_index_offset = file_table_offset + file_table_size;
        let dictionary_offset = path_index_offset + path_index_size;
        let postings_offset = dictionary_offset + dict_size;
        let block_index_offset = postings_offset + postings_data.len() as u64;
        let footer_offset = block_index_offset + block_index_data.len() as u64;

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
        let mut posting_offset: u64 = 0;
        for (gram, doc_ids) in &self.postings {
            buf.extend_from_slice(&gram.to_le_bytes());
            buf.extend_from_slice(&(doc_ids.len() as u32).to_le_bytes());
            buf.extend_from_slice(&posting_offset.to_le_bytes());
            // Advance by encoded size of this gram's postings
            let _num_blocks = (doc_ids.len() + POSTING_BLOCK_SIZE - 1) / POSTING_BLOCK_SIZE;
            // Each block: block_header(4+4+4=12) + varint-encoded deltas
            // We precomputed this in encode_postings, so use the block index
            posting_offset += 0; // Will be fixed up below
        }

        // Actually, we need to compute posting offsets properly.
        // Let's redo: compute per-gram posting sizes from the encoded data.
        // Rebuild the dictionary with correct offsets.
        let dict_start = buf.len() - dict_size as usize;
        buf.truncate(dict_start); // Remove the incorrect dictionary

        // Compute per-gram posting sizes
        let mut gram_posting_sizes: Vec<(u32, u64)> = Vec::new();
        let mut offset: u64 = 0;
        for (gram, doc_ids) in &self.postings {
            gram_posting_sizes.push((*gram, offset));
            let _num_blocks = (doc_ids.len() + POSTING_BLOCK_SIZE - 1) / POSTING_BLOCK_SIZE;
            // Each block: 12 bytes header + encoded deltas
            // We need the actual encoded size per gram — compute from block_index
            // Actually, let's just compute it from the encoded data.
            // For simplicity, re-encode per-gram and track sizes.
            let gram_encoded = self.encode_gram_postings(doc_ids);
            offset += gram_encoded.len() as u64;
        }

        // Write dictionary with correct offsets
        for (i, (gram, doc_ids)) in self.postings.iter().enumerate() {
            let (_, start_offset) = gram_posting_sizes[i];
            buf.extend_from_slice(&gram.to_le_bytes());
            buf.extend_from_slice(&(doc_ids.len() as u32).to_le_bytes());
            buf.extend_from_slice(&start_offset.to_le_bytes());
        }

        // ── Write postings ────────────────────────────────────────
        // Re-encode per-gram and write
        for (_, doc_ids) in &self.postings {
            let gram_data = self.encode_gram_postings(doc_ids);
            buf.extend_from_slice(&gram_data);
        }

        // ── Write block index ──────────────────────────────────────
        buf.extend_from_slice(&block_index_data);

        // ── Write footer ───────────────────────────────────────────
        // Simple checksum: XOR of all 8-byte chunks (not cryptographic, just integrity)
        let checksum = simple_checksum(&buf);
        buf.extend_from_slice(&checksum.to_le_bytes());
        buf.extend_from_slice(&FOOTER_MAGIC.to_le_bytes());
        buf.extend_from_slice(&(self.docs.len() as u32).to_le_bytes());

        // ── Atomic write to disk ──────────────────────────────────
        native::atomic_write(&target_path, &buf)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        Ok(())
    }

    /// Encode all postings across all grams, returning (postings_data, block_index_data)
    fn encode_postings(&self) -> (Vec<u8>, Vec<u8>) {
        let mut postings_buf = Vec::new();
        let mut block_index_buf = Vec::new();

        for (_, doc_ids) in &self.postings {
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

        (postings_buf, block_index_buf)
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
    _block_index_offset: u64,
    footer_offset: u64,
}

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
    /// Open and mmap a segment file.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let data = unsafe { memmap2::Mmap::map(&file)? };

        let header = Self::parse_header(&data)?;

        // Verify footer
        let footer_start = header.footer_offset as usize;
        if footer_start + 16 > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "segment too short for footer",
            ));
        }
        let footer_magic = u32::from_le_bytes(
            data[footer_start + 8..footer_start + 12]
                .try_into()
                .unwrap(),
        );
        if footer_magic != FOOTER_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid footer magic",
            ));
        }

        Ok(Self { data, header })
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
            _block_index_offset: u64::from_le_bytes(data[58..66].try_into().unwrap()),
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
        let data: &[u8] = &self.data;
        std::iter::from_fn(move || {
            if offset + 12 > data.len() {
                return None;
            }
            let id = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            let root_id = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap());
            let path_len =
                u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap()) as usize;
            if offset + 12 + path_len + 40 > data.len() {
                return None;
            }
            let path = data[offset + 12..offset + 12 + path_len].to_vec();
            let rest = &data[offset + 12 + path_len..];
            let byte_length = u64::from_le_bytes(rest[0..8].try_into().unwrap());
            let device = u64::from_le_bytes(rest[8..16].try_into().unwrap());
            let inode = u64::from_le_bytes(rest[16..24].try_into().unwrap());
            let mtime_ns = i64::from_le_bytes(rest[24..32].try_into().unwrap());
            let ctime_ns = i64::from_le_bytes(rest[32..40].try_into().unwrap());

            offset += 12 + path_len + 40;

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

        local_ids.iter().map(|id| result_map.remove(id)).collect()
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
        // Path index entries are variable-size, so we have to scan from the start.
        // For a real implementation, we'd add a fixed-size secondary index.
        // For now, linear scan (acceptable for segments with <64K docs).
        let mut offset = self.header.path_index_offset as usize;
        for _ in 0..index {
            if offset + 8 > self.data.len() {
                return None;
            }
            let path_len =
                u32::from_le_bytes(self.data[offset + 4..offset + 8].try_into().unwrap()) as usize;
            offset += 8 + path_len + 4;
        }
        if offset + 8 > self.data.len() {
            return None;
        }
        let root_id = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
        let path_len =
            u32::from_le_bytes(self.data[offset + 4..offset + 8].try_into().unwrap()) as usize;
        if offset + 8 + path_len + 4 > self.data.len() {
            return None;
        }
        let path = self.data[offset + 8..offset + 8 + path_len].to_vec();
        let file_index = u32::from_le_bytes(
            self.data[offset + 8 + path_len..offset + 8 + path_len + 4]
                .try_into()
                .unwrap(),
        );
        Some((root_id, path, file_index))
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
        let mut result = Vec::with_capacity(doc_freq as usize);
        let mut offset = self.header.postings_offset as usize + posting_offset as usize;
        let mut remaining = doc_freq as usize;

        while remaining > 0 && offset + 12 <= self.data.len() {
            let _max_id = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
            let enc_len =
                u32::from_le_bytes(self.data[offset + 4..offset + 8].try_into().unwrap()) as usize;
            let _checksum =
                u32::from_le_bytes(self.data[offset + 8..offset + 12].try_into().unwrap());

            if offset + 12 + enc_len > self.data.len() {
                break;
            }

            // Decode varint deltas
            let encoded = &self.data[offset + 12..offset + 12 + enc_len];
            let mut pos = 0;
            let mut prev: u32 = 0;
            let mut first = true;
            let block_count = remaining.min(POSTING_BLOCK_SIZE);

            for _ in 0..block_count {
                if pos >= encoded.len() {
                    break;
                }
                let (delta, consumed) = decode_varint_u32(&encoded[pos..]);
                if consumed == 0 {
                    break;
                }
                pos += consumed;
                if first {
                    result.push(delta);
                    prev = delta;
                    first = false;
                } else {
                    prev = prev.wrapping_add(delta);
                    result.push(prev);
                }
            }
            remaining -= block_count;
            offset += 12 + enc_len;
        }
        result
    }

    /// Iterate all (gram, doc_ids) pairs in the segment.
    pub fn iter_postings(&self) -> Vec<(u32, Vec<u32>)> {
        let dict_start = self.header.dictionary_offset as usize;
        let dict_end = self.header.postings_offset as usize;
        let dict_entry_size = 16;
        let num_entries = (dict_end - dict_start) / dict_entry_size;

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
    let manifest = Manifest {
        generation,
        segments: segments.to_vec(),
        profile_hash: 0, // TODO: compute from segment headers
    };
    let json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let manifest_path = state_dir.join("manifest.json");
    native::atomic_write(&manifest_path, json.as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    Ok(())
}

pub fn load_manifest(state_dir: &Path) -> Option<Manifest> {
    let manifest_path = state_dir.join("manifest.json");
    let data = std::fs::read(&manifest_path).ok()?;
    serde_json::from_slice(&data).ok()
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    pub generation: u64,
    pub segments: Vec<String>,
    pub profile_hash: u64,
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

fn simple_checksum(data: &[u8]) -> u64 {
    let mut checksum: u64 = 0;
    let mut i = 0;
    while i + 8 <= data.len() {
        let chunk = u64::from_le_bytes(data[i..i + 8].try_into().unwrap());
        checksum ^= chunk;
        i += 8;
    }
    // Handle remaining bytes
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
}
