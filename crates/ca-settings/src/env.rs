//! The environment, read once at start. Code Arena reads variables only through this, so a test can give any.

use std::collections::BTreeMap;

/// A snapshot of the variables Code Arena reads (the schema's and the connection's), taken once.
#[derive(Debug, Clone, Default)]
pub struct Env {
    vars: BTreeMap<String, String>,
}

/// The variables Code Arena reads that are not settings: the connection's, and where its folders are.
pub const CONNECTION_VARS: &[(&str, &str)] = &[
    ("ARENA_URL", "the Arena's address, for this run"),
    ("ARENA_API_KEY", "the API key, for this run (CI, scripts); never printed"),
    ("ARENA_GATEWAY_URL", "the gateway, when it is not at https://gateway.DOMAIN"),
    ("ARENA_MCP_URL", "Arena's tools, when they are not at https://DOMAIN/mcp"),
    ("ARENA_CODE_INTEL_URL", "Farzan Graph's tools, when they are not at https://code-intel.DOMAIN/mcp"),
    ("ARENA_CA_CERT", "a CA file to trust besides the system's, for this run"),
    ("SSL_CERT_FILE", "a CA file to trust, read last"),
    ("CODE_ARENA_HOME", "one folder for settings and data (config/ and data/), for a portable copy"),
    ("XDG_CONFIG_HOME", "where settings go on Linux, as the system says"),
    ("XDG_DATA_HOME", "where data goes on Linux, as the system says"),
    ("NO_COLOR", "no colours in the terminal"),
];

impl Env {
    /// The process's own environment: only the variables Code Arena knows, so nothing else is kept in memory.
    pub fn from_process() -> Self {
        let mut vars = BTreeMap::new();
        let known =
            crate::schema::ALL.iter().filter_map(|s| s.env).chain(CONNECTION_VARS.iter().map(|(name, _)| *name));
        for name in known {
            if let Ok(value) = std::env::var(name) {
                vars.insert(name.to_string(), value);
            }
        }
        Self { vars }
    }

    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self { vars: pairs.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }

    /// A variable's value; an empty one counts as not set.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str).filter(|v| !v.is_empty())
    }
}
