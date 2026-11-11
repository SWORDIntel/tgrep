// tgrep Phase 11: Full benchmark suite runner
//
// Runs tgrep vs rg across a comprehensive pattern set, with configurable
// trials per pattern, verified outputs, and summary reporting.
//
// Usage:
//   ./target/release/bench_suite <path> [trials] [--indexed-only] [--cold-cache]
//   ./target/release/bench_suite /rpool/scratch/tgrep_corpus 10
//   ./target/release/bench_suite /rpool/scratch/tgrep_corpus 10 --indexed-only

mod common;

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
            tgrep_args: &["-i"],
            rg_args: &["-i"],
        },
        BenchPattern {
            pattern: "struct",
            category: PatternCategory::CaseInsensitive,
            tgrep_args: &["-i"],
            rg_args: &["-i"],
        },
    ]
}

fn run_cmd(cmd: &mut Command) -> Result<(u64, Vec<u8>), String> {
    let start = Instant::now();
    let output = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    let elapsed = start.elapsed().as_millis() as u64;
    if !matches!(output.status.code(), Some(0 | 1)) {
        return Err(format!(
            "{cmd:?}: {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok((elapsed, output.stdout))
}

fn run_measured(cmd: &mut Command, cold_cache: bool) -> Result<(u64, Vec<u8>), String> {
    if cold_cache {
        let reset = Command::new("sudo")
            .args([
                "-n",
                "sh",
                "-c",
                "sync && echo 3 > /proc/sys/vm/drop_caches",
            ])
            .output()
            .map_err(|e| format!("OS page-cache reset: {e}"))?;
        if !reset.status.success() {
            return Err(format!(
                "OS page-cache reset failed: {}: {}",
                reset.status,
                String::from_utf8_lossy(&reset.stderr)
            ));
        }
    }
    run_cmd(cmd)
}

fn files(output: &[u8]) -> Vec<&[u8]> {
    let mut names: Vec<_> = output
        .split(|&b| b == b'\n')
        .filter(|s| !s.is_empty())
        .collect();
    names.sort_unstable();
    names
}

// Both lists are sorted by files().
fn results_match(tgrep: &[&[u8]], rg: &[&[u8]], indexed_only: bool) -> bool {
    tgrep == rg || (indexed_only && tgrep.iter().all(|name| rg.binary_search(name).is_ok()))
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn median(sorted: &[u64]) -> u64 {
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        sorted[middle - 1] + (sorted[middle] - sorted[middle - 1]) / 2
    } else {
        sorted[middle]
    }
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
    tools: &common::BenchmarkTools<'_>,
    indexed_only: bool,
    cold_cache: bool,
) -> Result<PatternResult, String> {
    let mut tgrep_times = Vec::new();
    let mut rg_times = Vec::new();
    let mut t_count = 0;
    let mut r_count = 0;
    for trial in 1..=trials {
        let mut tgrep_cmd = Command::new(tools.tgrep);
        tgrep_cmd.env("TGREP_STATE_DIR", tools.state).arg("-l");
        if indexed_only {
            tgrep_cmd.arg("--indexed-only");
        }
        tgrep_cmd
            .args(pat.tgrep_args)
            .arg("--")
            .arg(pat.pattern)
            .arg(path);
        let mut rg_cmd = Command::new(tools.ripgrep);
        rg_cmd
            .arg("--threads=4")
            .arg("-l")
            .args(pat.rg_args)
            .arg("--")
            .arg(pat.pattern)
            .arg(path);
        let (t, r) = if trial % 2 == 1 {
            (
                run_measured(&mut tgrep_cmd, cold_cache)?,
                run_measured(&mut rg_cmd, cold_cache)?,
            )
        } else {
            let r = run_measured(&mut rg_cmd, cold_cache)?;
            (run_measured(&mut tgrep_cmd, cold_cache)?, r)
        };
        let t_files = files(&t.1);
        let r_files = files(&r.1);
        if !results_match(&t_files, &r_files, indexed_only) {
            return Err(format!(
                "pattern {:?} trial {trial}: result mismatch: tgrep={} rg={}",
                pat.pattern,
                t_files.len(),
                r_files.len()
            ));
        }
        t_count = t_files.len();
        r_count = r_files.len();
        tgrep_times.push(t.0);
        rg_times.push(r.0);
    }
    let correct = true;

    tgrep_times.sort();
    rg_times.sort();
    let t_med = median(&tgrep_times);
    let r_med = median(&rg_times);
    let t_p95_idx = ((tgrep_times.len() as f64) * 0.95).ceil() as usize;
    let r_p95_idx = ((rg_times.len() as f64) * 0.95).ceil() as usize;

    let speedup = if !indexed_only && r_med > 0 && t_med > 0 {
        t_med as f64 / r_med as f64
    } else {
        f64::NAN
    };
    let meets_target = speedup <= pat.category.target();

    let ratio = if speedup.is_finite() {
        format!("{speedup:.3}")
    } else {
        "unmeasured".into()
    };
    eprintln!(
        "  {:<30} {:<18} tgrep={t_med}ms rg={r_med}ms ratio(tgrep/rg)={ratio} OK",
        pat.pattern,
        pat.category.label()
    );

    Ok(PatternResult {
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
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    let mut next = 2;
    let trials = match args.get(next).filter(|s| !s.starts_with("--")) {
        Some(value) => {
            next += 1;
            value.parse::<usize>()?
        }
        None => 10,
    };
    if trials == 0 {
        return Err("trials must be positive".into());
    }
    let mut indexed_only = false;
    let mut cold_cache = false;
    let mut filter_patterns: Option<Vec<String>> = None;
    for arg in &args[next..] {
        if arg == "--indexed-only" {
            indexed_only = true;
        } else if arg == "--cold-cache" {
            cold_cache = true;
        } else if let Some(value) = arg.strip_prefix("--patterns=") {
            filter_patterns = Some(value.split(',').map(String::from).collect());
        } else {
            return Err(format!("unknown argument: {arg}").into());
        }
    }

    let tgrep_bin = common::tgrep_binary()?;
    let state_dir = tgrep::build::default_state_dir();

    let mode_label = if indexed_only { "indexed-only" } else { "full" };
    let cache_label = if cold_cache {
        "OS page-cache cold; app/query/device caches uncontrolled"
    } else {
        "uncontrolled filesystem cache; repeated process runs"
    };
    let rg_bin = common::ripgrep_binary(&tgrep_bin)?;
    let tools = common::BenchmarkTools {
        tgrep: &tgrep_bin,
        ripgrep: &rg_bin,
        state: &state_dir,
    };

    eprintln!("=== tgrep Phase 11 Benchmark Suite ===");
    eprintln!("  path:       {}", path);
    eprintln!("  trials:     {}", trials);
    eprintln!("  mode:       {}", mode_label);
    eprintln!("  cache:      {}", cache_label);
    eprintln!("  tgrep:      {}", tgrep_bin.display());
    eprintln!("  state dir:  {}", state_dir.display());
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

    if patterns.is_empty() {
        return Err("pattern filter selected no patterns".into());
    }
    eprintln!("Running {} patterns...", patterns.len());
    eprintln!();

    let mut results = Vec::new();
    for pat in &patterns {
        eprintln!("[{}]", pat.pattern);
        let result = run_pattern(pat, path, trials, &tools, indexed_only, cold_cache)?;
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
        let speedup_str = if !r.speedup.is_finite() {
            "unmeasured".into()
        } else if r.speedup < 1.0 {
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
            if !r.speedup.is_finite() {
                "N/A"
            } else if r.meets_target {
                "PASS"
            } else {
                "FAIL"
            },
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
        let cat_results: Vec<&PatternResult> = results
            .iter()
            .filter(|r| r.category == *cat && r.speedup.is_finite())
            .collect();
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
            1.0 / avg_speedup,
            if cat.target() < 1.0 {
                1.0 / cat.target()
            } else {
                cat.target()
            },
        );
    }

    // Write CSV
    let csv_path = format!(
        "{}/bench_suite_{}_{}.csv",
        env!("CARGO_MANIFEST_DIR"),
        mode_label,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let mut csv = String::new();
    csv.push_str("pattern,category,tgrep_median_ms,tgrep_min_ms,tgrep_p95_ms,rg_median_ms,rg_min_ms,rg_p95_ms,tgrep_count,rg_count,correct,speedup,meets_target\n");
    for r in &results {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            csv_field(&r.pattern),
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
            if r.speedup.is_finite() {
                format!("{:.4}", r.speedup)
            } else {
                String::new()
            },
            if r.speedup.is_finite() {
                r.meets_target.to_string()
            } else {
                String::new()
            },
        ));
    }
    std::fs::write(&csv_path, &csv)?;
    eprintln!();
    eprintln!("Raw CSV: {}", csv_path);

    // Exit code: 0 if all pass, 1 if any fail
    if total_pass == total_patterns && total_correct == total_patterns {
        Ok(())
    } else {
        Err("one or more performance targets failed or could not be measured".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn child_failure_with_output_is_an_error() {
        let error = run_cmd(&mut fixture(
            "printf 'same\n'; printf fixture-error >&2; exit 2",
        ))
        .unwrap_err();
        assert!(error.contains("exit status: 2"), "{error}");
        assert!(error.contains("fixture-error"), "{error}");
    }

    #[test]
    fn signalled_child_is_an_error() {
        let error = run_cmd(&mut fixture("kill -TERM $$")).unwrap_err();
        assert!(error.contains("signal"), "{error}");
    }

    #[test]
    fn no_match_status_is_a_valid_empty_result() {
        let (_, output) = run_cmd(&mut fixture("exit 1")).unwrap();
        assert!(output.is_empty());
        assert_eq!(files(&output).len(), 0);
    }

    #[test]
    fn comparison_uses_filenames_and_requires_real_indexed_subset() {
        let left = files(b"second\nfirst\n");
        let same = files(b"first\nsecond\n");
        let different = files(b"first\nother\n");
        assert!(results_match(&left, &same, false));
        assert!(!results_match(&left, &different, false));
        assert!(!results_match(&left, &different, true));
        assert!(results_match(&files(b"first\n"), &same, true));
        assert!(!results_match(&files(b"absent\n"), &same, true));
    }

    #[test]
    fn csv_quotes_delimiters_quotes_and_line_breaks() {
        assert_eq!(
            csv_field("one,\"two\"\nthree"),
            "\"one,\"\"two\"\"\nthree\""
        );
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("one\rtwo"), "\"one\rtwo\"");
    }

    #[test]
    fn even_median_does_not_overflow() {
        assert_eq!(median(&[2, 4]), 3);
        assert_eq!(median(&[u64::MAX - 2, u64::MAX]), u64::MAX - 1);
        assert_eq!(median(&[0, u64::MAX]), u64::MAX / 2);
    }
}
