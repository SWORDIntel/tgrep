// SPDX-License-Identifier: LicenseRef-SRSL-1.1

use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
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

fn oracle() -> Option<&'static Path> {
    let rg = Path::new("/usr/bin/rg");
    if rg.is_file() {
        Some(rg)
    } else {
        eprintln!("SKIP search CLI oracle test: /usr/bin/rg is unavailable");
        None
    }
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
fn indexed_patterns_flags_and_display_aliases_match_ripgrep() {
    let Some(rg) = oracle() else {
        return;
    };
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
    let Some(rg) = oracle() else {
        return;
    };
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
    let Some(rg) = oracle() else {
        return;
    };
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
