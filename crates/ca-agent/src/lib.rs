//! The agent: a turn's loop of requests and tool calls, what runs without asking (the mode and the kept rules),
//! calls mended before they run, and the instructions it reads.

pub mod instructions;
pub mod mend;
pub mod policy;
mod turn;

pub use policy::{Answer, Approver, Decision, Mode, NoAsking, Rules};
pub use turn::{Agent, Events, Recorder, TurnError, TurnOutcome};
