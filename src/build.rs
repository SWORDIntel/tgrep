use std::collections::BTreeMap;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::native::{self, KeystoneIndex, Wal};
use crate::store::{self, DocRecord, SegmentWriter};

mod compaction;

// ── File list cache ─────────────────────────────────────────────────

/// Metadata for a single file, stored in the file list cache.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct CachedFileMeta {
    pub path: String, // absolute path
    pub size: u64,
    pub mtime_ns: i64,
    pub inode: u64,
    pub device: u64,
}

/// The file list cache — all files seen during the last build walk.
/// Used during search to skip the filesystem walk when the corpus
/// hasn't changed.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct FileListCache {
    pub roots: Vec<String>,
    pub files: Vec<CachedFileMeta>,
    pub built_at: u64, // unix timestamp
    pub hidden: bool,  // whether hidden files were included
}

/// Save the file list cache to the state directory.
pub fn save_file_list_cache(state_dir: &Path, cache: &FileListCache) -> std::io::Result<()> {
    let json = serde_json::to_string(cache).map_err(std::io::Error::other)?;
    let path = state_dir.join("file_list_cache.json");
    native::atomic_write(&path, json.as_bytes()).map_err(std::io::Error::other)?;
    Ok(())
}

/// Load the file list cache from the state directory.
pub fn load_file_list_cache(state_dir: &Path) -> Option<FileListCache> {
    let path = state_dir.join("file_list_cache.json");
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

const MAX_FILE_SIZE: u64 = 64 * 1024 * 1024; // 64 MiB
const FLUSH_SOURCE_BYTES: u64 = 64 * 1024 * 1024; // 64 MiB source
const _FLUSH_POSTING_COUNT: usize = 2_000_000; // ~128 MiB postings
const FLUSH_TIME_SECS: u64 = 30;
const READ_CHUNK_SIZE: usize = 256 * 1024; // 256 KiB chunks

// ── State directory ─────────────────────────────────────────────────

pub fn default_state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("TGREP_STATE_DIR") {
        return PathBuf::from(dir);
    }
    // XDG Base Directory Specification: persistent state (survives reboots,
    // not user-facing data) belongs in $XDG_STATE_HOME (~/.local/state).
    if let Ok(state_home) = std::env::var("XDG_STATE_HOME") {
        if !state_home.is_empty() {
            return PathBuf::from(state_home).join("tgrep");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".local/state/tgrep");
    }
    PathBuf::from(".tgrep")
}

fn ensure_state_dirs(state_dir: &Path) -> std::io::Result<()> {
    // 0700: the state dir enumerates indexed paths and feeds search results;
    // other local users have no business reading or writing it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mk = |p: &Path| -> std::io::Result<()> {
            fs::create_dir_all(p)?;
            fs::set_permissions(p, fs::Permissions::from_mode(0o700))?;
            Ok(())
        };
        mk(state_dir)?;
        mk(&state_dir.join("segments"))?;
        mk(&state_dir.join("tmp"))?;
        mk(&state_dir.join("wal"))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(state_dir)?;
        fs::create_dir_all(state_dir.join("segments"))?;
        fs::create_dir_all(state_dir.join("tmp"))?;
        fs::create_dir_all(state_dir.join("wal"))?;
        Ok(())
    }
}

// ── WAL recovery ────────────────────────────────────────────────────

/// Recover committed segments from the WAL.
///
/// On startup, this replays the WAL to find segments that were fully
/// committed (BEGIN + INSERT + COMMIT). It returns the list of committed
/// segment names without publishing a new manifest: WAL records do not bind build roots.
/// Committed files are retained. Segments from uncommitted transactions are considered
/// partial and should be cleaned up.
///
/// Refuses while another writer owns the state. WAL and manifest errors are returned without
/// orphan cleanup. A missing WAL directory returns an empty list.
pub fn recover_committed_segments(state_dir: &Path) -> std::io::Result<Vec<String>> {
    if !state_dir.try_exists()? {
        return Ok(Vec::new());
    }
    let _lock = WriterLock::acquire(state_dir)?;
    recover_committed_segments_locked(state_dir)
}

fn recover_committed_segments_locked(state_dir: &Path) -> std::io::Result<Vec<String>> {
    let wal_dir = state_dir.join("wal");
    if !wal_dir.try_exists()? {
        return Ok(Vec::new());
    }
    let wal = Wal::create(&wal_dir).map_err(std::io::Error::other)?;
    let records = wal.replay().map_err(std::io::Error::other)?;

    // Extract committed segment names (INSERT records with segment names as keys)
    let mut segments: Vec<String> = Vec::new();
    for record in &records {
        // op_type 1 = INSERT
        if record.op_type == 1 {
            let name = String::from_utf8(record.key.clone())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let mut parts = Path::new(&name).components();
            if name.contains('/')
                || name.contains('\0')
                || !matches!(parts.next(), Some(std::path::Component::Normal(_)))
                || parts.next().is_some()
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid WAL segment name",
                ));
            }
            segments.push(name);
        }
    }

    segments.sort();
    segments.dedup();
    eprintln!(
        "tgrep: WAL recovery found {} committed segment(s)",
        segments.len()
    );

    // Clean up orphaned segment files (in segments/ but not committed in WAL
    // and not in the current manifest)
    cleanup_orphaned_segments(state_dir, &segments)?;

    Ok(segments)
}

/// Remove segment files that are neither in the committed WAL list nor
/// in the current manifest. These are leftovers from crashed builds.
fn cleanup_orphaned_segments(state_dir: &Path, wal_committed: &[String]) -> std::io::Result<()> {
    let segments_dir = state_dir.join("segments");
    let manifest_segments = match store::read_manifest(state_dir) {
        Ok(m) => m.segments,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    let wal_set: std::collections::HashSet<&String> = wal_committed.iter().collect();
    let manifest_set: std::collections::HashSet<&String> = manifest_segments.iter().collect();
    let entries = match fs::read_dir(&segments_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let file_path = entry.path();
        if !matches!(
            file_path.extension().and_then(|e| e.to_str()),
            Some("tgs" | "thi" | "qwi")
        ) {
            continue;
        }
        let segment_path = file_path.with_extension("tgs");
        if let Some(name) = segment_path.file_name().and_then(|n| n.to_str()) {
            if !wal_set.contains(&name.to_string()) && !manifest_set.contains(&name.to_string()) {
                eprintln!("tgrep: cleaning orphaned segment: {name}");
                fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}

/// Remove segment files no longer named by the published replacement manifest.
/// The caller holds the writer lock and has atomically published the new snapshot.
fn cleanup_unreferenced_segments(state_dir: &Path) -> std::io::Result<()> {
    let manifest = store::read_manifest(state_dir)?;
    let active: std::collections::HashSet<&String> = manifest.segments.iter().collect();
    let segments_dir = state_dir.join("segments");
    for entry in fs::read_dir(&segments_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("tgs" | "thi" | "qwi")
        ) {
            continue;
        }
        let segment_path = path.with_extension("tgs");
        let Some(name) = segment_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !active.contains(&name.to_owned()) {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

/// Checkpoint the WAL after a successful manifest publication.
/// This truncates old WAL entries, keeping the log bounded.
fn wal_checkpoint(state_dir: &Path) {
    let wal_dir = state_dir.join("wal");
    match Wal::create(&wal_dir) {
        Ok(wal) => {
            let lsn = wal.current_lsn();
            if let Err(e) = wal.checkpoint(lsn) {
                eprintln!("tgrep: index already published; warning: WAL checkpoint failed: {e}");
            }
        }
        Err(e) => {
            eprintln!("tgrep: index already published; warning: WAL checkpoint open failed: {e}")
        }
    }
}

// ── Writer lock ──────────────────────────────────────────────────────

struct WriterLock {
    _file: fs::File,
    _path: PathBuf,
}

impl WriterLock {
    fn acquire(state_dir: &Path) -> std::io::Result<Self> {
        let lock_path = state_dir.join("writer.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;
        // Try exclusive lock (non-blocking)
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        let rc = unsafe { libc_flock(fd, 2 | 4) }; // LOCK_EX | LOCK_NB
        if rc != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ResourceBusy,
                "another build/compaction is running (writer.lock held)",
            ));
        }
        Ok(WriterLock {
            _file: file,
            _path: lock_path,
        })
    }
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        let _ = unsafe { libc_flock(self._file.as_raw_fd(), 8) }; // LOCK_UN
    }
}

// Minimal libc flock binding (avoid adding a libc crate dependency)
extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}
unsafe fn libc_flock(fd: i32, op: i32) -> i32 {
    flock(fd, op)
}

// ── Build state ──────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize)]
struct BuildState {
    build_id: String,
    roots: Vec<String>,
    threads: usize,
    max_memory: String,
    io_limit: String,
    status: String, // "running", "paused", "completed"
    files_indexed: u64,
    bytes_indexed: u64,
    segments_written: u64,
    started_at: u64,
    updated_at: u64,
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn write_build_state(state_dir: &Path, bs: &BuildState) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(bs).map_err(std::io::Error::other)?;
    let path = state_dir.join("build.json");
    native::atomic_write(&path, json.as_bytes()).map_err(std::io::Error::other)?;
    Ok(())
}

fn new_build_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{}-{nanos}", std::process::id())
}

fn pause_requested(state_dir: &Path, build_id: &str) -> std::io::Result<bool> {
    let pause_path = state_dir.join("pause.request");
    match fs::read_to_string(pause_path) {
        Ok(requested_id) => Ok(requested_id.trim() == build_id),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn wait_for_resume(state_dir: &Path, build_state: &mut BuildState) -> std::io::Result<()> {
    if !pause_requested(state_dir, &build_state.build_id)? {
        return Ok(());
    }
    build_state.status = "paused".into();
    build_state.updated_at = now_unix_secs();
    write_build_state(state_dir, build_state)?;
    eprintln!("tgrep: build paused at a file boundary; run `tgrep index resume` to continue");
    while pause_requested(state_dir, &build_state.build_id)? {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    build_state.status = "running".into();
    build_state.updated_at = now_unix_secs();
    write_build_state(state_dir, build_state)?;
    eprintln!("tgrep: build resumed");
    Ok(())
}

/// Read one stable regular-file snapshot of optional build progress/legacy root metadata.
pub fn read_build_metadata(state_dir: &Path) -> std::io::Result<serde_json::Value> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(state_dir.join("build.json"))?;
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "build metadata is not a regular file",
        ));
    }
    let limit = before.len().checked_add(1).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "build metadata length overflow",
        )
    })?;
    let mut data = Vec::new();
    (&file).take(limit).read_to_end(&mut data)?;
    let after = file.metadata()?;
    if data.len() as u64 != before.len()
        || before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "build metadata changed while reading",
        ));
    }
    serde_json::from_slice(&data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}
#[cfg(test)]
mod metadata_tests {
    use super::tests::Scratch;
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn reads_regular_json_snapshot_without_small_fixed_cap() {
        let scratch = Scratch::new();
        let expected = serde_json::json!({"roots":["/corpus"], "status":"completed", "label":"x".repeat(256*1024)});
        fs::write(
            scratch.0.join("build.json"),
            serde_json::to_vec(&expected).unwrap(),
        )
        .unwrap();
        assert_eq!(read_build_metadata(&scratch.0).unwrap(), expected);
    }

    #[test]
    fn missing_metadata_remains_not_found() {
        let scratch = Scratch::new();
        assert_eq!(
            read_build_metadata(&scratch.0).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[test]
    fn malformed_json_is_invalid_data() {
        let scratch = Scratch::new();
        fs::write(scratch.0.join("build.json"), b"not JSON").unwrap();
        assert_eq!(
            read_build_metadata(&scratch.0).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn fifo_metadata_is_rejected_without_waiting_for_a_writer() {
        const CHILD: &str = "TGREP_METADATA_FIFO_TEST_CHILD";
        if let Some(path) = std::env::var_os(CHILD) {
            let path = PathBuf::from(path);
            assert_eq!(
                read_build_metadata(&path).unwrap_err().kind(),
                std::io::ErrorKind::InvalidData
            );
            fs::write(path.join("child-finished"), b"checked").unwrap();
            return;
        }
        let scratch = Scratch::new();
        let path = scratch.0.join("build.json");
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "build::metadata_tests::fifo_metadata_is_rejected_without_waiting_for_a_writer",
            ])
            .env(CHILD, &scratch.0)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    let status = child.wait().unwrap();
                    assert!(status.success(), "FIFO metadata child exited {status}");
                    assert!(
                        scratch.0.join("child-finished").exists(),
                        "exact child test did not run"
                    );
                    break;
                }
                Ok(None) if started.elapsed() < std::time::Duration::from_secs(5) => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                result => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("FIFO metadata child failed to finish before deadline: {result:?}");
                }
            }
        }
    }

    #[test]
    fn directory_metadata_is_rejected() {
        let scratch = Scratch::new();
        fs::create_dir(scratch.0.join("build.json")).unwrap();
        assert_eq!(
            read_build_metadata(&scratch.0).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn regular_symlink_metadata_keeps_existing_behavior() {
        let scratch = Scratch::new();
        fs::write(scratch.0.join("target"), b"{\"roots\":[\"/corpus\"]}").unwrap();
        std::os::unix::fs::symlink(scratch.0.join("target"), scratch.0.join("build.json")).unwrap();
        assert_eq!(
            read_build_metadata(&scratch.0).unwrap(),
            serde_json::json!({"roots":["/corpus"]})
        );
    }
}

fn legacy_manifest_roots(state_dir: &Path, allow_empty: bool) -> Result<Vec<String>, String> {
    let metadata = read_build_metadata(state_dir).map_err(|e| e.to_string())?;
    let roots: Vec<String> = serde_json::from_value(
        metadata
            .get("roots")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    )
    .map_err(|e| e.to_string())?;
    if (!allow_empty && roots.is_empty()) || roots.iter().any(|root| !Path::new(root).is_absolute())
    {
        return Err("legacy roots lack an absolute build-root binding".into());
    }
    match metadata.get("status") {
        Some(status) if status.as_str() != Some("completed") => {
            return Err(format!("legacy build status is {status}"));
        }
        None => eprintln!("tgrep: legacy root-only metadata has no completion status; assuming its roots bind the old manifest"),
        _ => {},
    }
    Ok(roots)
}

fn read_source_file(path: &Path) -> std::io::Result<(Vec<u8>, fs::Metadata)> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)?;
    let before = file.metadata()?;
    if !before.is_file() || before.len() > MAX_FILE_SIZE {
        return Err(std::io::Error::other(
            "source is not a bounded regular file",
        ));
    }
    let mut content = vec![0; before.len() as usize];
    (&file).read_exact(&mut content)?;
    let mut extra = [0u8; 1];
    if (&file).read(&mut extra)? != 0 {
        return Err(std::io::Error::other("source changed while indexing"));
    }
    let after = file.metadata()?;
    if content.len() as u64 != after.len()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err(std::io::Error::other("source changed while indexing"));
    }
    Ok((content, after))
}

// ── Build pipeline ───────────────────────────────────────────────────

fn parse_resource_quantity(value: &str, rate: bool) -> Result<u64, &'static str> {
    let quantity = if rate {
        value.strip_suffix("/s").ok_or("rate must end with /s")?
    } else {
        value
    };
    let digits = quantity.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return Err("expected a positive integer quantity");
    }
    let multiplier: u64 = match &quantity[digits..] {
        "" | "B" => 1,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "TB" => 1_000_000_000_000,
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        "TiB" => 1 << 40,
        _ => return Err("expected bytes or B/KB/MB/GB/TB/KiB/MiB/GiB/TiB units"),
    };
    let number: u64 = quantity[..digits]
        .parse()
        .map_err(|_| "quantity overflows u64")?;
    let bytes = number
        .checked_mul(multiplier)
        .ok_or("byte quantity overflows u64")?;
    if bytes == 0 {
        return Err("quantity must be greater than zero");
    }
    Ok(bytes)
}

fn cgroup_v2_path() -> Option<PathBuf> {
    let membership = fs::read_to_string("/proc/self/cgroup").ok()?;
    let group = membership
        .lines()
        .find_map(|line| line.strip_prefix("0::"))?;
    let path = Path::new("/sys/fs/cgroup").join(group.trim_start_matches('/'));
    path.is_dir().then_some(path)
}

fn cgroup_ancestors(path: &Path) -> impl Iterator<Item = &Path> {
    path.ancestors()
        .take_while(|ancestor| ancestor.starts_with("/sys/fs/cgroup"))
}

fn meminfo_bytes(meminfo: &str, key: &str) -> Option<u64> {
    let line = meminfo.lines().find(|line| line.starts_with(key))?;
    let mut fields = line.split_whitespace();
    fields.next()?;
    let kib = fields.next()?.parse::<u64>().ok()?;
    (fields.next()? == "kB")
        .then(|| kib.checked_mul(1024))
        .flatten()
}

fn auto_memory_bytes(meminfo: &str, cgroup: Option<&Path>) -> std::io::Result<u64> {
    let available = meminfo_bytes(meminfo, "MemAvailable:")
        .ok_or_else(|| std::io::Error::other("cannot determine MemAvailable from /proc/meminfo"))?;
    let total = meminfo_bytes(meminfo, "MemTotal:")
        .ok_or_else(|| std::io::Error::other("cannot determine MemTotal from /proc/meminfo"))?;
    let mut budget = (total / 8).min(available / 4);
    if let Some(group) = cgroup {
        for ancestor in cgroup_ancestors(group) {
            let Ok(limit) = fs::read_to_string(ancestor.join("memory.max")) else {
                continue;
            };
            let Ok(limit) = limit.trim().parse::<u64>() else {
                continue;
            };
            let current = fs::read_to_string(ancestor.join("memory.current"))?
                .trim()
                .parse::<u64>()
                .map_err(|_| std::io::Error::other("invalid cgroup memory.current"))?;
            budget = budget.min(limit.saturating_sub(current) / 4);
        }
    }
    if budget == 0 {
        return Err(std::io::Error::other(
            "no available memory for automatic builder budget",
        ));
    }
    Ok(budget)
}

fn cgroup_io_cap(io_max: &str, device: u64) -> Option<u64> {
    let major = (device >> 8) & 0xfff;
    let minor = (device & 0xff) | ((device >> 12) & 0xfff00);
    let key = format!("{major}:{minor}");
    io_max
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            (fields.next()? == key)
                .then(|| {
                    fields
                        .filter_map(|field| {
                            let (name, value) = field.split_once('=')?;
                            if name == "rbps" || name == "wbps" {
                                value.parse::<u64>().ok()
                            } else {
                                None
                            }
                        })
                        .min()
                })
                .flatten()
        })
        .min()
}

fn auto_io_rate(cgroup: Option<&Path>, device: u64) -> Option<u64> {
    let group = cgroup?;
    cgroup_ancestors(group)
        .filter_map(|ancestor| fs::read_to_string(ancestor.join("io.max")).ok())
        .filter_map(|limits| cgroup_io_cap(&limits, device))
        .min()
}

fn io_probe_rate(state_dir: &Path, probe_bytes: usize) -> std::io::Result<u64> {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;

    let path = state_dir.join("tmp").join(format!(
        ".io-probe-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(std::io::Error::other)?
            .as_nanos()
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let result = (|| {
        let mut buffer = vec![0x5a; probe_bytes];
        let started = Instant::now();
        file.write_all(&buffer)?;
        file.sync_all()?;
        let write_elapsed = started.elapsed();
        unsafe {
            libc::posix_fadvise(
                file.as_raw_fd(),
                0,
                probe_bytes as i64,
                libc::POSIX_FADV_DONTNEED,
            );
        }
        drop(file);

        let mut file = fs::File::open(&path)?;
        let started = Instant::now();
        file.read_exact(&mut buffer)?;
        let read_elapsed = started.elapsed();
        let bytes_per_second = |elapsed: std::time::Duration| {
            ((probe_bytes as u128) * 1_000_000_000 / elapsed.as_nanos().max(1))
                .min(u64::MAX as u128) as u64
        };
        Ok(bytes_per_second(write_elapsed)
            .min(bytes_per_second(read_elapsed))
            .saturating_div(4)
            .max(1))
    })();
    if let Err(error) = fs::remove_file(&path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error);
        }
    }
    result
}

fn resolve_build_resources(
    max_memory: &str,
    io_limit: &str,
    state_dir: &Path,
) -> std::io::Result<(String, String)> {
    let cgroup = cgroup_v2_path();
    let memory = if max_memory == "auto" {
        let meminfo = fs::read_to_string("/proc/meminfo")?;
        format!("{}B", auto_memory_bytes(&meminfo, cgroup.as_deref())?)
    } else {
        max_memory.to_string()
    };
    let io = if io_limit == "auto" {
        let device = fs::metadata(state_dir)?.dev();
        let cap = auto_io_rate(cgroup.as_deref(), device);
        if cap == Some(0) {
            return Err(std::io::Error::other("cgroup I/O byte-rate cap is zero"));
        }
        let memory_bytes =
            parse_resource_quantity(&memory, false).map_err(std::io::Error::other)?;
        let probe_bytes = usize::try_from(memory_bytes / 16)
            .unwrap_or(usize::MAX)
            .min(4 * 1024 * 1024);
        if probe_bytes == 0 {
            return Err(std::io::Error::other(
                "memory budget too small for I/O probe",
            ));
        }
        let measured = io_probe_rate(state_dir, probe_bytes)?;
        let rate = cap.map_or(measured, |cap| measured.min(cap));
        format!("{rate}B/s")
    } else {
        io_limit.to_string()
    };
    Ok((memory, io))
}

fn process_io_bytes() -> std::io::Result<(u64, u64)> {
    let counters = fs::read_to_string("/proc/self/io")?;
    let value = |name: &str| -> std::io::Result<u64> {
        counters
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|raw| raw.trim().parse().ok())
            .ok_or_else(|| std::io::Error::other(format!("missing {name} in /proc/self/io")))
    };
    Ok((value("rchar:")?, value("wchar:")?))
}

struct IoPacer {
    started: Instant,
    baseline: (u64, u64),
    bytes_per_second: u64,
}

fn resident_bytes() -> std::io::Result<u64> {
    let statm = fs::read_to_string("/proc/self/statm")?;
    let pages = statm
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| std::io::Error::other("invalid /proc/self/statm resident pages"))?;
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return Err(std::io::Error::other("cannot determine Linux page size"));
    }
    pages
        .checked_mul(page_size as u64)
        .ok_or_else(|| std::io::Error::other("resident memory byte count overflow"))
}

struct MemoryGuard {
    baseline: u64,
    budget: u64,
}

// Reserve room for the per-file peak before reading the file. Besides the
// source buffer, tokenization deduplicates borrowed token slices in a Rust set,
// and the native indexes may grow while the content is being fed. Native
// allocations are observed in resident RSS on later checks; this multiplier
// protects the transient work that occurs before that RSS check.
fn file_working_set_reserve(file_bytes: u64, path_bytes: u64) -> u64 {
    file_bytes
        .saturating_mul(6)
        .saturating_add(path_bytes.saturating_mul(4))
        .saturating_add(
            (std::mem::size_of::<DocRecord>() + std::mem::size_of::<CachedFileMeta>()) as u64,
        )
}

// Finalizing KEYSTONE exports its postings into Rust, then SegmentWriter
// encodes postings and materializes the complete atomic-write image. Reserve
// several index-sized copies plus source-sized slack for metadata and sidecar
// serialization. This is deliberately conservative because QIHSE and
// KEYSTONE do not expose allocation counters for every internal structure.
fn flush_working_set_reserve(keystone_bytes: u64, batch_source_bytes: u64) -> u64 {
    keystone_bytes
        .saturating_mul(4)
        .saturating_add(batch_source_bytes.saturating_mul(2))
}

impl MemoryGuard {
    fn new(budget: u64) -> std::io::Result<Self> {
        Ok(Self {
            baseline: resident_bytes()?,
            budget,
        })
    }

    fn used(&self) -> std::io::Result<u64> {
        Ok(resident_bytes()?.saturating_sub(self.baseline))
    }

    fn check(&self, reserve: u64) -> std::io::Result<u64> {
        let used = self.used()?;
        if used.saturating_add(reserve) > self.budget {
            return Err(std::io::Error::other(format!(
                "builder memory budget exceeded: {used} resident bytes plus {reserve} reserved bytes exceeds {}",
                self.budget
            )));
        }
        Ok(used)
    }
}

impl IoPacer {
    fn new(bytes_per_second: u64) -> std::io::Result<Self> {
        if bytes_per_second == 0 {
            return Err(std::io::Error::other("automatic I/O rate is zero"));
        }
        Ok(Self {
            started: Instant::now(),
            baseline: process_io_bytes()?,
            bytes_per_second,
        })
    }

    fn pace(&self) -> std::io::Result<()> {
        let current = process_io_bytes()?;
        let bytes = current
            .0
            .saturating_sub(self.baseline.0)
            .saturating_add(current.1.saturating_sub(self.baseline.1));
        let seconds = bytes / self.bytes_per_second;
        let nanos = (u128::from(bytes % self.bytes_per_second) * 1_000_000_000
            / u128::from(self.bytes_per_second)) as u32;
        let target = std::time::Duration::new(seconds, nanos);
        if let Some(wait) = target.checked_sub(self.started.elapsed()) {
            std::thread::sleep(wait);
        }
        Ok(())
    }
}

fn validate_build_options(threads: usize, max_memory: &str, io_limit: &str) -> std::io::Result<()> {
    if threads == 0 || threads > 64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--threads must be between 1 and 64",
        ));
    }
    for (flag, value, rate) in [
        ("--max-memory", max_memory, false),
        ("--io-limit", io_limit, true),
    ] {
        if value == "auto" {
            continue;
        }
        parse_resource_quantity(value, rate).map_err(|reason| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid {flag} {value:?}: {reason}"),
            )
        })?;
    }
    Ok(())
}

fn for_each_discovered_file(
    root: &Path,
    threads: usize,
    excluded_state: Option<&Path>,
    mut consume: impl FnMut(PathBuf) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let excluded_state = excluded_state.map(Path::to_path_buf);
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .add_custom_ignore_filename(".rgignore")
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .threads(threads)
        .filter_entry(move |entry| {
            !excluded_state
                .as_ref()
                .is_some_and(|state| entry.path().starts_with(state))
        })
        .build_parallel();

    std::thread::scope(|scope| {
        let (sender, receiver) = crossbeam_channel::bounded(threads.saturating_mul(4));
        scope.spawn(move || {
            walker.run(|| {
                let sender = sender.clone();
                Box::new(move |entry| {
                    let result = match entry {
                        Ok(entry) if entry.file_type().is_some_and(|kind| kind.is_file()) => {
                            Some(Ok(entry.into_path()))
                        }
                        Ok(_) => None,
                        Err(error) => {
                            Some(Err(std::io::Error::other(format!("walk failed: {error}"))))
                        }
                    };
                    match result {
                        None => ignore::WalkState::Continue,
                        Some(result) => {
                            if sender.send(result).is_ok() {
                                ignore::WalkState::Continue
                            } else {
                                ignore::WalkState::Quit
                            }
                        }
                    }
                })
            });
        });
        for entry in receiver {
            consume(entry?)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod resource_tests {
    use super::tests::Scratch;
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn roots_inside_state_are_rejected_without_replacing_published_state() {
        fn snapshot(directory: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
            let mut files = std::collections::BTreeMap::new();
            for entry in fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files.extend(snapshot(&path));
                } else {
                    files.insert(path.clone(), fs::read(path).unwrap());
                }
            }
            files
        }

        let scratch = Scratch::new();
        let source = scratch.corpus("source", "one.txt");
        let state = scratch.state();
        run_build(
            &[source.to_str().unwrap().into()],
            1,
            "512MiB",
            "1GiB/s",
            &state,
        )
        .unwrap();
        let nested = state.join("nested-source");
        fs::create_dir(&nested).unwrap();
        let nested_file = nested.join("two.txt");
        fs::write(&nested_file, b"needle\n").unwrap();
        let root_alias = scratch.0.join("source-alias");
        let state_alias = scratch.0.join("state-alias");
        std::os::unix::fs::symlink(&nested, &root_alias).unwrap();
        std::os::unix::fs::symlink(&state, &state_alias).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();
        let before = snapshot(&state);
        for configured_state in [&state, &state_alias] {
            for invalid_root in [&state, &nested, &nested_file, &root_alias] {
                // Reject the whole request even when a valid root precedes it.
                let roots = [
                    source.to_str().unwrap().into(),
                    invalid_root.to_str().unwrap().into(),
                ];
                let error = run_build(&roots, 1, "512MiB", "1GiB/s", configured_state).unwrap_err();
                assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
                assert!(error.to_string().contains("state directory"));
                assert_eq!(snapshot(&state), before);
                assert_eq!(
                    fs::metadata(&state).unwrap().permissions().mode() & 0o777,
                    0o755
                );
            }
        }
    }

    #[test]
    fn nested_state_is_excluded_on_initial_and_replacement_builds() {
        let scratch = Scratch::new();
        let root = scratch.corpus("home", "source.txt");
        let state = root.join(".local/state/tgrep");
        let alias = scratch.0.join("state-alias");
        fs::create_dir_all(&state).unwrap();
        std::os::unix::fs::symlink(&state, &alias).unwrap();
        for configured_state in [&state, &alias, &state] {
            run_build(
                &[root.to_str().unwrap().into()],
                2,
                "512MiB",
                "1GiB/s",
                configured_state,
            )
            .unwrap();
            let manifest = store::read_manifest(&state).unwrap();
            assert_eq!(manifest.segments.len(), 1);
            let reader =
                store::SegmentReader::open(&state.join("segments").join(&manifest.segments[0]))
                    .unwrap();
            assert_eq!(reader.doc_count(), 1);
            assert_eq!(reader.get_doc(0).unwrap().path, b"source.txt");
            let cache = load_file_list_cache(&state).unwrap();
            assert_eq!(cache.files.len(), 1);
            assert_eq!(
                cache.files[0].path,
                root.join("source.txt").to_str().unwrap()
            );
            assert!(!state.join("segments").read_dir().unwrap().any(|entry| {
                entry
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "thi")
            }));
        }
    }

    #[test]
    fn bom_encoded_text_remains_scan_only_and_binary_tokens_are_indexed() {
        let scratch = Scratch::new();
        let root = scratch.corpus("encoding", "plain.txt");
        fs::write(root.join("little.txt"), b"\xff\xfen\0e\0e\0d\0l\0e\0").unwrap();
        fs::write(root.join("big.txt"), b"\xfe\xff\0n\0e\0e\0d\0l\0e").unwrap();
        fs::write(root.join("binary.txt"), b"needle\0tail").unwrap();
        let state = scratch.state();
        run_build(
            &[root.to_str().unwrap().into()],
            1,
            "512MiB",
            "1GiB/s",
            &state,
        )
        .unwrap();
        let manifest = store::read_manifest(&state).unwrap();
        let segment = state.join("segments").join(&manifest.segments[0]);
        let reader = store::SegmentReader::open(&segment).unwrap();
        let mut docs: Vec<_> = reader.iter_docs().map(|doc| doc.path).collect();
        docs.sort();
        assert_eq!(docs, vec![b"binary.txt".to_vec(), b"plain.txt".to_vec()]);
        let qwi = native::MmapWordIndex::load_for_segment(
            &segment.with_extension("qwi"),
            reader.generation(),
            reader.metadata_fingerprint(),
        )
        .unwrap();
        assert_eq!(qwi.search(b"needle", 10).unwrap().len(), 2);
    }

    #[test]
    fn discovery_respects_rgignore() {
        let scratch = Scratch::new();
        let root = scratch.corpus("ignore", "visible.txt");
        fs::write(root.join("ignored.txt"), b"needle").unwrap();
        fs::write(root.join(".rgignore"), b"ignored.txt\n").unwrap();
        let mut files = Vec::new();
        for_each_discovered_file(&root, 1, None, |path| {
            files.push(path);
            Ok(())
        })
        .unwrap();
        assert!(files.contains(&root.join("visible.txt")));
        assert!(!files.contains(&root.join("ignored.txt")));
    }

    #[test]
    fn memory_reservations_cover_file_and_flush_temporary_work() {
        let file_reserve = file_working_set_reserve(1024, 64);
        assert!(file_reserve >= 6 * 1024 + 4 * 64);
        assert!(flush_working_set_reserve(2048, 1024) >= 4 * 2048 + 2 * 1024);
        assert_eq!(file_working_set_reserve(u64::MAX, 1), u64::MAX);
        assert_eq!(flush_working_set_reserve(u64::MAX, 1), u64::MAX);
    }

    #[test]
    fn pause_requests_are_bound_to_the_build_that_created_them() {
        let scratch = Scratch::new();
        let state_dir = scratch.state();
        ensure_state_dirs(&state_dir).unwrap();
        fs::write(state_dir.join("pause.request"), b"old-build\n").unwrap();
        assert!(!pause_requested(&state_dir, "new-build").unwrap());
        fs::write(state_dir.join("pause.request"), b"new-build\n").unwrap();
        assert!(pause_requested(&state_dir, "new-build").unwrap());
    }

    #[test]
    fn resume_after_interruption_restarts_saved_roots_and_clears_stale_request() {
        let scratch = Scratch::new();
        let root = scratch.corpus("resume", "one.txt");
        let roots = vec![root.to_string_lossy().into_owned()];
        let state_dir = scratch.state();
        run_build(&roots, 1, "512MiB", "32MiB/s", &state_dir).unwrap();
        let mut state: BuildState =
            serde_json::from_slice(&fs::read(state_dir.join("build.json")).unwrap()).unwrap();
        let previous_id = state.build_id.clone();
        state.status = "paused".into();
        write_build_state(&state_dir, &state).unwrap();
        fs::write(state_dir.join("pause.request"), format!("{previous_id}\n")).unwrap();

        resume_at(&state_dir).unwrap();

        let resumed: BuildState =
            serde_json::from_slice(&fs::read(state_dir.join("build.json")).unwrap()).unwrap();
        assert_eq!(resumed.status, "completed");
        assert_ne!(resumed.build_id, previous_id);
        assert!(!state_dir.join("pause.request").exists());
        assert_eq!(store::read_manifest(&state_dir).unwrap().roots, Some(roots));
    }

    #[test]
    fn compact_keeps_a_single_valid_snapshot_without_reindexing() {
        let scratch = Scratch::new();
        let root = scratch.corpus("compact", "one.txt");
        let state_dir = scratch.state();
        run_build(
            &[root.to_string_lossy().into_owned()],
            1,
            "512MiB",
            "32MiB/s",
            &state_dir,
        )
        .unwrap();
        let old_manifest = store::read_manifest(&state_dir).unwrap();
        let old_segments = old_manifest.segments.clone();
        compaction::compact_at(&state_dir, "512MiB", "32MiB/s").unwrap();
        let manifest = store::read_manifest(&state_dir).unwrap();
        assert_eq!(manifest.roots, old_manifest.roots);
        assert_eq!(manifest.segments, old_segments);
        assert!(old_segments
            .iter()
            .all(|old| state_dir.join("segments").join(old).exists()));
        assert!(WriterLock::acquire(&state_dir).is_ok());
    }

    #[test]
    fn io_pacer_accounts_for_logical_write_bytes() {
        let scratch = Scratch::new();
        let pacer = IoPacer::new(1 << 20).unwrap();
        let started = Instant::now();
        fs::write(scratch.0.join("paced-write"), vec![0x5a; 32 << 10]).unwrap();
        pacer.pace().unwrap();
        assert!(started.elapsed() >= std::time::Duration::from_millis(20));
    }

    #[test]
    fn too_small_memory_budget_preserves_unpublished_manifest() {
        let scratch = Scratch::new();
        let root = scratch.corpus("small-budget", "one.txt");
        let state = scratch.state();
        let roots = [root.to_string_lossy().into_owned()];
        run_build(&roots, 1, "512MiB", "32MiB/s", &state).unwrap();
        let published = fs::read(state.join("manifest.json")).unwrap();
        let error = run_build(&roots, 1, "32B", "32MiB/s", &state).unwrap_err();
        assert!(error.to_string().contains("builder memory budget exceeded"));
        assert_eq!(fs::read(state.join("manifest.json")).unwrap(), published);
    }

    #[test]
    fn auto_memory_scales_with_available_ram() {
        assert_eq!(
            auto_memory_bytes("MemTotal: 8388608 kB\nMemAvailable: 1048576 kB\n", None).unwrap(),
            256 << 20
        );
        assert_eq!(
            auto_memory_bytes("MemTotal: 134217728 kB\nMemAvailable: 67108864 kB\n", None).unwrap(),
            16 << 30
        );
        assert!(auto_memory_bytes("MemTotal: 1 kB\nMemAvailable: invalid kB\n", None).is_err());
        assert!(auto_memory_bytes("MemTotal: 1 kB\nMemAvailable: 0 kB\n", None).is_err());
    }

    #[test]
    fn auto_io_uses_matching_device_byte_caps_only() {
        let device = (8 << 8) | 16;
        let limits = "8:0 rbps=1048576 wbps=max\n8:16 rbps=33554432 wbps=16777216\n";
        assert_eq!(cgroup_io_cap(limits, device), Some(16 << 20));
        assert_eq!(cgroup_io_cap(limits, 1 << 8), None);
        assert_eq!(cgroup_io_cap("8:16 rbps=max wbps=max", device), None);
        assert_eq!(cgroup_io_cap("8:16 rbps=0 wbps=max", device), Some(0));
    }

    #[test]
    fn auto_settings_resolve_before_recording_progress() {
        let scratch = Scratch::new();
        let root = scratch.corpus("auto-corpus", "one.txt");
        let state = scratch.state();
        run_build(
            &[root.to_string_lossy().into_owned()],
            1,
            "auto",
            "auto",
            &state,
        )
        .unwrap();
        let progress: BuildState =
            serde_json::from_slice(&fs::read(state.join("build.json")).unwrap()).unwrap();
        assert!(parse_resource_quantity(&progress.max_memory, false).is_ok());
        assert!(
            parse_resource_quantity(&progress.io_limit, true).is_ok()
                || progress.io_limit == "0B/s"
        );
        assert!(!state.join("tmp").read_dir().unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".io-probe-")));
    }

    #[test]
    fn automatic_io_probe_fits_small_explicit_memory_budget() {
        let scratch = Scratch::new();
        let state = scratch.state();
        ensure_state_dirs(&state).unwrap();
        let (memory, io) = resolve_build_resources("8MiB", "auto", &state).unwrap();
        assert_eq!(memory, "8MiB");
        assert!(parse_resource_quantity(&io, true).is_ok());
        assert!(!state.join("tmp").read_dir().unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".io-probe-")));
    }

    #[test]
    fn source_reader_uses_exact_file_length_and_rejects_symlinks() {
        let scratch = Scratch::new();
        let path = scratch.0.join("source");
        for size in [0, 1, READ_CHUNK_SIZE + 1] {
            let expected = vec![b'x'; size];
            fs::write(&path, &expected).unwrap();
            let (content, metadata) = read_source_file(&path).unwrap();
            assert_eq!(content, expected);
            assert_eq!(metadata.len(), size as u64);
        }
        let link = scratch.0.join("source-link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read_source_file(&link).is_err());
    }

    #[test]
    fn accepts_documented_units_and_checked_boundary() {
        for (value, expected) in [
            ("1", 1),
            ("2KB", 2_000),
            ("0002MB", 2_000_000),
            ("512MiB", 512 << 20),
            ("1GiB", 1 << 30),
            ("1TiB", 1 << 40),
            ("18446744073709551615B", u64::MAX),
        ] {
            assert_eq!(parse_resource_quantity(value, false), Ok(expected));
            assert_eq!(
                parse_resource_quantity(&format!("{value}/s"), true),
                Ok(expected)
            );
        }
    }

    #[test]
    fn rejects_invalid_zero_and_overflowing_options() {
        validate_build_options(1, "auto", "auto").unwrap();
        for value in [
            "",
            "-1",
            "+1",
            "1.5MiB",
            "1unknown",
            "0",
            "0MiB",
            "18446744073709551616B",
            "18446744073709551615KiB",
        ] {
            let error = validate_build_options(1, value, "32MiB/s").unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("--max-memory"));
        }
        for value in [
            "",
            "32MiB",
            "0B/s",
            "1unknown/s",
            "18446744073709551616B/s",
            "18446744073709551615KiB/s",
        ] {
            assert!(validate_build_options(1, "512MiB", value)
                .unwrap_err()
                .to_string()
                .contains("--io-limit"));
        }
        assert!(validate_build_options(0, "512MiB", "32MiB/s")
            .unwrap_err()
            .to_string()
            .contains("--threads"));
        assert!(validate_build_options(65, "512MiB", "32MiB/s")
            .unwrap_err()
            .to_string()
            .contains("--threads"));
    }

    #[test]
    fn invalid_settings_do_not_create_state() {
        for (threads, memory, rate) in [
            (0, "512MiB", "32MiB/s"),
            (1, "invalid", "32MiB/s"),
            (1, "512MiB", "18446744073709551615KiB/s"),
        ] {
            let scratch = Scratch::new();
            let root = scratch.corpus("resource-corpus", "one.txt");
            let roots = vec![root.to_str().unwrap().into()];
            assert_eq!(
                run_build(&roots, threads, memory, rate, &scratch.state())
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::InvalidInput
            );
            assert!(!scratch.state().exists());
        }
    }

    #[test]
    fn invalid_settings_preserve_existing_state_and_permissions() {
        let scratch = Scratch::new();
        let root = scratch.corpus("resource-corpus", "one.txt");
        let roots = vec![root.to_str().unwrap().into()];
        let state = scratch.state();
        fs::create_dir(&state).unwrap();
        fs::write(state.join("manifest.json"), b"existing manifest sentinel").unwrap();
        fs::write(state.join("build.json"), b"existing progress sentinel").unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();
        let error = run_build(&roots, 1, "18446744073709551615MiB", "32MiB/s", &state).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(
            fs::read(state.join("manifest.json")).unwrap(),
            b"existing manifest sentinel"
        );
        assert_eq!(
            fs::read(state.join("build.json")).unwrap(),
            b"existing progress sentinel"
        );
        assert_eq!(
            fs::metadata(&state).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(!state.join("writer.lock").exists());
    }

    #[test]
    fn parallel_discovery_preserves_root_ids_and_file_set() {
        let scratch = Scratch::new();
        let first = scratch.corpus("first", "first.txt");
        let second = scratch.corpus("second", "second.txt");
        for i in 0..16 {
            fs::write(first.join(format!("file-{i}.txt")), b"alpha needle").unwrap();
            fs::write(second.join(format!("file-{i}.txt")), b"beta needle").unwrap();
        }
        let roots = vec![
            first.to_str().unwrap().into(),
            second.to_str().unwrap().into(),
        ];
        let mut results = Vec::new();
        for threads in [1, 4] {
            let state = scratch.0.join(format!("state-{threads}"));
            run_build(&roots, threads, "512MiB", "32MiB/s", &state).unwrap();
            let manifest = store::read_manifest(&state).unwrap();
            let mut files = Vec::new();
            for segment in manifest.segments {
                let reader =
                    store::SegmentReader::open(&state.join("segments").join(segment)).unwrap();
                files.extend(
                    reader
                        .iter_docs()
                        .map(|doc| (doc.root_id, doc.path.to_vec())),
                );
            }
            files.sort();
            assert_eq!(files.len(), 34);
            results.push(files);
        }
        assert_eq!(results[0], results[1]);
        assert_eq!(results[0].iter().filter(|(root, _)| *root == 0).count(), 17);
        assert_eq!(results[0].iter().filter(|(root, _)| *root == 1).count(), 17);
    }

    #[test]
    fn consumer_error_releases_discovery_workers() {
        let scratch = Scratch::new();
        let root = scratch.corpus("cancel", "first.txt");
        for i in 0..256 {
            fs::write(root.join(format!("file-{i}.txt")), b"content").unwrap();
        }
        let mut received = 0;
        let error = for_each_discovered_file(&root, 4, None, |_| {
            received += 1;
            std::thread::sleep(std::time::Duration::from_millis(20));
            Err(std::io::Error::other("consumer stopped"))
        })
        .unwrap_err();
        assert_eq!(received, 1);
        assert_eq!(error.to_string(), "consumer stopped");
    }
}

pub fn start_build(roots: &[String], threads: usize, max_memory: &str, io_limit: &str) {
    let state_dir = default_state_dir();
    if let Err(e) = run_build(roots, threads, max_memory, io_limit, &state_dir) {
        eprintln!("tgrep: build failed: {}", e);
        std::process::exit(1);
    }
}

fn run_build(
    roots: &[String],
    threads: usize,
    max_memory: &str,
    io_limit: &str,
    state_dir: &Path,
) -> std::io::Result<()> {
    run_build_checked(roots, threads, max_memory, io_limit, state_dir, None)
}

fn run_build_checked(
    roots: &[String],
    threads: usize,
    max_memory: &str,
    io_limit: &str,
    state_dir: &Path,
    expected_current_roots: Option<&[String]>,
) -> std::io::Result<()> {
    validate_build_options(threads, max_memory, io_limit)?;
    if roots.is_empty() {
        return Err(std::io::Error::other("no roots supplied"));
    }
    let roots: Vec<String> = roots
        .iter()
        .map(|root| {
            let path = fs::canonicalize(root)?;
            let metadata = fs::metadata(&path)?;
            if !metadata.is_file() && !metadata.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "build root is not a regular file or directory",
                ));
            }
            path.to_str()
                .map(String::from)
                .ok_or_else(|| std::io::Error::other("root path is not UTF-8"))
        })
        .collect::<std::io::Result<_>>()?;
    let reject_state_roots = |canonical_state: &Path| {
        if roots
            .iter()
            .any(|root| Path::new(root).starts_with(canonical_state))
        {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "build roots must be outside the tgrep state directory",
            ))
        } else {
            Ok(())
        }
    };
    // Reject existing state roots before even changing permissions, progress or
    // WAL. Canonical paths cover aliases of both the state and source roots.
    match fs::canonicalize(state_dir) {
        Ok(canonical_state) => reject_state_roots(&canonical_state)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    ensure_state_dirs(state_dir)?;
    let canonical_state = fs::canonicalize(state_dir)?;
    reject_state_roots(&canonical_state)?;
    let _lock = WriterLock::acquire(state_dir)?;
    if let Some(expected_roots) = expected_current_roots {
        let current_roots = current_index_roots(state_dir)?;
        if current_roots != expected_roots {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "index roots changed before replacement began; retry the operation",
            ));
        }
    }
    let (resolved_memory, resolved_io) = resolve_build_resources(max_memory, io_limit, state_dir)?;
    let memory_guard = MemoryGuard::new(
        parse_resource_quantity(&resolved_memory, false).map_err(std::io::Error::other)?,
    )?;
    let io_pacer =
        IoPacer::new(parse_resource_quantity(&resolved_io, true).map_err(std::io::Error::other)?)?;
    eprintln!("tgrep: builder settings: memory {resolved_memory}, I/O {resolved_io}; --threads controls discovery only; I/O pacing and resident-memory checks are cooperative");

    // Keep the published snapshot intact until the full replacement is ready.
    let segments_dir = state_dir.join("segments");
    let wal_dir = state_dir.join("wal");
    let mut defer_progress = false;
    let mut last_generation = match store::read_manifest(state_dir) {
        Ok(mut manifest) => {
            if manifest.roots.is_none() {
                // Bind determinate legacy roots before progress metadata can change.
                let frozen = legacy_manifest_roots(state_dir, manifest.segments.is_empty())
                    .and_then(|roots| {
                        manifest.roots = Some(roots);
                        store::write_manifest(state_dir, &manifest).map_err(|e| e.to_string())
                    });
                if let Err(reason) = frozen {
                    defer_progress = true;
                    eprintln!("tgrep: old root binding cannot be verified/frozen ({reason}); preserving legacy manifest and progress while staging a new bound index");
                }
            } else if manifest.roots.as_ref().is_some_and(|roots| {
                (!manifest.segments.is_empty() && roots.is_empty())
                    || roots.iter().any(|root| !Path::new(root).is_absolute())
            }) {
                defer_progress = true;
                eprintln!("tgrep: old manifest root binding is ambiguous; preserving metadata while staging a new bound index");
            }
            manifest.generation
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => return Err(e),
    };
    for entry in fs::read_dir(&segments_dir)? {
        let name = entry?.file_name();
        if let Some(id) = name
            .to_str()
            .and_then(|n| n.strip_prefix("seg_"))
            .and_then(|n| n.split('.').next())
            .and_then(|n| n.parse::<u64>().ok())
        {
            last_generation = last_generation.max(id);
        }
    }
    // Open WAL for crash-safe segment publication.
    let wal = Wal::create(&wal_dir).map_err(std::io::Error::other)?;
    let first_generation = last_generation
        .max(wal.current_lsn())
        .checked_add(1)
        .ok_or_else(|| std::io::Error::other("segment generation exhausted"))?;

    let mut txn_id = first_generation;

    let started_at = now_unix_secs();
    let mut build_state = BuildState {
        build_id: new_build_id(),
        roots: roots.to_vec(),
        threads,
        max_memory: resolved_memory,
        io_limit: resolved_io,
        status: "running".into(),
        files_indexed: 0,
        bytes_indexed: 0,
        segments_written: 0,
        started_at,
        updated_at: started_at,
    };
    if !defer_progress {
        write_build_state(state_dir, &build_state)?;
        io_pacer.pace()?;
    } else {
        eprintln!(
            "tgrep: staging new index; progress metadata deferred until bound manifest publication"
        );
    }

    // Fresh build uses new segment names and replaces the manifest only when complete.
    let mut generation = first_generation;
    let mut all_segment_names: Vec<String> = Vec::new();

    let mut keystone = KeystoneIndex::new(4096).map_err(std::io::Error::other)?;

    let mut qwi_index = native::WordIndexBuilder::create().map_err(std::io::Error::other)?;

    let mut doc_records: Vec<DocRecord> = Vec::new();
    let mut batch_source_bytes: u64 = 0;
    let mut batch_start = Instant::now();
    let mut batch_rss_floor: u64 = 0;
    let mut doc_id: u32 = 0;
    let mut root_id: u32 = 0;
    let mut cached_files: Vec<CachedFileMeta> = Vec::new();

    for root_str in &roots {
        let root_path = PathBuf::from(root_str);
        if !root_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("root disappeared: {root_str}"),
            ));
        }

        for_each_discovered_file(&root_path, threads, Some(&canonical_state), |path| {
            let meta = match fs::metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    return Err(std::io::Error::other(format!(
                        "stat {}: {e}",
                        path.display()
                    )));
                }
            };

            let file_size = meta.len();
            if file_size > MAX_FILE_SIZE {
                // Skip large files — they'll be scanned at search time
                return Ok(());
            }

            let path_bytes = u64::try_from(path.as_os_str().as_bytes().len()).unwrap_or(u64::MAX);
            memory_guard.check(file_working_set_reserve(file_size, path_bytes))?;

            let (content, meta) = read_source_file(&path)?;
            let file_size = content.len() as u64;

            // grep-searcher decodes UTF-16 BOM text. Raw-byte trigrams cannot
            // safely exclude a match in that decoded text, so leave it unindexed.
            if content.starts_with(b"\xff\xfe") || content.starts_with(b"\xfe\xff") {
                io_pacer.pace()?;
                return Ok(());
            }

            // Get relative path from root
            let rel_path = path.strip_prefix(&root_path).unwrap_or(&path);

            // Feed to KEYSTONE streaming ingestion
            let stream_result = {
                let mut stream = match keystone.begin_document(rel_path.to_str()) {
                    Ok(s) => s,
                    Err(e) => {
                        return Err(std::io::Error::other(format!(
                            "keystone {}: {e}",
                            path.display()
                        )));
                    }
                };

                // Feed in chunks
                let mut fed_ok = true;
                for chunk in content.chunks(READ_CHUNK_SIZE) {
                    if let Err(e) = stream.feed(chunk) {
                        eprintln!("tgrep: feed error {}: {}", path.display(), e);
                        fed_ok = false;
                        break;
                    }
                }

                if fed_ok {
                    stream.finish(Some(&mut doc_id))
                } else {
                    // stream.cancel() via Drop
                    Err("feed failed".into())
                }
            };

            if let Err(e) = stream_result {
                return Err(std::io::Error::other(format!(
                    "index {}: {e}",
                    path.display()
                )));
            }

            // Word candidates are always verified against live content. Binary
            // files can contain matches, so NUL does not exclude their tokens.
            add_tokens_to_qwi_index(&mut qwi_index, &content, doc_id)?;
            drop(content);
            io_pacer.pace()?;

            // Collect doc record
            let mtime_ns = meta
                .mtime()
                .checked_mul(1_000_000_000)
                .and_then(|n| n.checked_add(meta.mtime_nsec()))
                .ok_or_else(|| std::io::Error::other("mtime outside nanosecond range"))?;
            let ctime_ns = meta
                .ctime()
                .checked_mul(1_000_000_000)
                .and_then(|n| n.checked_add(meta.ctime_nsec()))
                .ok_or_else(|| std::io::Error::other("ctime outside nanosecond range"))?;
            doc_records.push(DocRecord {
                local_id: doc_id,
                root_id,
                path: rel_path.as_os_str().as_bytes().to_vec(),
                byte_length: file_size,
                device: meta.dev(),
                inode: meta.ino(),
                mtime_ns,
                ctime_ns,
            });

            // Collect file metadata for the file list cache
            cached_files.push(CachedFileMeta {
                path: path.to_string_lossy().to_string(),
                size: file_size,
                mtime_ns,
                inode: meta.ino(),
                device: meta.dev(),
            });
            // Account for persistent per-file metadata immediately, including
            // the final file in a corpus (which otherwise has no next-file
            // check to observe its allocations).
            memory_guard.check(0)?;

            doc_id = doc_id
                .checked_add(1)
                .ok_or_else(|| std::io::Error::other("document ID exhausted"))?;
            build_state.files_indexed += 1;
            build_state.bytes_indexed += file_size;
            batch_source_bytes += file_size;

            wait_for_resume(state_dir, &mut build_state)?;

            // Check flush thresholds
            let elapsed = batch_start.elapsed().as_secs();
            let ks_mem = keystone.memory_usage();
            let resident_used = memory_guard.check(0)?;
            let should_flush = batch_source_bytes >= FLUSH_SOURCE_BYTES
                || ks_mem as u64 >= FLUSH_SOURCE_BYTES * 2
                || resident_used.saturating_sub(batch_rss_floor) >= memory_guard.budget / 2
                || elapsed >= FLUSH_TIME_SECS;

            if should_flush && !doc_records.is_empty() {
                memory_guard.check(flush_working_set_reserve(ks_mem as u64, batch_source_bytes))?;
                // WAL: log segment publication as a transaction
                if wal.begin(txn_id) == 0 {
                    return Err(std::io::Error::other("WAL begin failed"));
                }

                let flush_result = flush_batch(
                    &mut keystone,
                    &mut qwi_index,
                    &doc_records,
                    generation,
                    state_dir,
                )
                .and_then(|name| {
                    memory_guard.check(0)?;
                    io_pacer.pace()?;
                    Ok(name)
                });

                match flush_result {
                    Ok(seg_name) => {
                        // Log the segment in the WAL
                        let metadata = format!(
                            "{{\"generation\":{},\"files\":{},\"bytes\":{}}}",
                            generation,
                            doc_records.len(),
                            batch_source_bytes
                        );
                        if wal.log_segment(txn_id, &seg_name, metadata.as_bytes()) == 0 {
                            return Err(std::io::Error::other("WAL segment append failed"));
                        }

                        all_segment_names.push(seg_name);
                        generation = generation
                            .checked_add(1)
                            .ok_or_else(|| std::io::Error::other("segment generation exhausted"))?;
                        build_state.segments_written += 1;
                        build_state.updated_at = now_unix_secs();
                        if !defer_progress {
                            write_build_state(state_dir, &build_state)?;
                            io_pacer.pace()?;
                        }

                        wal.commit(txn_id).map_err(std::io::Error::other)?;
                        io_pacer.pace()?;

                        // Reset for next batch
                        doc_records.clear();
                        batch_source_bytes = 0;
                        batch_start = Instant::now();
                        batch_rss_floor = memory_guard.used()?;
                        doc_id = 0;

                        eprintln!(
                            "tgrep: flushed segment {} ({} files, {} MB source)",
                            build_state.segments_written,
                            build_state.files_indexed,
                            build_state.bytes_indexed / 1_000_000,
                        );
                    }
                    Err(e) => {
                        // Abort the WAL transaction
                        let _ = wal.abort(txn_id);
                        return Err(e);
                    }
                }
                txn_id = txn_id
                    .checked_add(1)
                    .ok_or_else(|| std::io::Error::other("transaction ID exhausted"))?;
            }
            Ok(())
        })?;

        root_id = root_id
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("root ID exhausted"))?;
    }
    wait_for_resume(state_dir, &mut build_state)?;

    // Flush remaining docs
    if !doc_records.is_empty() {
        memory_guard.check(flush_working_set_reserve(
            keystone.memory_usage() as u64,
            batch_source_bytes,
        ))?;
        if wal.begin(txn_id) == 0 {
            return Err(std::io::Error::other("WAL begin failed"));
        }
        let flush_result = flush_batch(
            &mut keystone,
            &mut qwi_index,
            &doc_records,
            generation,
            state_dir,
        )
        .and_then(|name| {
            memory_guard.check(0)?;
            io_pacer.pace()?;
            Ok(name)
        });
        match flush_result {
            Ok(seg_name) => {
                let metadata = format!(
                    "{{\"generation\":{},\"files\":{},\"bytes\":{}}}",
                    generation,
                    doc_records.len(),
                    batch_source_bytes
                );
                if wal.log_segment(txn_id, &seg_name, metadata.as_bytes()) == 0 {
                    return Err(std::io::Error::other("WAL segment append failed"));
                }
                all_segment_names.push(seg_name);
                build_state.segments_written += 1;
            }
            Err(e) => {
                let _ = wal.abort(txn_id);
                return Err(e);
            }
        }
    }

    if !doc_records.is_empty() {
        wal.commit(txn_id).map_err(std::io::Error::other)?;
        io_pacer.pace()?;
    }
    wait_for_resume(state_dir, &mut build_state)?;
    store::publish_manifest_with_roots(state_dir, &all_segment_names, generation, &roots).map_err(
        |error| {
            std::io::Error::new(
                error.kind(),
                format!("manifest publication failed; replacement may already be visible: {error}"),
            )
        },
    )?;
    let pace_published = || {
        if let Err(error) = io_pacer.pace() {
            eprintln!("tgrep: index already published; warning: I/O pacing failed: {error}");
        }
    };
    pace_published();
    wal_checkpoint(state_dir);
    pace_published();
    if let Err(error) = cleanup_unreferenced_segments(state_dir) {
        eprintln!(
            "tgrep: index already published; warning: obsolete segment cleanup failed: {error}"
        );
    }

    build_state.status = "completed".into();
    build_state.updated_at = now_unix_secs();
    write_build_state(state_dir, &build_state).map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("index already published; failed to update build state: {e}"),
        )
    })?;
    pace_published();

    // Save file list cache for search-time walk optimization
    let cache = FileListCache {
        roots: roots.to_vec(),
        files: cached_files,
        built_at: started_at,
        hidden: true, // build always includes hidden files
    };
    if let Err(e) = save_file_list_cache(state_dir, &cache) {
        eprintln!(
            "tgrep: index already published; warning: failed to save file list cache: {}",
            e
        );
    }
    pace_published();

    eprintln!(
        "tgrep: build complete — {} files, {} MB, {} segments",
        build_state.files_indexed,
        build_state.bytes_indexed / 1_000_000,
        build_state.segments_written,
    );

    Ok(())
}

/// Finalize KEYSTONE, visit postings, build and flush a segment.
fn flush_batch(
    keystone: &mut KeystoneIndex,
    qwi_index: &mut native::WordIndexBuilder,
    doc_records: &[DocRecord],
    generation: u64,
    state_dir: &Path,
) -> std::io::Result<String> {
    keystone.finalize().map_err(std::io::Error::other)?;

    // Collect postings from KEYSTONE visitor
    let postings = collect_postings(keystone)?;

    // Build segment
    let seg_path = state_dir
        .join("tmp")
        .join(format!("seg_{:08}.tgs", generation));
    let mut writer = SegmentWriter::new(seg_path, generation);
    for doc in doc_records {
        writer.add_doc_record(doc.clone());
    }
    writer.set_postings(postings);

    let segments_dir = state_dir.join("segments");
    let fingerprint = writer.flush(&segments_dir)?;

    let seg_name = format!("seg_{:08}.tgs", generation);

    // Save the optional word sidecar bound to the exact segment metadata.
    let qwi_path = segments_dir.join(format!("seg_{:08}.qwi", generation));
    if let Err(e) = qwi_index.save_for_segment(&qwi_path, generation, fingerprint) {
        eprintln!(
            "tgrep: ERROR: qwi word index save failed for seg_{:08}: {} \
             (word searches will use trigram or streaming fallback)",
            generation, e
        );
        if let Err(remove) = fs::remove_file(&qwi_path) {
            if remove.kind() != std::io::ErrorKind::NotFound {
                return Err(remove);
            }
        }
    }

    // Reset KEYSTONE for next batch
    // (We create a new index since KEYSTONE doesn't have a reset API)
    *keystone = KeystoneIndex::new(4096).map_err(std::io::Error::other)?;

    // Reset QIHSE word index for next batch
    *qwi_index = native::WordIndexBuilder::create().map_err(std::io::Error::other)?;

    Ok(seg_name)
}

/// Collect all postings from KEYSTONE via the visitor trampoline.
fn collect_postings(idx: &KeystoneIndex) -> std::io::Result<BTreeMap<u32, Vec<u32>>> {
    idx.collect_postings().map_err(std::io::Error::other)
}

/// Tokenize file content and add unique tokens to the QIHSE word index.
/// Tokens are ASCII alphanumeric/underscore runs, deduplicated per document.
fn add_tokens_to_qwi_index(
    qwi: &mut native::WordIndexBuilder,
    content: &[u8],
    doc_id: u32,
) -> std::io::Result<()> {
    let mut seen = std::collections::HashSet::new();
    let mut start = None;
    for (i, &b) in content.iter().enumerate() {
        let is_word = b.is_ascii_alphanumeric() || b == b'_';
        if is_word {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start {
            let token = &content[s..i];
            if token.len() >= 2 && seen.insert(token.to_vec()) {
                qwi.add(token, doc_id).map_err(std::io::Error::other)?;
            }
            start = None;
        }
    }
    if let Some(s) = start {
        let token = &content[s..];
        if token.len() >= 2 && seen.insert(token.to_vec()) {
            qwi.add(token, doc_id).map_err(std::io::Error::other)?;
        }
    }
    Ok(())
}

fn current_index_roots(state_dir: &Path) -> std::io::Result<Vec<String>> {
    let manifest = store::read_manifest(state_dir)?;
    if let Some(roots) = manifest.roots {
        if roots.is_empty() {
            return Err(std::io::Error::other("index manifest has no bound roots"));
        }
        return Ok(roots);
    }
    legacy_manifest_roots(state_dir, manifest.segments.is_empty()).map_err(std::io::Error::other)
}

fn replace_current_index(action: &str) -> std::io::Result<()> {
    replace_current_index_at(action, &default_state_dir())
}

fn replace_current_index_at(action: &str, state_dir: &Path) -> std::io::Result<()> {
    let roots = current_index_roots(state_dir)?;
    eprintln!(
        "tgrep: {action} performs a full source reindex of {} root(s)",
        roots.len()
    );
    run_build_checked(&roots, 1, "auto", "auto", state_dir, Some(&roots))
}

pub fn pause() -> std::io::Result<()> {
    let state_dir = default_state_dir();
    let state = read_build_metadata(&state_dir)?;
    if state.get("status").and_then(|value| value.as_str()) != Some("running") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no running index build",
        ));
    }
    let build_id = state
        .get("build_id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| std::io::Error::other("running build has no identity"))?;
    match WriterLock::acquire(&state_dir) {
        Ok(_lock) => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no index build is running",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::ResourceBusy => {
            let current_state = read_build_metadata(&state_dir)?;
            if current_state
                .get("build_id")
                .and_then(|value| value.as_str())
                != Some(build_id)
                || current_state.get("status").and_then(|value| value.as_str()) != Some("running")
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "the build changed before the pause request was recorded",
                ));
            }
            native::atomic_write(
                &state_dir.join("pause.request"),
                format!("{build_id}\n").as_bytes(),
            )
            .map_err(std::io::Error::other)?;
            eprintln!("tgrep: pause requested; build will stop at the next file boundary");
            Ok(())
        }
        Err(error) => Err(error),
    }
}

pub fn resume() -> std::io::Result<()> {
    resume_at(&default_state_dir())
}

fn resume_at(state_dir: &Path) -> std::io::Result<()> {
    let pause_path = state_dir.join("pause.request");

    let lock = match WriterLock::acquire(state_dir) {
        Ok(lock) => Some(lock),
        Err(error) if error.kind() == std::io::ErrorKind::ResourceBusy => None,
        Err(error) => return Err(error),
    };

    // Read state after attempting to acquire the writer lock. When the lock is
    // available, this prevents restarting stale roots/options from a build that
    // completed or was replaced after an earlier, unlocked state read.
    let state = read_build_metadata(state_dir)?;
    let build_id = state
        .get("build_id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| std::io::Error::other("build metadata has no identity"))?
        .to_owned();
    let status = state
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");

    if lock.is_none() {
        if status != "running" || !pause_requested(state_dir, &build_id)? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no matching live pause request",
            ));
        }
        match fs::remove_file(pause_path) {
            Ok(()) => {
                eprintln!("tgrep: resume requested");
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "pause request was already cleared",
                ));
            }
            Err(error) => return Err(error),
        }
    }

    if !matches!(status, "paused" | "running") {
        let _ = fs::remove_file(pause_path);
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no interrupted or paused build to resume",
        ));
    }

    // A free writer lock means the paused build exited or was interrupted. Rewalk its roots
    // with the saved options; completed builds are rejected above.
    eprintln!("tgrep: restarting the interrupted build from its saved roots and options");
    let roots: Vec<String> = serde_json::from_value(
        state
            .get("roots")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    )
    .map_err(std::io::Error::other)?;
    let threads = state
        .get("threads")
        .and_then(|value| value.as_u64())
        .unwrap_or(1) as usize;
    let memory = state
        .get("max_memory")
        .and_then(|value| value.as_str())
        .unwrap_or("auto")
        .to_owned();
    let io = state
        .get("io_limit")
        .and_then(|value| value.as_str())
        .unwrap_or("auto")
        .to_owned();
    if let Err(error) = fs::remove_file(pause_path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error);
        }
    }
    drop(lock);
    run_build(&roots, threads, &memory, &io, state_dir)
}

pub fn refresh() -> std::io::Result<()> {
    replace_current_index("refresh")
}

pub fn compact() -> std::io::Result<()> {
    compaction::compact_at(&default_state_dir(), "auto", "auto")
}

pub fn rebuild() -> std::io::Result<()> {
    replace_current_index("rebuild")
}

#[cfg(test)]
mod tests {
    use super::{ensure_state_dirs, recover_committed_segments, run_build, BuildState, WriterLock};
    use crate::native::Wal;
    use crate::store::{self, DocRecord, Manifest, SegmentReader, SegmentWriter};
    use std::collections::BTreeMap;
    use std::ffi::{c_char, c_int, c_void, CString, OsString};
    use std::fs;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    const PREVIOUS_SEGMENT: &str = "seg_00000001.tgs";

    pub(super) struct Scratch(pub(super) PathBuf);

    impl Scratch {
        pub(super) fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            for _ in 0..128 {
                let path = std::env::temp_dir().join(format!(
                    "tgrep-build-tests-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create {}: {error}", path.display()),
                }
            }
            panic!("could not create an owned test directory");
        }

        pub(super) fn state(&self) -> PathBuf {
            self.0.join("state")
        }

        pub(super) fn corpus(&self, name: &str, filename: &str) -> PathBuf {
            let directory = self.0.join(name);
            fs::create_dir(&directory).unwrap();
            fs::write(directory.join(filename), b"needle alpha beta gamma\n").unwrap();
            fs::canonicalize(directory).unwrap()
        }

        fn previous_index(&self) -> Manifest {
            let state = self.state();
            ensure_state_dirs(&state).unwrap();
            let root = self.corpus("old", "previous.txt");
            let metadata = fs::metadata(root.join("previous.txt")).unwrap();
            let mut writer = SegmentWriter::new(state.join("tmp/previous"), 1);
            writer.add_document(
                DocRecord {
                    local_id: 0,
                    root_id: 0,
                    path: b"previous.txt".to_vec(),
                    byte_length: metadata.len(),
                    device: metadata.dev(),
                    inode: metadata.ino(),
                    mtime_ns: metadata.mtime() * 1_000_000_000 + metadata.mtime_nsec(),
                    ctime_ns: metadata.ctime() * 1_000_000_000 + metadata.ctime_nsec(),
                },
                &[0x616c70],
            );
            writer.flush(&state.join("segments")).unwrap();
            store::publish_manifest_with_roots(
                &state,
                &[PREVIOUS_SEGMENT.into()],
                1,
                &[root.to_str().unwrap().into()],
            )
            .unwrap();
            store::read_manifest(&state).unwrap()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.0) {
                if std::thread::panicking() {
                    eprintln!("remove {}: {error}", self.0.display());
                } else {
                    panic!("remove {}: {error}", self.0.display());
                }
            }
        }
    }

    #[derive(Debug, PartialEq)]
    struct PublishedSnapshot {
        manifest: Vec<u8>,
        segments: BTreeMap<OsString, Vec<u8>>,
    }

    fn directory_bytes(directory: &Path) -> BTreeMap<OsString, Vec<u8>> {
        fs::read_dir(directory)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                assert!(entry.file_type().unwrap().is_file());
                (entry.file_name(), fs::read(entry.path()).unwrap())
            })
            .collect()
    }

    fn published_snapshot(state: &Path) -> PublishedSnapshot {
        PublishedSnapshot {
            manifest: fs::read(state.join("manifest.json")).unwrap(),
            segments: directory_bytes(&state.join("segments")),
        }
    }

    fn append_committed_segment(state: &Path, name: &str) {
        let wal = Wal::create(&state.join("wal")).unwrap();
        let transaction = wal.current_lsn().checked_add(1).unwrap();
        assert_ne!(wal.begin(transaction), 0);
        assert_ne!(wal.log_segment(transaction, name, b"{}"), 0);
        wal.commit(transaction).unwrap();
    }

    // log_segment uses a C string. The public length-delimited append API can represent a
    // malformed embedded NUL without depending on the native Rust handle or WAL file format.
    extern "C" {
        fn tgrep_wal_create(directory: *const c_char) -> *mut c_void;
        fn tgrep_wal_destroy(wal: *mut c_void);
        fn tgrep_wal_current_lsn(wal: *mut c_void) -> u64;
        fn tgrep_wal_begin(wal: *mut c_void, transaction: u64) -> u64;
        fn tgrep_wal_commit(wal: *mut c_void, transaction: u64) -> c_int;
        fn qihse_wal_append(
            wal: *mut c_void,
            transaction: u64,
            engine: u8,
            operation: c_int,
            key: *const c_void,
            key_length: u32,
            value: *const c_void,
            value_length: u32,
        ) -> u64;
    }

    struct RawWal(*mut c_void);

    impl Drop for RawWal {
        fn drop(&mut self) {
            unsafe { tgrep_wal_destroy(self.0) };
        }
    }

    fn append_committed_nul_key(state: &Path) {
        let directory = CString::new(state.join("wal").as_os_str().as_bytes()).unwrap();
        let key = b"pending.tgs\0suffix";
        let metadata = b"{}";
        let wal = RawWal(unsafe { tgrep_wal_create(directory.as_ptr()) });
        assert!(!wal.0.is_null());
        unsafe {
            let transaction = tgrep_wal_current_lsn(wal.0).checked_add(1).unwrap();
            assert_ne!(tgrep_wal_begin(wal.0, transaction), 0);
            assert_ne!(
                qihse_wal_append(
                    wal.0,
                    transaction,
                    0,
                    1, // QIHSE_WAL_OP_INSERT from the public qihse_wal.h enum.
                    key.as_ptr().cast(),
                    key.len().try_into().unwrap(),
                    metadata.as_ptr().cast(),
                    metadata.len().try_into().unwrap(),
                ),
                0
            );
            assert_eq!(tgrep_wal_commit(wal.0, transaction), 0);
        }
    }

    #[test]
    fn invalid_manifest_names_stop_recovery_before_segment_cleanup() {
        for name in [
            "seg_00000001.tgs/",
            "seg_00000001.tgs/.",
            "seg_00000001.tgs\0suffix",
        ] {
            let scratch = Scratch::new();
            let mut manifest = scratch.previous_index();
            let state = scratch.state();
            drop(Wal::create(&state.join("wal")).unwrap());
            manifest.segments = vec![name.into()];
            fs::write(
                state.join("manifest.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            let before = published_snapshot(&state);

            let error = recover_committed_segments(&state).unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{name:?}");
            assert_eq!(published_snapshot(&state), before, "{name:?}");
        }
    }

    #[test]
    fn invalid_committed_wal_names_stop_recovery_before_segment_cleanup() {
        for name in ["pending.tgs/", "pending.tgs/.", "pending.tgs\0suffix"] {
            let scratch = Scratch::new();
            scratch.previous_index();
            let state = scratch.state();
            fs::copy(
                state.join("segments").join(PREVIOUS_SEGMENT),
                state.join("segments/pending.tgs"),
            )
            .unwrap();
            if name.contains('\0') {
                append_committed_nul_key(&state);
            } else {
                append_committed_segment(&state, name);
            }
            let before = published_snapshot(&state);

            let error = recover_committed_segments(&state).unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{name:?}");
            assert_eq!(published_snapshot(&state), before, "{name:?}");
        }
    }

    #[test]
    fn held_writer_lock_protects_build_and_recovery() {
        let scratch = Scratch::new();
        scratch.previous_index();
        let state = scratch.state();
        let corpus = scratch.corpus("new", "current.txt");
        fs::copy(
            state.join("segments").join(PREVIOUS_SEGMENT),
            state.join("segments/orphan.tgs"),
        )
        .unwrap();
        append_committed_segment(&state, PREVIOUS_SEGMENT);
        let before = published_snapshot(&state);
        let wal_before = directory_bytes(&state.join("wal"));
        let lock = WriterLock::acquire(&state).unwrap();

        assert_eq!(
            recover_committed_segments(&state).unwrap_err().kind(),
            io::ErrorKind::ResourceBusy
        );
        assert_eq!(
            run_build(
                &[corpus.to_str().unwrap().into()],
                1,
                "512MiB",
                "32MiB/s",
                &state
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::ResourceBusy
        );
        assert_eq!(published_snapshot(&state), before);
        assert_eq!(directory_bytes(&state.join("wal")), wal_before);

        drop(lock);
        recover_committed_segments(&state).unwrap();
        assert!(!state.join("segments/orphan.tgs").exists());
        assert!(state.join("segments").join(PREVIOUS_SEGMENT).exists());
    }

    #[test]
    fn missing_root_preserves_published_snapshot_and_wal() {
        let scratch = Scratch::new();
        scratch.previous_index();
        let state = scratch.state();
        append_committed_segment(&state, PREVIOUS_SEGMENT);
        let before = published_snapshot(&state);
        let wal_before = directory_bytes(&state.join("wal"));
        let missing = scratch.0.join("missing");

        let error = run_build(
            &[missing.to_str().unwrap().into()],
            1,
            "512MiB",
            "32MiB/s",
            &state,
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(published_snapshot(&state), before);
        assert_eq!(directory_bytes(&state.join("wal")), wal_before);
    }

    #[test]
    fn tiny_rebuild_binds_new_segments_to_canonical_roots_including_legacy() {
        for legacy in [false, true] {
            let scratch = Scratch::new();
            let mut previous = scratch.previous_index();
            let state = scratch.state();
            let old_roots = previous.roots.clone().unwrap();
            if legacy {
                previous.roots = None;
                store::write_manifest(&state, &previous).unwrap();
                fs::write(
                    state.join("build.json"),
                    serde_json::to_vec(&serde_json::json!({
                        "roots": old_roots,
                        "status": "completed",
                    }))
                    .unwrap(),
                )
                .unwrap();
            }
            append_committed_segment(&state, PREVIOUS_SEGMENT);
            let old_segment_path = state.join("segments").join(PREVIOUS_SEGMENT);
            let old_reader = SegmentReader::open(&old_segment_path).unwrap();
            let corpus = scratch.corpus("new", "current.txt");
            let input = corpus.join("../new");
            let expected_roots = vec![corpus.to_str().unwrap().to_owned()];

            run_build(
                &[input.to_str().unwrap().into()],
                1,
                "512MiB",
                "32MiB/s",
                &state,
            )
            .unwrap();

            let manifest = store::read_manifest(&state).unwrap();
            assert_eq!(manifest.roots.as_ref(), Some(&expected_roots));
            assert!(manifest.generation > previous.generation);
            assert_eq!(manifest.segments.len(), 1);
            assert_ne!(manifest.segments, previous.segments);
            assert!(!old_segment_path.exists());
            assert_eq!(old_reader.get_doc(0).unwrap().path, b"previous.txt");
            let reader =
                SegmentReader::open(&state.join("segments").join(&manifest.segments[0])).unwrap();
            let document = reader.get_doc(0).unwrap();
            assert_eq!(reader.doc_count(), 1);
            assert!(reader.posting_count() > 0);
            assert_eq!(document.root_id, 0);
            assert_eq!(document.path, b"current.txt");
            let metadata = fs::metadata(corpus.join("current.txt")).unwrap();
            assert_eq!(document.byte_length, metadata.len());
            assert_eq!(document.device, metadata.dev());
            assert_eq!(document.inode, metadata.ino());
            let progress: BuildState =
                serde_json::from_slice(&fs::read(state.join("build.json")).unwrap()).unwrap();
            assert_eq!(progress.status, "completed");
            assert_eq!(progress.roots, expected_roots);
        }
    }
}
