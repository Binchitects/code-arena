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

use serde_json::Value;

use crate::ide_ext::{Area, Ctx, Rule, Sent, Shared, kind};

mod history;
pub(crate) mod library;
mod mentions;
pub mod replay;

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] =
        &[("history", Rule::Turn), ("library", Rule::Turn), ("folders", Rule::Turn)];

    fn shared(sh: &Shared, m: &Value) -> bool {
        match kind(m) {
            "history" => history::asked(sh, m),
            "library" => library::asked(sh, m),
            "folders" => mentions::asked(sh, m),
            _ => false,
        }
    }

    fn shape(sh: &Shared, sent: &mut Sent) {
        history::sent(sh, sent);
    }

    async fn before_turn(ctx: &mut Ctx<'_>, message: &str) {
        mentions::before_turn(ctx, message).await;
    }
}
