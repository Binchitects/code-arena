//! Code Arena's shared ground: its version, where it keeps things, and the `Secret` type that is never printed.

pub mod paths;
pub mod secret;

pub use paths::Paths;
pub use secret::Secret;

/// This build's version, as Cargo has it (`0.1.0`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The product's name, as people see it.
pub const PRODUCT: &str = "Code Arena";

/// Where the source is.
pub const SOURCE: &str = "https://github.com/Binchitects/code-arena";

/// The User-Agent every request carries: Arena keeps who runs which version by it.
pub fn user_agent() -> String {
    format!("code-arena/{VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_names_the_version() {
        assert_eq!(user_agent(), format!("code-arena/{}", env!("CARGO_PKG_VERSION")));
        assert!(user_agent().starts_with("code-arena/0."));
    }
}
