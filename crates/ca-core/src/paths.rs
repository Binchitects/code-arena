//! Where Code Arena keeps its settings and its data, per system, or all under one folder (`CODE_ARENA_HOME`).

use std::path::{Path, PathBuf};

/// The two folders: settings (`settings.json`, `ARENA.md`, credentials) and data (sessions, history, logs, caches).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config: PathBuf,
    pub data: PathBuf,
}

impl Paths {
    /// `home` (`CODE_ARENA_HOME`) puts both under one folder (`config/`, `data/`), for a portable copy; else the
    /// system's own places (`~/.config/code-arena` and `~/.local/share/code-arena`, or `$XDG_*`; on Windows
    /// `%APPDATA%\code-arena` and `%LOCALAPPDATA%\code-arena`).
    pub fn resolve(home: Option<PathBuf>, xdg_config: Option<PathBuf>, xdg_data: Option<PathBuf>) -> Self {
        if let Some(home) = home.filter(|h| !h.as_os_str().is_empty()) {
            return Self::under(&home);
        }
        let config = xdg_config.unwrap_or_else(config_base).join("code-arena");
        let data = xdg_data.unwrap_or_else(data_base).join("code-arena");
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
    if cfg!(target_os = "macos") {
        // As on Linux: ~/.config, as the C# Code Arena and most command-line tools on macOS have it.
        return dirs::home_dir().unwrap_or_default().join(".config");
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn data_base() -> PathBuf {
    if cfg!(target_os = "macos") {
        return dirs::home_dir().unwrap_or_default().join(".local").join("share");
    }
    if cfg!(windows) {
        return dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    }
    dirs::data_dir().unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_home_holds_both() {
        let p = Paths::resolve(Some(PathBuf::from("/media/usb/ca")), None, None);
        assert_eq!(p.config, PathBuf::from("/media/usb/ca/config"));
        assert_eq!(p.data, PathBuf::from("/media/usb/ca/data"));
        assert_eq!(p.credentials_file(), PathBuf::from("/media/usb/ca/config/credentials.json"));
    }

    #[test]
    fn an_empty_home_is_none() {
        let p = Paths::resolve(Some(PathBuf::new()), Some(PathBuf::from("/x/cfg")), None);
        assert_eq!(p.config, PathBuf::from("/x/cfg/code-arena"));
        assert!(p.data.ends_with("code-arena"));
    }
}
