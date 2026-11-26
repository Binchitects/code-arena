//! The model, through Arena's gateway (OpenAI's chat completions, streamed) or another provider that speaks it.

mod gateway;
pub mod overflow;
pub mod providers;
mod sse;
pub mod types;

pub use gateway::{Gateway, LlmError, ModelInfo};
pub use types::{Completion, Finish, Message, Quiet, Request, Role, Sink, Thinking, ToolCall, ToolDef, Usage};
