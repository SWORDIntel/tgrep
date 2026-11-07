use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::exit;

use grep_printer::{SummaryBuilder, SummaryKind};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::SearcherBuilder;

use crate::build::default_state_dir;
use crate::store::{self, SegmentReader};

// ── Public configuration ────────────────────────────────────────────

pub struct SearchConfig {
    pub pattern: String,
    pub paths: Vec<PathBuf>,
    pub fixed_strings: bool,
    pub smart_case: bool,
    pub ignore_case: bool,
    pub word_regexp: bool,
    pub files_with_matches: bool,
    pub line_number: bool,
    pub count: bool,
    pub quiet: bool,
    pub patterns: Vec<String>,
    pub explain: bool,
    pub indexed_only: bool,
    pub list_files: bool,
}

// ── Ripgrep config ──────────────────────────────────────────────────

/// Parsed settings from RIPGREP_CONFIG_PATH (or ~/.ripgreprc).
/// CLI flags override these defaults.
#[derive(Clone, Debug)]
struct RipgrepConfig {
    smart_case: bool,
    ignore_case: bool,
    hidden: bool,
    globs: Vec<String>,
    threads: usize,
}

impl Default for RipgrepConfig {
    fn default() -> Self {
        RipgrepConfig {
            smart_case: false,
            ignore_case: false,
            hidden: false,
            globs: Vec::new(),
            threads: 4,
        }
    }
}

/// Parse the ripgrep config file (one `--flag=value` per line).
fn parse_ripgrep_config() -> RipgrepConfig {
    let path = std::env::var("RIPGREP_CONFIG_PATH")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_default();
            format!("{}/.ripgreprc", home)
        });

    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return RipgrepConfig::default(),
    };

    parse_ripgrep_config_content(&content)
}

fn parse_ripgrep_config_content(content: &str) -> RipgrepConfig {
    let mut cfg = RipgrepConfig::default();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Parse --flag or --flag=value
        let (flag, value) = if let Some(eq_pos) = line.find('=') {
            (&line[..eq_pos], Some(&line[eq_pos + 1..]))
        } else {
            (line, None)
        };

        match flag {
            "--smart-case" | "-S" => {
                cfg.smart_case = true;
                cfg.ignore_case = false;
            }
            "--ignore-case" | "-i" => {
                cfg.ignore_case = true;
                cfg.smart_case = false;
            }
            "--case-sensitive" | "-s" => {
                cfg.smart_case = false;
                cfg.ignore_case = false;
            }
            "--hidden" => cfg.hidden = true,
            "--glob" => {
                if let Some(v) = value {
                    cfg.globs.push(v.to_string());
                }
            }
            "--threads" | "-j" => {
                if let Some(v) = value {
                    if let Ok(n) = v.parse() {
                        cfg.threads = n;
                    }
                }
            }
            _ => {} // Ignore unknown flags
        }
    }
    cfg
}

fn effective_case_mode(cli_smart: bool, cli_ignore: bool, config: &RipgrepConfig) -> (bool, bool) {
    if cli_smart || cli_ignore {
        (cli_smart, cli_ignore)
    } else {
        (config.smart_case, config.ignore_case)
    }
}

// ── Query plan ──────────────────────────────────────────────────────

pub enum QueryPlan {
    /// All literal trigrams were found in the index; we can intersect
    /// postings to get a candidate set.
    Indexed { grams: Vec<u32> },
    /// Whole-word search via hash index (O(1) token lookup).
    /// Only used when --word-regexp is set and the pattern is a single word.
    HashIndex { token: Vec<u8> },
    /// No usable literal trigrams (pattern too short, regex-only, etc.)
    /// or the search is case-insensitive (trigram index is case-sensitive).
    /// Fall back to streaming.
    Streaming,
}

/// Plan a query by extracting literal trigrams from the pattern.
///
/// For fixed-string patterns we extract trigrams directly. For regex
/// patterns we use `regex-syntax` to find literal substrings and extract
/// trigrams from the longest one. If no literal of length >= 3 exists,
/// we fall back to streaming.
///
/// If `case_insensitive` is true, we always use streaming because the
/// trigram index is case-sensitive.
pub fn plan_query(
    pattern: &str,
    fixed: bool,
    case_insensitive: bool,
    word_regexp: bool,
) -> QueryPlan {
    if case_insensitive {
        return QueryPlan::Streaming;
    }

    let literal = if fixed {
        pattern.to_string()
    } else {
        match extract_regex_literal(pattern) {
            Some(lit) => lit,
            None => return QueryPlan::Streaming,
        }
    };

    // Hash index fast path: if --word-regexp is set and the pattern is a
    // single word (all word characters), use the hash index for O(1) lookup.
    let exact_literal = fixed
        || regex_syntax::Parser::new()
            .parse(pattern)
            .ok()
            .is_some_and(|hir| matches!(hir.kind(), regex_syntax::hir::HirKind::Literal(_)));
    if word_regexp && exact_literal && is_single_word(&literal) && literal.len() >= 2 {
        return QueryPlan::HashIndex {
            token: literal.into_bytes(),
        };
    }

    if literal.len() < 3 {
        return QueryPlan::Streaming;
    }

    let grams = extract_unique_trigrams(literal.as_bytes());
    if grams.is_empty() {
        return QueryPlan::Streaming;
    }

    QueryPlan::Indexed { grams }
}

/// Check if a string is a single word (all ASCII word characters, no spaces).
fn is_single_word(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Find a mandatory exact-case literal in the parsed regex. An alternative
/// or optional branch cannot establish a required literal.
fn extract_regex_literal(pattern: &str) -> Option<String> {
    use regex_syntax::hir::{Hir, HirKind};
    fn mandatory(node: &Hir) -> Option<String> {
        match node.kind() {
            HirKind::Literal(lit) => String::from_utf8(lit.0.to_vec()).ok(),
            HirKind::Capture(capture) => mandatory(&capture.sub),
            HirKind::Repetition(repetition) if repetition.min > 0 => mandatory(&repetition.sub),
            HirKind::Concat(parts) => parts.iter().filter_map(mandatory).max_by_key(|s| s.len()),
            _ => None,
        }
    }
    mandatory(&regex_syntax::Parser::new().parse(pattern).ok()?)
}

/// Extract unique trigrams from a byte slice.
/// Each trigram is 3 consecutive bytes packed into a u32 as (b0<<16 | b1<<8 | b2).
pub fn extract_unique_trigrams(data: &[u8]) -> Vec<u32> {
    if data.len() < 3 {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for window in data.windows(3) {
        let gram = ((window[0] as u32) << 16) | ((window[1] as u32) << 8) | (window[2] as u32);
        if seen.insert(gram) {
            result.push(gram);
        }
    }
    result
}

/// A decoded uppercase literal proves smart-case matching is case-sensitive.
/// Syntax such as hex digits, Unicode property names and flags does not.
/// Class-only uppercase remains conservative: without proof, avoid raw-byte pruning.
fn smart_case_may_ignore_case(pattern: &str, fixed: bool) -> bool {
    if fixed {
        return !pattern.chars().any(char::is_uppercase);
    }
    fn has_uppercase_literal(ast: &regex_syntax::ast::Ast) -> bool {
        use regex_syntax::ast::Ast;
        match ast {
            Ast::Literal(literal) => literal.c.is_uppercase(),
            Ast::Repetition(repetition) => has_uppercase_literal(&repetition.ast),
            Ast::Group(group) => has_uppercase_literal(&group.ast),
            Ast::Alternation(alternation) => alternation.asts.iter().any(has_uppercase_literal),
            Ast::Concat(concat) => concat.asts.iter().any(has_uppercase_literal),
            _ => false,
        }
    }
    regex_syntax::ast::parse::Parser::new()
        .parse(pattern)
        .map(|ast| !has_uppercase_literal(&ast))
        .unwrap_or(true)
}

/// Snapshot legacy roots before opening the manifest. A rebuild binds its old
/// roots before changing progress roots; manifest-owned roots take precedence.
fn load_index_snapshot(
    state_dir: &Path,
    read_manifest: impl FnOnce(&Path) -> Option<store::Manifest>,
) -> (Option<store::Manifest>, Option<Vec<String>>) {
    let legacy = crate::build::read_build_metadata(state_dir)
        .ok()
        .and_then(|value| {
            let assumed_completed = match value.get("status") {
                None => true,
                Some(serde_json::Value::String(status)) if status == "completed" => false,
                _ => return None,
            };
            let roots = value
                .get("roots")?
                .as_array()?
                .iter()
                .map(|root| root.as_str().map(String::from))
                .collect::<Option<Vec<_>>>()?;
            Some((roots, assumed_completed))
        });
    let manifest = read_manifest(state_dir);
    let roots = manifest.as_ref().and_then(|manifest| {
        let (roots, assumed_completed) = match manifest.roots.as_ref() {
            Some(roots) => (roots, false),
            None => {
                let (roots, assumed) = legacy.as_ref()?;
                (roots, *assumed)
            }
        };
        if (roots.is_empty() && !manifest.segments.is_empty())
            || roots
                .iter()
                .any(|root| root.is_empty() || !Path::new(root).is_absolute())
        {
            return None;
        }
        if assumed_completed {
            eprintln!("tgrep: assuming legacy root-only metadata describes a completed build");
        }
        Some(roots.clone())
    });
    if manifest.is_some() && roots.is_none() {
        eprintln!("tgrep: index root metadata is unavailable or ambiguous");
    }
    (manifest, roots)
}

// ── Search entry point ──────────────────────────────────────────────

pub fn run_search(config: SearchConfig) {
    let mut patterns = vec![config.pattern.clone()];
    patterns.extend(config.patterns.iter().cloned());
    let pattern = patterns
        .iter()
        .map(|p| format!("(?:{})", p))
        .collect::<Vec<_>>()
        .join("|");
    let plan_pattern = if patterns.len() == 1 {
        patterns[0].as_str()
    } else {
        ""
    };

    // Default to current dir if no paths given
    let paths: Vec<PathBuf> = if config.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        config.paths.clone()
    };

    // Parse ripgrep config and merge with CLI flags (CLI takes precedence)
    let rg_cfg = parse_ripgrep_config();

    let (smart_case, ignore_case) =
        effective_case_mode(config.smart_case, config.ignore_case, &rg_cfg);
    // WalkBuilder::hidden(true) means "skip hidden files".
    // rg_cfg.hidden = true means user wants --hidden (include hidden files).
    // So we invert: include hidden files when rg_cfg.hidden is true.
    let walk_hidden = !rg_cfg.hidden;
    let globs = rg_cfg.globs.clone();
    let threads = rg_cfg.threads;

    // Determine if the search is effectively case-insensitive.
    // Smart-case examines decoded literals; syntax letters cannot prove exact case.
    let case_insensitive =
        ignore_case || (smart_case && smart_case_may_ignore_case(&pattern, config.fixed_strings));

    // Plan the query
    let plan = plan_query(
        plan_pattern,
        config.fixed_strings,
        case_insensitive,
        config.word_regexp,
    );

    // Load the manifest and open segments
    let state_dir = default_state_dir();
    let (manifest, root_snapshot) = load_index_snapshot(&state_dir, store::load_manifest);
    let can_use_index = root_snapshot.is_some();
    if config.indexed_only && !can_use_index {
        eprintln!(
            "tgrep: --indexed-only requires an index with a meaningful absolute root mapping"
        );
        exit(2);
    }
    let indexed_roots = root_snapshot.unwrap_or_default();
    let mut plan = if can_use_index {
        plan
    } else {
        QueryPlan::Streaming
    };
    let segments_dir = state_dir.join("segments");

    let mut readers: Vec<SegmentReader> = Vec::new();
    if let Some(manifest) = manifest.as_ref().filter(|_| can_use_index) {
        for name in &manifest.segments {
            match SegmentReader::open(&segments_dir.join(name)) {
                Ok(reader) => readers.push(reader),
                Err(error) => {
                    eprintln!("tgrep: failed to read index segment {}: {}", name, error);
                    if config.indexed_only {
                        exit(2);
                    }
                    readers.clear();
                    plan = QueryPlan::Streaming;
                    break;
                }
            }
        }
    }
    if readers.iter().any(|reader| {
        reader
            .iter_docs()
            .any(|doc| doc.root_id as usize >= indexed_roots.len())
    }) {
        eprintln!("tgrep: index records refer to unavailable root metadata");
        if config.indexed_only {
            exit(2);
        }
        readers.clear();
        plan = QueryPlan::Streaming;
    }

    // Phase 11: Search-result cache (v2: grouped entries with file IDs).
    // Query flags and generation scope candidate-ordering hints.
    // Filesystem traversal and content verification remain authoritative.
    let manifest_generation = manifest.as_ref().map(|m| m.generation as i64).unwrap_or(0);
    let cache_flags = crate::native::compute_cache_flags(
        case_insensitive,
        config.word_regexp,
        config.fixed_strings,
    );
    let cache_path = state_dir.join("search_cache.qsc");
    let mut cache = crate::cache_v2::SearchCacheV2::create().ok();
    if let Some(ref mut cache) = cache {
        let _ = cache.load(cache_path.to_str().unwrap_or(""));
    }

    // Cached paths are ordering hints; live traversal and verification determine results.
    let cached_hints = cache
        .as_ref()
        .and_then(|cache| cache.lookup(&pattern, cache_flags, manifest_generation))
        .unwrap_or_default();
    if config.explain && cache.is_some() {
        eprintln!(
            "tgrep: cache hints={} (gen={})",
            cached_hints.len(),
            manifest_generation
        );
    }

    if config.explain {
        let features = crate::native::detect_cpu_features();
        let mut feature_parts = Vec::new();
        if features & 0x01 != 0 {
            feature_parts.push("SSE4.2");
        }
        if features & 0x02 != 0 {
            feature_parts.push("AVX2");
        }
        if features & 0x04 != 0 {
            feature_parts.push("AVX512");
        }
        let feature_str = if feature_parts.is_empty() {
            "scalar".to_string()
        } else {
            feature_parts.join(" ")
        };
        eprintln!("tgrep: cpu_features = {}", feature_str);
        eprintln!(
            "tgrep: backend = auto ({} preferred)",
            if features & 0x01 != 0 {
                "SSE4.2"
            } else {
                "scalar"
            }
        );
        match &plan {
            QueryPlan::Indexed { grams } => {
                eprintln!("tgrep: plan = indexed ({} trigrams)", grams.len());
                for g in grams {
                    eprintln!("  {:06x}", g);
                }
                // Phase 10: Show optimization DB status in explain mode
                let opt_db_path = state_dir.join("optimization.qdb");
                let mut opt_db = crate::native::OptimizationDatabase::create(
                    1000,
                    Some(opt_db_path.to_str().unwrap_or("")),
                )
                .unwrap_or_else(|_| {
                    crate::native::OptimizationDatabase::create(1000, None).unwrap()
                });
                let total_postings: usize = readers
                    .iter()
                    .map(|r| {
                        grams
                            .iter()
                            .map(|g| r.read_postings(*g).len())
                            .sum::<usize>()
                    })
                    .sum();
                let sig = crate::native::compute_query_signature(grams, total_postings);
                if let Some(cfg) = opt_db.get_config(&sig, 5) {
                    eprintln!("tgrep: optimization = threads={} backend={} pipeline={} dims={} speedup={:.2}x samples={}",
                        cfg.optimal_threads, cfg.optimal_backend, cfg.best_pipeline,
                        cfg.optimal_dimensions, cfg.avg_speedup, cfg.samples);
                } else {
                    eprintln!(
                        "tgrep: optimization = no recommendation yet (learning, {} entries)",
                        opt_db.count()
                    );
                }
            }
            QueryPlan::HashIndex { token } => {
                eprintln!(
                    "tgrep: plan = hash_index (token: {})",
                    String::from_utf8_lossy(token)
                );
            }
            QueryPlan::Streaming => {
                if case_insensitive {
                    eprintln!("tgrep: plan = streaming (case-insensitive, trigram index is case-sensitive)");
                } else {
                    eprintln!("tgrep: plan = streaming (no safe index plan)");
                }
            }
        }
        return;
    }

    // Build the regex matcher
    let mut builder = RegexMatcherBuilder::new();
    builder
        .case_insensitive(ignore_case)
        .case_smart(smart_case)
        .fixed_strings(config.fixed_strings)
        .word(config.word_regexp)
        .multi_line(true);

    let matcher = match builder.build_many(&patterns) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("tgrep: invalid pattern: {}", e);
            exit(2);
        }
    };

    // Phase 10: Initialize the QIHSE optimization DB.
    // Records per-query performance and recommends optimal thread/backend
    // config for future searches with similar data signatures.
    let opt_db_path = state_dir.join("optimization.qdb");
    // Ensure the state directory exists so save() can write the file.
    let _ = std::fs::create_dir_all(&state_dir);
    let mut opt_db =
        crate::native::OptimizationDatabase::create(1000, Some(opt_db_path.to_str().unwrap_or("")))
            .unwrap_or_else(|e| {
                if config.explain {
                    eprintln!("tgrep: warning: optimization DB init failed: {}", e);
                }
                crate::native::OptimizationDatabase::create(1000, None).unwrap()
            });

    // Phase 10: Consult the optimization DB for recommended config.
    let opt_config = if let QueryPlan::Indexed { grams } = &plan {
        let total_postings: usize = readers
            .iter()
            .map(|r| {
                grams
                    .iter()
                    .map(|g| r.read_postings(*g).len())
                    .sum::<usize>()
            })
            .sum();
        let sig = crate::native::compute_query_signature(grams, total_postings);
        opt_db.get_config(&sig, 5)
    } else {
        None
    };

    if config.explain {
        if let Some(ref cfg) = opt_config {
            eprintln!("tgrep: optimization = threads={} backend={} pipeline={} dims={} speedup={:.2}x samples={}",
                cfg.optimal_threads, cfg.optimal_backend, cfg.best_pipeline,
                cfg.optimal_dimensions, cfg.avg_speedup, cfg.samples);
        } else {
            eprintln!("tgrep: optimization = no recommendation yet (learning)");
        }
    }

    let (visible_files, walk_errors) = match collect_all_files(&paths, walk_hidden, &globs, threads)
    {
        Ok(result) => result,
        Err(error) => {
            eprintln!("tgrep: {}", error);
            exit(2);
        }
    };
    let mut visible_map: std::collections::HashMap<PathBuf, Vec<PathBuf>> =
        std::collections::HashMap::new();
    for path in &visible_files {
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        visible_map.entry(canonical).or_default().push(path.clone());
    }

    // Compute the candidate set from indexed segments
    let indexed_candidates: Vec<(PathBuf, u64)> = match &plan {
        QueryPlan::Indexed { grams } => intersect_postings(&readers, grams, &indexed_roots),
        QueryPlan::HashIndex { token } => {
            // Prefer the mmap-backed QWI sidecar. For ordinary (>= 3 byte)
            // words, fall back directly to trigram postings instead of loading
            // legacy `.thi` hash tables into memory; those sidecars can be
            // hundreds of MiB. Very short tokens have no trigrams, so use the
            // legacy hash index only for that case before scanning unchanged
            // indexed paths.
            qihse_word_index_candidates(&readers, &segments_dir, token, &indexed_roots)
                .or_else(|| {
                    let grams = extract_unique_trigrams(token);
                    if grams.is_empty() {
                        hash_index_candidates(&readers, &segments_dir, token, &indexed_roots)
                    } else {
                        Some(intersect_postings(&readers, &grams, &indexed_roots))
                    }
                })
                .unwrap_or_else(|| {
                    let unchanged = collect_unchanged_indexed_paths(&readers, &indexed_roots);
                    visible_files
                        .iter()
                        .filter(|p| {
                            let canonical = fs::canonicalize(p).unwrap_or_else(|_| (*p).clone());
                            unchanged.contains(&canonical)
                        })
                        .map(|p| (p.clone(), 0))
                        .collect()
                })
        }
        QueryPlan::Streaming => Vec::new(),
    };

    // Build a set of ALL indexed file paths (not just candidates).
    // This lets us skip scanning indexed non-candidates — we know they
    // don't contain all trigrams, so they can't match the pattern.
    // Only needed for indexed plan with streaming fallback.
    let need_indexed_paths = !config.indexed_only
        && (matches!(plan, QueryPlan::Indexed { .. })
            || matches!(plan, QueryPlan::HashIndex { .. }))
        && !readers.is_empty()
        && !indexed_roots.is_empty();

    let indexed_paths: std::collections::HashSet<PathBuf> = if need_indexed_paths {
        collect_unchanged_indexed_paths(&readers, &indexed_roots)
    } else {
        std::collections::HashSet::new()
    };

    let indexed_candidates = expand_visible_aliases(
        indexed_candidates
            .into_iter()
            .map(|(path, _)| fs::canonicalize(&path).unwrap_or(path)),
        &visible_map,
    );
    let indexed_candidate_count = indexed_candidates.len();
    let mut all_files = indexed_candidates;
    if !config.indexed_only {
        all_files.extend(visible_files.into_iter().filter(|path| {
            let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            matches!(plan, QueryPlan::Streaming) || !indexed_paths.contains(&canonical)
        }));
    }
    let mut seen = std::collections::HashSet::new();
    all_files.retain(|path| seen.insert(path.as_os_str().to_os_string()));
    let hints: std::collections::HashSet<&str> = cached_hints.iter().map(String::as_str).collect();
    all_files.sort_by_key(|path| !hints.contains(path.to_string_lossy().as_ref()));

    // Debug: list files that would be searched
    if config.list_files {
        for f in &all_files {
            println!("{}", f.display());
        }
        eprintln!(
            "Total: {} files ({} indexed candidates)",
            all_files.len(),
            indexed_candidate_count
        );
        exit(0);
    }

    // Speculative file prefetch (SPECTRE-inspired Level 1):
    // The index "predicted" these files as candidates. Issue
    // posix_fadvise(WILLNEED) for ALL candidates immediately, before
    // any verification begins. While the kernel pulls pages into the
    // page cache, we proceed to set up grep_searcher and start
    // verification. By the time we read each file, it's likely already
    // in page cache. Wasted prefetch (files that don't match) is
    // harmless — the page cache evicts naturally.
    // Keep speculative prefetch opt-in until representative cold-storage
    // benchmarks show a benefit. The available ARC-warm trial was slower.
    if !all_files.is_empty() && speculative_prefetch_enabled() {
        speculative_prefetch(&all_files);
    }

    // Parallel search (only for files that need verification)
    let search_start = std::time::Instant::now();
    let (match_found, matched_paths, search_errors) = parallel_search(
        &all_files,
        &matcher,
        config.line_number,
        config.files_with_matches,
        config.count,
        config.quiet,
        threads,
    );
    let search_elapsed = search_start.elapsed();

    // Phase 11: Store results in cache for future lookups.
    // Only cache -l (files_with_matches) mode results.
    if config.files_with_matches {
        if let Some(ref mut cache) = cache {
            let path_strings: Vec<String> = matched_paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            cache.store_results(&pattern, cache_flags, manifest_generation, &path_strings);
            let _ = cache.save(cache_path.to_str().unwrap_or(""));
        }
    }

    // Phase 10: Record performance for this query signature.
    // Only for indexed queries where we have a meaningful data signature.
    if let QueryPlan::Indexed { grams } = &plan {
        let total_postings: usize = readers
            .iter()
            .map(|r| {
                grams
                    .iter()
                    .map(|g| r.read_postings(*g).len())
                    .sum::<usize>()
            })
            .sum();
        let sig = crate::native::compute_query_signature(grams, total_postings);
        // Speedup vs baseline (rough heuristic: indexed search vs full scan).
        // Confidence is 1.0 for exact-match verification.
        let speedup = if !all_files.is_empty() && indexed_candidate_count > 0 {
            (all_files.len() as f64) / (indexed_candidate_count.max(1) as f64)
        } else {
            1.0
        };
        let backend = if crate::native::detect_cpu_features() & 0x01 != 0 {
            1
        } else {
            0
        };
        opt_db.record(
            &sig,
            1, // pipeline: balanced
            grams.len(),
            speedup,
            1.0, // confidence
            threads as i32,
            backend,
        );
        // Save the DB so future searches benefit
        let _ = opt_db.save();
    }

    if config.explain {
        eprintln!(
            "tgrep: search took {:.3}ms",
            search_elapsed.as_secs_f64() * 1000.0
        );
    }

    // Match ripgrep exit codes: 0 = match found, 1 = no match, 2 = error
    if walk_errors || search_errors {
        if !(config.quiet && match_found) {
            exit(2);
        }
    }
    if match_found {
        exit(0);
    } else {
        exit(1);
    }
}

/// Result of searching a single file.
struct FileResult {
    _path: PathBuf,
    matched: bool,
    output: Vec<u8>,
    error: Option<String>,
}

/// Speculative file prefetch (SPECTRE-inspired Level 1).
///
/// Issues `posix_fadvise(FADV_WILLNEED)` for all candidate files
/// immediately after index intersection, before verification begins.
/// The kernel starts pulling these files into the page cache while
/// we proceed to set up grep_searcher and begin verification.
///
/// "Rollback" is free: if a file was speculatively prefetched but
/// doesn't match after verification, the prefetch was wasted I/O
/// but harmless. The page cache entry will be evicted naturally.
///
/// This overlaps I/O with computation, hiding disk latency behind
/// the index-trust metadata checks and grep_searcher setup.
fn speculative_prefetch(files: &[PathBuf]) {
    // POSIX_FADV_WILLNEED = 3
    const POSIX_FADV_WILLNEED: i32 = 3;

    for path in files {
        // Open O_RDONLY | O_NONBLOCK (don't block on open)
        let path_c = match std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Use a non-blocking open to avoid stalling on inaccessible files
        let fd = unsafe { libc::open(path_c.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
        if fd < 0 {
            continue;
        }

        // Advise the kernel we'll need this file's pages soon
        unsafe {
            libc::posix_fadvise(fd, 0, 0, POSIX_FADV_WILLNEED);
            libc::close(fd);
        }
    }
}

fn speculative_prefetch_enabled() -> bool {
    prefetch_setting_enabled(std::env::var_os("TGREP_PREFETCH").as_deref())
}

fn prefetch_setting_enabled(value: Option<&std::ffi::OsStr>) -> bool {
    value == Some(std::ffi::OsStr::new("1"))
}

/// Search files in parallel using scoped threads.
/// Returns (true if any match was found, list of matching file paths).
fn parallel_search(
    files: &[PathBuf],
    matcher: &grep_regex::RegexMatcher,
    line_number: bool,
    files_with_matches: bool,
    count: bool,
    quiet: bool,
    num_threads: usize,
) -> (bool, Vec<PathBuf>, bool) {
    if files.is_empty() {
        return (false, Vec::new(), false);
    }

    // For quiet mode, use atomic flag for early exit
    let found = std::sync::atomic::AtomicBool::new(false);
    let found_ref = &found;

    // Determine thread count (at least 1)
    let nthreads = num_threads.max(1).min(files.len());

    if nthreads == 1 {
        // Single-threaded fast path
        let results = search_chunk(
            files,
            matcher,
            line_number,
            files_with_matches,
            count,
            quiet,
            found_ref,
        );
        let matched_paths: Vec<PathBuf> = results
            .iter()
            .filter(|r| r.matched)
            .map(|r| r._path.clone())
            .collect();
        let errors = output_results(&results, files_with_matches, count, quiet);
        return (
            found.load(std::sync::atomic::Ordering::Relaxed),
            matched_paths,
            errors,
        );
    }

    // Split files into chunks
    let chunk_size = (files.len() + nthreads - 1) / nthreads;
    let chunks: Vec<&[PathBuf]> = files.chunks(chunk_size).collect();

    let results: Vec<Vec<FileResult>> = std::thread::scope(|s| {
        let handles: Vec<_> = chunks
            .iter()
            .map(|chunk| {
                s.spawn(move || {
                    search_chunk(
                        chunk,
                        matcher,
                        line_number,
                        files_with_matches,
                        count,
                        quiet,
                        found_ref,
                    )
                })
            })
            .collect();

        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    // Merge results in order (chunks are already in file order)
    let mut all_results: Vec<FileResult> = Vec::with_capacity(files.len());
    for chunk_results in results {
        all_results.extend(chunk_results);
    }

    let matched_paths: Vec<PathBuf> = all_results
        .iter()
        .filter(|r| r.matched)
        .map(|r| r._path.clone())
        .collect();

    let errors = output_results(&all_results, files_with_matches, count, quiet);
    (
        found.load(std::sync::atomic::Ordering::Relaxed),
        matched_paths,
        errors,
    )
}

/// Search a chunk of files in a single thread.
fn search_chunk(
    files: &[PathBuf],
    matcher: &grep_regex::RegexMatcher,
    line_number: bool,
    files_with_matches: bool,
    count: bool,
    quiet: bool,
    found: &std::sync::atomic::AtomicBool,
) -> Vec<FileResult> {
    let mut results = Vec::with_capacity(files.len());

    // Reuse a single Searcher for all files in this chunk
    let mut searcher = SearcherBuilder::new()
        .line_number(line_number)
        .binary_detection(grep_searcher::BinaryDetection::quit(b'\x00'))
        .build();

    for path in files {
        // Early exit for quiet mode
        if quiet && found.load(std::sync::atomic::Ordering::Relaxed) {
            results.push(FileResult {
                _path: path.clone(),
                matched: false,
                output: Vec::new(),
                error: None,
            });
            continue;
        }

        let mut output_buf: Vec<u8> = Vec::new();
        let mut error = None;

        let matched = if files_with_matches || count || quiet {
            // Use Summary printer writing to a buffer
            let kind = if quiet {
                SummaryKind::Quiet
            } else if count {
                SummaryKind::Count
            } else {
                SummaryKind::PathWithMatch
            };

            let mut printer = SummaryBuilder::new()
                .kind(kind)
                .build_no_color(&mut output_buf);

            let mut sink = printer.sink_with_path(matcher, path);
            let result = searcher.search_path(matcher, path, &mut sink);
            if let Err(e) = &result {
                error = Some(format!("{}: {}", path.display(), e));
            }
            if result.is_ok() {
                let m = sink.has_match();
                if m {
                    found.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                m
            } else {
                false
            }
        } else {
            // Use Standard printer writing to a buffer
            let mut printer = grep_printer::StandardBuilder::new().build_no_color(&mut output_buf);

            let mut sink = printer.sink_with_path(matcher, path);
            let result = searcher.search_path(matcher, path, &mut sink);
            if let Err(e) = &result {
                error = Some(format!("{}: {}", path.display(), e));
            }
            if result.is_ok() {
                let m = sink.has_match();
                if m {
                    found.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                m
            } else {
                false
            }
        };

        results.push(FileResult {
            _path: path.clone(),
            matched,
            output: output_buf,
            error,
        });
    }

    results
}

/// Output results in order.
fn output_results(
    results: &[FileResult],
    _files_with_matches: bool,
    count: bool,
    quiet: bool,
) -> bool {
    let mut errors = false;
    for result in results {
        if let Some(error) = &result.error {
            eprintln!("tgrep: {}", error);
            errors = true;
        }
    }
    if quiet {
        return errors;
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();

    for r in results {
        if r.matched || count {
            if let Err(error) = out.write_all(&r.output) {
                eprintln!("tgrep: stdout: {}", error);
                return true;
            }
        }
    }
    errors
}

// ── Posting intersection ────────────────────────────────────────────

/// Intersect posting lists across all segments for the given trigrams.
/// Returns a list of (file_path, byte_length) for candidate documents.
fn intersect_postings(
    readers: &[SegmentReader],
    grams: &[u32],
    indexed_roots: &[String],
) -> Vec<(PathBuf, u64)> {
    if readers.is_empty() || grams.is_empty() {
        return Vec::new();
    }

    let mut candidates: Vec<(PathBuf, u64)> = Vec::new();

    for reader in readers {
        // Read posting lists, but stop immediately if any trigram has zero postings in this segment
        let mut posting_lists: Vec<Vec<u32>> = Vec::with_capacity(grams.len());
        let mut any_empty = false;
        for &g in grams {
            let p = reader.read_postings(g);
            if p.is_empty() {
                any_empty = true;
                break;
            }
            posting_lists.push(p);
        }

        if any_empty {
            continue;
        }

        // Intersect: sort by length, then intersect smallest-first
        let mut sorted_lists: Vec<&Vec<u32>> = posting_lists.iter().collect();
        sorted_lists.sort_by_key(|l| l.len());

        let result = intersect_sorted(&sorted_lists);

        // Map doc IDs to paths (bulk lookup for efficiency)
        let docs = reader.get_docs_bulk(&result);
        for doc in docs.into_iter().flatten() {
            let root_idx = doc.root_id as usize;
            if root_idx < indexed_roots.len() {
                let root = PathBuf::from(&indexed_roots[root_idx]);
                use std::os::unix::ffi::OsStringExt;
                let full_path = root.join(std::ffi::OsString::from_vec(doc.path.to_vec()));
                candidates.push((full_path, doc.byte_length));
            }
        }
    }

    candidates
}

/// Look up whole-token candidates via the per-segment hash indexes.
/// Each segment has a sidecar `.thi` file containing the hash index.
/// Returns (path, byte_length) pairs for files containing the token.
fn hash_index_candidates(
    readers: &[SegmentReader],
    segments_dir: &Path,
    token: &[u8],
    indexed_roots: &[String],
) -> Option<Vec<(PathBuf, u64)>> {
    if readers.is_empty() || token.is_empty() {
        return None;
    }

    // Parallel load + search across segments to avoid serial I/O bottleneck.
    // Each segment's .thi file is loaded and searched independently.
    use std::sync::Mutex;
    use std::thread;

    let token_owned = token.to_vec();
    let segments_dir = segments_dir.to_path_buf();
    let indexed_roots: Vec<String> = indexed_roots.to_vec();

    // Collect (reader_index, hash_path) pairs that have hash index sidecars
    let work: Vec<(usize, std::path::PathBuf)> = readers
        .iter()
        .enumerate()
        .filter_map(|(i, reader)| {
            let seg_name = reader.segment_name();
            let hash_path = segments_dir.join(format!("{}.thi", seg_name));
            if hash_path.exists() {
                Some((i, hash_path))
            } else {
                None
            }
        })
        .collect();

    if work.len() != readers.len() || work.is_empty() {
        return None;
    }

    // Chunk work across threads (4 threads, like the search itself)
    let num_threads = 4usize.min(work.len());
    let chunk_size = (work.len() + num_threads - 1) / num_threads;
    let chunks: Vec<Vec<(usize, std::path::PathBuf)>> =
        work.chunks(chunk_size).map(|c| c.to_vec()).collect();

    let results = Mutex::new(Vec::new());
    let errors = Mutex::new(Vec::new());

    thread::scope(|s| {
        for chunk in &chunks {
            let results = &results;
            let errors = &errors;
            let readers = readers;
            let indexed_roots = &indexed_roots;
            let token_owned = &token_owned;
            s.spawn(move || {
                let mut local = Vec::new();
                for (reader_idx, hash_path) in chunk {
                    let hash_index = match crate::native::HashIndex::load(hash_path) {
                        Ok(h) => h,
                        Err(e) => {
                            errors.lock().unwrap().push(e);
                            continue;
                        }
                    };
                    let doc_ids = match hash_index.search_all(token_owned) {
                        Ok(ids) => ids,
                        Err(error) => {
                            errors.lock().unwrap().push(error);
                            continue;
                        }
                    };
                    if doc_ids.is_empty() {
                        continue;
                    }
                    let reader = &readers[*reader_idx];
                    let docs = reader.get_docs_bulk(&doc_ids);
                    if docs.iter().any(Option::is_none) {
                        errors
                            .lock()
                            .unwrap()
                            .push("word index refers to a missing document".to_string());
                        continue;
                    }
                    for doc in docs.into_iter().flatten() {
                        let root_idx = doc.root_id as usize;
                        if root_idx < indexed_roots.len() {
                            let root = PathBuf::from(&indexed_roots[root_idx]);
                            use std::os::unix::ffi::OsStringExt;
                            let full_path =
                                root.join(std::ffi::OsString::from_vec(doc.path.to_vec()));
                            local.push((full_path, doc.byte_length));
                        }
                    }
                }
                results.lock().unwrap().extend(local);
            });
        }
    });

    let errors = errors.into_inner().unwrap();
    let complete = errors.is_empty();
    for e in errors {
        eprintln!("tgrep: warning: hash index unavailable: {}", e);
    }

    if complete {
        Some(results.into_inner().unwrap())
    } else {
        None
    }
}

/// Look up whole-token candidates via the per-segment QIHSE B+ tree indexes.
/// Each segment has a sidecar `.qwi` file containing the persistent B+ tree.
/// Returns (path, byte_length) pairs for files containing the token.
///
/// This is the QIHSE-backed alternative to the KEYSTONE `.thi` hash index.
/// It is preferred when `.qwi` files are present because the B+ tree format
/// is more compact and supports prefix scans natively.
fn qihse_word_index_candidates(
    readers: &[SegmentReader],
    segments_dir: &Path,
    token: &[u8],
    indexed_roots: &[String],
) -> Option<Vec<(PathBuf, u64)>> {
    if readers.is_empty() || token.is_empty() {
        return None;
    }

    use std::sync::Mutex;
    use std::thread;

    let token_owned = token.to_vec();
    let segments_dir = segments_dir.to_path_buf();
    let indexed_roots: Vec<String> = indexed_roots.to_vec();

    // Collect (reader_index, qwi_path) pairs that have QIHSE word index sidecars
    let work: Vec<(usize, std::path::PathBuf)> = readers
        .iter()
        .enumerate()
        .filter_map(|(i, reader)| {
            let seg_name = reader.segment_name();
            let qwi_path = segments_dir.join(format!("{}.qwi", seg_name));
            if qwi_path.exists() {
                Some((i, qwi_path))
            } else {
                None
            }
        })
        .collect();

    if work.len() != readers.len() || work.is_empty() {
        return None;
    }

    let num_threads = 4usize.min(work.len());
    let chunk_size = (work.len() + num_threads - 1) / num_threads;
    let chunks: Vec<Vec<(usize, std::path::PathBuf)>> =
        work.chunks(chunk_size).map(|c| c.to_vec()).collect();

    let results = Mutex::new(Vec::new());
    let errors = Mutex::new(Vec::new());

    thread::scope(|s| {
        for chunk in &chunks {
            let results = &results;
            let errors = &errors;
            let readers = readers;
            let indexed_roots = &indexed_roots;
            let token_owned = &token_owned;
            s.spawn(move || {
                let mut local = Vec::new();
                for (reader_idx, qwi_path) in chunk {
                    let qwi = match crate::native::MmapWordIndex::load(qwi_path) {
                        Ok(h) => h,
                        Err(e) => {
                            errors.lock().unwrap().push(e);
                            continue;
                        }
                    };
                    // Hard cap: qwi.size() tracks the file's entry count, but the
                    // usable output is doc ids (u32). Bound the C-side buffer too.
                    let max_results = qwi.size().min(1 << 22).max(1);
                    let doc_ids_u64 = match qwi.search(token_owned, max_results) {
                        Ok(ids) => ids,
                        Err(error) => {
                            errors.lock().unwrap().push(error);
                            continue;
                        }
                    };
                    if doc_ids_u64.len() >= max_results
                        || doc_ids_u64.iter().any(|&id| id > u32::MAX as u64)
                    {
                        errors
                            .lock()
                            .unwrap()
                            .push("word-index results exceed supported bounds".to_string());
                        continue;
                    }
                    if doc_ids_u64.is_empty() {
                        continue;
                    }
                    let doc_ids: Vec<u32> = doc_ids_u64.into_iter().map(|d| d as u32).collect();
                    let reader = &readers[*reader_idx];
                    let docs = reader.get_docs_bulk(&doc_ids);
                    if docs.iter().any(Option::is_none) {
                        errors
                            .lock()
                            .unwrap()
                            .push("word index refers to a missing document".to_string());
                        continue;
                    }
                    for doc in docs.into_iter().flatten() {
                        let root_idx = doc.root_id as usize;
                        if root_idx < indexed_roots.len() {
                            let root = PathBuf::from(&indexed_roots[root_idx]);
                            use std::os::unix::ffi::OsStringExt;
                            let full_path =
                                root.join(std::ffi::OsString::from_vec(doc.path.to_vec()));
                            local.push((full_path, doc.byte_length));
                        }
                    }
                }
                results.lock().unwrap().extend(local);
            });
        }
    });

    let errors = errors.into_inner().unwrap();
    let complete = errors.is_empty();
    for e in errors {
        eprintln!("tgrep: warning: qihse word index load failed: {}", e);
    }

    if complete {
        Some(results.into_inner().unwrap())
    } else {
        None
    }
}

/// Intersect multiple sorted vectors, returning the intersection.
fn intersect_sorted(lists: &[&Vec<u32>]) -> Vec<u32> {
    if lists.is_empty() {
        return Vec::new();
    }
    if lists.len() == 1 {
        return lists[0].clone();
    }

    let mut result = lists[0].clone();
    for other in &lists[1..] {
        result = intersect_two(&result, other);
        if result.is_empty() {
            break;
        }
    }
    result
}

/// Intersect two sorted vectors with adaptive binary search for skewed list sizes
/// (inspired by KEYSTONE Phase 1 ks_intersect_u32_adaptive).
fn intersect_two(a: &[u32], b: &[u32]) -> Vec<u32> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    // Ensure `a` is the smaller list for skewed optimization
    if a.len() > b.len() {
        return intersect_two(b, a);
    }

    let mut result = Vec::with_capacity(a.len());

    // Skewed lists: binary search fast-path (like KEYSTONE ks_lower_bound_gallop_u32)
    if b.len() >= a.len() * 8 {
        let mut j = 0;
        for &val in a {
            if j >= b.len() {
                break;
            }
            match b[j..].binary_search(&val) {
                Ok(offset) => {
                    result.push(val);
                    j += offset + 1;
                }
                Err(offset) => {
                    j += offset;
                }
            }
        }
        return result;
    }

    // Balanced lists: linear two-pointer merge
    let mut i = 0;
    let mut j = 0;
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => {
                result.push(a[i]);
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    result
}

// Expand indexed identities to every visible spelling requested by the walk.
fn expand_visible_aliases(
    candidates: impl IntoIterator<Item = PathBuf>,
    visible: &std::collections::HashMap<PathBuf, Vec<PathBuf>>,
) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    candidates
        .into_iter()
        .flat_map(|path| visible.get(&path).into_iter().flatten().cloned())
        .filter(|path| seen.insert(path.as_os_str().to_os_string()))
        .collect()
}

// ── File collection ─────────────────────────────────────────────────

/// Collect all files under the given paths.
/// `hidden` controls whether hidden files are included (ripgrep --hidden).
/// `globs` applies ripgrep-style glob patterns (e.g. "!.git/*").
fn collect_all_files(
    paths: &[PathBuf],
    hidden: bool,
    globs: &[String],
    threads: usize,
) -> Result<(Vec<PathBuf>, bool), String> {
    let mut files = Vec::new();
    let mut errors = false;

    for root in paths {
        if root.is_file() {
            files.push(root.clone());
            continue;
        }

        let mut builder = ignore::WalkBuilder::new(root);
        builder
            .hidden(hidden)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .parents(true)
            .threads(threads);

        // Apply glob patterns from ripgrep config
        if !globs.is_empty() {
            let mut override_builder = ignore::overrides::OverrideBuilder::new(root);
            for glob in globs {
                override_builder.add(glob).map_err(|e| e.to_string())?;
            }
            builder.overrides(override_builder.build().map_err(|e| e.to_string())?);
        }

        let walker = builder.build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(error) => {
                    eprintln!("tgrep: {}", error);
                    errors = true;
                    continue;
                }
            };
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                files.push(entry.path().to_path_buf());
            }
        }
    }

    Ok((files, errors))
}

/// Collect files whose complete metadata still matches an indexed record.
/// Only these files can safely be excluded by a negative index result.
fn collect_unchanged_indexed_paths(
    readers: &[SegmentReader],
    indexed_roots: &[String],
) -> std::collections::HashSet<PathBuf> {
    let mut paths = std::collections::HashSet::new();
    for reader in readers {
        for doc in reader.iter_docs() {
            let root_idx = doc.root_id as usize;
            if root_idx < indexed_roots.len() {
                let root = PathBuf::from(&indexed_roots[root_idx]);
                use std::os::unix::ffi::OsStringExt;
                let full_path = root.join(std::ffi::OsString::from_vec(doc.path.to_vec()));
                use std::os::unix::fs::MetadataExt;
                if let Ok(meta) = fs::metadata(&full_path) {
                    if meta.len() == doc.byte_length
                        && meta.dev() == doc.device
                        && meta.ino() == doc.inode
                        && meta.mtime() * 1_000_000_000 + meta.mtime_nsec() == doc.mtime_ns
                        && meta.ctime() * 1_000_000_000 + meta.ctime_nsec() == doc.ctime_ns
                    {
                        paths.insert(fs::canonicalize(&full_path).unwrap_or(full_path));
                    }
                }
            }
        }
    }
    paths
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speculative_prefetch_is_opt_in() {
        assert!(!prefetch_setting_enabled(None));
        assert!(!prefetch_setting_enabled(Some(std::ffi::OsStr::new("0"))));
        assert!(!prefetch_setting_enabled(Some(std::ffi::OsStr::new(
            "true"
        ))));
        assert!(prefetch_setting_enabled(Some(std::ffi::OsStr::new("1"))));
    }

    #[test]
    fn test_extract_unique_trigrams() {
        let grams = extract_unique_trigrams(b"hello");
        // "hello" -> hel, ell, llo = 3 unique
        assert_eq!(grams.len(), 3);

        let grams = extract_unique_trigrams(b"abcabc");
        // abc, bca, cab, abc -> 3 unique
        assert_eq!(grams.len(), 3);
    }

    #[test]
    fn test_extract_unique_trigrams_short() {
        assert!(extract_unique_trigrams(b"ab").is_empty());
        assert!(extract_unique_trigrams(b"").is_empty());
    }

    #[test]
    fn test_plan_query_fixed() {
        let plan = plan_query("hello", true, false, false);
        match plan {
            QueryPlan::Indexed { grams } => assert_eq!(grams.len(), 3),
            _ => panic!("expected indexed plan"),
        }
    }

    #[test]
    fn test_plan_query_short() {
        let plan = plan_query("ab", true, false, false);
        assert!(matches!(plan, QueryPlan::Streaming));
    }

    #[test]
    fn test_plan_query_case_insensitive() {
        // Case-insensitive search should always stream
        let plan = plan_query("hello", true, true, false);
        assert!(matches!(plan, QueryPlan::Streaming));
    }

    #[test]
    fn test_plan_query_regex() {
        let plan = plan_query("foo.*bar", false, false, false);
        match plan {
            QueryPlan::Indexed { grams } => assert!(!grams.is_empty()),
            _ => panic!("expected indexed plan for foo.*bar"),
        }
    }

    #[test]
    fn test_plan_query_regex_no_literal() {
        let plan = plan_query(".+", false, false, false);
        assert!(matches!(plan, QueryPlan::Streaming));
    }

    #[test]
    fn test_plan_query_word_regexp() {
        // Whole-word search should use hash index for single-word patterns
        let plan = plan_query("hello", true, false, true);
        match plan {
            QueryPlan::HashIndex { token } => assert_eq!(token, b"hello"),
            _ => panic!("expected hash_index plan"),
        }
    }

    #[test]
    fn test_plan_query_word_regexp_multiword() {
        // Multi-word patterns should not use hash index
        let plan = plan_query("hello world", true, false, true);
        match plan {
            QueryPlan::Indexed { .. } => {}
            _ => panic!("expected indexed plan for multiword"),
        }
    }

    #[test]
    fn test_intersect_two() {
        let a = vec![1, 3, 5, 7, 9];
        let b = vec![2, 3, 5, 8, 9];
        let result = intersect_two(&a, &b);
        assert_eq!(result, vec![3, 5, 9]);
    }

    #[test]
    fn test_intersect_two_empty() {
        let a: Vec<u32> = vec![];
        let b = vec![1, 2, 3];
        assert!(intersect_two(&a, &b).is_empty());
    }

    #[test]
    fn test_intersect_two_skewed() {
        // Skewed ratio >= 8 triggers binary search path
        let small = vec![5, 42, 99, 500, 1000];
        let large: Vec<u32> = (0..1000).collect();
        let result1 = intersect_two(&small, &large);
        let result2 = intersect_two(&large, &small);
        assert_eq!(result1, vec![5, 42, 99, 500]);
        assert_eq!(result2, vec![5, 42, 99, 500]);
    }

    #[test]
    fn test_intersect_sorted_multi() {
        let a = vec![1, 2, 3, 4, 5];
        let b = vec![2, 3, 5];
        let c = vec![3, 5];
        let lists: Vec<&Vec<u32>> = vec![&a, &b, &c];
        let result = intersect_sorted(&lists);
        assert_eq!(result, vec![3, 5]);
    }

    #[test]
    fn test_intersect_sorted_single() {
        let a = vec![1, 2, 3];
        let lists: Vec<&Vec<u32>> = vec![&a];
        let result = intersect_sorted(&lists);
        assert_eq!(result, vec![1, 2, 3]);
    }

    #[test]
    fn test_extract_regex_literal_simple() {
        let lit = extract_regex_literal("hello");
        assert_eq!(lit, Some("hello".to_string()));
    }

    #[test]
    fn test_extract_regex_literal_with_meta() {
        let lit = extract_regex_literal("foo.*bar");
        assert!(lit.is_some());
        assert!(lit.as_ref().unwrap().len() >= 3);
    }

    #[test]
    fn test_extract_regex_literal_no_literal() {
        let lit = extract_regex_literal(".");
        assert!(lit.is_none() || lit.as_ref().map_or(true, |s| s.is_empty()));
    }

    #[test]
    fn test_smart_case_literal_proof() {
        assert!(smart_case_may_ignore_case("hello", false));
        assert!(smart_case_may_ignore_case("hello world 123", false));
        assert!(!smart_case_may_ignore_case("Hello", false));
        assert!(!smart_case_may_ignore_case("helloWorld", false));
    }
}

#[cfg(test)]
mod audit_tests {
    use super::*;
    #[test]
    fn alternation_cannot_require_only_one_branch() {
        assert!(matches!(
            plan_query("foo|bar", false, false, false),
            QueryPlan::Streaming
        ));
    }
    #[test]
    fn inline_case_folding_cannot_require_lowercase_trigram() {
        assert!(matches!(
            plan_query("(?i)foo", false, false, false),
            QueryPlan::Streaming
        ));
    }
    #[test]
    fn word_regex_is_not_a_whole_token() {
        assert!(!matches!(
            plan_query("foo.*bar", false, false, true),
            QueryPlan::HashIndex { .. }
        ));
    }
    #[test]
    fn optional_branch_does_not_restrict_matches() {
        assert!(matches!(
            plan_query("(?:longliteral)?x", false, false, false),
            QueryPlan::Streaming
        ));
    }
}

#[cfg(test)]
mod manifest_root_tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Barrier,
    };

    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);
    struct Fixture {
        state: PathBuf,
        old: Vec<String>,
        new: Vec<String>,
    }
    impl Fixture {
        fn new() -> Self {
            let state = std::env::temp_dir().join(format!(
                "hax-search-root-map-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&state).unwrap();
            let old = vec![
                state.join("old first").to_string_lossy().into_owned(),
                state.join("old-second").to_string_lossy().into_owned(),
            ];
            let new = vec![
                state.join("new-second").to_string_lossy().into_owned(),
                state.join("new first").to_string_lossy().into_owned(),
            ];
            Self { state, old, new }
        }
        fn progress(&self, roots: &[String]) {
            fs::write(
                self.state.join("build.json"),
                serde_json::to_vec(&serde_json::json!({"roots": roots, "status": "completed"}))
                    .unwrap(),
            )
            .unwrap();
        }
        fn manifest(&self, roots: Option<Vec<String>>) {
            store::write_manifest(
                &self.state,
                &store::Manifest {
                    generation: 7,
                    segments: vec!["seg_00000007.tgs".to_string()],
                    profile_hash: 91,
                    roots,
                },
            )
            .unwrap();
        }
        fn migrate(&self) {
            let current = store::load_manifest(&self.state).unwrap();
            store::write_manifest(
                &self.state,
                &store::Manifest {
                    generation: current.generation,
                    segments: current.segments,
                    profile_hash: current.profile_hash,
                    roots: Some(self.old.clone()),
                },
            )
            .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.state);
        }
    }

    #[test]
    fn delayed_legacy_reader_uses_roots_captured_before_manifest() {
        let fixture = Fixture::new();
        fixture.progress(&fixture.old);
        fixture.manifest(None);
        let reader_ready = Barrier::new(2);
        let writer_done = Barrier::new(2);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                reader_ready.wait();
                fixture.migrate();
                fixture.progress(&fixture.new);
                writer_done.wait();
            });
            let (snapshot, roots) = load_index_snapshot(&fixture.state, |state| {
                let snapshot = store::load_manifest(state);
                reader_ready.wait();
                writer_done.wait();
                snapshot
            });
            let snapshot = snapshot.unwrap();
            assert!(snapshot.roots.is_none());
            assert_eq!(snapshot.generation, 7);
            assert_eq!(roots, Some(fixture.old.clone()));
            assert_eq!(
                store::load_manifest(&fixture.state).unwrap().roots,
                Some(fixture.old.clone())
            );
        });
    }

    #[test]
    fn bound_old_roots_override_new_progress_roots() {
        let fixture = Fixture::new();
        fixture.manifest(Some(fixture.old.clone()));
        fixture.progress(&fixture.new);
        let (snapshot, roots) = load_index_snapshot(&fixture.state, store::load_manifest);
        assert_eq!(snapshot.unwrap().profile_hash, 91);
        assert_eq!(roots, Some(fixture.old.clone()));
    }

    #[test]
    fn final_manifest_roots_override_delayed_old_legacy_snapshot() {
        let fixture = Fixture::new();
        fixture.progress(&fixture.old);
        fixture.manifest(None);
        let (snapshot, roots) = load_index_snapshot(&fixture.state, |state| {
            fixture.migrate();
            fixture.progress(&fixture.new);
            store::publish_manifest_with_roots(
                state,
                &["seg_00000008.tgs".to_string()],
                8,
                &fixture.new,
            )
            .unwrap();
            let snapshot = store::load_manifest(state);
            fs::write(state.join("build.json"), serde_json::to_vec(&serde_json::json!({
                "roots": [fixture.state.join("third-root").to_string_lossy()], "status": "running"
            })).unwrap()).unwrap();
            snapshot
        });
        let snapshot = snapshot.unwrap();
        assert_eq!(snapshot.generation, 8);
        assert_eq!(snapshot.segments, vec!["seg_00000008.tgs".to_string()]);
        assert_eq!(roots, Some(fixture.new.clone()));
    }

    #[test]
    fn empty_bound_roots_are_authoritative_not_legacy_fallback() {
        let fixture = Fixture::new();
        fixture.progress(&fixture.old);
        fixture.manifest(Some(Vec::new()));
        let (_, roots) = load_index_snapshot(&fixture.state, store::load_manifest);
        assert!(roots.is_none());
    }

    #[test]
    fn bound_roots_work_without_progress_file() {
        let fixture = Fixture::new();
        fixture.manifest(Some(fixture.old.clone()));
        let (_, roots) = load_index_snapshot(&fixture.state, store::load_manifest);
        assert_eq!(roots, Some(fixture.old.clone()));
    }

    #[test]
    fn legacy_manifest_preserves_exact_root_order() {
        let fixture = Fixture::new();
        fixture.progress(&fixture.old);
        fixture.manifest(None);
        let (snapshot, roots) = load_index_snapshot(&fixture.state, store::load_manifest);
        assert!(snapshot.unwrap().roots.is_none());
        assert_eq!(roots, Some(fixture.old.clone()));
    }

    #[test]
    fn noncompleted_legacy_progress_is_unavailable() {
        let fixture = Fixture::new();
        fixture.manifest(None);
        for status in [
            serde_json::json!("running"),
            serde_json::json!("paused"),
            serde_json::json!("failed"),
            serde_json::Value::Null,
            serde_json::json!(7),
        ] {
            fs::write(
                fixture.state.join("build.json"),
                serde_json::to_vec(&serde_json::json!({"roots": fixture.old, "status": status}))
                    .unwrap(),
            )
            .unwrap();
            let (_, roots) = load_index_snapshot(&fixture.state, store::load_manifest);
            assert!(roots.is_none());
        }
    }
    #[test]
    fn missing_malformed_or_relative_legacy_roots_are_unavailable() {
        let fixture = Fixture::new();
        fixture.manifest(None);
        for value in [
            serde_json::json!({}),
            serde_json::json!({"roots": null}),
            serde_json::json!({"roots": [7]}),
            serde_json::json!({"roots": [""]}),
            serde_json::json!({"roots": ["relative-root"]}),
        ] {
            fs::write(
                fixture.state.join("build.json"),
                serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
            let (_, roots) = load_index_snapshot(&fixture.state, store::load_manifest);
            assert!(roots.is_none());
        }
        fs::write(fixture.state.join("build.json"), b"{").unwrap();
        assert!(load_index_snapshot(&fixture.state, store::load_manifest)
            .1
            .is_none());
        fs::remove_file(fixture.state.join("build.json")).unwrap();
        assert!(load_index_snapshot(&fixture.state, store::load_manifest)
            .1
            .is_none());
    }
    #[test]
    fn relative_bound_roots_never_fall_back_to_legacy_or_cwd() {
        let fixture = Fixture::new();
        fixture.progress(&fixture.old);
        fixture.manifest(Some(vec!["relative-root".to_string()]));
        assert!(load_index_snapshot(&fixture.state, store::load_manifest)
            .1
            .is_none());
    }
    #[test]
    fn empty_root_map_is_valid_for_an_empty_segment_set() {
        let fixture = Fixture::new();
        fixture.progress(&fixture.old);
        store::write_manifest(
            &fixture.state,
            &store::Manifest {
                generation: 7,
                segments: Vec::new(),
                profile_hash: 91,
                roots: Some(Vec::new()),
            },
        )
        .unwrap();
        assert_eq!(
            load_index_snapshot(&fixture.state, store::load_manifest).1,
            Some(Vec::new())
        );
    }
    #[test]
    fn root_only_absolute_legacy_map_remains_compatible() {
        let fixture = Fixture::new();
        fixture.manifest(None);
        fs::write(
            fixture.state.join("build.json"),
            serde_json::to_vec(&serde_json::json!({"roots": fixture.old})).unwrap(),
        )
        .unwrap();
        assert_eq!(
            load_index_snapshot(&fixture.state, store::load_manifest).1,
            Some(fixture.old.clone())
        );
    }

    fn fifo_progress(fixture: &Fixture) {
        let path = std::ffi::CString::new(
            fixture
                .state
                .join("build.json")
                .as_os_str()
                .as_encoded_bytes(),
        )
        .unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    }

    #[test]
    fn fifo_snapshot_child() {
        let Some(state) = std::env::var_os("TGREP_SEARCH_TEST_FIFO_STATE") else {
            return;
        };
        let expected: Option<Vec<String>> =
            serde_json::from_str(&std::env::var("TGREP_SEARCH_TEST_FIFO_ROOTS").unwrap()).unwrap();
        let (snapshot, roots) = load_index_snapshot(Path::new(&state), store::load_manifest);
        assert_eq!(snapshot.unwrap().generation, 7);
        assert_eq!(roots, expected);
    }

    fn check_fifo_snapshot(bound: bool) {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let fixture = Fixture::new();
        let roots = bound.then(|| fixture.old.clone());
        fixture.manifest(roots.clone());
        fifo_progress(&fixture);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "search::manifest_root_tests::fifo_snapshot_child",
            ])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &fixture.state)
            .env("XDG_STATE_HOME", &fixture.state)
            .env("XDG_CONFIG_HOME", &fixture.state)
            .env("XDG_CACHE_HOME", &fixture.state)
            .env("XDG_DATA_HOME", &fixture.state)
            .env("XDG_RUNTIME_DIR", &fixture.state)
            .env("TGREP_STATE_DIR", &fixture.state)
            .env("TMPDIR", &fixture.state)
            .env("TGREP_SEARCH_TEST_FIFO_STATE", &fixture.state)
            .env(
                "TGREP_SEARCH_TEST_FIFO_ROOTS",
                serde_json::to_string(&roots).unwrap(),
            )
            .current_dir(&fixture.state)
            .stdin(Stdio::null())
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
                panic!("search build metadata FIFO waited for a writer");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn fifo_legacy_progress_cannot_supply_root_metadata() {
        check_fifo_snapshot(false);
    }

    #[test]
    fn bound_manifest_roots_remain_authoritative_with_fifo_progress() {
        check_fifo_snapshot(true);
    }
}

#[cfg(test)]
mod unicode_fold_planner_tests {
    use super::*;

    #[test]
    fn case_insensitive_queries_never_use_raw_byte_indexes() {
        for pattern in ["kilo", "kilogram", "longsword", "ssssss", r"\x6bilo"] {
            for fixed in [false, true] {
                for word in [false, true] {
                    assert!(matches!(
                        plan_query(pattern, fixed, true, word),
                        QueryPlan::Streaming
                    ));
                }
            }
        }
    }

    #[test]
    fn unicode_inline_casefold_classes_cannot_require_ascii_bytes() {
        for pattern in [
            "(?i)kilo",
            "(?i)kilogram",
            "(?i)longsword",
            "(?i)ssssss",
            "(?i:kilogram)",
            r"(?i)\x6bilo",
        ] {
            assert!(
                matches!(
                    plan_query(pattern, false, false, false),
                    QueryPlan::Streaming
                ),
                "{}",
                pattern
            );
        }
    }

    #[test]
    fn escaped_regex_literals_are_decoded_before_indexing() {
        for (pattern, literal) in [
            (r"\x6bilo", "kilo"),
            (r"kilo\.gram", "kilo.gram"),
            (r"kilo\(gram\)", "kilo(gram)"),
        ] {
            match plan_query(pattern, false, false, false) {
                QueryPlan::Indexed { grams } => {
                    assert_eq!(grams, extract_unique_trigrams(literal.as_bytes()))
                }
                _ => panic!("expected exact decoded literal for {}", pattern),
            }
        }
    }

    #[test]
    fn dot_alternation_and_optional_literals_never_require_a_branch() {
        for pattern in [
            "ki.o",
            "kilogram|longsword",
            "(?:kilogram)?x",
            "(?:kilogram|longsword)",
        ] {
            assert!(
                matches!(
                    plan_query(pattern, false, false, false),
                    QueryPlan::Streaming
                ),
                "{}",
                pattern
            );
        }
        assert_eq!(extract_regex_literal("kilo.gram").as_deref(), Some("gram"));
        assert_eq!(
            extract_regex_literal("(?i:k)ilo123456").as_deref(),
            Some("ilo123456")
        );
    }
}

#[cfg(test)]
mod smart_case_literal_tests {
    use super::*;

    #[test]
    fn syntax_uppercase_cannot_disable_smart_case_fold() {
        for pattern in [r"\x6Bilo", r"\u{006B}ilo", "(?U)kilo", r"kilo\p{Ll}?"] {
            let may_fold = smart_case_may_ignore_case(pattern, false);
            assert!(may_fold, "{}", pattern);
            assert!(matches!(
                plan_query(pattern, false, may_fold, false),
                QueryPlan::Streaming
            ));
        }
    }

    #[test]
    fn decoded_uppercase_literals_prove_exact_smart_case() {
        for pattern in [
            r"\x4Bilo",
            r"\u{004B}ilo",
            "Kilo",
            "Éllo",
            "(?:Kilo)?x",
            "Kilo|word",
        ] {
            assert!(!smart_case_may_ignore_case(pattern, false), "{}", pattern);
        }
        assert!(smart_case_may_ignore_case("[A-Z]kilo", false));
    }

    #[test]
    fn fixed_smart_case_uses_actual_unicode_characters() {
        assert!(!smart_case_may_ignore_case(r"\x6Bilo", true));
        assert!(!smart_case_may_ignore_case("Kilo", true));
        assert!(!smart_case_may_ignore_case("Éllo", true));
        assert!(smart_case_may_ignore_case("éllo", true));
    }
}

#[cfg(test)]
mod display_alias_tests {
    use super::*;
    #[test]
    fn indexed_identity_expands_every_distinct_display_spelling() {
        let canonical = PathBuf::from("/synthetic/root/a.txt");
        let alias = PathBuf::from("/synthetic/root/./a.txt");
        let visible = std::collections::HashMap::from([(
            canonical.clone(),
            vec![canonical.clone(), alias.clone()],
        )]);
        let paths = expand_visible_aliases([canonical.clone(), canonical.clone()], &visible);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].as_os_str(), canonical.as_os_str());
        assert_eq!(paths[1].as_os_str(), alias.as_os_str());
    }
}

#[cfg(test)]
mod configured_case_tests {
    use super::*;
    #[test]
    fn explicit_cli_case_mode_overrides_config_defaults() {
        let ignore = parse_ripgrep_config_content("--ignore-case\n");
        assert_eq!(effective_case_mode(true, false, &ignore), (true, false));
        let smart = parse_ripgrep_config_content("--smart-case\n");
        assert_eq!(effective_case_mode(false, true, &smart), (false, true));
        assert_eq!(effective_case_mode(false, false, &smart), (true, false));
    }

    #[test]
    fn final_case_flag_in_config_wins() {
        for (text, expected) in [
            ("-i\n-S\n", (true, false)),
            ("-S\n-i\n", (false, true)),
            ("--ignore-case\n--case-sensitive\n", (false, false)),
            ("-s\n--ignore-case\n", (false, true)),
        ] {
            let config = parse_ripgrep_config_content(text);
            assert_eq!(effective_case_mode(false, false, &config), expected);
        }
    }
}
