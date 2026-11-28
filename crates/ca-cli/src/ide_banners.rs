//! What the window says across its top: no model, not signed in, Arena's broadcasts; and Arena's settings as
//! they change.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn).
//!
//! - `state` gains `no_model` (the session has no model to ask: the editor, the terminal, git and the tasks work,
//!   a message gets a notice instead of an answer), `signed_out` (no Arena and no provider of one's own) and
//!   `broadcasts`, as below.
//! - `broadcasts` [T] → `broadcasts{items: [{id, level, text}]}`, the newest first (`level`: `info`, `warning`,
//!   `critical`). It is also said by itself once a minute when Arena's list changed (asked of Arena by
//!   `ca_arena::broadcasts`, managed edition only). A `key` the request carries is said again.
//! - `broadcast_dismiss{id}` [T]: hidden for the person (a critical one stays), the list said again at once and
//!   Arena told (`POST …/broadcasts/{id}/dismiss`); Arena's refusal is a notice.
//! - Arena's settings: `session::watch_settings` (the terminal's 30 s stamp poll of `GET /settings`) reads Arena's
//!   skills again when the stamp moves; the gateway's models are then asked again and, between turns
//!   (`_arena_settings`), the driver's models replaced and `settings_changed{models, model_info}` said, then
//!   `library` (the skills as they are now) and a notice.

use std::sync::{Arc, Mutex, PoisonError};

use ca_arena::broadcasts::{Broadcast, Level};
use serde_json::{Value, json};

use crate::ide_ext::{Area, Ctx, Rule, Sent, Shared, kind, send};

/// This area in the driver.
pub struct Here;

/// What this area keeps for the driver's life: its background work (ended when it begins again), the broadcasts
/// last read, and the gateway's models asked once Arena's settings moved.
#[derive(Default)]
struct Kept {
    work: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    list: Mutex<Vec<Broadcast>>,
    models: Mutex<Option<Vec<ca_llm::ModelInfo>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The words in place of an answer when the session has no model.
pub fn no_model_text(signed_out: bool) -> &'static str {
    if signed_out {
        "Not signed in to an Arena: the editor, terminal, git and tasks work, the agent has no model. Sign in with code-arena login --url https://YOUR-ARENA, or give a provider of your own with code-arena login --provider NAME, and open the IDE again."
    } else {
        "The gateway lists no model for your key: the editor, terminal, git and tasks work, the agent has no model. Ask your Arena's admin for a model, or set one with code-arena config set model NAME."
    }
}

/// The `broadcasts` message for this list (and the key of the request it answers).
fn listed(list: &[Broadcast], key: Option<&Value>) -> Value {
    let mut v = json!({"type": "broadcasts", "items": list});
    if let Some(k) = key.filter(|k| k.is_string() || k.is_number()) {
        v["key"] = k.clone();
    }
    v
}

/// The list without the one dismissed; a critical one stays. None: there was nothing to take away.
fn without(list: &[Broadcast], id: &str) -> Option<Vec<Broadcast>> {
    let at = list.iter().position(|b| b.id == id && b.level != Level::Critical)?;
    let mut rest = list.to_vec();
    rest.remove(at);
    Some(rest)
}

fn signed_out(sh: &Shared) -> bool {
    sh.edition == ca_settings::Edition::None
}

/// The session has no model to ask: none was found, or there is nothing to ask one of (a model named by `--model`,
/// the settings or a session taken up is no model when no Arena and no provider is there to answer).
fn modelless(model: &str, edition: ca_settings::Edition) -> bool {
    model.is_empty() || edition == ca_settings::Edition::None
}

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] =
        &[("broadcasts", Rule::Turn), ("broadcast_dismiss", Rule::Turn), ("_arena_settings", Rule::Latest)];

    fn start(sh: &Shared) {
        let kept = sh.kept.of::<Kept>();
        let mut work = lock(&kept.work);
        work.drain(..).for_each(|w| w.abort());
        let Some(arena) = sh.arena.clone().filter(|_| sh.edition == ca_settings::Edition::Managed) else { return };
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        // The broadcasts: now, then once a minute, said when they are other than those said last.
        let (out, kept2, arena2) = (sh.out.clone(), kept.clone(), arena.clone());
        work.push(rt.spawn(async move {
            loop {
                if let Ok(now) = arena2.broadcasts().await {
                    let changed = {
                        let mut list = lock(&kept2.list);
                        let changed = *list != now;
                        *list = now.clone();
                        changed
                    };
                    if changed {
                        send(&out, listed(&now, None));
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        }));
        // Arena's settings: once its skills are read again, the gateway's models asked again, and the rest between
        // turns.
        let sh2 = sh.clone();
        let changed = move || {
            let sh = sh2.clone();
            tokio::spawn(async move {
                let asked = tokio::time::timeout(std::time::Duration::from_secs(15), sh.gateway.models()).await;
                if let Ok(Ok(models)) = asked {
                    *lock(&sh.kept.of::<Kept>().models) = Some(models);
                }
                sh.post(json!({"type": "_arena_settings"}));
            });
        };
        let builtin = sh.settings.bool(&ca_settings::schema::BUILTIN_SKILLS);
        work.push(crate::session::watch_settings(arena, sh.paths.data.clone(), builtin, sh.notices.clone(), changed));
    }

    fn shared(sh: &Shared, m: &Value) -> bool {
        let kept = sh.kept.of::<Kept>();
        match kind(m) {
            "broadcasts" => {
                let list = lock(&kept.list).clone();
                send(&sh.out, listed(&list, Some(&m["key"])));
                true
            }
            "broadcast_dismiss" => {
                let id = m["id"].as_str().map(str::to_string).unwrap_or_else(|| m["id"].to_string());
                let rest = {
                    let mut list = lock(&kept.list);
                    let rest = without(&list, &id);
                    if let Some(r) = &rest {
                        list.clone_from(r);
                    }
                    rest
                };
                let Some(rest) = rest else { return true };
                let mut reply = listed(&rest, None);
                reply["id"] = json!(id);
                send(&sh.out, reply);
                if let (Some(arena), Ok(rt)) = (sh.arena.clone(), tokio::runtime::Handle::try_current()) {
                    let out = sh.out.clone();
                    rt.spawn(async move {
                        if let Err(e) = arena.dismiss_broadcast(&id).await {
                            crate::ide_ext::say(&out, &format!("The broadcast could not be dismissed in Arena: {e}"));
                        }
                    });
                }
                true
            }
            _ => false,
        }
    }

    async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
        if kind(m) != "_arena_settings" {
            return false;
        }
        let models = lock(&ctx.sh.kept.of::<Kept>().models).take();
        let mut now = ctx.sh.clone();
        if let Some(models) = models.filter(|ms| !ms.is_empty()) {
            ctx.drv.models = models.iter().map(|m| m.id.clone()).collect();
            ctx.sh.kept.of::<crate::ide_chat::pickers::Infos>().set(models);
            now.models = Arc::new(ctx.drv.models.clone());
        }
        let mut v = json!({"type": "settings_changed", "models": now.models.as_slice()});
        crate::ide_chat::pickers::state(&now, &mut v);
        send(&ctx.sh.out, json!({"type": "settings_changed", "models": v["models"], "model_info": v["model_info"]}));
        crate::ide_chat::library::asked(ctx.sh, &Value::Null);
        ctx.sh.say("Arena's settings changed: its models and skills are read again, and used from your next message.");
        true
    }

    /// A message to a session with no model is answered here, with what to do; a `/command` or a `!command` is
    /// left to the areas that run them.
    async fn command(ctx: &mut Ctx<'_>, sent: &mut Sent) {
        let text = sent.shown.trim_start();
        if sent.done || !modelless(&ctx.p.agent.model, ctx.sh.edition) || text.starts_with('/') || text.starts_with('!')
        {
            return;
        }
        ctx.sh.say(no_model_text(signed_out(ctx.sh)));
        sent.done = true;
    }

    fn state(sh: &Shared, p: Option<&crate::session::Prepared>, v: &mut Value) {
        let model = p.map_or(sh.was.model.as_str(), |p| p.agent.model.as_str());
        v["no_model"] = json!(modelless(model, sh.edition));
        v["signed_out"] = json!(signed_out(sh));
        v["broadcasts"] = json!(*lock(&sh.kept.of::<Kept>().list));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ide_ext::rule;

    fn b(id: &str, level: Level) -> Broadcast {
        Broadcast { id: id.into(), level, text: format!("about {id}"), at: String::new() }
    }

    #[test]
    fn a_broadcast_is_dismissed_by_its_id_and_a_critical_one_stays() {
        let list = vec![b("a", Level::Critical), b("b", Level::Warning), b("c", Level::Info)];
        assert_eq!(without(&list, "b").expect("dismissed"), [b("a", Level::Critical), b("c", Level::Info)]);
        assert_eq!(without(&list, "a"), None, "a critical one stays");
        assert_eq!(without(&list, "x"), None);
    }

    #[test]
    fn the_list_is_said_with_the_key_it_answers() {
        let v = listed(&[b("a", Level::Warning)], Some(&json!(7)));
        assert_eq!(
            v,
            json!({"type": "broadcasts", "items": [{"id": "a", "level": "warning", "text": "about a"}], "key": 7})
        );
        assert!(listed(&[], Some(&Value::Null)).get("key").is_none());
    }

    #[test]
    fn a_session_has_no_model_without_one_or_without_anything_to_ask() {
        use ca_settings::Edition;
        assert!(modelless("", Edition::Managed));
        assert!(modelless("qwen", Edition::None), "a model named in the settings, signed out");
        assert!(!modelless("qwen", Edition::Managed));
        assert!(!modelless("gpt", Edition::Standalone));
    }

    #[test]
    fn its_messages_are_answered_by_their_rules() {
        assert_eq!(rule("broadcasts"), Some(Rule::Turn));
        assert_eq!(rule("broadcast_dismiss"), Some(Rule::Turn));
        assert_eq!(rule("_arena_settings"), Some(Rule::Latest));
        assert!(no_model_text(true).starts_with("Not signed in to an Arena: the editor, terminal, git and tasks work"));
        assert!(no_model_text(false).contains("the agent has no model"));
    }
}
