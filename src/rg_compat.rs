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
const SUPPORTED_SHORT_FLAGS: &[char] = &['w', 'l', 'c', 'n', 'q', 'F', 'e', 'H'];

const SUPPORTED_LONG_FLAGS: &[&str] = &[
    "--word-regexp",
    "--files-with-matches",
    "--count",
    "--line-number",
    "--quiet",
    "--fixed-strings",
    "--regexp",
    "--with-filename",
    "--no-line-number",
];

/// Flags that take an argument (value follows the flag).
const FLAGS_WITH_ARGS_SHORT: &[char] = &['e', 'j', 'g'];

const FLAGS_WITH_ARGS_LONG: &[&str] = &["--regexp", "--threads", "--glob"];

/// Find the real rg binary path.
/// Checks TGREP_REAL_RG env var, then searches PATH for a non-tgrep rg.
fn find_real_rg() -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let self_metadata = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok());
    let is_self = |path: &std::path::Path| {
        self_metadata.as_ref().is_some_and(|this| {
            std::fs::metadata(path).ok().is_some_and(|candidate| {
                candidate.dev() == this.dev() && candidate.ino() == this.ino()
            })
        })
    };
    // 1. Environment variable (set by installer wrapper)
    if let Ok(path) = std::env::var("TGREP_REAL_RG") {
        if !path.is_empty()
            && std::path::Path::new(&path).exists()
            && !is_self(std::path::Path::new(&path))
        {
            return Some(PathBuf::from(path));
        }
    }

    // 2. Config file (~/.config/tgrep/rg_path)
    if let Ok(home) = std::env::var("HOME") {
        let config_path = format!("{}/.config/tgrep/rg_path", home);
        if let Ok(path) = std::fs::read_to_string(&config_path) {
            let path = path.trim();
            if !path.is_empty()
                && std::path::Path::new(path).exists()
                && !is_self(std::path::Path::new(path))
            {
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
            if candidate.exists() && !is_self(candidate) {
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

    // The configured command is trusted. This probe catches accidental path
    // mistakes; it does not establish the executable's integrity.
    let version = std::process::Command::new(&rg_path)
        .arg("--version")
        .output();
    match version {
        Ok(out) if out.status.success() => {
            let s = String::from_utf8_lossy(&out.stdout);
            if !s.to_lowercase().contains("ripgrep") {
                eprintln!(
                    "tgrep: refusing to delegate to {} — does not identify as ripgrep",
                    rg_path.display()
                );
                exit(2);
            }
        }
        _ => {
            eprintln!(
                "tgrep: refusing to delegate to {} — --version failed",
                rg_path.display()
            );
            exit(2);
        }
    }

    let err = std::process::Command::new(&rg_path)
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .exec();

    eprintln!(
        "tgrep: failed to exec real rg ({}): {}",
        rg_path.display(),
        err
    );
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

            if !supported_long.contains(flag) || (arg.contains('=') && !args_long.contains(flag)) {
                return false;
            }

            // If this flag takes an argument and no '=' was used, consume next arg
            if arg.find('=').is_none() && args_long.contains(flag) {
                if i + 1 >= args.len() {
                    return false;
                }
                i += 1; // skip the value
            }
            i += 1;
            continue;
        }

        // Short flags (may be combined: -wl)
        if arg.starts_with('-') && arg.len() > 1 {
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
                    if i + 1 >= args.len() {
                        return false;
                    }
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
fn parse_rg_args(args: &[String]) -> (crate::search::SearchConfig, bool) {
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
                "--files-with-matches" => {
                    files_with_matches = true;
                    count = false;
                }
                "--count" => {
                    count = true;
                    files_with_matches = false;
                }
                "--line-number" => line_number = true,
                "--no-line-number" => line_number = false,
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
        if arg.starts_with('-') && arg.len() > 1 {
            let chars: Vec<char> = arg[1..].chars().collect();
            let mut ci = 0;
            while ci < chars.len() {
                let c = chars[ci];
                match c {
                    'w' => word_regexp = true,
                    'l' => {
                        files_with_matches = true;
                        count = false;
                    }
                    'c' => {
                        count = true;
                        files_with_matches = false;
                    }
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

    // With -e, every positional argument names a path.
    if !patterns.is_empty() {
        if let Some(path) = pattern.take() {
            paths.insert(0, path);
        }
    }

    // If -e was used, patterns are from -e; otherwise use positional pattern
    let final_pattern = if !patterns.is_empty() {
        // Join multiple -e patterns with | (like rg does)
        if patterns.len() == 1 {
            patterns[0].clone()
        } else {
            patterns
                .iter()
                .map(|p| format!("(?:{})", p))
                .collect::<Vec<_>>()
                .join("|")
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
                line_number,
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
    use std::io::IsTerminal;
    // Multiple roots can expose the same file through distinct display paths.
    // Delegate to preserve ripgrep output multiplicity and spelling.
    let benefits = paths.len() <= 1
        && !(fixed_strings && patterns.len() > 1)
        && !(paths.is_empty() && !std::io::stdin().is_terminal())
        && !final_pattern.contains('\n')
        && !paths
            .iter()
            .any(|p| p == "-" || !std::path::Path::new(p).is_dir());

    (
        crate::search::SearchConfig {
            pattern: final_pattern,
            paths: paths.iter().map(PathBuf::from).collect(),
            fixed_strings,
            smart_case,
            ignore_case,
            word_regexp,
            files_with_matches,
            line_number,
            count,
            quiet,
            patterns: Vec::new(),
            explain: false,
            indexed_only: false,
            list_files: false,
        },
        benefits,
    )
}

/// Delegate configurations whose effects the indexed frontend cannot represent.
fn supports_rg_config() -> bool {
    let configured = std::env::var("RIPGREP_CONFIG_PATH")
        .ok()
        .filter(|p| !p.is_empty());
    let Some(path) = configured else {
        return !std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".ripgreprc").exists())
            .unwrap_or(false);
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let mut case_flags = 0;
    for line in content
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
    {
        match line {
            "--hidden" => {}
            "--smart-case" | "-S" | "--ignore-case" | "-i" => {
                case_flags += 1;
            }
            _ if line.starts_with("--glob=") => {}
            _ if line
                .strip_prefix("--threads=")
                .is_some_and(|n| n.parse::<usize>().is_ok()) => {}
            _ => return false,
        }
    }
    case_flags <= 1
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
    if !check_supported_flags(&rg_args) || !supports_rg_config() {
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

#[cfg(test)]
mod audit_tests {
    use super::*;
    fn a(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn explicit_pattern_preserves_first_path() {
        let (c, _) = parse_rg_args(&a(&["-e", "needle", "/scratch/root"]));
        assert_eq!(c.paths, vec![PathBuf::from("/scratch/root")]);
        assert_eq!(c.pattern, "needle");
        assert!(c.patterns.is_empty());
    }
    #[test]
    fn ignored_flags_must_delegate() {
        for args in [
            &["-g", "*.txt", "needle"][..],
            &["--hidden", "needle"],
            &["-I", "needle"],
            &["-j", "2", "needle"],
            &["-i", "-S", "Needle"],
        ] {
            assert!(!check_supported_flags(&a(args)), "{:?}", args);
        }
    }
    #[test]
    fn valued_boolean_must_delegate() {
        assert!(!check_supported_flags(&a(&["--quiet=bad", "needle"])));
    }
    #[test]
    fn missing_value_must_delegate() {
        assert!(!check_supported_flags(&a(&["-e"])));
        assert!(!check_supported_flags(&a(&["--regexp"])));
    }
    #[test]
    fn numeric_flag_must_delegate() {
        assert!(!check_supported_flags(&a(&["-1", "needle"])));
    }
    #[test]
    fn fixed_multiple_patterns_must_delegate() {
        let (_, use_tgrep) = parse_rg_args(&a(&["-F", "-e", "foo", "-e", "bar"]));
        assert!(!use_tgrep);
    }
    #[test]
    fn line_number_order_is_preserved() {
        let (c, _) = parse_rg_args(&a(&["--no-line-number", "-n", "needle"]));
        assert!(c.line_number);
    }
    #[test]
    fn explicit_stdin_must_delegate() {
        let (_, use_tgrep) = parse_rg_args(&a(&["needle", "-"]));
        assert!(!use_tgrep);
    }

    #[test]
    fn explicit_self_binary_must_not_recurse() {
        let original: [Option<std::ffi::OsString>; 3] = [
            std::env::var_os("TGREP_REAL_RG"),
            std::env::var_os("PATH"),
            std::env::var_os("HOME"),
        ];
        let executable = std::env::current_exe().unwrap();
        let alias = std::env::temp_dir().join(format!("hax-rg-self-{}", std::process::id()));
        std::os::unix::fs::symlink(&executable, &alias).unwrap();
        std::env::set_var("PATH", "");
        std::env::set_var("HOME", "/tmp/hax-q1-rust-search-audit/no-home");
        let results = [executable, alias.clone()].map(|path| {
            std::env::set_var("TGREP_REAL_RG", path);
            find_real_rg().is_none()
        });
        for (key, value) in ["TGREP_REAL_RG", "PATH", "HOME"].into_iter().zip(original) {
            if let Some(v) = value {
                std::env::set_var(key, v)
            } else {
                std::env::remove_var(key)
            }
        }
        std::fs::remove_file(alias).unwrap();
        assert_eq!(results, [true, true]);
    }
}

#[cfg(test)]
mod output_mode_tests {
    use super::*;
    #[test]
    fn final_count_or_files_flag_selects_one_mode() {
        for (flags, files) in [
            (vec!["-c", "-l"], true),
            (vec!["-l", "-c"], false),
            (vec!["--count", "--files-with-matches"], true),
            (vec!["--files-with-matches", "--count"], false),
            (vec!["-cl"], true),
            (vec!["-lc"], false),
        ] {
            let mut args: Vec<String> = flags.into_iter().map(String::from).collect();
            args.push("needle".into());
            let (config, _) = parse_rg_args(&args);
            assert_eq!(config.files_with_matches, files, "{:?}", args);
            assert_eq!(config.count, !files, "{:?}", args);
        }
    }
}

#[cfg(test)]
mod multiple_root_tests {
    use super::*;
    #[test]
    fn multiple_root_aliases_delegate_without_collapsing_paths() {
        let root = std::env::temp_dir();
        let args = vec![
            "needle".to_string(),
            root.to_string_lossy().into_owned(),
            format!("{}/.", root.display()),
        ];
        let (config, indexed) = parse_rg_args(&args);
        assert_eq!(config.paths.len(), 2);
        assert!(!indexed);
    }
}
