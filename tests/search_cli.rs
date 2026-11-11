// SPDX-License-Identifier: LicenseRef-SRSL-1.1

use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct ChildProcess(Option<Child>);

impl Drop for ChildProcess {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct Fixture {
    root: PathBuf,
    corpus: PathBuf,
    state: PathBuf,
    config: PathBuf,
    next_command: Cell<u64>,
}

impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "tgrep-search-cli-{}-{stamp}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        DirBuilder::new().mode(0o700).create(&root).unwrap();
        let fixture = Self {
            corpus: root.join("corpus"),
            state: root.join("state"),
            config: root.join("ripgrep-config"),
            root,
            next_command: Cell::new(0),
        };
        for directory in [
            "corpus",
            "home",
            "xdg-config",
            "xdg-state",
            "xdg-cache",
            "xdg-data",
            "xdg-runtime",
            "tmp",
        ] {
            fs::create_dir(fixture.root.join(directory)).unwrap();
        }
        fixture.set_config(&[]);
        for (name, contents) in files {
            fs::write(fixture.corpus.join(name), contents).unwrap();
        }
        fixture
    }

    fn set_config(&self, flags: &[&str]) {
        fs::write(&self.config, format!("--threads=4\n{}\n", flags.join("\n"))).unwrap();
    }

    fn run(&self, executable: &Path, args: &[OsString]) -> Output {
        self.run_with_rg(executable, args, None)
    }

    fn run_with_rg(&self, executable: &Path, args: &[OsString], rg: Option<&Path>) -> Output {
        let sequence = self.next_command.get();
        self.next_command.set(sequence + 1);
        let stdout = self.root.join(format!("command-{sequence}.stdout"));
        let stderr = self.root.join(format!("command-{sequence}.stderr"));
        let mut command = Command::new(executable);
        command
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8")
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("xdg-config"))
            .env("XDG_STATE_HOME", self.root.join("xdg-state"))
            .env("XDG_CACHE_HOME", self.root.join("xdg-cache"))
            .env("XDG_DATA_HOME", self.root.join("xdg-data"))
            .env("XDG_RUNTIME_DIR", self.root.join("xdg-runtime"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("TGREP_STATE_DIR", &self.state)
            .env("TGREP_NO_PREFETCH", "1")
            .env("RIPGREP_CONFIG_PATH", &self.config)
            .env_remove("TGREP_REAL_RG")
            .current_dir(&self.corpus)
            .stdin(Stdio::null())
            .stdout(Stdio::from(File::create(&stdout).unwrap()))
            .stderr(Stdio::from(File::create(&stderr).unwrap()));
        if let Some(rg) = rg {
            command.env("TGREP_REAL_RG", rg);
        }
        let mut child = ChildProcess(Some(command.spawn().unwrap()));
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if child.0.as_mut().unwrap().try_wait().unwrap().is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out: {executable:?} {args:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let status = child.0.take().unwrap().wait().unwrap();
        Output {
            status,
            stdout: fs::read(stdout).unwrap(),
            stderr: fs::read(stderr).unwrap(),
        }
    }

    fn build(&self) {
        let output = self.run(
            binary(),
            &[
                "index".into(),
                "build".into(),
                self.corpus.as_os_str().to_owned(),
                "--threads".into(),
                "1".into(),
            ],
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(self.state.join("manifest.json").is_file());
    }

    fn query(&self, args: &[&str]) -> Vec<OsString> {
        let mut args: Vec<OsString> = args.iter().map(OsString::from).collect();
        args.push(self.corpus.as_os_str().to_owned());
        args
    }

    fn assert_parity(&self, rg: &Path, args: &[OsString], config_flags: &[&str]) -> Output {
        let mut oracle: Vec<OsString> = ["--no-config", "-j", "4"].map(OsString::from).into();
        oracle.extend(config_flags.iter().map(OsString::from));
        oracle.extend_from_slice(args);
        let expected = self.run(rg, &oracle);
        assert!(
            matches!(expected.status.code(), Some(0 | 1)),
            "rg oracle failed: {}",
            String::from_utf8_lossy(&expected.stderr)
        );
        let actual = self.run(binary(), args);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "{args:?}: {}",
            String::from_utf8_lossy(&actual.stderr)
        );
        assert_eq!(
            sorted_lines(&actual.stdout),
            sorted_lines(&expected.stdout),
            "{args:?}"
        );
        actual
    }

    fn assert_wrapper_parity(&self, rg: &Path, args: &[OsString]) -> Output {
        let expected = self.run(rg, args);
        let actual = self.run_with_rg(binary(), args, Some(rg));
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "{args:?}: {}",
            String::from_utf8_lossy(&actual.stderr)
        );
        assert_eq!(
            sorted_lines(&actual.stdout),
            sorted_lines(&expected.stdout),
            "{args:?}"
        );
        actual
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root) {
            if std::thread::panicking() {
                eprintln!("remove {}: {error}", self.root.display());
            } else {
                panic!("remove {}: {error}", self.root.display());
            }
        }
    }
}

fn binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_tgrep"))
}

fn oracle() -> &'static Path {
    static ORACLE: OnceLock<PathBuf> = OnceLock::new();
    ORACLE
        .get_or_init(|| {
            let configured = std::env::var_os("TGREP_TEST_RG").map(PathBuf::from);
            let candidates = configured.into_iter().chain(
                std::env::var_os("PATH")
                    .into_iter()
                    .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
                    .map(|directory| directory.join("rg")),
            );
            candidates
                .filter(|path| path.is_file())
                .find(|path| {
                    Command::new(path)
                        .arg("--version")
                        .output()
                        .is_ok_and(|output| {
                            output.status.success()
                                && String::from_utf8_lossy(&output.stdout).starts_with("ripgrep ")
                        })
                })
                .and_then(|path| path.canonicalize().ok())
                .expect("search CLI tests require real ripgrep in PATH, or TGREP_TEST_RG")
        })
        .as_path()
}

fn sorted_lines(bytes: &[u8]) -> Vec<&[u8]> {
    let mut lines: Vec<_> = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    lines.sort_unstable();
    lines
}

#[test]
fn broad_cost_routing_preserves_live_changes_and_indexed_only_semantics() {
    let rg = oracle();
    let fixture = Fixture::new(&[]);
    for id in 0..160 {
        let contents = if id == 0 {
            "commonword rarecostmarker\n"
        } else if id < 120 {
            "commonword\n"
        } else {
            "otherword\n"
        };
        fs::write(fixture.corpus.join(format!("file-{id:03}.txt")), contents).unwrap();
    }
    fixture.build();
    let explain = fixture.run(
        binary(),
        &fixture.query(&["--explain", "-l", "-F", "commonword"]),
    );
    assert!(explain.status.success());
    assert!(explain.stdout.is_empty());
    let explanation = String::from_utf8_lossy(&explain.stderr);
    assert!(explanation.contains("streaming preferred"), "{explanation}");
    assert!(explanation.contains("plan = streaming"), "{explanation}");
    let initial = fixture.assert_parity(rg, &fixture.query(&["-l", "-F", "commonword"]), &[]);
    assert_eq!(sorted_lines(&initial.stdout).len(), 120);
    assert!(!fixture.state.join("search_cache.qsc").exists());

    fs::write(fixture.corpus.join("file-120.txt"), "commonword\n").unwrap();
    fs::write(fixture.corpus.join("new.txt"), "commonword\n").unwrap();
    let live = fixture.assert_parity(rg, &fixture.query(&["-l", "-F", "commonword"]), &[]);
    assert_eq!(sorted_lines(&live.stdout).len(), 122);
    let snapshot = fixture.run(
        binary(),
        &fixture.query(&["--indexed-only", "-l", "-F", "commonword"]),
    );
    assert!(
        snapshot.status.success(),
        "{}",
        String::from_utf8_lossy(&snapshot.stderr)
    );
    assert_eq!(sorted_lines(&snapshot.stdout).len(), 120);

    let selective = fixture.run(
        binary(),
        &fixture.query(&["--explain", "-l", "-F", "rarecostmarker"]),
    );
    assert!(selective.status.success());
    assert!(String::from_utf8_lossy(&selective.stderr).contains("plan = indexed"));
    let rare = fixture.assert_parity(rg, &fixture.query(&["-l", "-F", "rarecostmarker"]), &[]);
    assert_eq!(sorted_lines(&rare.stdout).len(), 1);
}

#[test]
fn indexed_patterns_flags_and_display_aliases_match_ripgrep() {
    let rg = oracle();
    let fixture = Fixture::new(&[
        ("branches.txt", "bar\nx\n"),
        ("folded.txt", "Kilogram\nlongſword\nKilo\n"),
        ("case.txt", "foo\nfoo\n"),
        ("literal.txt", "kilo.gram\n"),
    ]);
    fixture.build();
    let cases: &[&[&str]] = &[
        &["-l", "foo|bar"],
        &["-l", "(?:longliteral)?x"],
        &["-l", "(?i)kilogram"],
        &["-l", "(?i)longsword"],
        &["-l", "-S", r"\x6Bilo"],
        &["-l", "-S", r"\u{006B}ilo"],
        &["-l", r"kilo\.gram"],
        &["-l", "kilo.gram"],
        &["-c", "-l", "foo"],
        &["-l", "-c", "foo"],
        &["-i", "-S", "-l", "Foo"],
        &["-S", "-i", "-l", "Foo"],
        &["-l", "-e", "foo"],
        &["-l", "-F", "-e", "foo", "-e", "bar"],
    ];
    for args in cases {
        fixture.assert_parity(rg, &fixture.query(args), &[]);
    }
    let mut aliases = fixture.query(&["-l", "foo"]);
    aliases.push(OsString::from(format!("{}/.", fixture.corpus.display())));
    fixture.assert_parity(rg, &aliases, &[]);

    let symlink = fixture.root.join("case-alias.txt");
    std::os::unix::fs::symlink("corpus/case.txt", &symlink).unwrap();
    for flags in [&["-l", "foo"][..], &["-i", "-l", "fOo"][..]] {
        let mut query: Vec<OsString> = flags.iter().map(OsString::from).collect();
        query.extend([
            fixture.corpus.join("case.txt").into_os_string(),
            symlink.as_os_str().to_owned(),
            OsString::from("case.txt"),
        ]);
        for _ in 0..2 {
            let output = fixture.assert_parity(rg, &query, &[]);
            assert_eq!(sorted_lines(&output.stdout).len(), 3);
        }
    }
    fs::remove_file(&symlink).unwrap();
    std::os::unix::fs::symlink("corpus/branches.txt", &symlink).unwrap();
    let retargeted = vec!["-l".into(), "foo".into(), symlink.into_os_string()];
    let output = fixture.assert_parity(rg, &retargeted, &[]);
    assert_eq!(output.status.code(), Some(1));

    for (config, cli) in [
        (vec!["-i"], vec!["-S"]),
        (vec!["-S"], vec!["-i"]),
        (vec!["-i", "-S"], vec![]),
        (vec!["-S", "-i"], vec![]),
        (vec!["-i", "-s"], vec![]),
    ] {
        fixture.set_config(&config);
        let mut query = cli;
        query.extend(["-l", "Foo"]);
        fixture.assert_parity(rg, &fixture.query(&query), &config);
    }
}

#[test]
fn repeated_full_queries_verify_edits_new_files_and_deletions() {
    let rg = oracle();
    let fixture = Fixture::new(&[("a.txt", "needle\n"), ("b.txt", "other\n")]);
    fixture.build();
    let query = fixture.query(&["-l", "-F", "needle"]);
    for _ in 0..2 {
        fixture.assert_parity(rg, &query, &[]);
    }
    assert!(fixture.state.join("search_cache.qsc").is_file());
    fs::write(fixture.corpus.join("a.txt"), "other now\n").unwrap();
    fixture.assert_parity(rg, &query, &[]);
    fs::write(fixture.corpus.join("new.txt"), "needle\n").unwrap();
    fixture.assert_parity(rg, &query, &[]);
    fs::write(
        fixture.corpus.join("b.txt"),
        "needle in edited indexed file\n",
    )
    .unwrap();
    fixture.assert_parity(rg, &query, &[]);
    fs::remove_file(fixture.corpus.join("new.txt")).unwrap();
    fixture.assert_parity(rg, &query, &[]);
    fs::remove_file(fixture.corpus.join("b.txt")).unwrap();
    fixture.assert_parity(rg, &query, &[]);
}

#[test]
fn corrupt_or_missing_segments_fall_back_only_in_full_mode() {
    let rg = oracle();
    let fixture = Fixture::new(&[("a.txt", "needle\n"), ("b.txt", "other\n")]);
    fixture.build();
    let segments: Vec<_> = fs::read_dir(fixture.state.join("segments"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension() == Some(OsStr::new("tgs")))
        .map(|path| {
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    assert!(!segments.is_empty());
    let full = fixture.query(&["-l", "-F", "needle"]);
    fixture.assert_parity(rg, &full, &[]);
    for missing in [false, true] {
        for (path, original) in &segments {
            fs::write(path, original).unwrap();
            if missing {
                fs::remove_file(path).unwrap();
            } else {
                fs::write(path, b"invalid segment\n").unwrap();
            }
        }
        let output = fixture.assert_parity(rg, &full, &[]);
        assert!(String::from_utf8_lossy(&output.stderr).contains("segment"));
        let indexed = fixture.run(
            binary(),
            &fixture.query(&["--indexed-only", "-l", "-F", "needle"]),
        );
        assert_eq!(indexed.status.code(), Some(2));
        assert!(indexed.stdout.is_empty());
        assert!(String::from_utf8_lossy(&indexed.stderr).contains("segment"));
    }
}

#[test]
fn utf16_bom_files_remain_searchable_after_indexing() {
    let rg = oracle();
    let fixture = Fixture::new(&[("ordinary.txt", "other\n")]);
    for (name, little_endian) in [("little.txt", true), ("big.txt", false)] {
        let mut bytes = if little_endian {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for unit in "needle\n".encode_utf16() {
            bytes.extend(if little_endian {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        fs::write(fixture.corpus.join(name), bytes).unwrap();
    }
    for indexed in [false, true] {
        if indexed {
            fixture.build();
        }
        for args in [
            &["-l", "needle"][..],
            &["-F", "-l", "needle"][..],
            &["-w", "-l", "needle"][..],
        ] {
            let query = fixture.query(args);
            let output = fixture.assert_parity(rg, &query, &[]);
            assert_eq!(sorted_lines(&output.stdout).len(), 2);
            fixture.assert_wrapper_parity(rg, &query);
        }
    }
}

#[test]
fn binary_detection_is_decided_by_verification_not_word_sidecar_omissions() {
    let rg = oracle();
    let fixture = Fixture::new(&[("ordinary.txt", "needle\n")]);
    fs::write(fixture.corpus.join("before.bin"), b"needle\n\0trailer\n").unwrap();
    fs::write(fixture.corpus.join("after.bin"), b"\0needle\n").unwrap();
    fixture.build();
    assert!(fs::read_dir(fixture.state.join("segments"))
        .unwrap()
        .any(|entry| entry.unwrap().path().extension() == Some(OsStr::new("qwi"))));
    for args in [&["-l", "needle"][..], &["-w", "-l", "needle"][..]] {
        let query = fixture.query(args);
        fixture.assert_parity(rg, &query, &[]);
        fixture.assert_wrapper_parity(rg, &query);
    }
}

#[test]
fn rgignore_rules_apply_to_live_indexed_searches() {
    let rg = oracle();
    let fixture = Fixture::new(&[("kept.txt", "needle\n"), ("excluded.txt", "needle\n")]);
    fs::create_dir(fixture.corpus.join("nested")).unwrap();
    fs::write(fixture.corpus.join("nested/kept.txt"), "needle\n").unwrap();
    fs::write(fixture.corpus.join("nested/excluded.txt"), "needle\n").unwrap();
    fs::write(fixture.corpus.join(".rgignore"), "excluded.txt\n").unwrap();
    fs::write(fixture.corpus.join("nested/.rgignore"), "!excluded.txt\n").unwrap();
    fixture.build();
    let query = fixture.query(&["-l", "needle"]);
    let output = fixture.assert_wrapper_parity(rg, &query);
    assert_eq!(sorted_lines(&output.stdout).len(), 3);
    fixture.assert_parity(rg, &query, &[]);
    // Ignore files can change independently of the published index.
    fs::write(fixture.corpus.join(".rgignore"), "*.txt\n").unwrap();
    fixture.assert_wrapper_parity(rg, &query);
    fixture.assert_parity(rg, &query, &[]);
}

#[test]
fn wrapper_delegates_non_utf8_filename_bytes_unchanged() {
    let rg = oracle();
    let fixture = Fixture::new(&[("ordinary.txt", "other\n")]);
    let path = fixture
        .corpus
        .join(OsString::from_vec(b"bad_\xff.txt".to_vec()));
    fs::write(&path, "needle\n").unwrap();
    fixture.build();
    let query = vec!["needle".into(), path.into_os_string()];
    let output = fixture.assert_wrapper_parity(rg, &query);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"needle\n");
}

#[test]
fn newline_regex_is_an_error_instead_of_no_matches() {
    let rg = oracle();
    let fixture = Fixture::new(&[("a.txt", "needle\n")]);
    fixture.build();
    let query = fixture.query(&[r"\n"]);
    assert_eq!(fixture.run(rg, &query).status.code(), Some(2));
    let direct = fixture.run(binary(), &query);
    assert_eq!(direct.status.code(), Some(2));
    assert!(!direct.stderr.is_empty());
    assert_eq!(
        fixture.assert_wrapper_parity(rg, &query).status.code(),
        Some(2)
    );
}

#[test]
fn accessed_posting_corruption_falls_back_but_indexed_only_errors() {
    let rg = oracle();
    let fixture = Fixture::new(&[("a.txt", "aaaaaaaaaa")]);
    fixture.build();
    let segment = fs::read_dir(fixture.state.join("segments"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension() == Some(OsStr::new("tgs")))
        .unwrap();
    let mut bytes = fs::read(&segment).unwrap();
    assert_eq!(u16::from_le_bytes(bytes[4..6].try_into().unwrap()), 2);
    let postings = u64::from_le_bytes(bytes[50..58].try_into().unwrap()) as usize;
    // Damage the accessed block's encoded ID, leaving checked metadata intact.
    bytes[postings + 12] ^= 1;
    fs::write(segment, bytes).unwrap();
    let query = fixture.query(&["-F", "-l", "aaa"]);
    let output = fixture.assert_parity(rg, &query, &[]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("postings unavailable"));
    fixture.assert_wrapper_parity(rg, &query);
    let output = fixture.run(
        binary(),
        &fixture.query(&["--indexed-only", "-F", "-l", "aaa"]),
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

#[test]
fn short_words_ignore_unbound_legacy_sidecars_when_qwi_is_missing() {
    let rg = oracle();
    let fixture = Fixture::new(&[
        ("a.txt", "x xy\n"),
        ("b.txt", "x xy again\n"),
        ("c.txt", "other\n"),
    ]);
    fixture.build();
    for entry in fs::read_dir(fixture.state.join("segments")).unwrap() {
        let segment = entry.unwrap().path();
        if segment.extension() != Some(OsStr::new("tgs")) {
            continue;
        }
        fs::remove_file(segment.with_extension("qwi")).unwrap();
        let mut legacy = tgrep::native::HashIndex::create(1).unwrap();
        legacy.finalize().unwrap();
        let sidecar = segment.with_extension("thi");
        legacy.save(&sidecar).unwrap();
        assert_eq!(tgrep::native::HashIndex::load(&sidecar).unwrap().count(), 0);
    }
    for word in ["x", "xy"] {
        let query = fixture.query(&["-w", "-l", word]);
        let output = fixture.assert_parity(rg, &query, &[]);
        assert_eq!(sorted_lines(&output.stdout).len(), 2);
        fixture.assert_wrapper_parity(rg, &query);
    }
    let indexed = fixture.run(
        binary(),
        &fixture.query(&["--indexed-only", "-w", "-l", "xy"]),
    );
    assert_eq!(indexed.status.code(), Some(0));
    assert_eq!(sorted_lines(&indexed.stdout).len(), 2);
}

#[test]
fn indexed_only_rejects_queries_that_require_live_scanning() {
    let rg = oracle();
    let fixture = Fixture::new(&[("a.txt", "needle\n")]);
    fixture.build();
    for args in [&["-i", "-l", "NEEDLE"][..], &["-F", "-l", "n"][..]] {
        let query = fixture.query(args);
        let scanned = fixture.assert_parity(rg, &query, &[]);
        assert_eq!(scanned.status.code(), Some(0));
        let mut indexed_query = vec![OsString::from("--indexed-only")];
        indexed_query.extend(query);
        let indexed = fixture.run(binary(), &indexed_query);
        assert_eq!(indexed.status.code(), Some(2));
        assert!(indexed.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&indexed.stderr).contains("live scanning is required"),
            "{}",
            String::from_utf8_lossy(&indexed.stderr)
        );
    }
}

#[test]
fn streaming_plan_does_not_open_unneeded_segments_or_write_optimizer_state() {
    let rg = oracle();
    let fixture = Fixture::new(&[("a.txt", "needle\n")]);
    fixture.build();
    for entry in fs::read_dir(fixture.state.join("segments")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension() == Some(OsStr::new("tgs")) {
            fs::write(path, b"unreadable index").unwrap();
        }
    }
    let output = fixture.assert_parity(rg, &fixture.query(&["-i", "-l", "NEEDLE"]), &[]);
    assert!(!String::from_utf8_lossy(&output.stderr).contains("segment"));
    assert!(!fixture.state.join("optimization.qdb").exists());
}

#[test]
fn closing_the_output_pipe_stops_workers_without_an_error() {
    use std::io::{BufRead, BufReader, Read};
    let fixture = Fixture::new(&[]);
    for index in 0..8 {
        fs::write(
            fixture.corpus.join(format!("{index:02}.txt")),
            b"needle and enough output to spill before filling a pipe\n".repeat(32768),
        )
        .unwrap();
    }
    let mut child = ChildProcess(Some(
        Command::new(binary())
            .args(fixture.query(&["-F", "needle"]))
            .env("TGREP_STATE_DIR", &fixture.state)
            .env("TMPDIR", fixture.root.join("tmp"))
            .env("RIPGREP_CONFIG_PATH", &fixture.config)
            .env_remove("TGREP_REAL_RG")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let mut output = BufReader::new(child.0.as_mut().unwrap().stdout.take().unwrap());
    let mut first_line = String::new();
    assert!(output.read_line(&mut first_line).unwrap() > 0);
    assert!(first_line.contains("needle"));
    drop(output);
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.0.as_mut().unwrap().try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "workers did not stop after pipe closure"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut process = child.0.take().unwrap();
    assert!(process.wait().unwrap().success());
    let mut errors = String::new();
    process
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut errors)
        .unwrap();
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(fs::read_dir(fixture.root.join("tmp")).unwrap().count(), 0);
}

#[test]
fn large_output_spills_without_interleaving_files_or_changing_matches() {
    let rg = oracle();
    let fixture = Fixture::new(&[]);
    for index in 0..3 {
        fs::write(
            fixture.corpus.join(format!("{index:02}.txt")),
            b"needle with enough text to exceed the output memory limit\n".repeat(32768),
        )
        .unwrap();
    }
    let output = fixture.assert_parity(rg, &fixture.query(&["-n", "-F", "needle"]), &[]);
    assert!(output.stdout.len() > 3 * 1024 * 1024);
    let mut finished = std::collections::HashSet::new();
    let mut current = None;
    for line in output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let path = line.split(|byte| *byte == b':').next().unwrap();
        if current != Some(path) {
            assert!(!finished.contains(path), "file output was interleaved");
            if let Some(previous) = current.replace(path) {
                finished.insert(previous);
            }
        }
    }
    assert_eq!(finished.len(), 2);
    assert_eq!(fs::read_dir(fixture.root.join("tmp")).unwrap().count(), 0);
}

#[test]
fn unavailable_output_spool_returns_an_error_without_partial_file_output() {
    let fixture = Fixture::new(&[]);
    fs::write(
        fixture.corpus.join("large.txt"),
        b"needle with output that cannot fit in memory\n".repeat(32768),
    )
    .unwrap();
    let temporary = fixture.root.join("tmp");
    fs::remove_dir(&temporary).unwrap();
    fs::write(&temporary, b"not a directory").unwrap();
    let output = fixture.run(binary(), &fixture.query(&["-F", "needle"]));
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("large.txt"));
    assert_eq!(fs::read(&temporary).unwrap(), b"not a directory");
}

#[test]
fn relative_searches_populate_reusable_cache_hints() {
    let fixture = Fixture::new(&[("a.txt", "needle\n")]);
    fixture.build();
    let query = ["-l", "needle", "."].map(OsString::from);
    let output = fixture.run(binary(), &query);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"./a.txt\n");
    let explain = fixture.run(
        binary(),
        &["--explain", "-l", "needle", "."].map(OsString::from),
    );
    assert!(explain.status.success());
    assert!(
        String::from_utf8_lossy(&explain.stderr).contains("cache hints=1"),
        "{}",
        String::from_utf8_lossy(&explain.stderr)
    );
}

#[test]
fn native_version_is_attributable_and_wrapper_version_stays_ripgrep() {
    let rg = oracle();
    let fixture = Fixture::new(&[]);
    let args = [OsString::from("--version")];
    let direct = fixture.run(binary(), &args);
    assert!(direct.status.success());
    assert_eq!(
        String::from_utf8(direct.stdout).unwrap().trim(),
        format!("tgrep {}", env!("TGREP_BUILD_VERSION"))
    );
    let expected = fixture.run(rg, &args);
    let wrapped = fixture.run_with_rg(binary(), &args, Some(rg));
    assert_eq!(wrapped.status.code(), expected.status.code());
    assert_eq!(wrapped.stdout, expected.stdout);
    assert_eq!(wrapped.stderr, expected.stderr);
}
