use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, fs, io};

/// Set by the release workflow. Local builds have none and never look for updates.
pub const VERSION: Option<&str> = option_env!("SKYBIN_VERSION");
const REPO: &str = "realChriss/skybin";

fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder().user_agent("skybin").build()
}

/// The latest release tag, if it is newer than this build.
pub async fn check() -> Option<String> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
    }
    let current = VERSION?;
    let body = client()
        .ok()?
        .get(format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;
    let tag = serde_json::from_slice::<Release>(&body).ok()?.tag_name;
    newer(&tag, current).then_some(tag)
}

pub async fn install(tag: &str) -> Result<(), String> {
    let ext = if cfg!(windows) { ".exe" } else { "" };
    let url = format!(
        "https://github.com/{REPO}/releases/download/{tag}/skybin-{}-{}{ext}",
        env::consts::OS,
        env::consts::ARCH
    );
    let bin = async {
        client()?
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await
    }
    .await
    .map_err(|e| e.to_string())?;
    let exe = env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|e| e.to_string())?;
    replace_exe(&exe, &bin).map_err(|e| e.to_string())
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut p = exe.as_os_str().to_owned();
    p.push(suffix);
    p.into()
}

fn replace_exe(exe: &Path, bin: &[u8]) -> io::Result<()> {
    let new = sibling(exe, ".new");
    let r = write_exe(&new, bin).and_then(|()| {
        if !cfg!(windows) {
            return fs::rename(&new, exe);
        }
        // Windows can't overwrite a running exe, but it can rename it
        let old = sibling(exe, ".old");
        fs::rename(exe, &old)?;
        fs::rename(&new, exe).inspect_err(|_| {
            let _ = fs::rename(&old, exe);
        })
    });
    if r.is_err() {
        let _ = fs::remove_file(&new);
    }
    r
}

fn write_exe(path: &Path, bin: &[u8]) -> io::Result<()> {
    fs::write(path, bin)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn newer(tag: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u32> {
        v.trim_start_matches('v')
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    parse(tag) > parse(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_compares_numerically() {
        assert!(newer("v0.1.10", "v0.1.9"));
        assert!(newer("v1.0.0", "0.9.9"));
        assert!(!newer("v0.1.9", "v0.1.9"));
        assert!(!newer("v0.1.8", "v0.1.9"));
    }

    #[test]
    fn replaces_exe() {
        let exe = env::temp_dir().join(format!("skybin-update-test-{}", std::process::id()));
        fs::write(&exe, b"old").unwrap();
        replace_exe(&exe, b"new").unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        assert!(!sibling(&exe, ".new").exists());
        for p in [sibling(&exe, ".old"), exe] {
            let _ = fs::remove_file(p);
        }
    }
}
