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
    #[arg(short = 'S', long)]
    smart_case: bool,

    /// Case insensitive
    #[arg(short = 'i', long)]
    ignore_case: bool,

    /// Only print matches surrounded by word boundaries (enables hash index fast path)
    #[arg(short = 'w', long)]
    word_regexp: bool,

    /// Filename only
    #[arg(short = 'l', long)]
    files_with_matches: bool,

    /// Line numbers
    #[arg(short = 'n', long)]
    line_number: bool,

    /// Count
    #[arg(short = 'c', long)]
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
        #[arg(long, default_value = "1")]
        threads: usize,
        #[arg(long, default_value = "512MiB")]
        max_memory: String,
        #[arg(long, default_value = "32MiB/s")]
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
            tgrep::index::run_index_command(cmd);
        }
        Some(Commands::Cache { action }) => {
            tgrep::cache::run_cache_command(action);
        }
        None => {
            if cli.pattern.is_none() {
                eprintln!("tgrep: no pattern given");
                std::process::exit(2);
            }
            let config = SearchConfig {
                pattern: cli.pattern.unwrap(),
                paths: cli
                    .paths
                    .iter()
                    .map(|s| std::path::PathBuf::from(s))
                    .collect(),
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
            };
            tgrep::search::run_search(config);
        }
    }
}
