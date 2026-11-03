use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;

#[derive(Clone, Debug)]
pub struct Config {
    pub data_dir: PathBuf,
    pub demo: bool,
}

impl Config {
    pub fn load() -> Result<Self> {
        let data_dir = match std::env::var_os("BRAZAP_DATA_DIR") {
            Some(path) => PathBuf::from(path),
            None => ProjectDirs::from("app", "Brazap", "Brazap")
                .context("Could not locate the application data directory")?
                .data_local_dir()
                .to_owned(),
        };
        std::fs::create_dir_all(&data_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            data_dir,
            demo: false,
        })
    }

    pub fn database(&self) -> PathBuf {
        self.data_dir.join("brazap.db")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.data_dir.join("cache")
    }
}
