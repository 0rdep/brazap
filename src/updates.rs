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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateState {
    pub status: String,
    pub current: String,
    pub latest: Option<String>,
    pub release_url: Option<String>,
    pub notes: Option<String>,
    pub error: Option<String>,
    pub downloaded: bool,
    /// This copy is updated outside Brazap (a system package, or a location Brazap
    /// cannot write), so a newer release is offered from its release page instead.
    #[serde(default)]
    pub manual: bool,
}
impl UpdateState {
    pub fn newer_than(&self, current: &str) -> bool {
        self.latest
            .as_deref()
            .and_then(|v| semver::Version::parse(v).ok())
            .zip(semver::Version::parse(current).ok())
            .is_some_and(|(latest, current)| latest > current)
    }

    pub fn ready_for(&self, current: &str) -> bool {
        self.downloaded && self.newer_than(current)
    }
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
            manual: false,
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

/// How this copy of Brazap was installed. It decides which release asset updates it
/// and how that download replaces it.
#[derive(Clone, Debug)]
enum Install {
    /// A standalone executable, including the Windows installer's per-user copy.
    Executable(PathBuf),
    /// A macOS `.app` bundle, replaced as a whole.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Bundle(PathBuf),
    /// A Linux AppImage file, replaced in place.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    AppImage(PathBuf),
    /// A system package, or a location Brazap cannot write (such as an app that macOS
    /// runs from a read-only translocated copy). The user installs updates.
    External,
}

impl Install {
    fn detect() -> Self {
        #[cfg(target_os = "linux")]
        if let Some(image) = appimage() {
            return if writable(image.parent()) {
                Self::AppImage(image)
            } else {
                Self::External
            };
        }
        let Ok(exe) = std::env::current_exe() else {
            return Self::External;
        };
        #[cfg(target_os = "macos")]
        if let Some(bundle) = exe.canonicalize().ok().and_then(|exe| app_bundle(&exe)) {
            return if writable(bundle.parent()) {
                Self::Bundle(bundle)
            } else {
                Self::External
            };
        }
        // Distribution packages (the .deb installs /usr/bin/brazap) belong to the
        // package manager, even when Brazap runs with permission to write there.
        if cfg!(target_os = "linux") && exe.starts_with("/usr") && !exe.starts_with("/usr/local") {
            return Self::External;
        }
        if writable(exe.parent()) {
            Self::Executable(exe)
        } else {
            Self::External
        }
    }
}

/// The AppImage file running this process. The AppImage runtime sets `APPIMAGE` and
/// mounts the image at `APPDIR`; children inherit both, so the executable must also be
/// inside that mount for the file to be this app's.
#[cfg(target_os = "linux")]
fn appimage() -> Option<PathBuf> {
    let image = PathBuf::from(std::env::var_os("APPIMAGE")?);
    let mount = PathBuf::from(std::env::var_os("APPDIR")?)
        .canonicalize()
        .ok()?;
    let exe = std::env::current_exe().ok()?;
    (image.is_absolute() && image.is_file() && exe.starts_with(mount)).then_some(image)
}

/// `/Applications/Brazap.app` for `/Applications/Brazap.app/Contents/MacOS/brazap`.
#[cfg(target_os = "macos")]
fn app_bundle(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_owned())
}

fn writable(dir: Option<&Path>) -> bool {
    let Some(dir) = dir else {
        return false;
    };
    let probe = dir.join(format!(".brazap-update-check-{}", std::process::id()));
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .is_ok();
    if created {
        let _ = std::fs::remove_file(&probe);
    }
    created
}

/// The path that starts this copy of Brazap again, such as for an agent's MCP
/// configuration. An AppImage runs from a temporary mount, so that is the AppImage.
pub fn launcher() -> PathBuf {
    #[cfg(target_os = "linux")]
    if let Some(image) = appimage() {
        return image;
    }
    std::env::current_exe().unwrap_or_else(|_| "brazap".into())
}

/// The release asset that updates this installation. Keep in sync with the release
/// workflow and docs/RELEASING.md. Older releases look for the bare executables, so
/// those names must stay published.
fn asset_name(install: &Install) -> Option<String> {
    let platform = if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "windows-x86_64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "macos-aarch64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "macos-x86_64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x86_64"
    } else {
        return None;
    };
    Some(match install {
        Install::Executable(_) if cfg!(target_os = "windows") => format!("brazap-{platform}.exe"),
        Install::Executable(_) => format!("brazap-{platform}"),
        Install::Bundle(_) => format!("brazap-{platform}.app.zip"),
        Install::AppImage(_) => format!("brazap-{platform}.AppImage"),
        Install::External => return None,
    })
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

fn staged(config: &Config, install: &Install) -> Result<Staged> {
    let dir = config.cache_dir().join("updates");
    let mut staged: Staged = serde_json::from_slice(&std::fs::read(dir.join("staged.json"))?)?;
    staged.path = staged.path.canonicalize()?;
    ensure!(
        staged.path.starts_with(dir.canonicalize()?),
        "Update is outside its staging directory"
    );
    // A download for another kind of installation (say, a bare executable staged before
    // Brazap moved into an app bundle) cannot update this one.
    let asset = asset_name(install).context("This installation is updated outside Brazap")?;
    ensure!(
        staged
            .path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(&format!("-{asset}"))),
        "Staged update is for another kind of installation"
    );
    ensure!(
        hash_file(&staged.path)? == staged.sha256,
        "Staged update failed its SHA-256 check"
    );
    ensure!(
        semver::Version::parse(&staged.version)?
            > semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
        "Update is not newer than this version"
    );
    Ok(staged)
}

/// Downloaded installers pile up in the cache, roughly 100 MiB per release.
/// Keep only the verified update that is still waiting to be installed.
fn prune_downloads(folder: &Path, keep: Option<&std::ffi::OsStr>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let text = name.to_string_lossy();
        // Staged downloads are named `<version>-brazap-<platform>[.<kind>]`.
        if (text.contains("-brazap-") || text.ends_with(".partial")) && Some(&*name) != keep {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub(crate) fn restored(config: &Config) -> UpdateState {
    let folder = config.cache_dir().join("updates");
    let current = staged(config, &Install::detect());
    prune_downloads(
        &folder,
        current
            .as_ref()
            .ok()
            .and_then(|staged| staged.path.file_name()),
    );
    match current {
        Ok(staged) => UpdateState {
            status: "ready".into(),
            release_url: Some(format!(
                "https://github.com/{REPO}/releases/tag/v{}",
                staged.version
            )),
            latest: Some(staged.version),
            downloaded: true,
            ..Default::default()
        },
        Err(_) => UpdateState::default(),
    }
}

fn check(config: &Config, download: bool, progress: impl Fn(UpdateState)) -> Result<UpdateState> {
    // Once an update is ready, keep it available offline until the user restarts.
    let cached = restored(config);
    if cached.ready_for(env!("CARGO_PKG_VERSION")) {
        return Ok(cached);
    }
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
    let Some(asset) = asset_name(&Install::detect()) else {
        state.manual = true;
        return Ok(state);
    };
    if !download {
        return Ok(state);
    }
    state.status = "downloading".into();
    progress(state.clone());
    let binary = release
        .assets
        .iter()
        .find(|a| a.name == asset)
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
            (name == asset).then(|| digest.to_lowercase())
        })
        .context("No checksum for this download")?;
    ensure!(
        expected.len() == 64 && expected.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid release checksum"
    );
    let folder = config.cache_dir().join("updates");
    std::fs::create_dir_all(&folder)?;
    let target = folder.join(format!("{version}-{asset}"));
    if !target.exists() || hash_file(&target)? != expected {
        let partial = folder.join(format!("{version}-{asset}.partial"));
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
    let target_name = target.file_name().map(std::ffi::OsStr::to_os_string);
    std::fs::write(
        folder.join("staged.json"),
        serde_json::to_vec(&Staged {
            version: version.to_string(),
            path: target,
            sha256: expected,
        })?,
    )?;
    prune_downloads(&folder, target_name.as_deref());
    state.status = "ready".into();
    state.downloaded = true;
    Ok(state)
}

impl Service {
    pub(crate) fn spawn_update_checker(self: &std::sync::Arc<Self>) {
        let service = std::sync::Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(15)).await;
            loop {
                let Some(service) = service.upgrade() else {
                    break;
                };
                if service.stopping.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                if let Err(error) = service.check_updates(true).await {
                    log::debug!("Background update check: {error:#}");
                }
                drop(service);
                tokio::time::sleep(Duration::from_secs(5 * 60)).await;
            }
        });
    }

    pub async fn check_updates(&self, download: bool) -> Result<UpdateState> {
        let Ok(_guard) = self.update_lock.try_lock() else {
            return Ok(self.updates.read().await.clone());
        };
        if self.updates.read().await.status == "checking" {
            return Ok(self.updates.read().await.clone());
        }
        self.updates.write().await.status = "checking".into();
        self.changed();
        let config = self.config.clone();
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        let mut job = tokio::task::spawn_blocking(move || {
            check(&config, download, |state| {
                let _ = send.send(state);
            })
        });
        let result = loop {
            tokio::select! {
                result = &mut job => break result.context("Update worker failed").and_then(|result| result),
                Some(state) = receive.recv() => {
                    *self.updates.write().await = state;
                    self.changed();
                }
            }
        };
        match result {
            Ok(state) => {
                *self.updates.write().await = state.clone();
                self.changed();
                Ok(state)
            }
            Err(e) => {
                let mut state = self.updates.write().await;
                state.status = "error".into();
                state.downloaded = false;
                state.error = Some(e.to_string());
                self.changed();
                Err(e)
            }
        }
    }
}

/// Stage a separate process to replace the installation only after this UI exits.
pub fn install(config: &Config) -> Result<()> {
    let install = Install::detect();
    install_staged(config, staged(config, &install)?, install)
}

/// A fresh desktop launch applies an already verified update before mounting UI.
/// CLI/MCP invocations and launches attached to a running service do not apply it.
pub fn install_pending(config: &Config) -> Result<bool> {
    let install = Install::detect();
    let Ok(staged) = staged(config, &install) else {
        return Ok(false);
    };
    install_staged(config, staged, install)?;
    Ok(true)
}

fn install_staged(config: &Config, staged: Staged, install: Install) -> Result<()> {
    let source = staged.path;
    let pid = std::process::id();
    let mut command;
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let Install::Executable(target) = install else {
            anyhow::bail!("This installation is updated outside Brazap");
        };
        let q = |p: &Path| {
            let path = p.to_string_lossy();
            format!("'{}'", path.trim_start_matches(r"\\?\").replace('\'', "''"))
        };
        let backup = target.with_extension("exe.previous");
        let folder = target
            .parent()
            .context("Executable has no parent directory")?;
        let data_arg = format!("\"{}\"", config.data_dir.display());
        let data_arg = data_arg.replace('\'', "''");
        // Keep the version in Installed apps current when this is the installer's copy.
        // The key name comes from AppId in packaging/windows/brazap.iss.
        let installed = format!(
            r"try {{ $k='HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{{85DF23AD-C3D8-448E-B9A6-D3F30AAF4D43}}_is1'; if ((Get-ItemPropertyValue -LiteralPath $k -Name InstallLocation).TrimEnd('\') -eq {}) {{ Set-ItemProperty -LiteralPath $k -Name DisplayVersion -Value '{}' }} }} catch {{ }}",
            q(folder),
            staged.version.replace('\'', "''")
        );
        let script = format!(
            "$ErrorActionPreference='Stop'; Wait-Process -Id {pid} -Timeout 120 -ErrorAction SilentlyContinue; if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit 1 }}; Copy-Item -LiteralPath {} -Destination {} -Force; try {{ Copy-Item -LiteralPath {} -Destination {} -Force; {installed}; Start-Process -FilePath {} -WorkingDirectory {} -ArgumentList @('--data-dir', '{data_arg}') }} catch {{ Copy-Item -LiteralPath {} -Destination {} -Force; throw }}",
            q(&target),
            q(&backup),
            q(&source),
            q(&target),
            q(&target),
            q(folder),
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
        let wait = format!("while kill -0 {pid} 2>/dev/null; do sleep 1; done");
        let script = match install {
            // An AppImage is a single executable file, replaced like a bare binary.
            Install::Executable(target) | Install::AppImage(target) => {
                let mut next = target.clone().into_os_string();
                next.push(".update");
                let next = PathBuf::from(next);
                format!(
                    "{wait}; cp {} {} && chmod 755 {} && mv {} {} && exec {} --data-dir {}",
                    q(&source),
                    q(&next),
                    q(&next),
                    q(&next),
                    q(&target),
                    q(&target),
                    q(&config.data_dir)
                )
            }
            // Swap the whole bundle: extract beside it (same volume, so moves are atomic
            // renames), move the old bundle aside, and put it back if the swap fails.
            // A failed swap also drops the download, so relaunching the old bundle does
            // not retry it on every launch.
            Install::Bundle(bundle) => {
                let parent = bundle.parent().context("App bundle has no parent folder")?;
                format!(
                    "{wait}; ok=; work=$(mktemp -d {parent}/.brazap-update.XXXXXX) || work=; \
                     if [ -n \"$work\" ] && ditto -x -k {source} \"$work\" && [ -x \"$work/Brazap.app/Contents/MacOS/brazap\" ] && mv {bundle} \"$work/previous.app\"; then \
                       if mv \"$work/Brazap.app\" {bundle}; then ok=1; rm -rf \"$work\"; else mv \"$work/previous.app\" {bundle} && rm -rf \"$work\"; fi; \
                     elif [ -n \"$work\" ] && [ -e {bundle} ]; then rm -rf \"$work\"; fi; \
                     [ -n \"$ok\" ] || rm -f {source}; \
                     exec open -n {bundle} --args --data-dir {data}",
                    parent = q(parent),
                    source = q(&source),
                    bundle = q(&bundle),
                    data = q(&config.data_dir)
                )
            }
            Install::External => anyhow::bail!("This installation is updated outside Brazap"),
        };
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
