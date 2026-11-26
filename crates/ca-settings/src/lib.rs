//! Every setting Code Arena reads, declared once ([`schema`]), and the layers they come from ([`Settings`]):
//! the defaults, the person's `settings.json`, the project's `.code-arena/settings.json`, then the environment.
//! The Arena connection (address, API key) is apart, in [`connection`].
//!
//! Nothing else in Code Arena reads the environment or a settings file: a test fails when code does, so every
//! setting is in the schema and so in the Settings editor.

pub mod connection;
pub mod env;
pub mod jsonc;
pub mod schema;
pub mod settings;

pub use connection::{Connection, Edition, ProviderEntry};
pub use env::Env;
pub use schema::{Kind, Scope, Setting};
pub use settings::{Layer, Problem, Settings, Source};
