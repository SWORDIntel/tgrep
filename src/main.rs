use clap::{Parser, Subcommand};

use tgrep::cache::CacheSubcommands;
use tgrep::index::IndexCommand;
use tgrep::search::SearchConfig;

#[derive(Parser)]
#[command(
    name = "tgrep",
    about = "Persistent trigram index for repeated searches"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Pattern to search for
    #[arg(index = 1)]
    pattern: Option<String>,

    /// Paths to search
    #[arg(index = 2)]
    paths: Vec<String>,

    /// Fixed string search (-F)
    #[arg(short = 'F', long)]
    fixed_strings: bool,

    /// Smart case
    #[arg(short = 'S', long, overrides_with = "ignore_case")]
    smart_case: bool,

    /// Case insensitive
    #[arg(short = 'i', long, overrides_with = "smart_case")]
    ignore_case: bool,

    /// Only print matches surrounded by word boundaries (enables hash index fast path)
    #[arg(short = 'w', long)]
    word_regexp: bool,

    /// Filename only
    #[arg(short = 'l', long, overrides_with = "count")]
    files_with_matches: bool,

    /// Line numbers
    #[arg(short = 'n', long)]
    line_number: bool,

    /// Count
    #[arg(short = 'c', long, overrides_with = "files_with_matches")]
    count: bool,

    /// Quiet
    #[arg(short = 'q', long)]
    quiet: bool,

    /// Additional patterns
    #[arg(short = 'e', long = "regexp")]
    patterns: Vec<String>,

    /// Explain plan without searching
    #[arg(long)]
    explain: bool,

    /// Skip streaming fallback (indexed candidates only, for benchmarking)
    #[arg(long)]
    indexed_only: bool,

    /// List files that would be searched (debug)
    #[arg(long)]
    list_files: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Build or manage the index
    Index {
        #[command(subcommand)]
        action: IndexSubcommands,
    },
    /// Manage the search-result cache
    Cache {
        #[command(subcommand)]
        action: CacheSubcommands,
    },
}

#[derive(Subcommand)]
enum IndexSubcommands {
    /// Build index for given roots
    Build {
        roots: Vec<String>,
        /// File-discovery worker count (1-64); ingestion and publication remain serial
        #[arg(long, default_value = "1")]
        threads: usize,
        /// auto (total and available RAM plus cgroup headroom), or a positive byte quantity; soft resident-memory guard
        #[arg(long, default_value = "auto")]
        max_memory: String,
        /// auto (startup filesystem probe plus cgroup cap), or a positive byte rate with /s; cooperative pacing
        #[arg(long, default_value = "auto")]
        io_limit: String,
    },
    /// Show index status
    Status,
    /// Recover committed segments from WAL
    Recover,
    /// Pause indexing
    Pause,
    /// Resume indexing
    Resume,
    /// Refresh index with changed files
    Refresh,
    /// Compact segments
    Compact,
    /// Full rebuild
    Rebuild,
}

fn main() {
    let argv: Vec<String> = match std::env::args_os().map(|arg| arg.into_string()).collect() {
        Ok(argv) => argv,
        Err(_) => {
            eprintln!("tgrep: command arguments must be valid UTF-8");
            std::process::exit(2);
        }
    };

    // Detect rg-compat mode: invoked as `rg` via wrapper script or symlink.
    // The installer creates a wrapper that sets TGREP_REAL_RG and execs tgrep.
    // We also detect by argv[0] ending in "rg" (but not "tgrep").
    let prog_name = argv.first().map(|s| s.as_str()).unwrap_or("tgrep");
    let invoked_as_rg = std::env::var("TGREP_REAL_RG").is_ok()
        || (prog_name.ends_with("rg") && !prog_name.ends_with("tgrep"));

    if invoked_as_rg {
        tgrep::rg_compat::run_as_rg(&argv);
    }

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Index { action }) => {
            let cmd = match action {
                IndexSubcommands::Build {
                    roots,
                    threads,
                    max_memory,
                    io_limit,
                } => IndexCommand::Build {
                    roots,
                    threads,
                    max_memory,
                    io_limit,
                },
                IndexSubcommands::Status => IndexCommand::Status,
                IndexSubcommands::Recover => IndexCommand::Recover,
                IndexSubcommands::Pause => IndexCommand::Pause,
                IndexSubcommands::Resume => IndexCommand::Resume,
                IndexSubcommands::Refresh => IndexCommand::Refresh,
                IndexSubcommands::Compact => IndexCommand::Compact,
                IndexSubcommands::Rebuild => IndexCommand::Rebuild,
            };
            if let Err(error) = tgrep::index::run_index_command(cmd) {
                eprintln!("tgrep: index command failed: {error}");
                std::process::exit(2);
            }
        }
        Some(Commands::Cache { action }) => {
            tgrep::cache::run_cache_command(action);
        }
        None => {
            let config = match search_config(cli) {
                Ok(config) => config,
                Err(message) => {
                    eprintln!("tgrep: {message}");
                    std::process::exit(2);
                }
            };
            tgrep::search::run_search(config);
        }
    }
}

fn search_config(mut cli: Cli) -> Result<SearchConfig, &'static str> {
    let pattern = if cli.patterns.is_empty() {
        cli.pattern.ok_or("no pattern given")?
    } else {
        if let Some(path) = cli.pattern.take() {
            cli.paths.insert(0, path);
        }
        cli.patterns.remove(0)
    };
    Ok(SearchConfig {
        pattern,
        paths: cli.paths.iter().map(std::path::PathBuf::from).collect(),
        fixed_strings: cli.fixed_strings,
        smart_case: cli.smart_case,
        ignore_case: cli.ignore_case,
        word_regexp: cli.word_regexp,
        files_with_matches: cli.files_with_matches,
        line_number: cli.line_number,
        count: cli.count,
        quiet: cli.quiet,
        patterns: cli.patterns,
        explain: cli.explain,
        indexed_only: cli.indexed_only,
        list_files: cli.list_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn build_resource_defaults_are_automatic() {
        let cli = Cli::try_parse_from(["tgrep", "index", "build", "/tmp/corpus"]).unwrap();
        match cli.command {
            Some(Commands::Index {
                action:
                    IndexSubcommands::Build {
                        threads,
                        max_memory,
                        io_limit,
                        ..
                    },
            }) => {
                assert_eq!(threads, 1);
                assert_eq!(max_memory, "auto");
                assert_eq!(io_limit, "auto");
            }
            _ => panic!("expected index build"),
        }
    }

    #[test]
    fn regexp_only_uses_default_search_roots() {
        let config =
            search_config(Cli::try_parse_from(["tgrep", "-e", "needle"]).unwrap()).unwrap();
        assert_eq!(config.pattern, "needle");
        assert!(config.patterns.is_empty());
        assert!(config.paths.is_empty());
    }

    #[test]
    fn explicit_regexps_keep_all_positional_paths() {
        let config = search_config(
            Cli::try_parse_from([
                "tgrep", "-F", "-n", "-e", "one", "-e", "two", "a.txt", "b.txt",
            ])
            .unwrap(),
        )
        .unwrap();
        assert_eq!(config.pattern, "one");
        assert_eq!(config.patterns, ["two"]);
        assert_eq!(
            config.paths,
            [PathBuf::from("a.txt"), PathBuf::from("b.txt")]
        );
        assert!(config.fixed_strings && config.line_number);
    }

    #[test]
    fn positional_pattern_and_paths_are_unchanged() {
        let config =
            search_config(Cli::try_parse_from(["tgrep", "needle", "a.txt"]).unwrap()).unwrap();
        assert_eq!(config.pattern, "needle");
        assert_eq!(config.paths, [PathBuf::from("a.txt")]);
        assert!(config.patterns.is_empty());
    }

    #[test]
    fn missing_pattern_still_fails() {
        assert!(search_config(Cli::try_parse_from(["tgrep"]).unwrap()).is_err());
    }
    #[test]
    fn output_mode_last_count_or_filename_flag_wins() {
        for (flags, count) in [
            (vec!["-c", "-l"], false),
            (vec!["-l", "-c"], true),
            (vec!["-cl"], false),
            (vec!["-lc"], true),
            (vec!["--count", "--files-with-matches"], false),
            (vec!["--files-with-matches", "--count"], true),
        ] {
            let args: Vec<_> = ["tgrep"]
                .into_iter()
                .chain(flags.iter().copied())
                .chain(["needle", "a.txt"])
                .collect();
            let config = search_config(Cli::try_parse_from(args).unwrap()).unwrap();
            assert_eq!(config.count, count, "{flags:?}");
            assert_eq!(config.files_with_matches, !count, "{flags:?}");
            assert_eq!(config.pattern, "needle");
        }
    }
    #[test]
    fn last_case_flag_wins_for_uppercase_pattern() {
        for (flags, insensitive) in [
            (vec!["-i", "-S"], false),
            (vec!["-S", "-i"], true),
            (vec!["-iS"], false),
            (vec!["-Si"], true),
            (vec!["--ignore-case", "--smart-case"], false),
            (vec!["--smart-case", "--ignore-case"], true),
        ] {
            let args: Vec<_> = ["tgrep"]
                .into_iter()
                .chain(flags.iter().copied())
                .chain(["Foo", "a.txt"])
                .collect();
            let config = search_config(Cli::try_parse_from(args).unwrap()).unwrap();
            assert_eq!(config.ignore_case, insensitive, "{flags:?}");
            assert_eq!(config.smart_case, !insensitive, "{flags:?}");
            assert_eq!(config.pattern, "Foo");
        }
    }
}
