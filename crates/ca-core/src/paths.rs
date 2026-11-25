//! Where Code Arena keeps its settings and its data, per system, or all under one folder (`CODE_ARENA_HOME`).

use std::path::{Path, PathBuf};

/// The two folders: settings (`settings.json`, `ARENA.md`, credentials) and data (sessions, history, logs, caches).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config: PathBuf,
    pub data: PathBuf,
}

impl Paths {
    /// From the environment: `CODE_ARENA_HOME` puts both under one folder (`config/`, `data/`), for a portable copy;
    /// else the system's own places (`~/.config/code-arena`, `~/.local/share/code-arena` or `$XDG_*`; on Windows
    /// `%APPDATA%\code-arena` and `%LOCALAPPDATA%\code-arena`; on macOS `~/Library/Application Support/code-arena`).
    pub fn from_env() -> Self {
        Self::resolve(std::env::var_os("CODE_ARENA_HOME").map(PathBuf::from))
    }

    pub fn resolve(home: Option<PathBuf>) -> Self {
        if let Some(home) = home.filter(|h| !h.as_os_str().is_empty()) {
            return Self::under(&home);
        }
        let config = config_base().join("code-arena");
        let data = data_base().join("code-arena");
        Self { config, data }
    }

    /// Both folders under one: `HOME/config` and `HOME/data`.
    pub fn under(home: &Path) -> Self {
        Self { config: home.join("config"), data: home.join("data") }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    /// The Arena address and the API key (0600). Kept apart from settings.json, so settings can be shared or shown.
    pub fn credentials_file(&self) -> PathBuf {
        self.config.join("credentials.json")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.data.join("sessions")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data.join("logs")
    }

    /// What Code Arena learned of each Arena it talks to (its capabilities), by host.
    pub fn servers_dir(&self) -> PathBuf {
        self.data.join("servers")
    }
}

fn config_base() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        // As the C# Code Arena and most CLI tools on macOS do: XDG's place when set, else ~/.config.
        if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
            return PathBuf::from(x);
        }
        return dirs::home_dir().unwrap_or_default().join(".config");
    }
    #[allow(unreachable_code)]
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn data_base() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(x);
        }
        return dirs::home_dir().unwrap_or_default().join(".local").join("share");
    }
    #[cfg(windows)]
    {
        return dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    }
    #[allow(unreachable_code)]
    dirs::data_dir().unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_home_holds_both() {
        let p = Paths::resolve(Some(PathBuf::from("/media/usb/ca")));
        assert_eq!(p.config, PathBuf::from("/media/usb/ca/config"));
        assert_eq!(p.data, PathBuf::from("/media/usb/ca/data"));
        assert_eq!(p.credentials_file(), PathBuf::from("/media/usb/ca/config/credentials.json"));
    }

    #[test]
    fn an_empty_home_is_none() {
        let p = Paths::resolve(Some(PathBuf::new()));
        assert!(p.config.ends_with("code-arena"));
        assert!(p.data.ends_with("code-arena"));
    }
}
