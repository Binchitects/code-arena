//! The model, through Arena's gateway (OpenAI's chat completions, streamed) or another provider that speaks it.

mod gateway;
mod sse;
pub mod types;

pub use gateway::{Gateway, LlmError, ModelInfo};
pub use types::{Completion, Finish, Message, Quiet, Request, Role, Sink, Thinking, ToolCall, ToolDef, Usage};
