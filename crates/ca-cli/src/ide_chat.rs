//! The chat's own messages in the IDE: the box's history, the folders and the library a mention or a `/`
//! offers, a message's context, quote and rating, the preview of going back, and the built-in commands.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn). Nothing of this one is written yet.

use crate::ide_ext::{Area, Rule};

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[];
}
