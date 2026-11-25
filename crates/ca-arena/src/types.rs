//! The shapes Arena answers with. Fields that came in later Arenas are `Option`, so an 8.0 answer parses.

use serde::Deserialize;

/// `GET /api/info`: public, what the sign-in page shows.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    #[serde(default)]
    pub name: Option<String>,
    /// Shown only: features are never decided by it.
    #[serde(default)]
    pub version: Option<String>,
    /// Where Farzan Graph's MCP is, when it is on (null: off).
    #[serde(default)]
    pub code_intel_url: Option<String>,
    #[serde(default)]
    pub support_contact: Option<String>,
}

/// `GET /api/code-arena/me`: whose the key is.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    pub user_name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub manages_people: bool,
    /// The feature permissions the person does not hold (`feature.memory`…): Code Arena leaves those out.
    #[serde(default)]
    pub denied: Vec<String>,
}

impl Me {
    pub fn may(&self, feature: &str) -> bool {
        !self.denied.iter().any(|d| d == feature)
    }
}

/// Arena's refusal: `{"status": code, "error": message}`.
#[derive(Debug, Clone, Deserialize)]
pub struct Problem {
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_8_0_me_parses() {
        let me: Me = serde_json::from_str(
            r#"{"userName":"sara","displayName":"Sara","email":null,"managesPeople":false,"denied":["feature.memory"]}"#,
        )
        .expect("parses");
        assert_eq!(me.user_name, "sara");
        assert!(!me.may("feature.memory"));
        assert!(me.may("feature.projects"));
    }

    #[test]
    fn info_without_new_fields_parses() {
        let info: Info = serde_json::from_str(r#"{"name":"Arena","version":"8.0.0"}"#).expect("parses");
        assert_eq!(info.code_intel_url, None);
    }
}
