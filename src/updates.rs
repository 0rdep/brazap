use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{config::Config, service::Service};
const REPO: &str = "0rdep/brazap";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateState {
    pub status: String,
    pub current: String,
    pub latest: Option<String>,
    pub release_url: Option<String>,
    pub notes: Option<String>,
    pub error: Option<String>,
    pub downloaded: bool,
}
impl Default for UpdateState {
    fn default() -> Self {
        Self {
            status: "idle".into(),
            current: env!("CARGO_PKG_VERSION").into(),
            latest: None,
            release_url: None,
            notes: None,
            error: None,
            downloaded: false,
        }
    }
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}
#[derive(Serialize, Deserialize)]
struct Staged {
    version: String,
    path: PathBuf,
    sha256: String,
}

fn asset_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "brazap-windows-x86_64.exe"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "brazap-macos-aarch64"
    } else if cfg!(target_os = "macos") {
        "brazap-macos-x86_64"
    } else {
        "brazap-linux-x86_64"
    }
}
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(90)))
        .build()
        .into()
}
fn checked_url(url: &str) -> Result<()> {
    ensure!(
        url.starts_with(&format!("https://github.com/{REPO}/releases/download/")),
        "Unexpected update asset location"
    );
    Ok(())
}
fn hash_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn check(config: &Config, download: bool) -> Result<UpdateState> {
    let a = agent();
    let response = a
        .get(&format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ))
        .header("User-Agent", "Brazap-updater")
        .call();
    let mut response = match response {
        Err(ureq::Error::StatusCode(404)) => {
            return Ok(UpdateState {
                status: "unpublished".into(),
                ..Default::default()
            });
        }
        r => r?,
    };
    let release: Release = serde_json::from_str(&response.body_mut().read_to_string()?)?;
    let version = semver::Version::parse(release.tag_name.trim_start_matches('v'))?;
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))?;
    let mut state = UpdateState {
        status: "up_to_date".into(),
        latest: Some(version.to_string()),
        release_url: Some(release.html_url),
        notes: release.body,
        ..Default::default()
    };
    if release.draft || release.prerelease || version <= current {
        return Ok(state);
    }
    state.status = "available".into();
    if !download {
        return Ok(state);
    }
    let binary = release
        .assets
        .iter()
        .find(|a| a.name == asset_name())
        .context("This release has no download for your platform")?;
    let sums = release
        .assets
        .iter()
        .find(|a| a.name == "checksums.sha256")
        .context("Release checksums are missing")?;
    checked_url(&binary.browser_download_url)?;
    checked_url(&sums.browser_download_url)?;
    let sums = a
        .get(&sums.browser_download_url)
        .header("User-Agent", "Brazap-updater")
        .call()?
        .body_mut()
        .read_to_string()?;
    let expected = sums
        .lines()
        .find_map(|line| {
            let mut p = line.split_whitespace();
            let digest = p.next()?;
            let name = p.next()?.trim_start_matches('*');
            (name == asset_name()).then(|| digest.to_lowercase())
        })
        .context("No checksum for this download")?;
    ensure!(
        expected.len() == 64 && expected.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid release checksum"
    );
    let folder = config.cache_dir().join("updates");
    std::fs::create_dir_all(&folder)?;
    let target = folder.join(format!("{}-{}", version, asset_name()));
    if !target.exists() || hash_file(&target)? != expected {
        let partial = target.with_extension("partial");
        let outcome = (|| -> Result<()> {
            let mut response = a
                .get(&binary.browser_download_url)
                .header("User-Agent", "Brazap-updater")
                .call()?;
            let mut input = response.body_mut().as_reader().take(256 * 1024 * 1024 + 1);
            let mut output = std::fs::File::create(&partial)?;
            let mut hash = Sha256::new();
            let mut total = 0;
            let mut buf = [0u8; 65536];
            loop {
                let n = input.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                total += n;
                ensure!(total <= 256 * 1024 * 1024, "Update exceeds download limit");
                output.write_all(&buf[..n])?;
                hash.update(&buf[..n]);
            }
            output.sync_all()?;
            ensure!(
                format!("{:x}", hash.finalize()) == expected,
                "Downloaded update failed its SHA-256 check"
            );
            if target.exists() {
                std::fs::remove_file(&target)?;
            }
            std::fs::rename(&partial, &target)?;
            Ok(())
        })();
        if outcome.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        outcome?;
    }
    std::fs::write(
        folder.join("staged.json"),
        serde_json::to_vec(&Staged {
            version: version.to_string(),
            path: target,
            sha256: expected,
        })?,
    )?;
    state.status = "ready".into();
    state.downloaded = true;
    Ok(state)
}

impl Service {
    pub async fn check_updates(&self, download: bool) -> Result<UpdateState> {
        let Ok(_guard) = self.update_lock.try_lock() else {
            return Ok(self.updates.read().await.clone());
        };
        if self.updates.read().await.status == "checking" {
            return Ok(self.updates.read().await.clone());
        }
        self.updates.write().await.status = "checking".into();
        let config = self.config.clone();
        match tokio::task::spawn_blocking(move || check(&config, download)).await? {
            Ok(state) => {
                *self.updates.write().await = state.clone();
                Ok(state)
            }
            Err(e) => {
                let mut state = self.updates.write().await;
                state.status = "error".into();
                state.error = Some(e.to_string());
                Err(e)
            }
        }
    }
}

/// Stage a separate process to replace the executable only after this UI exits.
pub fn install(config: &Config) -> Result<()> {
    let dir = config.cache_dir().join("updates");
    let staged: Staged = serde_json::from_slice(&std::fs::read(dir.join("staged.json"))?)?;
    let source = staged.path.canonicalize()?;
    ensure!(
        source.starts_with(dir.canonicalize()?),
        "Update is outside its staging directory"
    );
    ensure!(
        hash_file(&source)? == staged.sha256,
        "Staged update failed its SHA-256 check"
    );
    ensure!(
        semver::Version::parse(&staged.version)?
            > semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
        "Update is not newer than this version"
    );
    let target = std::env::current_exe()?;
    let pid = std::process::id();
    let mut command;
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let q = |p: &Path| {
            let path = p.to_string_lossy();
            format!("'{}'", path.trim_start_matches(r"\\?\").replace('\'', "''"))
        };
        let backup = target.with_extension("exe.previous");
        let data_arg = format!("\"{}\"", config.data_dir.display());
        let data_arg = data_arg.replace('\'', "''");
        let script = format!(
            "$ErrorActionPreference='Stop'; Wait-Process -Id {pid} -Timeout 120 -ErrorAction SilentlyContinue; if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit 1 }}; Copy-Item -LiteralPath {} -Destination {} -Force; try {{ Copy-Item -LiteralPath {} -Destination {} -Force; Start-Process -FilePath {} -WorkingDirectory {} -ArgumentList @('--data-dir', '{data_arg}') }} catch {{ Copy-Item -LiteralPath {} -Destination {} -Force; throw }}",
            q(&target),
            q(&backup),
            q(&source),
            q(&target),
            q(&target),
            q(target
                .parent()
                .context("Executable has no parent directory")?),
            q(&backup),
            q(&target)
        );
        command = std::process::Command::new("powershell.exe");
        command
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x08000000);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let q = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"));
        let next = target.with_extension("update");
        let script = format!(
            "while kill -0 {pid} 2>/dev/null; do sleep 1; done; cp {} {} && chmod 755 {} && mv {} {} && exec {} --data-dir {}",
            q(&source),
            q(&next),
            q(&next),
            q(&next),
            q(&target),
            q(&target),
            q(&config.data_dir)
        );
        command = std::process::Command::new("sh");
        command.args(["-c", &script]);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Start update installer")?;
    Ok(())
}
