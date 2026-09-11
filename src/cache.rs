// Phase 11: Cache management subcommands.
//
// Provides `tgrep cache status`, `tgrep cache precache`, `tgrep cache clear`,
// and `tgrep cache keystone-stats`.

use std::path::PathBuf;

use clap::Subcommand;

use crate::build::default_state_dir;
use crate::native::{
    self, CACHE_FLAG_CASE_INSENSITIVE, CACHE_FLAG_FIXED_STRINGS, CACHE_FLAG_WORD_REGEXP,
};

// ── Subcommand enum (used by main.rs) ──────────────────────────────

#[derive(Subcommand)]
pub enum CacheSubcommands {
    /// Show cache status (entries, size, patterns, hit rate)
    Status,
    /// Pre-populate cache by running a list of patterns
    Precache {
        /// Patterns to cache (or use --patterns-file)
        patterns: Vec<String>,
        /// Read patterns from a file (one per line)
        #[arg(long)]
        patterns_file: Option<String>,
        /// Root path to search (default: current directory)
        #[arg(long, default_value = ".")]
        root: String,
        /// Also run case-insensitive variants
        #[arg(long)]
        case_insensitive: bool,
        /// Also run word-boundary (-w) variants
        #[arg(long)]
        word_regexp: bool,
    },
    /// Clear the cache
    Clear,
    /// Compact cache (drop stale-generation entries)
    Compact,
    /// Show KEYSTONE performance statistics
    KeystoneStats,
    /// Live terminal dashboard (cache, CPU, progress)
    Dashboard {
        /// Refresh interval in seconds (default: 2)
        #[arg(long, default_value = "2")]
        interval: f64,
    },
}

// ── Helpers ─────────────────────────────────────────────────────────

fn cache_path() -> PathBuf {
    default_state_dir().join("search_cache.qsc")
}

fn progress_path() -> PathBuf {
    default_state_dir().join("precache_progress.json")
}

/// Write precache progress as JSON for the dashboard to read.
fn write_progress(done: usize, total: usize, elapsed_secs: f64, current_pattern: &str) {
    let path = progress_path();
    let remaining = if done > 0 && done < total {
        let per_item = elapsed_secs / done as f64;
        Some(per_item * (total - done) as f64)
    } else {
        None
    };
    let json = format!(
        r#"{{"done":{}, "total":{}, "elapsed_secs":{:.3}, "current_pattern":{:?}, "eta_secs":{}}}"#,
        done,
        total,
        elapsed_secs,
        current_pattern,
        remaining
            .map(|r| format!("{:.3}", r))
            .unwrap_or_else(|| "null".to_string())
    );
    let _ = std::fs::write(&path, json);
}

fn clear_progress() {
    let path = progress_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn flags_string(flags: i32) -> String {
    let mut parts = Vec::new();
    if flags & CACHE_FLAG_CASE_INSENSITIVE != 0 {
        parts.push("-i");
    }
    if flags & CACHE_FLAG_WORD_REGEXP != 0 {
        parts.push("-w");
    }
    if flags & CACHE_FLAG_FIXED_STRINGS != 0 {
        parts.push("-F");
    }
    if parts.is_empty() {
        "default".to_string()
    } else {
        parts.join(" ")
    }
}

// ── Subcommand dispatch ─────────────────────────────────────────────

pub fn run_cache_command(action: CacheSubcommands) {
    match action {
        CacheSubcommands::Status => show_status(),
        CacheSubcommands::Precache {
            patterns,
            patterns_file,
            root,
            case_insensitive,
            word_regexp,
        } => run_precache(patterns, patterns_file, root, case_insensitive, word_regexp),
        CacheSubcommands::Clear => clear_cache(),
        CacheSubcommands::Compact => compact_cache(),
        CacheSubcommands::KeystoneStats => show_keystone_stats(),
        CacheSubcommands::Dashboard { interval } => run_dashboard(interval),
    }
}

// ── Status ──────────────────────────────────────────────────────────

fn show_status() {
    let path = cache_path();
    let mut cache = match crate::cache_v2::SearchCacheV2::create() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tgrep: failed to create cache: {}", e);
            std::process::exit(1);
        }
    };

    if path.exists() {
        if let Err(e) = cache.load(path.to_str().unwrap_or("")) {
            eprintln!("tgrep: warning: cache load failed: {}", e);
        }
    }

    let entries = cache.get_entries();
    let total_file_refs: usize = entries.iter().map(|e| e.file_count()).sum();

    // Get current manifest generation
    let state_dir = crate::build::default_state_dir();
    let manifest = crate::store::load_manifest(&state_dir);
    let current_gen = manifest.as_ref().map(|m| m.generation as i64).unwrap_or(0);
    let stale_count = entries.iter().filter(|e| e.generation != current_gen).count();

    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║              tgrep Cache Status (v2)                     ║");
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  Cache file:    {:<40}║", path.display());
    if path.exists() {
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        println!("║  File size:     {:<40}║", format_size(size));
    } else {
        println!("║  File size:     (not persisted)                         ║");
    }
    println!("║  Entries:      {:<40}║", entries.len());
    println!("║  File refs:    {:<40}║", total_file_refs);
    println!("║  Current gen:  {:<40}║", current_gen);
    if stale_count > 0 {
        println!("║  Stale entries:{:<40}║", stale_count);
    }
    let file_count = cache.file_list().map(|f| f.files.len()).unwrap_or(0);
    println!("║  File list:    {:<40}║", format!("{} files indexed", file_count));
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  Pattern                              Flags  Gen  Files  ║");
    println!("╠══════════════════════════════════════════════════════════╣");

    for e in entries.iter().take(50) {
        let pat_display = if e.pattern.len() > 30 {
            format!("{}...", &e.pattern[..27])
        } else {
            e.pattern.clone()
        };
        let gen_marker = if e.generation != current_gen { "*" } else { " " };
        println!(
            "║  {:<36} {:<6} {:>3}{} {:<6}║",
            pat_display,
            flags_string(e.flags),
            e.generation,
            gen_marker,
            e.file_count()
        );
    }

    if entries.len() > 50 {
        println!("║  ... and {} more entries                               ║", entries.len() - 50);
    }

    println!("╚══════════════════════════════════════════════════════════╝");
    if stale_count > 0 {
        eprintln!("  * = stale generation (run 'tgrep cache compact' to clean)");
    }
}

// ── Precache ────────────────────────────────────────────────────────

fn run_precache(
    patterns: Vec<String>,
    patterns_file: Option<String>,
    root: String,
    case_insensitive: bool,
    word_regexp: bool,
) {
    // Gather patterns
    let mut all_patterns = patterns;

    if let Some(file) = patterns_file {
        match std::fs::read_to_string(&file) {
            Ok(content) => {
                for line in content.lines() {
                    let line = line.trim();
                    if !line.is_empty() && !line.starts_with('#') {
                        all_patterns.push(line.to_string());
                    }
                }
            }
            Err(e) => {
                eprintln!("tgrep: failed to read patterns file '{}': {}", file, e);
                std::process::exit(2);
            }
        }
    }

    if all_patterns.is_empty() {
        eprintln!("tgrep: no patterns given for precache");
        eprintln!("Usage: tgrep cache precache <pattern1> [pattern2 ...] [--root PATH]");
        eprintln!("       tgrep cache precache --patterns-file FILE [--root PATH]");
        std::process::exit(2);
    }

    // Build variant list: (pattern, flags)
    let mut variants: Vec<(String, i32)> = Vec::new();
    for p in &all_patterns {
        variants.push((p.clone(), 0)); // default case-sensitive
        if case_insensitive {
            variants.push((p.clone(), CACHE_FLAG_CASE_INSENSITIVE));
        }
        if word_regexp {
            variants.push((p.clone(), CACHE_FLAG_WORD_REGEXP));
        }
    }

    let total = variants.len();
    println!("tgrep: precaching {} pattern variants over '{}'", total, root);
    let bar_len = 40;
    print!("tgrep: ");

    let start = std::time::Instant::now();
    let mut done = 0;

    for (pattern, flags) in &variants {
        // Write progress for dashboard
        write_progress(done, total, start.elapsed().as_secs_f64(), pattern);

        // Run tgrep -l for this pattern (populates cache)
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.arg("-l").arg(pattern).arg(&root);
        if *flags & CACHE_FLAG_CASE_INSENSITIVE != 0 {
            cmd.arg("-i");
        }
        if *flags & CACHE_FLAG_WORD_REGEXP != 0 {
            cmd.arg("-w");
        }
        if *flags & CACHE_FLAG_FIXED_STRINGS != 0 {
            cmd.arg("-F");
        }
        // Inherit TGREP_STATE_DIR
        if let Ok(dir) = std::env::var("TGREP_STATE_DIR") {
            cmd.env("TGREP_STATE_DIR", dir);
        }
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        let _ = cmd.status();

        done += 1;
        let pct = (done * 100) / total;
        let filled = (done * bar_len) / total;
        print!(
            "\r[{}{}] {:>3}% ({}/{}) ",
            "#".repeat(filled),
            " ".repeat(bar_len - filled),
            pct,
            done,
            total
        );
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }

    let elapsed = start.elapsed();
    clear_progress();
    println!();
    println!(
        "tgrep: precached {} variants in {:.2}s ({:.0}ms/variant)",
        total,
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1000.0 / total as f64
    );

    // Show cache stats
    show_status();
}

// ── Clear ───────────────────────────────────────────────────────────

fn clear_cache() {
    let path = cache_path();
    if path.exists() {
        match std::fs::remove_file(&path) {
            Ok(_) => println!("tgrep: cache cleared ({})", path.display()),
            Err(e) => {
                eprintln!("tgrep: failed to clear cache: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        println!("tgrep: cache file does not exist (already empty)");
    }
}

// ── Compact ─────────────────────────────────────────────────────────

fn compact_cache() {
    let path = cache_path();
    let mut cache = match crate::cache_v2::SearchCacheV2::create() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tgrep: failed to create cache: {}", e);
            std::process::exit(1);
        }
    };

    if !path.exists() {
        println!("tgrep: no cache file to compact");
        return;
    }

    if let Err(e) = cache.load(path.to_str().unwrap_or("")) {
        eprintln!("tgrep: failed to load cache: {}", e);
        std::process::exit(1);
    }

    let before = cache.entry_count();
    let state_dir = crate::build::default_state_dir();
    let manifest = crate::store::load_manifest(&state_dir);
    let current_gen = manifest.as_ref().map(|m| m.generation as i64).unwrap_or(0);

    let removed = cache.compact(current_gen);

    if let Err(e) = cache.save(path.to_str().unwrap_or("")) {
        eprintln!("tgrep: failed to save compacted cache: {}", e);
        std::process::exit(1);
    }

    let after = cache.entry_count();
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!(
        "tgrep: compacted cache: {} → {} entries (removed {} stale, gen={})",
        before, after, removed, current_gen
    );
    println!("tgrep: cache file now {}", format_size(size));
}

// ── KEYSTONE stats ──────────────────────────────────────────────────

fn show_keystone_stats() {
    native::set_keystone_performance_tracking(true);

    let features = native::detect_cpu_features();
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
        feature_parts.join(" + ")
    };

    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║            KEYSTONE Performance Stats                   ║");
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  CPU features:    {:<38}║", feature_str);

    if let Some(stats) = native::get_keystone_performance_stats() {
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║  Search Statistics:                                     ║");
        println!("║    Total searches:    {:>10}                        ║", stats.total_searches);
        println!("║    Successful:        {:>10}                        ║", stats.successful_searches);
        println!("║    Success rate:       {:>9.2}%                       ║", stats.search_success_rate * 100.0);
        println!("║    Avg search time:    {:>9.2} ns                      ║", stats.avg_search_time_ns);
        println!("║    Speedup vs binary:  {:>9.2}x                       ║", stats.speedup_vs_binary);
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║  Memory:                                                ║");
        println!("║    Peak memory:       {:>10}                        ║", format_size(stats.peak_memory_usage as u64));
        println!("║    Avg memory:        {:>10}                        ║", format_size(stats.avg_memory_usage as u64));
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║  Anchors:                                               ║");
        println!("║    Learned:           {:>10}                        ║", stats.anchors_learned);
        println!("║    Pruned:            {:>10}                        ║", stats.anchors_pruned);
        println!("║    Vec efficiency:    {:>9.2}%                       ║", stats.vectorization_efficiency * 100.0);
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║  Timing:                                                ║");
        println!("║    Total time:        {:>10.3} ms                    ║", stats.total_time_ns as f64 / 1_000_000.0);
        println!("║    Search time:       {:>10.3} ms                    ║", stats.search_time_ns as f64 / 1_000_000.0);
        if stats.archive_bytes_read > 0 {
            println!("╠══════════════════════════════════════════════════════════╣");
            println!("║  Archive I/O:                                           ║");
            println!("║    Bytes read:        {:>10}                        ║", format_size(stats.archive_bytes_read));
            println!("║    Decompress time:   {:>10.3} ms                    ║", stats.archive_decompress_time_ns as f64 / 1_000_000.0);
            println!("║    Parse time:        {:>10.3} ms                    ║", stats.archive_parse_time_ns as f64 / 1_000_000.0);
            println!("║    Members searched:  {:>10}                        ║", stats.archive_members_searched);
        }
    } else {
        println!("║  (performance tracking not available)                   ║");
    }
    println!("╚══════════════════════════════════════════════════════════╝");
}

// ── Dashboard ───────────────────────────────────────────────────────

fn run_dashboard(interval: f64) {
    let interval_ms = (interval * 1000.0) as u64;
    let interval_ms = interval_ms.max(100);

    loop {
        // Clear screen
        print!("\x1b[2J\x1b[H");

        let path = cache_path();
        let mut cache = match crate::cache_v2::SearchCacheV2::create() {
            Ok(c) => c,
            Err(_) => {
                println!("tgrep: failed to create cache");
                std::process::exit(1);
            }
        };

        if path.exists() {
            let _ = cache.load(path.to_str().unwrap_or(""));
        }

        let entries = cache.get_entries();
        let total_file_refs: usize = entries.iter().map(|e| e.file_count()).sum();
        let file_count = cache.file_list().map(|f| f.files.len()).unwrap_or(0);

        let state_dir = crate::build::default_state_dir();
        let manifest = crate::store::load_manifest(&state_dir);
        let current_gen = manifest.as_ref().map(|m| m.generation as i64).unwrap_or(0);
        let stale_count = entries.iter().filter(|e| e.generation != current_gen).count();

        // KEYSTONE stats
        let features = native::detect_cpu_features();
        let mut feature_parts = Vec::new();
        if features & 0x01 != 0 { feature_parts.push("SSE4.2"); }
        if features & 0x02 != 0 { feature_parts.push("AVX2"); }
        if features & 0x04 != 0 { feature_parts.push("AVX512"); }
        let feature_str = if feature_parts.is_empty() { "scalar".to_string() } else { feature_parts.join(" + ") };

        let keystone_stats = native::get_keystone_performance_stats();

        // Progress file
        let progress_path = state_dir.join("precache_progress.json");
        let progress = if progress_path.exists() {
            std::fs::read_to_string(&progress_path).ok()
        } else {
            None
        };

        let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

        println!("╔══════════════════════════════════════════════════════════╗");
        println!("║           tgrep Cache Dashboard                         ║");
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║  Cache file:     {:<38}║", path.display());
        println!("║  File size:      {:<38}║", if file_size > 0 { format_size(file_size) } else { "(not persisted)".to_string() });
        println!("║  Entries:        {:<38}║", entries.len());
        println!("║  File refs:      {:<38}║", total_file_refs);
        println!("║  File list:      {:<38}║", format!("{} files", file_count));
        println!("║  Current gen:    {:<38}║", current_gen);
        if stale_count > 0 {
            println!("║  Stale entries:  {:<38}║", stale_count);
        }
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║  KEYSTONE:                                              ║");
        println!("║    CPU features:  {:<36}║", feature_str);
        if let Some(ref s) = keystone_stats {
            println!("║    Searches:      {:<36}║", s.total_searches);
            println!("║    Success rate:  {:<36}║", format!("{:.1}%", s.search_success_rate * 100.0));
            println!("║    Avg time:      {:<36}║", format!("{:.0} ns", s.avg_search_time_ns));
            println!("║    Peak memory:   {:<36}║", format_size(s.peak_memory_usage as u64));
        } else {
            println!("║    (stats not available)                                 ║");
        }
        println!("╠══════════════════════════════════════════════════════════╣");

        // Progress
        if let Some(ref prog) = progress {
            println!("║  Precache progress:                                     ║");
            println!("║    {}", prog);
            println!("║                                                         ║");
        } else {
            println!("║  No active precache                                      ║");
        }

        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║  Top entries:                                           ║");
        for e in entries.iter().take(10) {
            let pat_display = if e.pattern.len() > 30 {
                format!("{}...", &e.pattern[..27])
            } else {
                e.pattern.clone()
            };
            let gen_marker = if e.generation != current_gen { "*" } else { " " };
            println!("║    {:<34} {:<5} {:>3}{} {:>5}║", pat_display, flags_string(e.flags), e.generation, gen_marker, e.file_count());
        }
        if entries.is_empty() {
            println!("║    (empty)                                              ║");
        }
        println!("╚══════════════════════════════════════════════════════════╝");
        println!("  [Ctrl+C to exit]  Refresh: {:.1}s", interval);

        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
    }
}
