//! The person's accounts in the IDE: the Arena signed in to (sign in, sign out, feedback) and the providers of
//! their own.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn). The providers of one's own are `providers`'s (its
//! messages are said there); the Arena's part is not written yet.

mod providers;

use serde_json::Value;

use crate::ide_ext::{Area, Ctx, Rule, Shared};

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] =
        &[("providers", Rule::Turn), ("provider_add", Rule::Queue), ("provider_remove", Rule::Queue)];

    fn shared(sh: &Shared, m: &Value) -> bool {
        providers::shared(sh, m)
    }

    async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
        providers::idle(ctx, m).await
    }
}
