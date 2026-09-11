use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::exit;

use grep_printer::{SummaryBuilder, SummaryKind};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::SearcherBuilder;

use crate::build::{default_state_dir, load_file_list_cache, CachedFileMeta};
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
            "--smart-case" | "-S" => cfg.smart_case = true,
            "--ignore-case" | "-i" => cfg.ignore_case = true,
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
    if word_regexp && is_single_word(&literal) && literal.len() >= 2 {
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

/// Extract the longest literal substring from a regex pattern using
/// `regex-syntax`'s `Parser` and AST traversal.
fn extract_regex_literal(pattern: &str) -> Option<String> {
    use regex_syntax::ast::parse::Parser;
    use regex_syntax::ast::{Ast, LiteralKind};

    let mut parser = Parser::new();
    let ast = parser.parse(pattern).ok()?;

    let mut best: Option<String> = None;

    fn visit(node: &Ast, best: &mut Option<String>) {
        match node {
            Ast::Literal(lit) => {
                if lit.kind == LiteralKind::Verbatim {
                    let s = format!("{}", lit.c);
                    if s.len() > best.as_ref().map_or(0, |b| b.len()) {
                        *best = Some(s);
                    }
                }
            }
            Ast::Concat(concat) => {
                let mut current = String::new();
                for item in &concat.asts {
                    match item {
                        Ast::Literal(lit) if lit.kind == LiteralKind::Verbatim => {
                            current.push(lit.c);
                            continue;
                        }
                        _ => {}
                    }
                    if current.len() > best.as_ref().map_or(0, |s| s.len()) {
                        *best = Some(current.clone());
                    }
                    current.clear();
                    visit(item, best);
                }
                if current.len() > best.as_ref().map_or(0, |s| s.len()) {
                    *best = Some(current);
                }
            }
            Ast::Group(g) => visit(&g.ast, best),
            Ast::Alternation(alts) => {
                for alt in &alts.asts {
                    visit(alt, best);
                }
            }
            _ => {}
        }
    }

    visit(&ast, &mut best);
    best
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

/// Check if a pattern is all lowercase ASCII (for smart-case determination).
fn is_all_lowercase_ascii(s: &str) -> bool {
    s.chars().all(|c| !c.is_ascii_uppercase())
}

// ── Search entry point ──────────────────────────────────────────────

pub fn run_search(config: SearchConfig) {
    // Build the combined pattern: if multiple -e patterns, join with |
    let pattern = if config.patterns.is_empty() {
        config.pattern.clone()
    } else {
        let mut all = vec![config.pattern.clone()];
        all.extend(config.patterns.iter().cloned());
        all.join("|")
    };

    if pattern.is_empty() {
        eprintln!("tgrep: no pattern given");
        exit(2);
    }

    // Default to current dir if no paths given
    let paths: Vec<PathBuf> = if config.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        config.paths.clone()
    };

    // Parse ripgrep config and merge with CLI flags (CLI takes precedence)
    let rg_cfg = parse_ripgrep_config();

    let smart_case = config.smart_case || rg_cfg.smart_case;
    let ignore_case = config.ignore_case || rg_cfg.ignore_case;
    // WalkBuilder::hidden(true) means "skip hidden files".
    // rg_cfg.hidden = true means user wants --hidden (include hidden files).
    // So we invert: include hidden files when rg_cfg.hidden is true.
    let walk_hidden = !rg_cfg.hidden;
    let globs = rg_cfg.globs.clone();
    let threads = rg_cfg.threads;

    // Determine if the search is effectively case-insensitive.
    // Smart-case: case-insensitive when pattern is all lowercase ASCII.
    let case_insensitive = ignore_case || (smart_case && is_all_lowercase_ascii(&pattern));

    // Plan the query
    let plan = plan_query(
        &pattern,
        config.fixed_strings,
        case_insensitive,
        config.word_regexp,
    );

    // Load the manifest and open segments
    let state_dir = default_state_dir();
    let manifest = store::load_manifest(&state_dir);
    let segments_dir = state_dir.join("segments");

    let readers: Vec<SegmentReader> = match &manifest {
        Some(m) => m
            .segments
            .iter()
            .filter_map(|name| {
                let path = segments_dir.join(name);
                SegmentReader::open(&path).ok()
            })
            .collect(),
        None => Vec::new(),
    };

    // Phase 11: Search-result cache (v2: grouped entries with file IDs).
    // Cache key: (pattern, flags, manifest_generation).
    // Cache invalidation is automatic — a new build increments the
    // generation, so old entries don't match.
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

    // Cache lookup: only for -l (files_with_matches) mode, where we can
    // return cached file paths directly without re-verifying.
    if config.files_with_matches && cache.is_some() {
        if let Some(ref cache) = cache {
            if let Some(cached_paths) = cache.lookup(&pattern, cache_flags, manifest_generation) {
                if config.explain {
                    eprintln!(
                        "tgrep: cache HIT ({} files, gen={})",
                        cached_paths.len(),
                        manifest_generation
                    );
                }
                for p in &cached_paths {
                    println!("{}", p);
                }
                if !cached_paths.is_empty() {
                    exit(0);
                } else {
                    exit(1);
                }
            }
        }
    }
    if config.explain && cache.is_some() {
        eprintln!("tgrep: cache MISS (gen={})", manifest_generation);
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
                    eprintln!("tgrep: plan = streaming (no usable trigrams)");
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

    let matcher = match builder.build(&pattern) {
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

    // Determine which roots were indexed (stored in build.json).
    let indexed_roots: Vec<String> = {
        let build_path = state_dir.join("build.json");
        fs::read_to_string(&build_path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                v.get("roots").and_then(|r| r.as_array()).map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
            })
            .unwrap_or_default()
    };

    // Compute the candidate set from indexed segments
    let indexed_candidates: Vec<(PathBuf, u64)> = match &plan {
        QueryPlan::Indexed { grams } => intersect_postings(&readers, grams, &indexed_roots),
        QueryPlan::HashIndex { token } => {
            // Prefer QIHSE-backed B+ tree index (.qwi) when available;
            // fall back to KEYSTONE hash index (.thi) for legacy segments.
            let qwi = qihse_word_index_candidates(&readers, &segments_dir, token, &indexed_roots);
            if !qwi.is_empty() {
                qwi
            } else {
                hash_index_candidates(&readers, &segments_dir, token, &indexed_roots)
            }
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
        collect_all_indexed_paths(&readers, &indexed_roots)
    } else {
        std::collections::HashSet::new()
    };

    // Determine which files to stream-scan:
    // - If streaming plan: all files under the search paths
    // - If indexed plan: only files NOT in the index (truly unindexed/new)
    //   Indexed non-candidates are skipped (known non-matches).
    //   Uses file list cache to skip filesystem walk when possible.
    // - If indexed_only flag: skip streaming entirely
    let stream_files: Vec<PathBuf> = if config.indexed_only {
        Vec::new()
    } else {
        match &plan {
            QueryPlan::Streaming => collect_all_files(&paths, walk_hidden, &globs, threads),
            QueryPlan::Indexed { .. } | QueryPlan::HashIndex { .. } => {
                collect_unindexed_files_fast(
                    &paths,
                    &indexed_paths,
                    walk_hidden,
                    &globs,
                    threads,
                    &state_dir,
                )
            }
        }
    };

    // Filter indexed candidates to only those under the requested search paths.
    // This is needed when searching a subdirectory of an indexed root.
    let indexed_candidates: Vec<(PathBuf, u64)> = indexed_candidates
        .into_iter()
        .filter(|(p, _)| paths.iter().any(|search_path| p.starts_with(search_path)))
        .collect();

    // Combine all files to search: indexed candidates first, then stream files
    let all_files: Vec<PathBuf> = indexed_candidates
        .iter()
        .map(|(p, _)| p.clone())
        .chain(stream_files.into_iter())
        .collect();

    // Debug: list files that would be searched
    if config.list_files {
        for f in &all_files {
            println!("{}", f.display());
        }
        eprintln!(
            "Total: {} files ({} indexed candidates)",
            all_files.len(),
            indexed_candidates.len()
        );
        exit(0);
    }

    // Parallel search
    let search_start = std::time::Instant::now();
    let (match_found, matched_paths) = parallel_search(
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
        let speedup = if !all_files.is_empty() && !indexed_candidates.is_empty() {
            (all_files.len() as f64) / (indexed_candidates.len().max(1) as f64)
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
) -> (bool, Vec<PathBuf>) {
    if files.is_empty() {
        return (false, Vec::new());
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
        output_results(&results, files_with_matches, count, quiet);
        return (
            found.load(std::sync::atomic::Ordering::Relaxed),
            matched_paths,
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

    output_results(&all_results, files_with_matches, count, quiet);

    (
        found.load(std::sync::atomic::Ordering::Relaxed),
        matched_paths,
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
            });
            continue;
        }

        let mut output_buf: Vec<u8> = Vec::new();

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
            if searcher.search_path(matcher, path, &mut sink).is_ok() {
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
            if searcher.search_path(matcher, path, &mut sink).is_ok() {
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
        });
    }

    results
}

/// Output results in order.
fn output_results(results: &[FileResult], _files_with_matches: bool, count: bool, quiet: bool) {
    if quiet {
        return;
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();

    for r in results {
        if r.matched || count {
            let _ = out.write_all(&r.output);
        }
    }
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
        // Get posting lists for all grams in this segment
        let posting_lists: Vec<Vec<u32>> = grams.iter().map(|&g| reader.read_postings(g)).collect();

        // If any trigram has zero postings in this segment, no candidates here
        if posting_lists.iter().any(|p| p.is_empty()) {
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
                let rel = String::from_utf8_lossy(&doc.path);
                let full_path = root.join(rel.as_ref());
                candidates.push((full_path, doc.byte_length));
            } else {
                let path = PathBuf::from(String::from_utf8_lossy(&doc.path).to_string());
                candidates.push((path, doc.byte_length));
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
) -> Vec<(PathBuf, u64)> {
    if readers.is_empty() || token.is_empty() {
        return Vec::new();
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

    if work.is_empty() {
        return Vec::new();
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
                    let doc_ids = hash_index.search_all(token_owned);
                    if doc_ids.is_empty() {
                        continue;
                    }
                    let reader = &readers[*reader_idx];
                    let docs = reader.get_docs_bulk(&doc_ids);
                    for doc in docs.into_iter().flatten() {
                        let root_idx = doc.root_id as usize;
                        if root_idx < indexed_roots.len() {
                            let root = PathBuf::from(&indexed_roots[root_idx]);
                            let rel = String::from_utf8_lossy(&doc.path);
                            let full_path = root.join(rel.as_ref());
                            local.push((full_path, doc.byte_length));
                        } else {
                            let path =
                                PathBuf::from(String::from_utf8_lossy(&doc.path).to_string());
                            local.push((path, doc.byte_length));
                        }
                    }
                }
                results.lock().unwrap().extend(local);
            });
        }
    });

    for e in errors.into_inner().unwrap() {
        eprintln!("tgrep: warning: hash index load failed: {}", e);
    }

    results.into_inner().unwrap()
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
) -> Vec<(PathBuf, u64)> {
    if readers.is_empty() || token.is_empty() {
        return Vec::new();
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

    if work.is_empty() {
        return Vec::new();
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
                    // Cap results per segment to avoid unbounded allocation
                    let max_results = qwi.size().max(1024);
                    let doc_ids_u64 = qwi.search(token_owned, max_results);
                    if doc_ids_u64.is_empty() {
                        continue;
                    }
                    let doc_ids: Vec<u32> = doc_ids_u64
                        .into_iter()
                        .map(|d| d as u32)
                        .collect();
                    let reader = &readers[*reader_idx];
                    let docs = reader.get_docs_bulk(&doc_ids);
                    for doc in docs.into_iter().flatten() {
                        let root_idx = doc.root_id as usize;
                        if root_idx < indexed_roots.len() {
                            let root = PathBuf::from(&indexed_roots[root_idx]);
                            let rel = String::from_utf8_lossy(&doc.path);
                            let full_path = root.join(rel.as_ref());
                            local.push((full_path, doc.byte_length));
                        } else {
                            let path =
                                PathBuf::from(String::from_utf8_lossy(&doc.path).to_string());
                            local.push((path, doc.byte_length));
                        }
                    }
                }
                results.lock().unwrap().extend(local);
            });
        }
    });

    for e in errors.into_inner().unwrap() {
        eprintln!("tgrep: warning: qihse word index load failed: {}", e);
    }

    results.into_inner().unwrap()
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

/// Intersect two sorted vectors.
fn intersect_two(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut result = Vec::with_capacity(a.len().min(b.len()));
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

// ── File collection ─────────────────────────────────────────────────

/// Collect all files under the given paths.
/// `hidden` controls whether hidden files are included (ripgrep --hidden).
/// `globs` applies ripgrep-style glob patterns (e.g. "!.git/*").
fn collect_all_files(
    paths: &[PathBuf],
    hidden: bool,
    globs: &[String],
    threads: usize,
) -> Vec<PathBuf> {
    let mut files = Vec::new();

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
                let _ = override_builder.add(glob);
            }
            if let Ok(ov) = override_builder.build() {
                builder.overrides(ov);
            }
        }

        let walker = builder.build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                files.push(entry.path().to_path_buf());
            }
        }
    }

    files
}

/// Collect ALL indexed file paths from all segments.
/// This is used to skip scanning indexed non-candidates (files in the
/// index that don't match all trigrams — known non-matches).
fn collect_all_indexed_paths(
    readers: &[SegmentReader],
    indexed_roots: &[String],
) -> std::collections::HashSet<PathBuf> {
    let mut paths = std::collections::HashSet::new();
    for reader in readers {
        for doc in reader.iter_docs() {
            let root_idx = doc.root_id as usize;
            if root_idx < indexed_roots.len() {
                let root = PathBuf::from(&indexed_roots[root_idx]);
                let rel = String::from_utf8_lossy(&doc.path);
                let full_path = root.join(rel.as_ref());
                paths.insert(full_path);
            }
        }
    }
    paths
}

/// Check if a file's metadata matches the cached metadata.
fn metadata_matches(path: &Path, cached: &CachedFileMeta) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => {
            use std::os::unix::fs::MetadataExt;
            let mtime_ns = meta.mtime() * 1_000_000_000 + meta.mtime_nsec();
            meta.len() == cached.size && meta.ino() == cached.inode && mtime_ns == cached.mtime_ns
        }
        Err(_) => false,
    }
}

/// Try to load the file list cache and verify it's still valid.
/// Returns the list of all files if the cache is valid, or None if
/// the cache is missing, stale, or invalid (fall back to walking).
///
/// Validity check: verify a random sample of files (up to 20) still
/// exist and match their stored metadata. If any sample fails, the
/// cache is considered stale.
fn try_load_file_cache(
    state_dir: &Path,
    search_paths: &[PathBuf],
    _walk_hidden: bool,
) -> Option<Vec<PathBuf>> {
    let cache = load_file_list_cache(state_dir)?;

    // Check that the cache roots overlap with the search paths
    let cache_roots: Vec<PathBuf> = cache.roots.iter().map(PathBuf::from).collect();
    let search_matches_cache = search_paths.iter().any(|sp| {
        cache_roots
            .iter()
            .any(|cr| sp == cr || sp.starts_with(cr) || cr.starts_with(sp))
    });
    if !search_matches_cache {
        return None;
    }

    // If search paths are subdirectories of cache roots, filter the cache
    // to only include files under the search paths.
    let filtered: Vec<&CachedFileMeta> =
        if search_paths.len() == 1 && cache_roots.contains(&search_paths[0]) {
            // Exact root match — use all cached files
            cache.files.iter().collect()
        } else {
            // Filter to files under any search path
            cache
                .files
                .iter()
                .filter(|f| {
                    let p = PathBuf::from(&f.path);
                    search_paths.iter().any(|sp| p.starts_with(sp) || p == *sp)
                })
                .collect()
        };

    if filtered.is_empty() {
        return None;
    }

    // Validity check: sample up to 20 files
    let sample_size = filtered.len().min(20);
    let step = filtered.len() / sample_size.max(1);
    let mut checked = 0;
    let mut failed = 0;
    for i in (0..filtered.len()).step_by(step.max(1)) {
        if checked >= sample_size {
            break;
        }
        let f = &filtered[i];
        let path = PathBuf::from(&f.path);
        if !metadata_matches(&path, f) {
            failed += 1;
        }
        checked += 1;
    }

    if failed > 0 {
        eprintln!(
            "tgrep: file list cache stale ({} of {} samples changed), falling back to walk",
            failed, checked
        );
        return None;
    }

    // Cache is valid — return the file list
    let files: Vec<PathBuf> = filtered.iter().map(|f| PathBuf::from(&f.path)).collect();
    eprintln!(
        "tgrep: using cached file list ({} files, verified {} samples)",
        files.len(),
        checked
    );
    Some(files)
}

/// Collect unindexed files using the file list cache if available,
/// falling back to filesystem walk if the cache is missing or stale.
fn collect_unindexed_files_fast(
    paths: &[PathBuf],
    indexed_paths: &std::collections::HashSet<PathBuf>,
    walk_hidden: bool,
    globs: &[String],
    threads: usize,
    state_dir: &Path,
) -> Vec<PathBuf> {
    // Try the file list cache first
    if let Some(cached_files) = try_load_file_cache(state_dir, paths, walk_hidden) {
        // Filter out indexed files
        return cached_files
            .into_iter()
            .filter(|p| !indexed_paths.contains(p))
            .collect();
    }

    // Fall back to walking the filesystem
    let all_files = collect_all_files(paths, walk_hidden, globs, threads);
    all_files
        .into_iter()
        .filter(|p| !indexed_paths.contains(p))
        .collect()
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_is_all_lowercase_ascii() {
        assert!(is_all_lowercase_ascii("hello"));
        assert!(is_all_lowercase_ascii("hello world 123"));
        assert!(!is_all_lowercase_ascii("Hello"));
        assert!(!is_all_lowercase_ascii("helloWorld"));
    }
}
