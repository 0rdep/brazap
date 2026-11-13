use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;

#[derive(Clone, Debug)]
pub struct Config {
    pub data_dir: PathBuf,
    pub demo: bool,
    /// `BRAZAP_MCP` override for the MCP server of the service started by this
    /// process. `None` follows the saved setting (Settings → MCP), which is off
    /// by default.
    pub mcp: Option<bool>,
}

/// Parse `BRAZAP_MCP`. Unset or empty follows the saved setting.
pub fn parse_mcp_override(value: Option<&std::ffi::OsStr>) -> Result<Option<bool>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.to_string_lossy();
    Ok(match value.trim().to_ascii_lowercase().as_str() {
        "" => None,
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => bail!("BRAZAP_MCP must be 1/true/on or 0/false/off, not {value:?}"),
    })
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
            mcp: parse_mcp_override(std::env::var_os("BRAZAP_MCP").as_deref())?,
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

#[cfg(test)]
mod tests {
    use super::parse_mcp_override;
    use std::ffi::OsStr;

    #[test]
    fn mcp_override_is_unset_by_default() {
        assert_eq!(parse_mcp_override(None).unwrap(), None);
        assert_eq!(parse_mcp_override(Some(OsStr::new(" "))).unwrap(), None);
    }

    #[test]
    fn mcp_override_accepts_on_and_off_values() {
        for value in ["1", "true", "ON", "yes"] {
            assert_eq!(
                parse_mcp_override(Some(OsStr::new(value))).unwrap(),
                Some(true)
            );
        }
        for value in ["0", "false", "Off", "no"] {
            assert_eq!(
                parse_mcp_override(Some(OsStr::new(value))).unwrap(),
                Some(false)
            );
        }
        assert!(parse_mcp_override(Some(OsStr::new("maybe"))).is_err());
    }
}
