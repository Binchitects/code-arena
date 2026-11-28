//! How full the model's window is and when the session compacts: what the Context panel and the ring's popover
//! read in `context_detail` (each part's pieces, the room kept for the answer, what is free, what could be freed),
//! the threshold with who set it in `state` and `status` (`compact`), and the threshold changed from the window
//! (`compact_at`), live and kept in settings.json.

use std::sync::Mutex;

use ca_settings::schema::{COMPACT_AT, COMPACT_TARGET};
use ca_settings::settings::Source;
use serde_json::{Value, json};

use crate::ide_ext::{Ctx, Shared, send};
use crate::session::Prepared;

/// The most pieces listed of one part; the rest are said as one row.
const MOST: usize = 60;

/// What the Arena account says the threshold is (`Some(0)`: only when needed), once asked.
#[derive(Default)]
struct Account(Mutex<Option<i64>>);

impl Account {
    fn get(&self) -> Option<i64> {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The kinds `context_detail` lists, in the order of [`ca_agent::context::kinds_text`].
const KINDS: [&str; 6] = ["Instructions", "Tools", "Your messages", "Answers", "Tool results", "Notes"];

/// The pieces of one part, the biggest first, at most [`MOST`] with a row for the rest: `[{name, tokens, at}]`.
fn listed(mut mine: Vec<(String, Option<usize>, u64)>) -> Vec<Value> {
    mine.sort_by_key(|p| std::cmp::Reverse(p.2));
    let rest: u64 = mine.iter().skip(MOST).map(|p| p.2).sum();
    let more = mine.len().saturating_sub(MOST);
    let mut out: Vec<Value> =
        mine.into_iter().take(MOST).map(|(name, at, tokens)| json!({"name": name, "tokens": tokens, "at": at})).collect();
    if more > 0 {
        out.push(json!({"name": format!("{more} more"), "tokens": rest, "more": true}));
    }
    out
}

/// What compacting now would free: the conversation less what it keeps whole (the recent part, `target` percent of
/// the window), and the summary's room. Nothing when the conversation is within what it keeps.
fn compact_frees(conversation: u64, size: u64, target: u64) -> u64 {
    let kept = size * target / 100;
    conversation.saturating_sub(kept + size / 50)
}

/// `context_detail`'s fields beside the kinds: each kind's `pieces`, the `reserve` kept for the answer, what is
/// `free`, how it was `counted`, and what `could_free` it (`[{group, tokens}]`: `compact` is compacting now).
pub fn detail(p: &Prepared, v: &mut Value) {
    let w = &p.agent.window;
    let used = p.agent.context_used();
    let mut by_kind: Vec<Vec<(String, Option<usize>, u64)>> = vec![Vec::new(); KINDS.len()];
    for piece in p.agent.context_pieces() {
        if let Some(list) = by_kind.get_mut(piece.kind) {
            list.push((piece.name, piece.at, w.tokens(piece.chars)));
        }
    }
    let conversation: u64 = by_kind[2..].iter().flatten().map(|p| p.2).sum();
    if let Some(kinds) = v["kinds"].as_array_mut() {
        for (k, mine) in kinds.iter_mut().zip(by_kind) {
            k["pieces"] = json!(listed(mine));
        }
    }
    v["reserve"] = json!(w.reserve);
    v["free"] = json!(w.size.saturating_sub(used + w.reserve));
    v["counted"] = json!("estimate");
    let frees = compact_frees(conversation, w.size, w.target);
    v["could_free"] = json!(if frees > 0 { vec![json!({"group": "compact", "tokens": frees})] } else { vec![] });
}

/// Who set the threshold: `settings` (a settings file), `flag`, `env`, `account` (the Arena account, the setting
/// not given), `default`.
fn source(sh: &Shared) -> &'static str {
    match sh.settings.resolve(&COMPACT_AT).1 {
        Source::User(_) | Source::Project(_) => "settings",
        Source::Flag => "flag",
        Source::Env(_) => "env",
        Source::Default if sh.kept.of::<Account>().get().is_some() => "account",
        Source::Default => "default",
    }
}

/// `compact`: where the session compacts (`at`: a share, 0: only when a request would not fit), what it keeps
/// whole (`target`), who set it (`source`) and what the Arena account says (`account`, null when it did not).
pub fn compact(sh: &Shared, p: Option<&Prepared>, v: &mut Value) {
    let (at, target) = match p {
        Some(p) => (p.agent.window.compact_at.unwrap_or(0), p.agent.window.target),
        None => (sh.was.context["compact_at"].as_u64().unwrap_or(0), sh.settings.integer(&COMPACT_TARGET).max(5) as u64),
    };
    v["compact"] = json!({"at": at, "target": target, "source": source(sh), "account": sh.kept.of::<Account>().get()});
    if let Some(p) = p {
        v["compact"]["reserve"] = json!(p.agent.window.reserve);
    }
}

/// What the Arena account says, asked once in the background (a managed edition signed in, the setting not given).
pub fn start(sh: &Shared) {
    if sh.edition != ca_settings::Edition::Managed || sh.kept.of::<Account>().get().is_some() {
        return;
    }
    let (Some(arena), Ok(runtime)) = (sh.arena.clone(), tokio::runtime::Handle::try_current()) else { return };
    let account = sh.kept.of::<Account>();
    runtime.spawn(async move {
        if let Ok(Ok(Some(n))) = tokio::time::timeout(std::time::Duration::from_secs(8), arena.compact_at()).await {
            *account.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(n);
        }
    });
}

/// The threshold asked for, checked: `Ok((at, target))`, `at` 0 for only when needed; else why not, in the
/// dialog's words. `at`: a share (20 to 95), 0 or null for only when needed, `"account"` for the account's (its
/// value given). What is kept is from 5% to ten points under the threshold (under 95% when only when needed).
pub fn checked(at: &Value, target: &Value, account: Option<i64>) -> Result<(u64, u64), String> {
    let at = match at {
        Value::Null => 0,
        Value::String(s) if s == "account" => match account.unwrap_or(80) {
            n if n <= 0 => 0,
            n => n.clamp(20, 95) as u64,
        },
        v => match v.as_u64() {
            Some(0) => 0,
            Some(n) if (20..=95).contains(&n) => n,
            _ => return Err("The threshold is from 20% to 95% of the window.".into()),
        },
    };
    let ceiling = if at == 0 { 95 } else { at } - 10;
    match target.as_u64() {
        Some(t) if (5..=ceiling).contains(&t) => Ok((at, t)),
        _ => Err(format!("What is kept is from 5% to {ceiling}% (10 points under the threshold).")),
    }
}

/// `compact_at{at, target}`: the threshold changed, in the live window and in the person's settings.json
/// (`"account"`: the setting taken out, so the account's is read). Answered with `compact_set{at, target, source,
/// context}`, or `compact_refused{at, target, error}`.
pub fn set(ctx: &mut Ctx<'_>, m: &Value) {
    let account = ctx.sh.kept.of::<Account>().get();
    let refused = |error: String| json!({"type": "compact_refused", "at": m["at"], "target": m["target"], "error": error});
    let (at, target) = match checked(&m["at"], &m["target"], account) {
        Ok(v) => v,
        Err(e) => return send(&ctx.drv.out, refused(e)),
    };
    let root = ctx.p.agent.ctx.workspace.root().to_path_buf();
    let user = ca_settings::Layer::User;
    let value = if m["at"] == "account" { Value::Null } else { json!(at) };
    let kept = crate::ide::keep_setting(ctx.app, &root, COMPACT_AT.key, &value, user, None)
        .and_then(|_| crate::ide::keep_setting(ctx.app, &root, COMPACT_TARGET.key, &json!(target), user, None));
    if let Err(e) = kept {
        return send(&ctx.drv.out, refused(e));
    }
    crate::ide_settings::kept(ctx.sh, &ctx.app.settings);
    let w = &mut ctx.p.agent.window;
    w.compact_at = (at > 0).then_some(at);
    w.target = target;
    let source = if m["at"] == "account" { "account" } else { "settings" };
    let context = json!({"used": ctx.p.agent.context_used(), "size": ctx.p.agent.window.size, "compact_at": ctx.p.agent.window.compact_at});
    send(
        &ctx.drv.out,
        json!({"type": "compact_set", "at": at, "target": target, "source": source, "account": account, "context": context}),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_threshold_is_a_share_from_20_to_95_or_only_when_needed_and_what_is_kept_ten_points_under_it() {
        assert_eq!(checked(&json!(80), &json!(25), None), Ok((80, 25)));
        assert_eq!(checked(&json!(0), &json!(85), None), Ok((0, 85)));
        assert_eq!(checked(&Value::Null, &json!(5), None), Ok((0, 5)));
        assert_eq!(checked(&json!("account"), &json!(25), Some(60)), Ok((60, 25)));
        assert_eq!(checked(&json!("account"), &json!(25), None), Ok((80, 25)), "the account said nothing: 80%");
        assert_eq!(checked(&json!(19), &json!(5), None), Err("The threshold is from 20% to 95% of the window.".into()));
        assert_eq!(checked(&json!(96), &json!(5), None).is_err(), true);
        assert_eq!(
            checked(&json!(30), &json!(25), None),
            Err("What is kept is from 5% to 20% (10 points under the threshold).".into())
        );
        assert!(checked(&json!(80), &json!(4), None).is_err());
    }

    #[test]
    fn a_part_lists_its_biggest_pieces_and_says_the_rest_as_one_row() {
        let many: Vec<(String, Option<usize>, u64)> = (0..65).map(|i| (format!("p{i}"), Some(i), i as u64)).collect();
        let l = listed(many);
        assert_eq!(l.len(), MOST + 1);
        assert_eq!(l[0]["name"], "p64");
        assert_eq!(l[MOST], json!({"name": "5 more", "tokens": 10, "more": true}));
        assert_eq!(listed(vec![("a".into(), None, 3)]), vec![json!({"name": "a", "tokens": 3, "at": null})]);
    }

    #[test]
    fn compacting_frees_what_the_conversation_holds_over_what_it_keeps() {
        assert_eq!(compact_frees(20_000, 32_768, 25), 20_000 - 8_192 - 655);
        assert_eq!(compact_frees(5_000, 32_768, 25), 0);
    }
}
