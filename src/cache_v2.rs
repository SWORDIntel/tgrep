// Phase 11: v2 search-result cache — grouped entries with file IDs.
//
// Replaces the v1 C wrapper (one row per file, full path strings) with:
//   - Grouped entries: one entry per (pattern, flags, generation)
//   - File IDs: u32 indices into the file_list_cache, not full paths
//   - Hash index: O(1) lookup by (pattern, flags, generation)
//   - Generation compaction: drop stale-generation entries
//
// Format (TGSC v2):
//   magic(4) "TGSC" | version(4) = 2 | entry_count(4)
//   For each entry:
//     pattern_len(2) + pattern_bytes
//     flags(4) | generation(8) | cached_at(8)
//     file_count(4) + file_ids: file_count × u32
//
// Backward compat: can load v1 files and auto-migrate.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::build::{default_state_dir, load_file_list_cache, FileListCache};

// ── Constants ───────────────────────────────────────────────────────

const TGSC_MAGIC: u32 = 0x5343_4754; // "TGSC" little-endian
const TGSC_VERSION_1: u32 = 1;
const TGSC_VERSION_2: u32 = 2;

pub const CACHE_FLAG_CASE_INSENSITIVE: i32 = 0x01;
pub const CACHE_FLAG_WORD_REGEXP: i32 = 0x02;
pub const CACHE_FLAG_FIXED_STRINGS: i32 = 0x04;

// ── Cache entry (grouped) ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub pattern: String,
    pub flags: i32,
    pub generation: i64,
    pub file_ids: Vec<u32>,
    pub cached_at: i64,
}

impl CacheEntry {
    pub fn file_count(&self) -> usize {
        self.file_ids.len()
    }
}

// ── Cache key for hash index ────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    pattern: String,
    flags: i32,
    generation: i64,
}

// ── v2 Cache ────────────────────────────────────────────────────────

pub struct SearchCacheV2 {
    entries: Vec<CacheEntry>,
    index: HashMap<CacheKey, usize>, // key → index in entries
    file_list: Option<FileListCache>, // for path ↔ ID resolution
    file_path_index: HashMap<String, u32>, // path → file ID
}

impl SearchCacheV2 {
    /// Create a new empty cache, loading the file list cache for
    /// path ↔ ID resolution.
    pub fn create() -> Result<Self, String> {
        let state_dir = default_state_dir();
        let file_list = load_file_list_cache(&state_dir);

        let mut file_path_index = HashMap::new();
        if let Some(ref flc) = file_list {
            for (i, f) in flc.files.iter().enumerate() {
                file_path_index.insert(f.path.clone(), i as u32);
            }
        }

        Ok(Self {
            entries: Vec::new(),
            index: HashMap::new(),
            file_list,
            file_path_index,
        })
    }

    /// Number of grouped entries (not individual files).
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Total number of file references across all entries.
    pub fn total_file_refs(&self) -> usize {
        self.entries.iter().map(|e| e.file_ids.len()).sum()
    }

    /// Lookup cached results for a query.
    /// Returns resolved file paths, or None on miss.
    pub fn lookup(
        &self,
        pattern: &str,
        flags: i32,
        generation: i64,
    ) -> Option<Vec<String>> {
        let key = CacheKey {
            pattern: pattern.to_string(),
            flags,
            generation,
        };
        let idx = *self.index.get(&key)?;
        let entry = &self.entries[idx];

        // Resolve file IDs to paths
        let flc = self.file_list.as_ref()?;
        let paths: Vec<String> = entry
            .file_ids
            .iter()
            .filter_map(|&id| flc.files.get(id as usize).map(|f| f.path.clone()))
            .collect();
        Some(paths)
    }

    /// Store results for a query. Converts paths to file IDs.
    /// If a path isn't in the file list, it's skipped (with a warning
    /// to stderr in debug mode).
    pub fn store_results(
        &mut self,
        pattern: &str,
        flags: i32,
        generation: i64,
        file_paths: &[String],
    ) {
        let key = CacheKey {
            pattern: pattern.to_string(),
            flags,
            generation,
        };

        // Convert paths to file IDs
        let file_ids: Vec<u32> = file_paths
            .iter()
            .filter_map(|p| self.file_path_index.get(p).copied())
            .collect();

        let cached_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let entry = CacheEntry {
            pattern: pattern.to_string(),
            flags,
            generation,
            file_ids,
            cached_at,
        };

        if let Some(&idx) = self.index.get(&key) {
            // Replace existing entry
            self.entries[idx] = entry;
        } else {
            let idx = self.entries.len();
            self.entries.push(entry);
            self.index.insert(key, idx);
        }
    }

    /// Compact: remove entries whose generation != current generation.
    /// Returns the number of entries removed.
    pub fn compact(&mut self, current_generation: i64) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| e.generation == current_generation);
        // Rebuild index
        self.index.clear();
        for (i, e) in self.entries.iter().enumerate() {
            self.index.insert(
                CacheKey {
                    pattern: e.pattern.clone(),
                    flags: e.flags,
                    generation: e.generation,
                },
                i,
            );
        }
        before - self.entries.len()
    }

    /// Get all entries (for status display).
    pub fn get_entries(&self) -> &[CacheEntry] {
        &self.entries
    }

    /// Get the file list cache (for path resolution).
    pub fn file_list(&self) -> Option<&FileListCache> {
        self.file_list.as_ref()
    }

    // ── Persistence ──────────────────────────────────────────────────

    /// Save cache to a .qsc file (v2 format).
    pub fn save(&self, path: &str) -> Result<(), String> {
        let path = Path::new(path);
        let tmp = path.with_extension("qsc.tmp");

        let mut f = std::fs::File::create(&tmp)
            .map_err(|e| format!("create {}: {}", path.display(), e))?;

        // Header
        f.write_all(&TGSC_MAGIC.to_le_bytes())
            .map_err(|e| e.to_string())?;
        f.write_all(&TGSC_VERSION_2.to_le_bytes())
            .map_err(|e| e.to_string())?;
        let entry_count = self.entries.len() as u32;
        f.write_all(&entry_count.to_le_bytes())
            .map_err(|e| e.to_string())?;

        // Entries
        for entry in &self.entries {
            // pattern
            let pat_bytes = entry.pattern.as_bytes();
            let pat_len = pat_bytes.len() as u16;
            f.write_all(&pat_len.to_le_bytes())
                .map_err(|e| e.to_string())?;
            f.write_all(pat_bytes).map_err(|e| e.to_string())?;

            // flags, generation, cached_at
            f.write_all(&entry.flags.to_le_bytes())
                .map_err(|e| e.to_string())?;
            f.write_all(&entry.generation.to_le_bytes())
                .map_err(|e| e.to_string())?;
            f.write_all(&entry.cached_at.to_le_bytes())
                .map_err(|e| e.to_string())?;

            // file IDs
            let fc = entry.file_ids.len() as u32;
            f.write_all(&fc.to_le_bytes())
                .map_err(|e| e.to_string())?;
            for &id in &entry.file_ids {
                f.write_all(&id.to_le_bytes())
                    .map_err(|e| e.to_string())?;
            }
        }

        f.flush().map_err(|e| e.to_string())?;
        drop(f);

        // Atomic rename
        std::fs::rename(&tmp, path)
            .map_err(|e| format!("rename: {}", e))?;

        Ok(())
    }

    /// Load cache from a .qsc file. Auto-detects v1 or v2 format.
    /// For v1, migrates to v2 in-memory (caller should re-save).
    pub fn load(&mut self, path: &str) -> Result<bool, String> {
        let path = Path::new(path);
        if !path.exists() {
            return Ok(false);
        }

        let mut data = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| format!("open {}: {}", path.display(), e))?
            .read_to_end(&mut data)
            .map_err(|e| format!("read: {}", e))?;

        if data.len() < 8 {
            return Err("file too small".to_string());
        }

        let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if magic != TGSC_MAGIC {
            return Err("bad magic".to_string());
        }

        let version = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        match version {
            TGSC_VERSION_1 => self.load_v1(&data),
            TGSC_VERSION_2 => self.load_v2(&data),
            _ => Err(format!("unsupported version: {}", version)),
        }
    }

    fn load_v2(&mut self, data: &[u8]) -> Result<bool, String> {
        let mut pos = 8;
        if data.len() < pos + 4 {
            return Err("truncated header".to_string());
        }
        let entry_count = u32::from_le_bytes([
            data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
        ]) as usize;
        pos += 4;

        self.entries.clear();
        self.index.clear();
        self.entries.reserve(entry_count);

        for _ in 0..entry_count {
            // pattern
            if pos + 2 > data.len() {
                return Err("truncated entry".to_string());
            }
            let pat_len = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
            pos += 2;
            if pos + pat_len > data.len() {
                return Err("truncated pattern".to_string());
            }
            let pattern = String::from_utf8(data[pos..pos + pat_len].to_vec())
                .map_err(|e| format!("bad utf8: {}", e))?;
            pos += pat_len;

            // flags, generation, cached_at
            if pos + 4 + 8 + 8 > data.len() {
                return Err("truncated entry header".to_string());
            }
            let flags = i32::from_le_bytes([
                data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
            ]);
            pos += 4;
            let generation = i64::from_le_bytes([
                data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
                data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7],
            ]);
            pos += 8;
            let cached_at = i64::from_le_bytes([
                data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
                data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7],
            ]);
            pos += 8;

            // file IDs
            if pos + 4 > data.len() {
                return Err("truncated file count".to_string());
            }
            let fc = u32::from_le_bytes([
                data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
            ]) as usize;
            pos += 4;

            if pos + fc * 4 > data.len() {
                return Err("truncated file IDs".to_string());
            }
            let mut file_ids = Vec::with_capacity(fc);
            for _ in 0..fc {
                let id = u32::from_le_bytes([
                    data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
                ]);
                pos += 4;
                file_ids.push(id);
            }

            let key = CacheKey {
                pattern: pattern.clone(),
                flags,
                generation,
            };
            let idx = self.entries.len();
            self.entries.push(CacheEntry {
                pattern,
                flags,
                generation,
                file_ids,
                cached_at,
            });
            self.index.insert(key, idx);
        }

        Ok(true)
    }

    /// Load v1 format (one row per file) and migrate to grouped entries.
    fn load_v1(&mut self, data: &[u8]) -> Result<bool, String> {
        // v1: magic(4) + version(4) + row_count(8) + rows...
        // Each row: pat_len(4) + pat + flags(4) + gen(8) + path_len(4) + path + cached_at(8)
        let mut pos = 8;
        if pos + 8 > data.len() {
            return Err("truncated v1 header".to_string());
        }

        // row_count is usize (8 bytes on 64-bit)
        let row_count = usize::from_le_bytes([
            data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
            data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7],
        ]);
        pos += 8;

        // Group rows by (pattern, flags, generation)
        let mut groups: HashMap<CacheKey, (i64, Vec<String>)> = HashMap::new();

        for _ in 0..row_count {
            if pos + 4 > data.len() {
                break;
            }
            let pat_len = u32::from_le_bytes([
                data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
            ]) as usize;
            pos += 4;
            if pos + pat_len > data.len() {
                break;
            }
            let pattern = String::from_utf8_lossy(&data[pos..pos + pat_len]).to_string();
            pos += pat_len;

            if pos + 4 + 8 > data.len() {
                break;
            }
            let flags = i32::from_le_bytes([
                data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
            ]);
            pos += 4;
            let generation = i64::from_le_bytes([
                data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
                data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7],
            ]);
            pos += 8;

            if pos + 4 > data.len() {
                break;
            }
            let fp_len = u32::from_le_bytes([
                data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
            ]) as usize;
            pos += 4;
            if pos + fp_len > data.len() {
                break;
            }
            let file_path = String::from_utf8_lossy(&data[pos..pos + fp_len]).to_string();
            pos += fp_len;

            if pos + 8 > data.len() {
                break;
            }
            let cached_at = i64::from_le_bytes([
                data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
                data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7],
            ]);
            pos += 8;

            let key = CacheKey {
                pattern: pattern.clone(),
                flags,
                generation,
            };
            groups
                .entry(key)
                .or_insert_with(|| (cached_at, Vec::new()))
                .1
                .push(file_path);
        }

        // Convert grouped v1 data to v2 entries (paths → file IDs)
        self.entries.clear();
        self.index.clear();

        for (key, (cached_at, paths)) in groups {
            let file_ids: Vec<u32> = paths
                .iter()
                .filter_map(|p| self.file_path_index.get(p).copied())
                .collect();
            let idx = self.entries.len();
            self.entries.push(CacheEntry {
                pattern: key.pattern.clone(),
                flags: key.flags,
                generation: key.generation,
                file_ids,
                cached_at,
            });
            self.index.insert(key, idx);
        }

        Ok(true)
    }
}

// ── Cache file path ─────────────────────────────────────────────────

pub fn cache_path() -> PathBuf {
    default_state_dir().join("search_cache.qsc")
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_cache() -> SearchCacheV2 {
        // Create a cache without file list (paths won't resolve to IDs,
        // but we can test the format directly)
        SearchCacheV2 {
            entries: Vec::new(),
            index: HashMap::new(),
            file_list: None,
            file_path_index: HashMap::new(),
        }
    }

    #[test]
    fn test_v2_round_trip() {
        let tmp = std::env::temp_dir().join("tgrep_cache_v2_test.qsc");
        let _ = std::fs::remove_file(&tmp);

        let mut cache = make_test_cache();

        // Manually insert entries with file IDs
        let key1 = CacheKey {
            pattern: "struct".to_string(),
            flags: 0,
            generation: 1,
        };
        cache.entries.push(CacheEntry {
            pattern: "struct".to_string(),
            flags: 0,
            generation: 1,
            file_ids: vec![1, 5, 10, 42],
            cached_at: 1234567890,
        });
        cache.index.insert(key1, 0);

        let key2 = CacheKey {
            pattern: "main".to_string(),
            flags: CACHE_FLAG_CASE_INSENSITIVE,
            generation: 1,
        };
        cache.entries.push(CacheEntry {
            pattern: "main".to_string(),
            flags: CACHE_FLAG_CASE_INSENSITIVE,
            generation: 1,
            file_ids: vec![0, 3, 7],
            cached_at: 1234567891,
        });
        cache.index.insert(key2, 1);

        cache.save(tmp.to_str().unwrap()).unwrap();

        let mut loaded = make_test_cache();
        loaded.load(tmp.to_str().unwrap()).unwrap();

        assert_eq!(loaded.entry_count(), 2);
        assert_eq!(loaded.total_file_refs(), 7);

        // Check first entry
        let e0 = &loaded.entries[0];
        assert_eq!(e0.pattern, "struct");
        assert_eq!(e0.flags, 0);
        assert_eq!(e0.generation, 1);
        assert_eq!(e0.file_ids, vec![1, 5, 10, 42]);
        assert_eq!(e0.cached_at, 1234567890);

        // Check second entry
        let e1 = &loaded.entries[1];
        assert_eq!(e1.pattern, "main");
        assert_eq!(e1.flags, CACHE_FLAG_CASE_INSENSITIVE);
        assert_eq!(e1.file_ids, vec![0, 3, 7]);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_compact() {
        let mut cache = make_test_cache();

        // Add entries with different generations
        for gen in [1, 1, 2, 2, 3] {
            let key = CacheKey {
                pattern: format!("p{}", gen),
                flags: 0,
                generation: gen,
            };
            let idx = cache.entries.len();
            cache.entries.push(CacheEntry {
                pattern: key.pattern.clone(),
                flags: 0,
                generation: gen,
                file_ids: vec![1, 2],
                cached_at: 0,
            });
            cache.index.insert(key, idx);
        }

        assert_eq!(cache.entry_count(), 5);
        let removed = cache.compact(2);
        assert_eq!(removed, 3);
        assert_eq!(cache.entry_count(), 2);
        assert!(cache.entries.iter().all(|e| e.generation == 2));
    }

    #[test]
    fn test_hash_lookup() {
        let mut cache = make_test_cache();

        let key = CacheKey {
            pattern: "test".to_string(),
            flags: 0x01,
            generation: 42,
        };
        cache.entries.push(CacheEntry {
            pattern: "test".to_string(),
            flags: 0x01,
            generation: 42,
            file_ids: vec![5, 10],
            cached_at: 0,
        });
        cache.index.insert(key, 0);

        // Lookup should find it
        let idx = cache.index.get(&CacheKey {
            pattern: "test".to_string(),
            flags: 0x01,
            generation: 42,
        });
        assert!(idx.is_some());
        assert_eq!(*idx.unwrap(), 0);

        // Wrong generation should miss
        let miss = cache.index.get(&CacheKey {
            pattern: "test".to_string(),
            flags: 0x01,
            generation: 99,
        });
        assert!(miss.is_none());
    }

    #[test]
    fn test_store_replaces_existing() {
        let mut cache = make_test_cache();

        // Store with empty file list — file IDs will be empty
        cache.store_results("foo", 0, 1, &["/a/b".to_string()]);
        assert_eq!(cache.entry_count(), 1);
        assert_eq!(cache.entries[0].file_ids.len(), 0);

        // Store again — should replace, not duplicate
        cache.store_results("foo", 0, 1, &["/c/d".to_string()]);
        assert_eq!(cache.entry_count(), 1);
    }
}
