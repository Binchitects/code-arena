//! The agent: a turn's loop of requests and tool calls, what runs without asking (the mode and the kept rules),
//! calls mended before they run, and the instructions it reads.

pub mod builtin_skills;
pub mod commits;
pub mod context;
pub mod goal;
pub mod hooks;
pub mod instructions;
pub mod library;
pub mod mend;
pub mod mentions;
pub mod policy;
pub mod skill_transfer;
pub mod steps;
pub mod task;
mod turn;

pub use context::Window;
pub use policy::{Answer, Approver, Decision, Mode, NoAsking, Rules};
pub use turn::{Agent, Aside, Events, Joined, Recorder, Skip, TurnError, TurnOutcome};
