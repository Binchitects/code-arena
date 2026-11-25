//! A value that must never reach a log, a terminal or an error message: an API key, a password.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A secret string. `Debug` and `Display` print `••••`; only [`Secret::expose`] gives the value, and each call to it
/// is a place to review.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value itself: for a header or a file only the owner reads.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(••••)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("••••")
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Secret)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_printed() {
        let s = Secret::new("sk-not-a-real-key");
        assert_eq!(format!("{s}"), "••••");
        assert_eq!(format!("{s:?}"), "Secret(••••)");
        assert!(!format!("{:?}", Some(&s)).contains("sk-"));
        assert_eq!(s.expose(), "sk-not-a-real-key");
    }

    #[test]
    fn round_trips_through_json() {
        let s = Secret::new("abc");
        let text = serde_json::to_string(&s).expect("serializes");
        let back: Secret = serde_json::from_str(&text).expect("parses");
        assert_eq!(back, s);
    }
}
