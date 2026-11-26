use crate::config::Config;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

/// Whether log lines are also written to stderr. Off once stderr feeds the
/// log itself (`capture_stderr`), or each line would come back in a loop.
static CONSOLE: AtomicBool = AtomicBool::new(true);

pub fn initialize(config: &Config) -> anyhow::Result<()> {
    std::fs::create_dir_all(config.logs_dir())?;
    let path = config.logs_dir().join("brazap.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 5 * 1024 * 1024) {
        for i in (1..=2).rev() {
            let _ = std::fs::rename(
                config.logs_dir().join(format!("brazap.log.{i}")),
                config.logs_dir().join(format!("brazap.log.{}", i + 1)),
            );
        }
        let _ = std::fs::rename(&path, config.logs_dir().join("brazap.log.1"));
    }
    let logger = Logger {
        console: env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or("warn"),
        )
        .build(),
        file: Mutex::new(OpenOptions::new().create(true).append(true).open(path)?),
    };
    log::set_max_level(logger.console.filter());
    log::set_boxed_logger(Box::new(logger)).map_err(|e| anyhow::anyhow!("Initialize logging: {e}"))
}
struct Logger {
    console: env_logger::Logger,
    file: Mutex<File>,
}
impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        self.console.enabled(m)
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            if CONSOLE.load(Ordering::Relaxed) {
                self.console.log(r);
            }
            if let Ok(mut f) = self.file.lock() {
                let _ = writeln!(
                    f,
                    "{} {} {}: {}",
                    chrono::Utc::now().to_rfc3339(),
                    r.level(),
                    r.target(),
                    r.args()
                );
            }
        }
    }
    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
    }
}

/// A window-subsystem Windows build starts without stderr, so Rune's own
/// reports (slow commits, GPU device loss, software fallback) vanished. Unless
/// stderr already goes somewhere (a console or a redirect), route it through a
/// pipe into this log, one warning per line.
#[cfg(all(target_os = "windows", feature = "desktop"))]
pub fn capture_stderr() {
    use std::io::BufRead;
    use std::os::windows::io::FromRawHandle;
    use windows::Win32::{
        Foundation::HANDLE,
        System::{
            Console::{GetStdHandle, STD_ERROR_HANDLE, SetStdHandle},
            Pipes::CreatePipe,
        },
    };
    // SAFETY: plain Win32 calls on handles this function owns. The read end
    // moves into a `File` (and its thread); the write end stays installed as
    // the process's stderr for its lifetime.
    unsafe {
        if GetStdHandle(STD_ERROR_HANDLE).is_ok_and(|h| !h.is_invalid() && !h.0.is_null()) {
            return;
        }
        let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
        if CreatePipe(&mut read, &mut write, None, 64 * 1024).is_err() {
            return;
        }
        // Before stderr points at the pipe: no log line may write into it.
        CONSOLE.store(false, Ordering::Relaxed);
        if SetStdHandle(STD_ERROR_HANDLE, write).is_err() {
            CONSOLE.store(true, Ordering::Relaxed);
            let _ = windows::Win32::Foundation::CloseHandle(read);
            let _ = windows::Win32::Foundation::CloseHandle(write);
            return;
        }
        let pipe = File::from_raw_handle(read.0);
        // Draining never stops, so a write to stderr cannot block on a full pipe.
        let drained = std::thread::Builder::new()
            .name("stderr-log".into())
            .spawn(move || {
                for line in std::io::BufReader::new(pipe).lines() {
                    match line {
                        Ok(line) if !line.trim().is_empty() => {
                            log::warn!(target: "stderr", "{line}")
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
            });
        if drained.is_err() {
            // No reader: put stderr back to nothing rather than a pipe that fills.
            let _ = SetStdHandle(STD_ERROR_HANDLE, HANDLE::default());
            let _ = windows::Win32::Foundation::CloseHandle(write);
            CONSOLE.store(true, Ordering::Relaxed);
        }
    }
}
