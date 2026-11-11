use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct BenchmarkTools<'a> {
    pub tgrep: &'a Path,
    pub ripgrep: &'a Path,
    pub state: &'a Path,
}

// Resolve an independent ripgrep baseline. A tgrep rg wrapper can report a
// genuine rg version while routing the measured query back through tgrep.
fn independent_ripgrep(path: &Path, tgrep: Option<&fs::Metadata>) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file()
        || tgrep.is_some_and(|other| other.dev() == metadata.dev() && other.ino() == metadata.ino())
    {
        return false;
    }
    let mut prefix = Vec::new();
    if fs::File::open(path)
        .and_then(|file| file.take(4096).read_to_end(&mut prefix))
        .is_err()
    {
        return false;
    }
    if prefix.starts_with(b"#!")
        && prefix
            .windows(b"TGREP_REAL_RG".len())
            .any(|part| part == b"TGREP_REAL_RG")
    {
        return false;
    }
    let Ok(version) = Command::new(path).arg("--version").output() else {
        return false;
    };
    let output = String::from_utf8_lossy(&version.stdout);
    version.status.success()
        && output.lines().next().is_some_and(|line| {
            line.strip_prefix("ripgrep ")
                .is_some_and(|number| number.starts_with(|c: char| c.is_ascii_digit()))
        })
}

pub fn tgrep_binary() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("TGREP_BIN").filter(|value| !value.is_empty()) {
        return Ok(path.into());
    }
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    let path = executable
        .parent()
        .ok_or("benchmark executable has no parent directory")?
        .join("tgrep");
    if !path.is_file() {
        return Err(format!(
            "{} not found; build tgrep in the same profile or set TGREP_BIN",
            path.display()
        ));
    }
    Ok(path)
}

pub fn ripgrep_binary(tgrep_binary: &Path) -> Result<PathBuf, String> {
    let explicit = env::var_os("RG_BIN")
        .filter(|value| !value.is_empty())
        .or_else(|| env::var_os("TGREP_REAL_RG").filter(|value| !value.is_empty()));
    let search_path = env::var_os("PATH").unwrap_or_default();
    let candidates: Vec<PathBuf> = match explicit.as_ref() {
        Some(value) if Path::new(value).components().count() > 1 => vec![value.into()],
        value => {
            let name = value.cloned().unwrap_or_else(|| OsString::from("rg"));
            env::split_paths(&search_path)
                .map(|directory| directory.join(&name))
                .collect()
        }
    };
    let tgrep = fs::metadata(tgrep_binary).ok();
    for candidate in candidates {
        if independent_ripgrep(&candidate, tgrep.as_ref()) {
            return fs::canonicalize(&candidate).map_err(|error| error.to_string());
        }
    }
    Err("no independent ripgrep baseline found; set RG_BIN to the real ripgrep executable".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn wrapper_and_tgrep_identity_are_not_valid_baselines() {
        let directory = env::temp_dir().join(format!("tgrep-benchmark-rg-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let genuine = directory.join("real-rg");
        let wrapper = directory.join("wrapper-rg");
        for (path, contents) in [
            (&genuine, "#!/bin/sh\nprintf 'ripgrep 15.1.0\\n'\n"),
            (
                &wrapper,
                "#!/bin/sh\n# TGREP_REAL_RG wrapper\nprintf 'ripgrep 15.1.0\\n'\n",
            ),
        ] {
            fs::write(path, contents).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert!(independent_ripgrep(&genuine, None));
        assert!(!independent_ripgrep(&wrapper, None));
        assert!(!independent_ripgrep(
            &genuine,
            fs::metadata(&genuine).ok().as_ref()
        ));
        fs::remove_dir_all(directory).unwrap();
    }
}
