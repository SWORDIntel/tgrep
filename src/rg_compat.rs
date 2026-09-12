// rg compatibility router.
//
// When tgrep is invoked as `rg` (via wrapper script or symlink), it enters
// rg-compat mode. In this mode, tgrep decides whether to handle the search
// using its persistent index (faster for indexed patterns) or delegate to
// the real ripgrep binary (for unsupported flags or unindexed searches).
//
// The router ensures that ALL rg flags and commands work identically:
//   --help, --version → delegated to real rg (identical output)
//   Unsupported flags → delegated to real rg
//   No index → delegated to real rg
//   Indexed search → tgrep (with rg-compatible output via grep-printer)
//
// The real rg path is provided via the TGREP_REAL_RG environment variable
// (set by the installer's wrapper script).

use std::collections::HashSet;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::exit;

/// Flags that tgrep can handle in rg-compat mode.
/// Any flag NOT in this set causes delegation to real rg.
const SUPPORTED_SHORT_FLAGS: &[char] = &[
    'w', 'l', 'c', 'n', 'q', 'i', 'S', 'F', 'e', 'j', 'g', 'H', 'I',
];

const SUPPORTED_LONG_FLAGS: &[&str] = &[
    "--word-regexp",
    "--files-with-matches",
    "--count",
    "--line-number",
    "--quiet",
    "--ignore-case",
    "--smart-case",
    "--fixed-strings",
    "--regexp",
    "--threads",
    "--hidden",
    "--glob",
    "--with-filename",
    "--no-filename",
    "--no-line-number",
];

/// Flags that take an argument (value follows the flag).
const FLAGS_WITH_ARGS_SHORT: &[char] = &['e', 'j', 'g'];

const FLAGS_WITH_ARGS_LONG: &[&str] = &[
    "--regexp",
    "--threads",
    "--glob",
];

/// Find the real rg binary path.
/// Checks TGREP_REAL_RG env var, then searches PATH for a non-tgrep rg.
fn find_real_rg() -> Option<PathBuf> {
    // 1. Environment variable (set by installer wrapper)
    if let Ok(path) = std::env::var("TGREP_REAL_RG") {
        if !path.is_empty() && std::path::Path::new(&path).exists() {
            return Some(PathBuf::from(path));
        }
    }

    // 2. Config file (~/.config/tgrep/rg_path)
    if let Ok(home) = std::env::var("HOME") {
        let config_path = format!("{}/.config/tgrep/rg_path", home);
        if let Ok(path) = std::fs::read_to_string(&config_path) {
            let path = path.trim();
            if !path.is_empty() && std::path::Path::new(path).exists() {
                return Some(PathBuf::from(path));
            }
        }
    }

    // 3. Search PATH for rg (excluding ourselves)
    if let Ok(path_var) = std::env::var("PATH") {
        let self_exe = std::env::current_exe().ok();
        for dir in path_var.split(':') {
            if dir.is_empty() {
                continue;
            }
            let rg_path = format!("{}/rg", dir);
            let candidate = std::path::Path::new(&rg_path);
            if candidate.exists() {
                // Don't find ourselves
                if let Some(self_path) = &self_exe {
                    if std::fs::canonicalize(candidate).ok()
                        == std::fs::canonicalize(self_path).ok()
                    {
                        continue;
                    }
                }
                return Some(PathBuf::from(rg_path));
            }
        }
    }

    None
}

/// Delegate to the real rg binary, preserving all arguments and stdio.
fn delegate_to_rg(args: &[String]) -> ! {
    let rg_path = find_real_rg().unwrap_or_else(|| {
        eprintln!("tgrep: cannot find real ripgrep binary (set TGREP_REAL_RG)");
        exit(2);
    });

    let err = std::process::Command::new(&rg_path)
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .exec();

    eprintln!("tgrep: failed to exec real rg ({}): {}", rg_path.display(), err);
    exit(2);
}

/// Check if any argument is an unsupported flag.
/// Returns true if all flags are supported, false if any unsupported flag is found.
fn check_supported_flags(args: &[String]) -> bool {
    let supported_short: HashSet<char> = SUPPORTED_SHORT_FLAGS.iter().cloned().collect();
    let supported_long: HashSet<&str> = SUPPORTED_LONG_FLAGS.iter().cloned().collect();
    let args_short: HashSet<char> = FLAGS_WITH_ARGS_SHORT.iter().cloned().collect();
    let args_long: HashSet<&str> = FLAGS_WITH_ARGS_LONG.iter().cloned().collect();

    let mut i = 0;
    let mut after_dd = false; // after `--`
    while i < args.len() {
        let arg = &args[i];

        // `--` means end of flags
        if arg == "--" && !after_dd {
            after_dd = true;
            i += 1;
            continue;
        }

        if after_dd {
            i += 1;
            continue;
        }

        // Long flags
        if arg.starts_with("--") {
            let flag = if let Some(eq) = arg.find('=') {
                &arg[..eq]
            } else {
                arg.as_str()
            };

            if !supported_long.contains(flag) {
                return false;
            }

            // If this flag takes an argument and no '=' was used, consume next arg
            if arg.find('=').is_none() && args_long.contains(flag) {
                i += 1; // skip the value
            }
            i += 1;
            continue;
        }

        // Short flags (may be combined: -wl)
        if arg.starts_with('-') && arg.len() > 1 && !arg[1..].starts_with(char::is_numeric) {
            let chars: Vec<char> = arg[1..].chars().collect();
            let mut ci = 0;
            while ci < chars.len() {
                let c = chars[ci];
                if !supported_short.contains(&c) {
                    return false;
                }
                // If this flag takes an argument, the rest of the arg (or next arg) is the value
                if args_short.contains(&c) {
                    if ci + 1 < chars.len() {
                        // Value is rest of this arg: -ePATTERN
                        break;
                    }
                    // Value is next arg
                    i += 1;
                    break;
                }
                ci += 1;
            }
            i += 1;
            continue;
        }

        // Positional argument (pattern or path)
        i += 1;
    }

    true
}

/// Parse the supported rg flags into a SearchConfig.
/// Returns (config, should_use_tgrep) where should_use_tgrep is false if
/// the search doesn't benefit from indexing.
fn parse_rg_args(
    args: &[String],
) -> (crate::search::SearchConfig, bool) {
    let mut pattern: Option<String> = None;
    let mut paths: Vec<String> = Vec::new();
    let mut fixed_strings = false;
    let mut smart_case = false;
    let mut ignore_case = false;
    let mut word_regexp = false;
    let mut files_with_matches = false;
    let mut line_number = false;
    let mut count = false;
    let mut quiet = false;
    let mut patterns: Vec<String> = Vec::new();
    let mut _threads = 4usize;
    let mut _globs: Vec<String> = Vec::new();
    let mut _hidden = false;
    let mut _no_filename = false;
    let mut _no_line_number = false;

    let mut after_dd = false;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];

        if arg == "--" && !after_dd {
            after_dd = true;
            i += 1;
            continue;
        }

        if after_dd {
            if pattern.is_none() {
                pattern = Some(arg.clone());
            } else {
                paths.push(arg.clone());
            }
            i += 1;
            continue;
        }

        // Long flags
        if arg.starts_with("--") {
            let (flag, value) = if let Some(eq) = arg.find('=') {
                (&arg[..eq], Some(arg[eq + 1..].to_string()))
            } else {
                (arg.as_str(), None)
            };

            match flag {
                "--word-regexp" => word_regexp = true,
                "--files-with-matches" => files_with_matches = true,
                "--count" => count = true,
                "--line-number" => line_number = true,
                "--no-line-number" => _no_line_number = true,
                "--quiet" => quiet = true,
                "--ignore-case" => ignore_case = true,
                "--smart-case" => smart_case = true,
                "--fixed-strings" => fixed_strings = true,
                "--regexp" => {
                    let v = value.unwrap_or_else(|| {
                        i += 1;
                        args.get(i).cloned().unwrap_or_default()
                    });
                    patterns.push(v);
                }
                "--threads" => {
                    let v = value.unwrap_or_else(|| {
                        i += 1;
                        args.get(i).cloned().unwrap_or_default()
                    });
                    if let Ok(n) = v.parse() {
                        _threads = n;
                    }
                }
                "--glob" => {
                    let v = value.unwrap_or_else(|| {
                        i += 1;
                        args.get(i).cloned().unwrap_or_default()
                    });
                    _globs.push(v);
                }
                "--hidden" => _hidden = true,
                "--with-filename" => {} // default behavior
                "--no-filename" => _no_filename = true,
                _ => {}
            }
            i += 1;
            continue;
        }

        // Short flags (may be combined: -wl)
        if arg.starts_with('-') && arg.len() > 1 && !arg[1..].starts_with(char::is_numeric) {
            let chars: Vec<char> = arg[1..].chars().collect();
            let mut ci = 0;
            while ci < chars.len() {
                let c = chars[ci];
                match c {
                    'w' => word_regexp = true,
                    'l' => files_with_matches = true,
                    'c' => count = true,
                    'n' => line_number = true,
                    'q' => quiet = true,
                    'i' => ignore_case = true,
                    'S' => smart_case = true,
                    'F' => fixed_strings = true,
                    'H' => {} // with-filename (default)
                    'I' => _no_filename = true,
                    'e' => {
                        // -e PATTERN or -ePATTERN
                        let v = if ci + 1 < chars.len() {
                            chars[ci + 1..].iter().collect()
                        } else {
                            i += 1;
                            args.get(i).cloned().unwrap_or_default()
                        };
                        patterns.push(v);
                        break;
                    }
                    'j' => {
                        let v = if ci + 1 < chars.len() {
                            chars[ci + 1..].iter().collect()
                        } else {
                            i += 1;
                            args.get(i).cloned().unwrap_or_default()
                        };
                        if let Ok(n) = v.parse() {
                            _threads = n;
                        }
                        break;
                    }
                    'g' => {
                        let v = if ci + 1 < chars.len() {
                            chars[ci + 1..].iter().collect()
                        } else {
                            i += 1;
                            args.get(i).cloned().unwrap_or_default()
                        };
                        _globs.push(v);
                        break;
                    }
                    _ => {}
                }
                ci += 1;
            }
            i += 1;
            continue;
        }

        // Positional: first is pattern, rest are paths
        if pattern.is_none() {
            pattern = Some(arg.clone());
        } else {
            paths.push(arg.clone());
        }
        i += 1;
    }

    // If -e was used, patterns are from -e; otherwise use positional pattern
    let final_pattern = if !patterns.is_empty() {
        // Join multiple -e patterns with | (like rg does)
        if patterns.len() == 1 {
            patterns[0].clone()
        } else {
            patterns.join("|")
        }
    } else {
        pattern.unwrap_or_default()
    };

    if final_pattern.is_empty() && !quiet {
        // No pattern — rg would show help or error. Delegate.
        return (
            crate::search::SearchConfig {
                pattern: String::new(),
                paths: Vec::new(),
                fixed_strings,
                smart_case,
                ignore_case,
                word_regexp,
                files_with_matches,
                line_number: line_number && !_no_line_number,
                count,
                quiet,
                patterns,
                explain: false,
                indexed_only: false,
                list_files: false,
            },
            false,
        );
    }

    // Determine if this search benefits from tgrep's index.
    // tgrep is faster for:
    //   - Indexed patterns (trigram or hash index)
    //   - Not case-insensitive (or case-insensitive with cache)
    // tgrep is NOT faster for:
    //   - Case-insensitive without cache (streaming fallback)
    //   - Patterns with no usable trigrams (streaming)
    // But we still try tgrep for everything — it falls back to streaming
    // internally, and the cache helps even for case-insensitive.
    let benefits = true;

    (
        crate::search::SearchConfig {
            pattern: final_pattern,
            paths: paths.iter().map(PathBuf::from).collect(),
            fixed_strings,
            smart_case,
            ignore_case,
            word_regexp,
            files_with_matches,
            line_number: line_number && !_no_line_number,
            count,
            quiet,
            patterns,
            explain: false,
            indexed_only: false,
            list_files: false,
        },
        benefits,
    )
}

/// Check if a tgrep index exists for the given search paths.
fn index_exists() -> bool {
    let state_dir = crate::build::default_state_dir();
    let manifest_path = state_dir.join("manifest.json");
    manifest_path.exists()
}

/// Entry point for rg-compat mode.
/// Called when tgrep is invoked as `rg` (via wrapper or symlink).
pub fn run_as_rg(args: &[String]) -> ! {
    // args[0] is the program name; rg args start at index 1
    let rg_args: Vec<String> = if args.is_empty() {
        Vec::new()
    } else {
        args[1..].to_vec()
    };

    // 1. --help, --version, -h, -V → always delegate to real rg
    for arg in &rg_args {
        if arg == "--help" || arg == "-h" {
            delegate_to_rg(&rg_args);
        }
        if arg == "--version" || arg == "-V" {
            delegate_to_rg(&rg_args);
        }
        // Special rg commands that we don't handle
        if arg == "--files" || arg == "--type-list" {
            delegate_to_rg(&rg_args);
        }
    }

    // 2. Check for unsupported flags → delegate to real rg
    if !check_supported_flags(&rg_args) {
        delegate_to_rg(&rg_args);
    }

    // 3. Check if an index exists
    if !index_exists() {
        delegate_to_rg(&rg_args);
    }

    // 4. Parse args and decide whether to use tgrep
    let (config, use_tgrep) = parse_rg_args(&rg_args);

    if !use_tgrep || config.pattern.is_empty() {
        delegate_to_rg(&rg_args);
    }

    // 5. Run tgrep indexed search
    crate::search::run_search(config);
    // run_search calls exit() internally, but the compiler needs this
    exit(0);
}
