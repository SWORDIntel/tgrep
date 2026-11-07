use crate::build::default_state_dir;
use crate::store;

pub fn run_index_command(cmd: IndexCommand) -> std::io::Result<()> {
    match cmd {
        IndexCommand::Build {
            roots,
            threads,
            max_memory,
            io_limit,
        } => {
            crate::build::start_build(&roots, threads, &max_memory, &io_limit);
        }
        IndexCommand::Status => {
            print_status(&default_state_dir())?;
        }
        IndexCommand::Recover => {
            let state_dir = default_state_dir();
            let recovered = crate::build::recover_committed_segments(&state_dir)?;
            if recovered.is_empty() {
                println!("tgrep: committed-segment scan found no committed WAL segments");
            } else {
                println!(
                    "tgrep: committed-segment scan retained {} segment(s):",
                    recovered.len()
                );
                for seg in &recovered {
                    println!("  {}", seg);
                }
            }
            println!("The index manifest is unchanged; run `tgrep index build <path>` to publish a new index.");
        }
        IndexCommand::Pause => {
            crate::build::pause()?;
        }
        IndexCommand::Resume => {
            crate::build::resume()?;
        }
        IndexCommand::Refresh => {
            crate::build::refresh()?;
        }
        IndexCommand::Compact => {
            crate::build::compact()?;
        }
        IndexCommand::Rebuild => {
            crate::build::rebuild()?;
        }
    }
    Ok(())
}

fn print_status(state_dir: &std::path::Path) -> std::io::Result<()> {
    // Legacy root IDs must use build metadata captured before the manifest read.
    // A builder binds old roots in the manifest before replacing build.json.
    let build_data = crate::build::read_build_metadata(state_dir);
    let manifest = match store::read_manifest(state_dir) {
        Ok(m) => m,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("tgrep: no index found at {}", state_dir.display());
            println!("  Run `tgrep index build <path>` to create one.");
            return Ok(());
        }
        Err(error) => return Err(error),
    };

    let build_state: Option<serde_json::Value> = match build_data {
        Ok(data) => Some(data),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };

    println!("tgrep index status");
    println!("  state dir:    {}", state_dir.display());
    println!("  generation:   {}", manifest.generation);
    println!("  segments:     {}", manifest.segments.len());
    match indexed_roots(&manifest, build_state.as_ref()) {
        Some(roots) => {
            println!(
                "  {}: {}",
                root_status_label(&manifest, &roots),
                roots.len()
            );
            for root in roots {
                println!("    {}", root);
            }
        }
        None => println!("  legacy root provenance: unavailable"),
    }

    // WAL status
    let wal_dir = state_dir.join("wal");
    match std::fs::read_dir(&wal_dir) {
        Ok(entries) => {
            let mut count = 0;
            for entry in entries {
                entry?;
                count += 1;
            }
            println!("  wal segments: {}", count);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    // Sum up segment stats
    let segments_dir = state_dir.join("segments");
    let mut total_docs = 0u64;
    let mut total_bytes = 0u64;
    let mut total_postings = 0u64;
    let mut segment_error = None;

    for seg_name in &manifest.segments {
        let seg_path = segments_dir.join(seg_name);
        match store::SegmentReader::open(&seg_path) {
            Ok(reader) => {
                total_docs += reader.doc_count() as u64;
                total_postings += reader.posting_count();
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
                segment_error = Some(e);
            }
        }
    }

    println!();
    println!("  total docs:     {}", total_docs);
    println!("  total postings: {}", total_postings);
    println!("  disk usage:     {} MB", total_bytes / 1_000_000);

    if let Some(bs) = build_state {
        println!();
        println!(
            "  build status:   {}",
            bs.get("status")
                .and_then(|value| value.as_str())
                .unwrap_or("?")
        );
        println!(
            "  build progress files: {}",
            bs.get("files_indexed")
                .and_then(|value| value.as_u64())
                .unwrap_or(0)
        );
        println!(
            "  build progress bytes: {} MB",
            bs.get("bytes_indexed")
                .and_then(|value| value.as_u64())
                .unwrap_or(0)
                / 1_000_000
        );
    }
    if let Some(error) = segment_error {
        return Err(error);
    }
    Ok(())
}

fn indexed_roots(
    manifest: &store::Manifest,
    build_state: Option<&serde_json::Value>,
) -> Option<Vec<String>> {
    if let Some(roots) = &manifest.roots {
        return Some(roots.clone());
    }
    build_state?
        .get("roots")?
        .as_array()?
        .iter()
        .map(|value| value.as_str().map(str::to_owned))
        .collect()
}

fn root_status_label(manifest: &store::Manifest, roots: &[String]) -> &'static str {
    if roots
        .iter()
        .any(|root| !std::path::Path::new(root).is_absolute())
    {
        "legacy root provenance (relative; original build directory unknown)"
    } else if manifest.roots.is_some() {
        "indexed roots"
    } else {
        "legacy root provenance (build metadata)"
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch() -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("tgrep-status-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn status_distinguishes_missing_corrupt_and_empty_manifest() {
        let dir = scratch();
        assert!(print_status(&dir).is_ok());
        std::fs::write(dir.join("manifest.json"), b"broken").unwrap();
        assert_eq!(
            print_status(&dir).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        std::fs::write(
            dir.join("manifest.json"),
            br#"{"generation":0,"segments":[],"profile_hash":0}"#,
        )
        .unwrap();
        assert!(print_status(&dir).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn status_reports_unreadable_segment() {
        let dir = scratch();
        std::fs::write(
            dir.join("manifest.json"),
            br#"{"generation":1,"segments":["missing.tgs"],"profile_hash":0}"#,
        )
        .unwrap();
        assert_eq!(
            print_status(&dir).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn status_reports_bad_wal_and_build_state() {
        let dir = scratch();
        std::fs::write(
            dir.join("manifest.json"),
            br#"{"generation":0,"segments":[],"profile_hash":0}"#,
        )
        .unwrap();
        std::fs::write(dir.join("wal"), b"not a directory").unwrap();
        assert!(print_status(&dir).is_err());
        std::fs::remove_file(dir.join("wal")).unwrap();
        std::fs::write(dir.join("build.json"), b"broken").unwrap();
        assert_eq!(
            print_status(&dir).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn status_prefers_bound_roots_and_preserves_legacy_fallback() {
        let state = serde_json::json!({"status":"running","roots":["/new-progress-root"]});
        let mut manifest: store::Manifest = serde_json::from_value(
            serde_json::json!({"generation":1,"segments":[],"profile_hash":0}),
        )
        .unwrap();
        assert_eq!(
            indexed_roots(&manifest, Some(&state)),
            Some(vec!["/new-progress-root".into()])
        );
        manifest.roots = Some(vec!["/old-indexed-root".into()]);
        assert_eq!(indexed_roots(&manifest, Some(&state)), manifest.roots);
        manifest.roots = Some(vec![]);
        assert_eq!(indexed_roots(&manifest, Some(&state)), Some(vec![]));
        manifest.roots = None;
        assert_eq!(indexed_roots(&manifest, None), None);
    }
    #[test]
    fn status_fifo_child() {
        let Some(path) = std::env::var_os("TGREP_STATUS_TEST_FIFO_DIR") else {
            return;
        };
        assert!(print_status(&PathBuf::from(path)).is_err());
    }

    #[test]
    fn status_build_fifo_is_rejected_without_writer() {
        use std::os::unix::ffi::OsStrExt;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let dir = scratch();
        let fifo = dir.join("build.json");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        std::fs::write(
            dir.join("manifest.json"),
            br#"{"generation":0,"segments":[],"profile_hash":0,"roots":["/bound-root"]}"#,
        )
        .unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "index::tests::status_fifo_child"])
            .env("TGREP_STATUS_TEST_FIFO_DIR", &dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                std::fs::remove_dir_all(&dir).unwrap();
                panic!("status blocked while opening build.json FIFO");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn status_relative_roots_remain_uncertified_provenance() {
        let mut manifest: store::Manifest = serde_json::from_value(
            serde_json::json!({"generation":1,"segments":[],"profile_hash":0}),
        )
        .unwrap();
        let relative = vec!["relative/../old-root/".to_string()];
        assert!(
            root_status_label(&manifest, &relative).contains("original build directory unknown")
        );
        manifest.roots = Some(relative.clone());
        assert!(
            root_status_label(&manifest, &relative).contains("original build directory unknown")
        );
        let absolute = vec!["/indexed-root".to_string()];
        manifest.roots = Some(absolute.clone());
        assert_eq!(root_status_label(&manifest, &absolute), "indexed roots");
        manifest.roots = None;
        assert_eq!(
            root_status_label(&manifest, &absolute),
            "legacy root provenance (build metadata)"
        );
    }
}
