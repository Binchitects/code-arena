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

/// `${NAME}` in an MCP server's `env` or `headers`, taken from the environment, so tokens stay out of settings
/// files. Names that are not set come back in the second list (and expand to nothing).
pub fn expand(text: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(text.len());
    let mut missing = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => missing.push(name.to_string()),
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    (out, missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_not_set_expand_to_nothing_and_are_said() {
        let (text, missing) = expand("Bearer ${CODE_ARENA_TEST_SURELY_UNSET} and ${unclosed");
        assert_eq!(text, "Bearer  and ${unclosed");
        assert_eq!(missing, vec!["CODE_ARENA_TEST_SURELY_UNSET"]);
    }
}
