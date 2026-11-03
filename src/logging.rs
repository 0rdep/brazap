use crate::config::Config;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    sync::Mutex,
};

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
            self.console.log(r);
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
