//! Speech model files in the derived cache: verified downloads, removal, and
//! Windows' mapped-file rule.
//!
//! transcribe.cpp memory-maps a loaded model's weights. Windows refuses to
//! delete or replace a file while any view of it is mapped, but still lets it be
//! renamed, so such a file is set aside (`<name>.<nanos>.set-aside`) and swept
//! once nothing maps it (Hex does the same, `transcription/models.rs`).
use std::{
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use anyhow::{Context, Result};

use super::catalog::{LEGACY_FILES, MODELS, SpeechModel};

const SET_ASIDE_SUFFIX: &str = ".set-aside";

pub(super) fn folder(cache: &Path) -> PathBuf {
    cache.join("models")
}

fn partial(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    target.with_file_name(name)
}

/// The download was cancelled; not reported as a failure.
#[derive(Debug)]
pub(super) struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The download was cancelled")
    }
}
impl std::error::Error for Cancelled {}

/// Downloads `model` next to its final path and renames it into place once its
/// size and SHA-256 match. `progress` receives whole percents.
pub(super) fn download(
    model: &SpeechModel,
    cache: &Path,
    cancel: &AtomicBool,
    progress: impl Fn(u32),
) -> Result<()> {
    use sha2::{Digest, Sha256};
    let target = model.path(cache);
    let folder = target.parent().context("Model path has no folder")?;
    std::fs::create_dir_all(folder)?;
    let partial = partial(&target);
    let outcome = (|| -> Result<()> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_body(Some(Duration::from_secs(60)))
            .build()
            .into();
        let mut response = agent
            .get(model.url())
            .header("User-Agent", "Brazap")
            .call()?;
        let mut input = response
            .body_mut()
            .with_config()
            .limit(model.bytes + 1)
            .reader();
        let mut output = std::fs::File::create(&partial)?;
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let mut buffer = vec![0u8; 256 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Cancelled.into());
            }
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            anyhow::ensure!(
                total <= model.bytes,
                "The speech model is larger than expected"
            );
            output.write_all(&buffer[..n])?;
            hash.update(&buffer[..n]);
            progress((total * 100 / model.bytes) as u32);
        }
        output.sync_all()?;
        drop(output);
        anyhow::ensure!(
            total == model.bytes && format!("{:x}", hash.finalize()) == model.sha256(),
            "The downloaded speech model failed its SHA-256 check"
        );
        replace(&partial, &target)?;
        Ok(())
    })();
    if outcome.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    outcome
}

/// Whether a failed removal or rename is Windows refusing a mapped file.
fn mapped(error: &std::io::Error) -> bool {
    cfg!(windows) && error.kind() == ErrorKind::PermissionDenied
}

fn set_aside(file: &Path) -> std::io::Result<()> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{nanos}{SET_ASIDE_SUFFIX}"));
    std::fs::rename(file, file.with_file_name(name))
}

/// Moves `from` over `to`, setting a mapped `to` aside first.
fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Err(error) if mapped(&error) && to.exists() => {
            set_aside(to)?;
            std::fs::rename(from, to)
        }
        renamed => renamed,
    }
}

/// Removes a file, setting it aside when it is still mapped.
fn remove(file: &Path) -> std::io::Result<bool> {
    match std::fs::remove_file(file) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) if mapped(&error) => set_aside(file).map(|_| true),
        Err(error) => Err(error),
    }
}

/// Deletes a model's file and any unfinished download of it.
pub(super) fn delete(model: &SpeechModel, cache: &Path) -> Result<()> {
    let path = model.path(cache);
    let _ = std::fs::remove_file(partial(&path));
    remove(&path).with_context(|| format!("Could not delete {}", path.display()))?;
    sweep(cache);
    Ok(())
}

/// Keeps only the models in `keep` on disk, with whisper.cpp files from
/// earlier versions removed as well. Downloads in progress keep their
/// `.partial` file.
pub(super) fn prune(cache: &Path, keep: &[&str]) {
    for model in &MODELS {
        if keep.contains(&model.id) {
            continue;
        }
        let path = model.path(cache);
        match remove(&path) {
            Ok(true) => log::info!("Removed the unused speech model {}", path.display()),
            Ok(false) => {}
            Err(error) => log::warn!("Could not remove {}: {error}", path.display()),
        }
    }
    let folder = folder(cache);
    for file in LEGACY_FILES {
        let path = folder.join(file);
        let _ = std::fs::remove_file(partial(&path));
        let _ = std::fs::remove_file(path.with_extension("partial"));
        if remove(&path).unwrap_or(false) {
            log::info!("Removed the earlier speech model {}", path.display());
        }
    }
    sweep(cache);
}

/// Deletes models set aside while they were mapped; one still mapped stays
/// for a later sweep.
pub(super) fn sweep(cache: &Path) {
    let Ok(entries) = std::fs::read_dir(folder(cache)) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .ends_with(SET_ASIDE_SUFFIX)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Unfinished downloads left by an earlier run, which can't be resumed.
pub(super) fn remove_partials(cache: &Path) {
    let Ok(entries) = std::fs::read_dir(folder(cache)) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().ends_with(".partial") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
