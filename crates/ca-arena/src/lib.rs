//! Farzan Arena's API as Code Arena uses it, for every Arena from 8.0 on.
//!
//! What a server has is found out by asking it ([`capabilities`]), never from its version string: an optional route
//! is probed once, and its answer says whether the server has it, has it turned off, or lacks it.

pub mod attachments;
pub mod automation;
pub mod broadcasts;
pub mod capabilities;
pub mod chats;
pub mod checklists;
mod client;
pub mod memory;
pub mod projects;
pub mod security;
pub mod signin;
pub mod skills;
pub mod types;

pub use capabilities::{Capabilities, Feature, Kept, State};
pub use client::{ArenaClient, ArenaError};
pub use types::{Info, Me};
