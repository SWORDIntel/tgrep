// tgrep vs ripgrep paired benchmark runner
//
// Runs tgrep and rg alternately for N trials per pattern, verifies result
// equality on every trial, and reports timing statistics.
//
// Usage:
//   cargo run --release --bin bench -- <pattern> <path> [trials] [--extra-args...]
//   ./target/release/bench "struct" /rpool/scratch/tgrep_corpus 10

mod common;

use std::env;
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
            csv_field(&self.tool),
            csv_field(&self.pattern),
            self.trial,
            self.time_ms,
            self.match_count,
            self.peak_rss_kb.map_or("".to_string(), |v| v.to_string()),
        )
    }
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn run_command(cmd: &mut Command) -> Result<(u64, Vec<u8>), String> {
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

fn run_benchmark(
    pattern: &str,
    path: &str,
    trials: usize,
    tools: &common::BenchmarkTools<'_>,
    extra_args: &[String],
    indexed_only: bool,
) -> Result<Vec<BenchmarkResult>, String> {
    let mut results = Vec::new();
    for trial in 1..=trials {
        let mut tgrep_cmd = Command::new(tools.tgrep);
        tgrep_cmd
            .env("TGREP_STATE_DIR", tools.state)
            .arg("-l")
            .args(extra_args)
            .arg("--")
            .arg(pattern)
            .arg(path);
        let mut rg_cmd = Command::new(tools.ripgrep);
        rg_cmd
            .arg("--threads=4")
            .arg("-l")
            .args(extra_args.iter().filter(|a| a.as_str() != "--indexed-only"))
            .arg("--")
            .arg(pattern)
            .arg(path);

        let (t_result, r_result) = if trial % 2 == 1 {
            (run_command(&mut tgrep_cmd)?, run_command(&mut rg_cmd)?)
        } else {
            let r = run_command(&mut rg_cmd)?;
            (run_command(&mut tgrep_cmd)?, r)
        };
        let (t_time, t_out) = t_result;
        let (r_time, r_out) = r_result;
        let t_files = files(&t_out);
        let r_files = files(&r_out);
        if !results_match(&t_files, &r_files, indexed_only) {
            return Err(format!(
                "trial {trial}: result mismatch: tgrep={} files, rg={} files",
                t_files.len(),
                r_files.len()
            ));
        }
        if indexed_only && t_files != r_files {
            eprintln!("  trial {trial}: indexed-only subset: tgrep={} files, rg={} files; workloads differ",
                t_files.len(), r_files.len());
        }
        eprintln!("  trial {trial}: tgrep={t_time}ms rg={r_time}ms");
        for (tool, time_ms, match_count) in [
            ("tgrep", t_time, t_files.len()),
            ("rg", r_time, r_files.len()),
        ] {
            results.push(BenchmarkResult {
                tool: tool.into(),
                pattern: pattern.into(),
                trial,
                time_ms,
                match_count,
                peak_rss_kb: None,
            });
        }
    }
    Ok(results)
}

fn compute_stats(times: &[u64]) -> (u64, u64, u64, u64, f64) {
    if times.is_empty() {
        return (0, 0, 0, 0, 0.0);
    }
    let mut sorted = times.to_vec();
    sorted.sort();

    let min = sorted[0];
    let max = sorted[sorted.len() - 1];
    let middle = sorted.len() / 2;
    let median = if sorted.len().is_multiple_of(2) {
        sorted[middle - 1] + (sorted[middle] - sorted[middle - 1]) / 2
    } else {
        sorted[middle]
    };
    let p95_idx = ((sorted.len() as f64) * 0.95).ceil() as usize;
    let p95 = sorted[(p95_idx - 1).min(sorted.len() - 1)];
    let mean = sorted.iter().map(|&n| n as f64).sum::<f64>() / sorted.len() as f64;

    (min, median, p95, max, mean)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "Usage: {} <pattern> <path> [trials] [-- extra-args...]",
            args[0]
        );
        eprintln!(
            "Example: {} \"struct\" /rpool/scratch/tgrep_corpus 10",
            args[0]
        );
        eprintln!("  Pass --indexed-only as an extra arg to benchmark indexed-only mode.");
        std::process::exit(2);
    }

    let pattern = &args[1];
    let path = &args[2];
    let mut next = 3;
    let trials = match args.get(next).filter(|s| s.as_str() != "--") {
        Some(value) => {
            next += 1;
            value.parse::<usize>()?
        }
        None => 10,
    };
    if trials == 0 {
        return Err("trials must be positive".into());
    }
    let extra_args = if next < args.len() {
        if args[next] != "--" {
            return Err("extra arguments must follow --".into());
        }
        args[next + 1..].to_vec()
    } else {
        Vec::new()
    };
    for arg in &extra_args {
        if !matches!(
            arg.as_str(),
            "--indexed-only"
                | "-F"
                | "--fixed-strings"
                | "-S"
                | "--smart-case"
                | "-i"
                | "--ignore-case"
                | "-w"
                | "--word-regexp"
        ) {
            return Err(format!("unsupported benchmark search option: {arg}").into());
        }
    }

    // Check for --indexed-only flag
    let indexed_only = extra_args.iter().any(|a| a == "--indexed-only");
    let label = if indexed_only { " (indexed-only)" } else { "" };

    let tgrep_bin = common::tgrep_binary()?;
    let state_dir = tgrep::build::default_state_dir();
    let rg_bin = common::ripgrep_binary(&tgrep_bin)?;
    let tools = common::BenchmarkTools {
        tgrep: &tgrep_bin,
        ripgrep: &rg_bin,
        state: &state_dir,
    };

    eprintln!("tgrep binary: {}", tgrep_bin.display());
    eprintln!("ripgrep bin:  {}", rg_bin.display());
    eprintln!("state dir:    {}", state_dir.display());
    eprintln!("pattern:      {}", pattern);
    eprintln!("path:         {}", path);
    eprintln!("trials:       {}", trials);
    if !extra_args.is_empty() {
        eprintln!("extra args:   {:?}", extra_args);
    }
    eprintln!();

    let results = run_benchmark(pattern, path, trials, &tools, &extra_args, indexed_only)?;

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
    eprintln!(
        "  {:<10} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "tool", "min", "median", "p95", "max", "mean"
    );
    eprintln!(
        "  {:<10} {:>7}ms {:>7}ms {:>7}ms {:>7}ms {:>7.1}ms",
        "tgrep", t_min, t_med, t_p95, t_max, t_mean
    );
    eprintln!(
        "  {:<10} {:>7}ms {:>7}ms {:>7}ms {:>7}ms {:>7.1}ms",
        "rg", r_min, r_med, r_p95, r_max, r_mean
    );

    if !indexed_only && r_med > 0 && t_med > 0 {
        let speedup = t_med as f64 / r_med as f64;
        if speedup < 1.0 {
            eprintln!("  tgrep is {:.1}x FASTER than rg (median)", 1.0 / speedup);
        } else {
            eprintln!("  tgrep is {:.1}x slower than rg (median)", speedup);
        }
    }

    // Write CSV
    let csv_path = format!(
        "{}/bench_results_{}.csv",
        env!("CARGO_MANIFEST_DIR"),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let mut csv = String::new();
    csv.push_str(&BenchmarkResult::csv_header());
    csv.push('\n');
    for r in &results {
        csv.push_str(&r.to_csv());
        csv.push('\n');
    }
    std::fs::write(&csv_path, &csv)?;
    eprintln!();
    eprintln!("Raw CSV: {}", csv_path);
    Ok(())
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
        let error = run_command(&mut fixture(
            "printf 'same\n'; printf fixture-error >&2; exit 2",
        ))
        .unwrap_err();
        assert!(error.contains("exit status: 2"), "{error}");
        assert!(error.contains("fixture-error"), "{error}");
    }

    #[test]
    fn signalled_child_is_an_error() {
        let error = run_command(&mut fixture("kill -TERM $$")).unwrap_err();
        assert!(error.contains("signal"), "{error}");
    }

    #[test]
    fn no_match_status_is_a_valid_empty_result() {
        let (_, output) = run_command(&mut fixture("exit 1")).unwrap();
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
    fn csv_quotes_pattern_delimiters_quotes_and_line_breaks() {
        let result = BenchmarkResult {
            tool: "tgrep".into(),
            pattern: "one,\"two\"\nthree".into(),
            trial: 1,
            time_ms: 7,
            match_count: 2,
            peak_rss_kb: None,
        };
        assert_eq!(result.to_csv(), "tgrep,\"one,\"\"two\"\"\nthree\",1,7,2,");
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("one\rtwo"), "\"one\rtwo\"");
    }

    #[test]
    fn even_median_and_large_mean_do_not_overflow() {
        assert_eq!(compute_stats(&[4, 2]).1, 3);
        let stats = compute_stats(&[u64::MAX, u64::MAX - 2]);
        assert_eq!(stats.1, u64::MAX - 1);
        assert!(stats.4.is_finite());
        assert_eq!(compute_stats(&[0, u64::MAX]).1, u64::MAX / 2);
    }
}
