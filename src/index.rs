use crate::build::default_state_dir;
use crate::store;

pub fn run_index_command(cmd: IndexCommand) {
    match cmd {
        IndexCommand::Build { roots, threads, max_memory, io_limit } => {
            crate::build::start_build(&roots, threads, &max_memory, &io_limit);
        }
        IndexCommand::Status => {
            print_status();
        }
        IndexCommand::Recover => {
            let state_dir = default_state_dir();
            let recovered = crate::build::recover_committed_segments(&state_dir);
            if recovered.is_empty() {
                println!("tgrep: no WAL records to recover");
            } else {
                println!("tgrep: recovered {} segment(s) from WAL:", recovered.len());
                for seg in &recovered {
                    println!("  {}", seg);
                }
                println!("Run `tgrep index build <path>` to continue indexing.");
            }
        }
        IndexCommand::Pause => {
            crate::build::pause();
        }
        IndexCommand::Resume => {
            crate::build::resume();
        }
        IndexCommand::Refresh => {
            crate::build::refresh();
        }
        IndexCommand::Compact => {
            crate::build::compact();
        }
        IndexCommand::Rebuild => {
            crate::build::rebuild();
        }
    }
}

fn print_status() {
    let state_dir = default_state_dir();

    // Load manifest
    let manifest = match store::load_manifest(&state_dir) {
        Some(m) => m,
        None => {
            println!("tgrep: no index found at {}", state_dir.display());
            println!("  Run `tgrep index build <path>` to create one.");
            return;
        }
    };

    println!("tgrep index status");
    println!("  state dir:    {}", state_dir.display());
    println!("  generation:   {}", manifest.generation);
    println!("  segments:     {}", manifest.segments.len());

    // WAL status
    let wal_dir = state_dir.join("wal");
    if wal_dir.exists() {
        let wal_entries: Vec<_> = std::fs::read_dir(&wal_dir)
            .map(|d| d.filter_map(|e| e.ok()).collect())
            .unwrap_or_default();
        println!("  wal segments: {}", wal_entries.len());
    }

    // Sum up segment stats
    let segments_dir = state_dir.join("segments");
    let mut total_docs = 0u64;
    let mut total_bytes = 0u64;
    let mut total_postings = 0u64;

    for seg_name in &manifest.segments {
        let seg_path = segments_dir.join(seg_name);
        match store::SegmentReader::open(&seg_path) {
            Ok(reader) => {
                total_docs += reader.doc_count() as u64;
                let postings: Vec<(u32, Vec<u32>)> = reader.iter_postings();
                total_postings += postings.iter().map(|(_, ids)| ids.len() as u64).sum::<u64>();
                let file_size = std::fs::metadata(&seg_path).map(|m| m.len()).unwrap_or(0);
                total_bytes += file_size;
                println!(
                    "    {} — {} docs, {} MB on disk",
                    seg_name,
                    reader.doc_count(),
                    file_size / 1_000_000,
                );
            }
            Err(e) => {
                println!("    {} — ERROR: {}", seg_name, e);
            }
        }
    }

    println!();
    println!("  total docs:     {}", total_docs);
    println!("  total postings: {}", total_postings);
    println!("  disk usage:     {} MB", total_bytes / 1_000_000);

    // Load build state if present
    let build_path = state_dir.join("build.json");
    if let Ok(data) = std::fs::read_to_string(&build_path) {
        if let Ok(bs) = serde_json::from_str::<serde_json::Value>(&data) {
            println!();
            println!("  build status:   {}", bs.get("status").and_then(|v| v.as_str()).unwrap_or("?"));
            println!("  files indexed:  {}", bs.get("files_indexed").and_then(|v| v.as_u64()).unwrap_or(0));
            println!("  bytes indexed:  {} MB", bs.get("bytes_indexed").and_then(|v| v.as_u64()).unwrap_or(0) / 1_000_000);
        }
    }
}

pub enum IndexCommand {
    Build {
        roots: Vec<String>,
        threads: usize,
        max_memory: String,
        io_limit: String,
    },
    Status,
    Recover,
    Pause,
    Resume,
    Refresh,
    Compact,
    Rebuild,
}
