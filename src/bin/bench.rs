// tgrep vs ripgrep paired benchmark runner
//
// Runs tgrep and rg alternately for N trials per pattern, verifies result
// equality on the first trial, and reports timing statistics.
//
// Usage:
//   cargo run --release --bin bench -- <pattern> <path> [trials] [--extra-args...]
//   ./target/release/bench "struct" /rpool/scratch/tgrep_corpus 10

use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

struct BenchmarkResult {
    tool: String,
    pattern: String,
    trial: usize,
    time_ms: u64,
    match_count: usize,
    peak_rss_kb: Option<u64>,
}

impl BenchmarkResult {
    fn csv_header() -> String {
        "tool,pattern,trial,time_ms,match_count,peak_rss_kb".to_string()
    }

    fn to_csv(&self) -> String {
        format!(
            "{},{},{},{},{},{}",
            self.tool,
            self.pattern,
            self.trial,
            self.time_ms,
            self.match_count,
            self.peak_rss_kb.map_or("".to_string(), |v| v.to_string()),
        )
    }
}

fn run_command(cmd: &mut Command) -> (u64, String, String, i32) {
    let start = Instant::now();
    let output = cmd.output();
    let elapsed = start.elapsed().as_millis() as u64;

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            (elapsed, stdout, stderr, out.status.code().unwrap_or(-1))
        }
        Err(e) => (elapsed, String::new(), e.to_string(), -1),
    }
}

fn count_matches(output: &str) -> usize {
    // For -l mode: count lines (each line is a file with a match)
    output.lines().filter(|l| !l.is_empty()).count()
}

fn run_benchmark(
    pattern: &str,
    path: &str,
    trials: usize,
    tgrep_bin: &str,
    state_dir: &str,
    extra_args: &[String],
    indexed_only: bool,
) -> Vec<BenchmarkResult> {
    let mut results = Vec::new();

    // First trial: verify result equality
    eprintln!("=== Verifying result equality ===");

    let mut tgrep_cmd = Command::new(tgrep_bin);
    tgrep_cmd
        .env("TGREP_STATE_DIR", state_dir)
        .arg("-l")
        .arg(pattern)
        .arg(path)
        .args(extra_args);
    let (t_time, t_out, _t_err, _t_code) = run_command(&mut tgrep_cmd);
    let t_count = count_matches(&t_out);

    let mut rg_cmd = Command::new("rg");
    rg_cmd
        .arg("--threads=4")
        .arg("-l")
        .arg(pattern)
        .arg(path);
    let (r_time, r_out, _r_err, _r_code) = run_command(&mut rg_cmd);
    let r_count = count_matches(&r_out);

    // Compare results
    let t_sorted: Vec<&str> = t_out.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>();
    let r_sorted: Vec<&str> = r_out.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>();
    let mut t_sorted = t_sorted.clone();
    let mut r_sorted = r_sorted.clone();
    t_sorted.sort();
    r_sorted.sort();

    if indexed_only && t_sorted != r_sorted {
        eprintln!(
            "  NOTE (indexed-only): tgrep={} matches, rg={} matches (diff expected — unindexed files skipped)",
            t_count, r_count
        );
    } else if t_sorted == r_sorted {
        eprintln!("  PASS: {} matches, results identical", t_count);
    } else {
        eprintln!("  FAIL: tgrep={} matches, rg={} matches", t_count, r_count);
        let t_set: std::collections::HashSet<&str> = t_sorted.iter().copied().collect();
        let r_set: std::collections::HashSet<&str> = r_sorted.iter().copied().collect();
        let only_tgrep: Vec<&&str> = t_sorted.iter().filter(|s| !r_set.contains(*s)).collect();
        let only_rg: Vec<&&str> = r_sorted.iter().filter(|s| !t_set.contains(*s)).collect();
        if !only_rg.is_empty() {
            eprintln!("  In rg but not tgrep (first 5):");
            for f in only_rg.iter().take(5) {
                eprintln!("    {}", f);
            }
        }
        if !only_tgrep.is_empty() {
            eprintln!("  In tgrep but not rg (first 5):");
            for f in only_tgrep.iter().take(5) {
                eprintln!("    {}", f);
            }
        }
        eprintln!("  Continuing benchmark anyway...");
    }

    results.push(BenchmarkResult {
        tool: "tgrep".into(),
        pattern: pattern.into(),
        trial: 1,
        time_ms: t_time,
        match_count: t_count,
        peak_rss_kb: None,
    });
    results.push(BenchmarkResult {
        tool: "rg".into(),
        pattern: pattern.into(),
        trial: 1,
        time_ms: r_time,
        match_count: r_count,
        peak_rss_kb: None,
    });

    eprintln!("  trial 1: tgrep={}ms rg={}ms", t_time, r_time);

    // Remaining trials: alternate tgrep and rg
    for trial in 2..=trials {
        // Run tgrep
        let mut tgrep_cmd = Command::new(tgrep_bin);
        tgrep_cmd
            .env("TGREP_STATE_DIR", state_dir)
            .arg("-l")
            .arg(pattern)
            .arg(path)
            .args(extra_args);
        let (t_time, _, _, _) = run_command(&mut tgrep_cmd);

        // Run rg
        let mut rg_cmd = Command::new("rg");
        rg_cmd
            .arg("--threads=4")
            .arg("-l")
            .arg(pattern)
            .arg(path);
        let (r_time, _, _, _) = run_command(&mut rg_cmd);

        eprintln!(
            "  trial {}: tgrep={}ms rg={}ms",
            trial, t_time, r_time
        );

        results.push(BenchmarkResult {
            tool: "tgrep".into(),
            pattern: pattern.into(),
            trial,
            time_ms: t_time,
            match_count: t_count,
            peak_rss_kb: None,
        });
        results.push(BenchmarkResult {
            tool: "rg".into(),
            pattern: pattern.into(),
            trial,
            time_ms: r_time,
            match_count: r_count,
            peak_rss_kb: None,
        });
    }

    results
}

fn compute_stats(times: &[u64]) -> (u64, u64, u64, u64, f64) {
    if times.is_empty() {
        return (0, 0, 0, 0, 0.0);
    }
    let mut sorted = times.to_vec();
    sorted.sort();

    let min = sorted[0];
    let max = sorted[sorted.len() - 1];
    let median = sorted[sorted.len() / 2];
    let p95_idx = ((sorted.len() as f64) * 0.95).ceil() as usize;
    let p95 = sorted[(p95_idx - 1).min(sorted.len() - 1)];
    let mean = sorted.iter().sum::<u64>() as f64 / sorted.len() as f64;

    (min, median, p95, max, mean)
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <pattern> <path> [trials] [-- extra-args...]", args[0]);
        eprintln!("Example: {} \"struct\" /rpool/scratch/tgrep_corpus 10", args[0]);
        eprintln!("  Pass --indexed-only as an extra arg to benchmark indexed-only mode.");
        std::process::exit(2);
    }

    let pattern = &args[1];
    let path = &args[2];
    let trials: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);

    // Collect extra args after "--"
    let extra_args: Vec<String> = {
        let mut extra = Vec::new();
        let mut skip = true;
        for a in &args[4..] {
            if skip && a == "--" {
                skip = false;
                continue;
            }
            if !skip {
                extra.push(a.clone());
            }
        }
        extra
    };

    // Check for --indexed-only flag
    let indexed_only = extra_args.iter().any(|a| a == "--indexed-only");
    let label = if indexed_only { " (indexed-only)" } else { "" };

    // Find tgrep binary
    let tgrep_bin = env::var("TGREP_BIN").unwrap_or_else(|_| {
        let candidate = PathBuf::from("./target/release/tgrep");
        if candidate.exists() {
            "./target/release/tgrep".to_string()
        } else {
            "./target/debug/tgrep".to_string()
        }
    });

    let state_dir = env::var("TGREP_STATE_DIR").unwrap_or_else(|_| {
        let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
        format!("{}/.local/share/tgrep", home)
    });

    eprintln!("tgrep binary: {}", tgrep_bin);
    eprintln!("state dir:    {}", state_dir);
    eprintln!("pattern:      {}", pattern);
    eprintln!("path:         {}", path);
    eprintln!("trials:       {}", trials);
    if !extra_args.is_empty() {
        eprintln!("extra args:   {:?}", extra_args);
    }
    eprintln!();

    let results = run_benchmark(pattern, path, trials, &tgrep_bin, &state_dir, &extra_args, indexed_only);

    // Split results by tool
    let mut tgrep_times: Vec<u64> = Vec::new();
    let mut rg_times: Vec<u64> = Vec::new();
    for r in &results {
        if r.tool == "tgrep" {
            tgrep_times.push(r.time_ms);
        } else {
            rg_times.push(r.time_ms);
        }
    }

    let (t_min, t_med, t_p95, t_max, t_mean) = compute_stats(&tgrep_times);
    let (r_min, r_med, r_p95, r_max, r_mean) = compute_stats(&rg_times);

    eprintln!();
    eprintln!("=== Summary: '{}'{} ===", pattern, label);
    eprintln!("  {:<10} {:>8} {:>8} {:>8} {:>8} {:>8}", "tool", "min", "median", "p95", "max", "mean");
    eprintln!("  {:<10} {:>7}ms {:>7}ms {:>7}ms {:>7}ms {:>7.1}ms", "tgrep", t_min, t_med, t_p95, t_max, t_mean);
    eprintln!("  {:<10} {:>7}ms {:>7}ms {:>7}ms {:>7}ms {:>7.1}ms", "rg", r_min, r_med, r_p95, r_max, r_mean);

    if r_med > 0 {
        let speedup = t_med as f64 / r_med as f64;
        if speedup < 1.0 {
            eprintln!("  tgrep is {:.1}x FASTER than rg (median)", 1.0 / speedup);
        } else {
            eprintln!("  tgrep is {:.1}x slower than rg (median)", speedup);
        }
    }

    // Write CSV
    let csv_path = format!(
        "/home/john/tgrep/bench_results_{}.csv",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let mut csv = String::new();
    csv.push_str(&BenchmarkResult::csv_header());
    csv.push('\n');
    for r in &results {
        csv.push_str(&r.to_csv());
        csv.push('\n');
    }
    let _ = std::fs::write(&csv_path, &csv);
    eprintln!();
    eprintln!("Raw CSV: {}", csv_path);
}
