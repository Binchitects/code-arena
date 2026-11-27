//! A virtual machine's sign-in, asked of the person when a tool needs it.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn). Nothing of this one is written yet.

use crate::ide_ext::{Area, Rule};

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[];
}
