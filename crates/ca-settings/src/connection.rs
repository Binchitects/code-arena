//! The Arena Code Arena works with: its address, the person's API key and where its gateway and tools are.
//! Kept in `credentials.json` in the config folder, readable by its owner only; `ARENA_*` override it for a run.

use std::io::Write;
use std::path::{Path, PathBuf};

use ca_core::Secret;
use serde::{Deserialize, Serialize};

use crate::Env;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    /// `https://DOMAIN`, no trailing slash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<Secret>,
    /// When the gateway is not at `https://gateway.DOMAIN`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    /// When Arena's tools are not at `https://DOMAIN/mcp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_intel_url: Option<String>,
    /// A CA file to trust besides the system's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca: Option<PathBuf>,
    /// Whose key it is, as Arena said at sign-in (shown, never used to sign in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Providers of models with the person's own keys, by name: `{"anthropic": {"apiKey": …}, "gpu": {"kind":
    /// "openai", "url": "http://gpu:8000", "models": ["qwen3"]}}`. A known one's key may come from its variable.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub providers: std::collections::BTreeMap<String, ProviderEntry>,
}

/// One provider, as `credentials.json` keeps it (only its owner may read the file).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEntry {
    /// `openai` or `anthropic`; a known provider's own when not given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<Secret>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
    /// Its key came from the environment for this run: never written to the file.
    #[serde(skip)]
    pub key_from_env: bool,
    /// It is there only for the environment's key: not written either.
    #[serde(skip)]
    pub from_env: bool,
}

impl ProviderEntry {
    /// Its settings as `ca_llm::providers::Provider::parse` reads them (the key apart).
    pub fn settings(&self) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        if let Some(k) = &self.kind {
            o.insert("kind".into(), serde_json::json!(k));
        }
        if let Some(u) = &self.url {
            o.insert("url".into(), serde_json::json!(u));
        }
        if !self.models.is_empty() {
            o.insert("models".into(), serde_json::json!(self.models));
        }
        if let Some(c) = self.context {
            o.insert("context".into(), serde_json::json!(c));
        }
        serde_json::Value::Object(o)
    }
}

/// Which Code Arena this is, by what it is connected to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edition {
    /// Signed in to Farzan Arena: everything (Arena's gateway, its tools, chats kept on the web, memory, skills,
    /// checklists, projects), and the person's own providers where Arena's admins allow them.
    Managed,
    /// No Arena: the person's own providers' models, and everything that needs no Arena.
    Standalone,
    /// Neither yet.
    None,
}

impl Edition {
    pub fn name(self) -> &'static str {
        match self {
            Edition::Managed => "managed",
            Edition::Standalone => "standalone",
            Edition::None => "not set up",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("{path}: {message}")]
    File { path: PathBuf, message: String },
    #[error("{0}")]
    Address(String),
}

impl Connection {
    /// The file's connection, then the environment's over it.
    pub fn load(file: &Path, env: &Env) -> Result<Self, ConnectionError> {
        let mut c = match std::fs::read_to_string(file) {
            Ok(text) => serde_json::from_str::<Connection>(&text)
                .map_err(|e| ConnectionError::File { path: file.to_path_buf(), message: format!("not read: {e}") })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Connection::default(),
            Err(e) => return Err(ConnectionError::File { path: file.to_path_buf(), message: e.to_string() }),
        };
        if let Some(u) = env.get("ARENA_URL") {
            c.url = Some(u.to_string());
        }
        if let Some(k) = env.get("ARENA_API_KEY") {
            c.api_key = Some(Secret::new(k));
        }
        if let Some(g) = env.get("ARENA_GATEWAY_URL") {
            c.gateway = Some(g.to_string());
        }
        if let Some(m) = env.get("ARENA_MCP_URL") {
            c.mcp_url = Some(m.to_string());
        }
        if let Some(m) = env.get("ARENA_CODE_INTEL_URL") {
            c.code_intel_url = Some(m.to_string());
        }
        if let Some(ca) = env.get("ARENA_CA_CERT") {
            c.ca = Some(PathBuf::from(ca));
        } else if c.ca.is_none()
            && let Some(ca) = env.get("SSL_CERT_FILE")
        {
            c.ca = Some(PathBuf::from(ca));
        }
        if let Some(u) = &c.url {
            c.url = Some(normalize_url(u)?);
        }
        // A known provider's key from its variable. Without Arena, a key in the environment is enough to use one.
        let arena = c.signed_in();
        for (name, var) in crate::env::PROVIDER_KEYS {
            let Some(key) = env.get(var) else { continue };
            match c.providers.get_mut(*name) {
                Some(p) if p.api_key.is_none() => {
                    p.api_key = Some(Secret::new(key));
                    p.key_from_env = true;
                }
                Some(_) => {}
                None if !arena && *name != "ollama" => {
                    c.providers.insert(
                        name.to_string(),
                        ProviderEntry {
                            api_key: Some(Secret::new(key)),
                            key_from_env: true,
                            from_env: true,
                            ..Default::default()
                        },
                    );
                }
                None => {}
            }
        }
        Ok(c)
    }

    /// Which edition this is: managed when signed in to Arena, standalone with a provider of one's own.
    pub fn edition(&self) -> Edition {
        if self.signed_in() {
            Edition::Managed
        } else if self.usable_providers().next().is_some() {
            Edition::Standalone
        } else {
            Edition::None
        }
    }

    /// The providers that can be asked: with a key, or one that needs none (a URL on this machine or the network the
    /// person named, such as Ollama).
    pub fn usable_providers(&self) -> impl Iterator<Item = (&String, &ProviderEntry)> {
        self.providers.iter().filter(|(name, p)| {
            p.api_key.as_ref().is_some_and(|k| !k.is_empty())
                || name.as_str() == "ollama"
                || p.url.as_deref().is_some_and(|u| u.starts_with("http://"))
        })
    }

    /// Writes it, readable by its owner only (0600 in a 0700 folder on Linux and macOS).
    pub fn save(&self, file: &Path) -> Result<(), ConnectionError> {
        let err = |e: std::io::Error| ConnectionError::File { path: file.to_path_buf(), message: e.to_string() };
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
            restrict_dir(dir).map_err(err)?;
        }
        // What the environment gave for this run stays out of the file.
        let mut kept = self.clone();
        kept.providers.retain(|_, p| !p.from_env);
        for p in kept.providers.values_mut() {
            if p.key_from_env {
                p.api_key = None;
            }
        }
        let text = serde_json::to_string_pretty(&kept)
            .map_err(|e| ConnectionError::File { path: file.to_path_buf(), message: e.to_string() })?;
        let tmp = file.with_extension("json.tmp");
        {
            let mut f = open_private(&tmp).map_err(err)?;
            f.write_all(text.as_bytes()).map_err(err)?;
            f.write_all(b"\n").map_err(err)?;
            f.sync_all().map_err(err)?;
        }
        std::fs::rename(&tmp, file).map_err(err)
    }

    pub fn signed_in(&self) -> bool {
        self.url.is_some() && self.api_key.as_ref().is_some_and(|k| !k.is_empty())
    }

    /// The gateway: as given, else `https://gateway.DOMAIN`. Without a trailing `/v1`.
    pub fn gateway_url(&self) -> Option<String> {
        if let Some(g) = &self.gateway {
            let g = g.trim().trim_end_matches('/');
            return Some(g.strip_suffix("/v1").unwrap_or(g).to_string());
        }
        let url = url::Url::parse(self.url.as_deref()?).ok()?;
        let host = url.host_str()?;
        if is_address(host) {
            // gateway.10.0.0.5 is no name: such an Arena gives its gateway explicitly.
            return None;
        }
        let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
        Some(format!("{}://gateway.{host}{port}", url.scheme()))
    }

    /// Arena's tools: as given, else `https://DOMAIN/mcp`.
    pub fn mcp_url(&self) -> Option<String> {
        self.mcp_url.clone().or_else(|| self.url.as_ref().map(|u| format!("{u}/mcp")))
    }

    /// Farzan Graph's tools: as given, else `https://code-intel.DOMAIN/mcp`; none for an Arena reached by an
    /// address or as localhost.
    pub fn code_intel_url(&self) -> Option<String> {
        if let Some(u) = &self.code_intel_url {
            return Some(u.clone());
        }
        let url = url::Url::parse(self.url.as_deref()?).ok()?;
        let host = url.host_str()?;
        if is_address(host) || host == "localhost" {
            return None;
        }
        Some(format!("{}://code-intel.{host}/mcp", url.scheme()))
    }
}

fn is_address(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[')
}

/// `llm.example.com` → `https://llm.example.com`; no trailing slash; only http and https.
pub fn normalize_url(text: &str) -> Result<String, ConnectionError> {
    let trimmed = text.trim();
    let with_scheme = if trimmed.contains("://") { trimmed.to_string() } else { format!("https://{trimmed}") };
    let url = url::Url::parse(&with_scheme)
        .map_err(|e| ConnectionError::Address(format!("{text:?} is not an address: {e}")))?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return Err(ConnectionError::Address(format!("{text:?}: only http and https addresses are taken")));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(ConnectionError::Address(format!("{text:?} names no host")));
    }
    Ok(with_scheme.trim_end_matches('/').to_string())
}

#[cfg(unix)]
fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)
}

#[cfg(not(unix))]
fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    // %APPDATA% is the person's own: Windows gives other users no access to it.
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(path)
}

#[cfg(unix)]
fn restrict_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_edition_by_what_it_is_connected_to() {
        let none = Connection::load(Path::new("/nonexistent/credentials.json"), &Env::from_pairs([])).expect("load");
        assert_eq!(none.edition(), Edition::None);
        let mine = Connection::load(
            Path::new("/nonexistent/credentials.json"),
            &Env::from_pairs([("DEEPSEEK_API_KEY", "sk-d")]),
        )
        .expect("load");
        assert_eq!(mine.edition(), Edition::Standalone);
        assert_eq!(mine.providers["deepseek"].api_key.as_ref().map(|k| k.expose().to_string()), Some("sk-d".into()));
        // Signed in to Arena: managed, and a key in the environment alone adds no provider.
        let arena = Connection::load(
            Path::new("/nonexistent/credentials.json"),
            &Env::from_pairs([
                ("ARENA_URL", "https://arena.example"),
                ("ARENA_API_KEY", "sk-a"),
                ("OPENAI_API_KEY", "sk-o"),
            ]),
        )
        .expect("load");
        assert_eq!(arena.edition(), Edition::Managed);
        assert!(arena.providers.is_empty());
        // The key is never in Debug, and what the environment gave is not written.
        assert!(!format!("{mine:?}").contains("sk-d"));
        let d = tempfile::tempdir().expect("temp");
        let f = d.path().join("credentials.json");
        let mut saved = mine.clone();
        saved
            .providers
            .insert("gpu".into(), ProviderEntry { url: Some("http://gpu:8000".into()), ..Default::default() });
        saved.save(&f).expect("save");
        let text = std::fs::read_to_string(&f).expect("read");
        assert!(!text.contains("sk-d") && !text.contains("deepseek") && text.contains("gpu"), "{text}");
    }

    #[test]
    fn addresses_are_normalized() {
        assert_eq!(normalize_url("llm.example.com/").expect("ok"), "https://llm.example.com");
        assert_eq!(normalize_url("http://10.0.0.5:8080").expect("ok"), "http://10.0.0.5:8080");
        assert!(normalize_url("ftp://x").is_err());
        assert!(normalize_url("https://").is_err());
    }

    #[test]
    fn gateway_and_tools_are_derived() {
        let c = Connection { url: Some("https://llm.example.com".into()), ..Default::default() };
        assert_eq!(c.gateway_url().as_deref(), Some("https://gateway.llm.example.com"));
        assert_eq!(c.mcp_url().as_deref(), Some("https://llm.example.com/mcp"));
        assert_eq!(c.code_intel_url().as_deref(), Some("https://code-intel.llm.example.com/mcp"));
        let ip = Connection { url: Some("http://10.0.0.5".into()), ..Default::default() };
        assert_eq!(ip.gateway_url(), None);
        assert_eq!(ip.code_intel_url(), None);
        let given = Connection { gateway: Some("https://gw.example.com/v1/".into()), ..c };
        assert_eq!(given.gateway_url().as_deref(), Some("https://gw.example.com"));
    }

    #[test]
    fn saved_privately_and_read_back_with_the_environment_over_it() {
        let dir = tempfile::tempdir().expect("temp");
        let file = dir.path().join("config").join("credentials.json");
        let c = Connection {
            url: Some("https://llm.example.com".into()),
            api_key: Some(Secret::new("sk-test-only")),
            ..Default::default()
        };
        c.save(&file).expect("saves");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let back = Connection::load(&file, &Env::default()).expect("loads");
        assert_eq!(back, c);
        let env = Env::from_pairs([("ARENA_URL", "other.example.com"), ("ARENA_API_KEY", "sk-env")]);
        let over = Connection::load(&file, &env).expect("loads");
        assert_eq!(over.url.as_deref(), Some("https://other.example.com"));
        assert_eq!(over.api_key.as_ref().map(Secret::expose), Some("sk-env"));
    }
}
