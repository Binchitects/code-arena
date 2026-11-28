//! The chat's own messages in the IDE: the box's history, the folders and the library a mention or a `/`
//! offers, a message's context, quote and rating, the preview of going back, and the built-in commands.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn).
//!
//! Each of the chat's parts is a file of its own under `ide_chat/`, and this file only hands each call on: a part
//! adds its messages to `MESSAGES`, an arm to the function that answers them, and a line to `shape` when it has a
//! share in a message sent. So two parts written at once touch one line each here.
//!
//! - `history.rs`: what was sent in this folder, for the box's ↑ (`history`), and each line sent kept in it.
//! - `library.rs`: the commands and skills the box's `/` offers (`library`), and a line of several `/names` sent.
//! - `replay.rs`: the running turn kept for a face that comes in late (the driver's turn loop keeps it).
//! - `mentions.rs`: any folder listed for a whole path typed after @ (`folders`), and what a message's mentions
//!   sent with it named for its chips (`attached`).
//! - `pickers.rs`: what the pickers offer, in `state` (the models as the gateway describes them, the levels of
//!   thinking, the modes).
//! - `queue.rs`: the messages sent while a turn runs, numbered, changed or taken back until read (`queue_edit`,
//!   `queue_cancel`, `queue_clear`), and said to be read (`took_in`).
//! - `context.rs`: what fills the window, piece by piece (`context_detail`'s parts), when it compacts and who set
//!   it (`compact` in `state` and `status`), and that changed from the window (`compact_at`).
//! - `pieces.rs`: the lines of files a message is sent with (its `context`), read for the model and named in its
//!   `files`.
//! - `rewind.rs`: what going back to a question would do (`rewind_preview`), and a question asked again.

use serde_json::Value;

use crate::ide_ext::{Area, Ctx, Rule, Sent, Shared, kind};
use crate::session::Prepared;

pub mod context;
mod history;
pub(crate) mod library;
mod mentions;
pub mod pickers;
mod pieces;
pub mod queue;
pub mod replay;
pub mod rewind;

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("history", Rule::Turn),
        ("library", Rule::Turn),
        ("folders", Rule::Turn),
        ("queue_edit", Rule::Turn),
        ("queue_cancel", Rule::Turn),
        ("queue_clear", Rule::Turn),
        ("rewind_preview", Rule::Turn),
        ("compact_at", Rule::Queue),
    ];

    fn shared(sh: &Shared, m: &Value) -> bool {
        match kind(m) {
            "history" => history::asked(sh, m),
            "library" => library::asked(sh, m),
            "folders" => mentions::asked(sh, m),
            "queue_edit" | "queue_cancel" | "queue_clear" => queue::asked(sh, m),
            "rewind_preview" => rewind::asked(sh, m),
            _ => false,
        }
    }

    fn state(sh: &Shared, p: Option<&Prepared>, v: &mut Value) {
        pickers::state(sh, v);
        v["queued"] = queue::items(sh);
        context::compact(sh, p, v);
    }

    fn status(sh: &Shared, p: Option<&Prepared>, v: &mut Value) {
        context::compact(sh, p, v);
    }

    fn start(sh: &Shared) {
        context::start(sh);
    }

    async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
        match kind(m) {
            "compact_at" => context::set(ctx, m),
            _ => return false,
        }
        true
    }

    fn shape(sh: &Shared, sent: &mut Sent) {
        pieces::sent(sh, sent);
        history::sent(sh, sent);
    }

    async fn before_turn(ctx: &mut Ctx<'_>, message: &str) {
        mentions::before_turn(ctx, message).await;
    }
}
