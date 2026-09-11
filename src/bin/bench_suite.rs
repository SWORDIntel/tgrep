// tgrep Phase 11: Full benchmark suite runner
//
// Runs tgrep vs rg across a comprehensive pattern set, with configurable
// trials per pattern, warm/cold cache separation, and summary reporting.
//
// Usage:
//   ./target/release/bench_suite <path> [trials] [--indexed-only] [--cold-cache]
//   ./target/release/bench_suite /rpool/scratch/tgrep_corpus 10
//   ./target/release/bench_suite /rpool/scratch/tgrep_corpus 10 --indexed-only

use std::env;
use std::process::Command;
use std::time::Instant;

/// A benchmark pattern with its category.
#[derive(Clone)]
struct BenchPattern {
    pattern: &'static str,
    category: PatternCategory,
    /// Extra args to pass to tgrep (e.g., ["-w"] for word search)
    tgrep_args: &'static [&'static str],
    /// Extra args to pass to rg (must match tgrep semantics)
    rg_args: &'static [&'static str],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PatternCategory {
    /// Rare or absent pattern — tgrep should be 10x+ faster
    Rare,
    /// Broad/common pattern — tgrep should be within 10% of rg
    Broad,
    /// Word-search pattern — uses hash index fast path
    Word,
    /// Case-insensitive pattern — falls back to streaming
    CaseInsensitive,
}

impl PatternCategory {
    fn label(&self) -> &'static str {
        match self {
            PatternCategory::Rare => "rare",
            PatternCategory::Broad => "broad",
            PatternCategory::Word => "word",
            PatternCategory::CaseInsensitive => "case-insensitive",
        }
    }

    /// Target speedup for this category (tgrep_med / rg_med).
    /// < 1.0 means tgrep is faster. 1.0 means parity.
    fn target(&self) -> f64 {
        match self {
            PatternCategory::Rare => 0.1,            // 10x faster
            PatternCategory::Broad => 1.1,           // within 10% of rg
            PatternCategory::Word => 0.01,           // 100x faster (hash index)
            PatternCategory::CaseInsensitive => 2.0, // slower expected (streaming)
        }
    }
}

/// The full benchmark pattern set.
/// 10 history-derived patterns + 6 known patterns + case-insensitive.
/// NOTE: Rare/broad patterns use mixed-case to stay case-sensitive (smart-case
/// triggers case-insensitive mode, which falls back to streaming since the
/// trigram index is case-sensitive).
fn default_patterns() -> Vec<BenchPattern> {
    vec![
        // ── Rare/absent patterns (should be 10x+ faster) ──
        // Mixed-case to stay case-sensitive and use the trigram index.
        BenchPattern {
            pattern: "RareNeedle",
            category: PatternCategory::Rare,
            tgrep_args: &[],
            rg_args: &[],
        },
        BenchPattern {
            pattern: "KeystoneTrigram",
            category: PatternCategory::Rare,
            tgrep_args: &[],
            rg_args: &[],
        },
        BenchPattern {
            pattern: "NonexistentXyz123",
            category: PatternCategory::Rare,
            tgrep_args: &[],
            rg_args: &[],
        },
        BenchPattern {
            pattern: "QihseOptimization",
            category: PatternCategory::Rare,
            tgrep_args: &[],
            rg_args: &[],
        },
        BenchPattern {
            pattern: "DsmilHashIndex",
            category: PatternCategory::Rare,
            tgrep_args: &[],
            rg_args: &[],
        },
        BenchPattern {
            pattern: "TgrepSegmentWriter",
            category: PatternCategory::Rare,
            tgrep_args: &[],
            rg_args: &[],
        },
        BenchPattern {
            pattern: "AtomicWriteFsync",
            category: PatternCategory::Rare,
            tgrep_args: &[],
            rg_args: &[],
        },
        BenchPattern {
            pattern: "WalCheckpointReplay",
            category: PatternCategory::Rare,
            tgrep_args: &[],
            rg_args: &[],
        },
        // ── Broad/common patterns (should be within 10% of rg) ──
        // Mixed-case to stay case-sensitive.
        BenchPattern {
            pattern: "Struct",
            category: PatternCategory::Broad,
            tgrep_args: &[],
            rg_args: &[],
        },
        BenchPattern {
            pattern: "Fn Main",
            category: PatternCategory::Broad,
            tgrep_args: &[],
            rg_args: &[],
        },
        BenchPattern {
            pattern: "Return",
            category: PatternCategory::Broad,
            tgrep_args: &[],
            rg_args: &[],
        },
        BenchPattern {
            pattern: "Use Std",
            category: PatternCategory::Broad,
            tgrep_args: &[],
            rg_args: &[],
        },
        // ── Word-search patterns (hash index fast path) ──
        BenchPattern {
            pattern: "Terminal",
            category: PatternCategory::Word,
            tgrep_args: &["-w"],
            rg_args: &["-w"],
        },
        BenchPattern {
            pattern: "Struct",
            category: PatternCategory::Word,
            tgrep_args: &["-w"],
            rg_args: &["-w"],
        },
        BenchPattern {
            pattern: "main",
            category: PatternCategory::Word,
            tgrep_args: &["-w"],
            rg_args: &["-w"],
        },
        BenchPattern {
            pattern: "nonexistent_xyz",
            category: PatternCategory::Word,
            tgrep_args: &["-w"],
            rg_args: &["-w"],
        },
        // ── Case-insensitive (streaming fallback) ──
        BenchPattern {
            pattern: "terminal",
            category: PatternCategory::CaseInsensitive,
            tgrep_args: &[],
            rg_args: &[],
        },
        BenchPattern {
            pattern: "struct",
            category: PatternCategory::CaseInsensitive,
            tgrep_args: &["-i"],
            rg_args: &["-i"],
        },
    ]
}

fn run_cmd(cmd: &mut Command) -> (u64, String, i32) {
    let start = Instant::now();
    let output = cmd.output();
    let elapsed = start.elapsed().as_millis() as u64;
    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            (elapsed, stdout, out.status.code().unwrap_or(-1))
        }
        Err(e) => (elapsed, String::new(), -1),
    }
}

fn count_lines(s: &str) -> usize {
    s.lines().filter(|l| !l.is_empty()).count()
}

struct PatternResult {
    pattern: String,
    category: PatternCategory,
    tgrep_median: u64,
    tgrep_min: u64,
    tgrep_p95: u64,
    rg_median: u64,
    rg_min: u64,
    rg_p95: u64,
    tgrep_count: usize,
    rg_count: usize,
    correct: bool,
    speedup: f64,
    meets_target: bool,
}

fn run_pattern(
    pat: &BenchPattern,
    path: &str,
    trials: usize,
    tgrep_bin: &str,
    state_dir: &str,
    indexed_only: bool,
    cold_cache: bool,
) -> PatternResult {
    // First trial: verify correctness
    let mut tgrep_cmd = Command::new(tgrep_bin);
    tgrep_cmd.env("TGREP_STATE_DIR", state_dir).arg("-l");
    if indexed_only {
        tgrep_cmd.arg("--indexed-only");
    }
    tgrep_cmd.args(pat.tgrep_args).arg(pat.pattern).arg(path);
    let (t_time, t_out, _) = run_cmd(&mut tgrep_cmd);
    let t_count = count_lines(&t_out);

    let mut rg_cmd = Command::new("rg");
    rg_cmd
        .arg("--threads=4")
        .arg("-l")
        .args(pat.rg_args)
        .arg(pat.pattern)
        .arg(path);
    let (r_time, r_out, _) = run_cmd(&mut rg_cmd);
    let r_count = count_lines(&r_out);

    // Verify correctness (sorted comparison)
    let mut t_lines: Vec<&str> = t_out.lines().filter(|l| !l.is_empty()).collect();
    let mut r_lines: Vec<&str> = r_out.lines().filter(|l| !l.is_empty()).collect();
    t_lines.sort();
    r_lines.sort();
    let correct = if indexed_only {
        // indexed-only skips unindexed files, so counts may differ
        t_count <= r_count
    } else {
        t_lines == r_lines
    };

    if !correct && !indexed_only {
        eprintln!(
            "  WARNING: mismatch for '{}' — tgrep={} rg={}",
            pat.pattern, t_count, r_count
        );
    }

    let mut tgrep_times = vec![t_time];
    let mut rg_times = vec![r_time];

    // Remaining trials
    for trial in 2..=trials {
        let mut tgrep_cmd = Command::new(tgrep_bin);
        tgrep_cmd.env("TGREP_STATE_DIR", state_dir).arg("-l");
        if indexed_only {
            tgrep_cmd.arg("--indexed-only");
        }
        tgrep_cmd.args(pat.tgrep_args).arg(pat.pattern).arg(path);
        let (t_time, _, _) = run_cmd(&mut tgrep_cmd);
        tgrep_times.push(t_time);

        let mut rg_cmd = Command::new("rg");
        rg_cmd
            .arg("--threads=4")
            .arg("-l")
            .args(pat.rg_args)
            .arg(pat.pattern)
            .arg(path);
        let (r_time, _, _) = run_cmd(&mut rg_cmd);
        rg_times.push(r_time);

        // Cold cache: drop caches between trials (requires root)
        if cold_cache {
            let _ = Command::new("sudo")
                .args(["sh", "-c", "echo 3 > /proc/sys/vm/drop_caches"])
                .output();
        }
    }

    tgrep_times.sort();
    rg_times.sort();
    let t_med = tgrep_times[tgrep_times.len() / 2];
    let r_med = rg_times[rg_times.len() / 2];
    let t_p95_idx = ((tgrep_times.len() as f64) * 0.95).ceil() as usize;
    let r_p95_idx = ((rg_times.len() as f64) * 0.95).ceil() as usize;

    let speedup = if r_med > 0 {
        t_med as f64 / r_med as f64
    } else {
        1.0
    };
    let meets_target = speedup <= pat.category.target();

    eprintln!(
        "  {:<30} {:<18} tgrep={:>5}ms rg={:>5}ms speedup={:>6.1}x {} {}",
        pat.pattern,
        pat.category.label(),
        t_med,
        r_med,
        if speedup < 1.0 {
            1.0 / speedup
        } else {
            1.0 / speedup
        },
        if correct { "OK" } else { "MISMATCH" },
        if meets_target { "PASS" } else { "FAIL" },
    );

    PatternResult {
        pattern: pat.pattern.to_string(),
        category: pat.category,
        tgrep_median: t_med,
        tgrep_min: tgrep_times[0],
        tgrep_p95: tgrep_times[(t_p95_idx - 1).min(tgrep_times.len() - 1)],
        rg_median: r_med,
        rg_min: rg_times[0],
        rg_p95: rg_times[(r_p95_idx - 1).min(rg_times.len() - 1)],
        tgrep_count: t_count,
        rg_count: r_count,
        correct,
        speedup,
        meets_target,
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Usage: {} <path> [trials] [--indexed-only] [--cold-cache] [--patterns=pat1,pat2]",
            args[0]
        );
        eprintln!("  Runs the full Phase 11 benchmark suite.");
        eprintln!("  Default: 10 trials, warm cache, full mode.");
        std::process::exit(2);
    }

    let path = &args[1];
    let trials: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
    let indexed_only = args.iter().any(|a| a == "--indexed-only");
    let cold_cache = args.iter().any(|a| a == "--cold-cache");

    // Filter patterns if --patterns= is specified
    let filter_patterns: Option<Vec<String>> = args.iter().find_map(|a| {
        a.strip_prefix("--patterns=")
            .map(|s| s.split(',').map(String::from).collect())
    });

    let tgrep_bin = env::var("TGREP_BIN").unwrap_or_else(|_| "./target/release/tgrep".to_string());
    let state_dir = env::var("TGREP_STATE_DIR").unwrap_or_else(|_| {
        let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
        format!("{}/.local/share/tgrep", home)
    });

    let mode_label = if indexed_only { "indexed-only" } else { "full" };
    let cache_label = if cold_cache {
        "cold-cache"
    } else {
        "warm-cache"
    };

    eprintln!("=== tgrep Phase 11 Benchmark Suite ===");
    eprintln!("  path:       {}", path);
    eprintln!("  trials:     {}", trials);
    eprintln!("  mode:       {}", mode_label);
    eprintln!("  cache:      {}", cache_label);
    eprintln!("  tgrep:      {}", tgrep_bin);
    eprintln!("  state dir:  {}", state_dir);
    eprintln!();

    let patterns = default_patterns();
    let patterns: Vec<BenchPattern> = patterns
        .into_iter()
        .filter(|p| {
            if let Some(ref fp) = filter_patterns {
                fp.iter().any(|f| f == p.pattern)
            } else {
                true
            }
        })
        .collect();

    eprintln!("Running {} patterns...", patterns.len());
    eprintln!();

    let mut results = Vec::new();
    for pat in &patterns {
        eprintln!("[{}]", pat.pattern);
        let result = run_pattern(
            pat,
            path,
            trials,
            &tgrep_bin,
            &state_dir,
            indexed_only,
            cold_cache,
        );
        results.push(result);
    }

    // Summary table
    eprintln!();
    eprintln!(
        "=== Summary ({} trials, {}, {}) ===",
        trials, mode_label, cache_label
    );
    eprintln!(
        "  {:<30} {:<14} {:>7} {:>7} {:>7} {:>7} {:>8} {:>6} {:>5}",
        "pattern", "category", "tg_med", "tg_p95", "rg_med", "rg_p95", "speedup", "match", "pass"
    );
    eprintln!("  {}", "-".repeat(100));

    let mut total_correct = 0;
    let mut total_pass = 0;
    let mut total_patterns = 0;
    for r in &results {
        let speedup_str = if r.speedup < 1.0 {
            format!("{:.1}x faster", 1.0 / r.speedup)
        } else {
            format!("{:.1}x slower", r.speedup)
        };
        eprintln!(
            "  {:<30} {:<14} {:>5}ms {:>5}ms {:>5}ms {:>5}ms {:>8} {:>6} {:>5}",
            r.pattern,
            r.category.label(),
            r.tgrep_median,
            r.tgrep_p95,
            r.rg_median,
            r.rg_p95,
            speedup_str,
            if r.correct { "OK" } else { "FAIL" },
            if r.meets_target { "PASS" } else { "FAIL" },
        );
        if r.correct {
            total_correct += 1;
        }
        if r.meets_target {
            total_pass += 1;
        }
        total_patterns += 1;
    }

    eprintln!();
    eprintln!(
        "  Correctness: {}/{} patterns match rg",
        total_correct, total_patterns
    );
    eprintln!(
        "  Performance: {}/{} patterns meet target",
        total_pass, total_patterns
    );

    // Category breakdown
    for cat in &[
        PatternCategory::Rare,
        PatternCategory::Broad,
        PatternCategory::Word,
        PatternCategory::CaseInsensitive,
    ] {
        let cat_results: Vec<&PatternResult> =
            results.iter().filter(|r| r.category == *cat).collect();
        if cat_results.is_empty() {
            continue;
        }
        let cat_pass = cat_results.iter().filter(|r| r.meets_target).count();
        let avg_speedup: f64 =
            cat_results.iter().map(|r| r.speedup).sum::<f64>() / cat_results.len() as f64;
        eprintln!(
            "  {:<18} {}/{} pass, avg speedup: {:.2}x (target: {:.2}x)",
            cat.label(),
            cat_pass,
            cat_results.len(),
            if avg_speedup < 1.0 {
                1.0 / avg_speedup
            } else {
                1.0 / avg_speedup
            },
            if cat.target() < 1.0 {
                1.0 / cat.target()
            } else {
                cat.target()
            },
        );
    }

    // Write CSV
    let csv_path = format!(
        "/home/john/tgrep/bench_suite_{}_{}.csv",
        mode_label,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let mut csv = String::new();
    csv.push_str("pattern,category,tgrep_median_ms,tgrep_min_ms,tgrep_p95_ms,rg_median_ms,rg_min_ms,rg_p95_ms,tgrep_count,rg_count,correct,speedup,meets_target\n");
    for r in &results {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{:.4},{}\n",
            r.pattern,
            r.category.label(),
            r.tgrep_median,
            r.tgrep_min,
            r.tgrep_p95,
            r.rg_median,
            r.rg_min,
            r.rg_p95,
            r.tgrep_count,
            r.rg_count,
            r.correct,
            r.speedup,
            r.meets_target,
        ));
    }
    let _ = std::fs::write(&csv_path, &csv);
    eprintln!();
    eprintln!("Raw CSV: {}", csv_path);

    // Exit code: 0 if all pass, 1 if any fail
    if total_pass == total_patterns && total_correct == total_patterns {
        std::process::exit(0);
    } else {
        std::process::exit(1);
    }
}
