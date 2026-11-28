//! The providers of the person's own, as the Other providers dialog keeps them: listed, added with a key, and
//! removed. They are kept in `credentials.json` (readable by its owner only, as `login --provider` keeps them); a
//! key is never said back to a face, only where it comes from.
//!
//! - `providers` [T] → `providers{allowed, items, known}` (and the request's `key`). `allowed`: false while Arena's
//!   admins do not let Code Arena use other providers (its chat config's `codeArenaProviders`); always true without
//!   Arena. `items: [{name, kind, url, key, key_env?, models, more}]`, by name: `kind` is `anthropic` (Messages
//!   API) or `openai` (chat completions); `key` is `here` (kept in the file), `env` (read from `key_env` at each
//!   run) or `none`; `models` the first four of its models the pickers offer (without the `name:`), `more` how
//!   many others. `known: [{name, kind, url, key_env, about}]`: the providers known by name.
//! - `provider_add{name, url?, kind?, api_key?}` [Q]: its models listed with that key (or a known one's variable's
//!   when none is given) to check it, then kept, and the session's models asked again. Answered by
//!   `provider_added{name, ok, error?, models?}`, then `settings_changed{models, model_info}`, `providers` and a
//!   notice. The key is under `api_key`, not `key`: what is kept for a turn's end is said back by its `key`.
//! - `provider_remove{name}` [Q]: forgotten with its key, the models asked again; `provider_removed{name, ok,
//!   error?}`, then as above. One that is there only for its variable cannot be (the notice says which).

use std::sync::Arc;
use std::time::Duration;

use ca_core::Secret;
use ca_llm::providers::{self, Kind};
use ca_settings::Connection;
use ca_settings::connection::ProviderEntry;
use serde_json::{Value, json};

use crate::ide_ext::{Ctx, Shared, kind, send};

/// How many of a provider's models its row names.
const SHOWN_MODELS: usize = 4;

/// The words when Arena's admins keep other providers out.
pub const BANNED: &str =
    "Arena's admins do not let Code Arena use other providers (Admin → Settings → Code Arena): only Arena's models are offered.";

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Anthropic => "anthropic",
        Kind::OpenAi => "openai",
    }
}

/// A known provider's key variable, by its name.
fn key_env(name: &str) -> Option<&'static str> {
    providers::known(name).map(|k| k.key_env)
}

/// One provider's row: what it is and where its key comes from, never the key.
pub fn item(name: &str, entry: &ProviderEntry, models: &[String]) -> Value {
    let parsed = providers::Provider::parse(name, &entry.settings()).ok();
    let kind = parsed.as_ref().map(|p| kind_name(p.kind)).or(entry.kind.as_deref()).unwrap_or("openai");
    let url = parsed.as_ref().map(|p| p.url.clone()).or_else(|| entry.url.clone()).unwrap_or_default();
    let has_key = entry.api_key.as_ref().is_some_and(|k| !k.is_empty());
    let key = if entry.key_from_env && has_key {
        "env"
    } else if has_key {
        "here"
    } else {
        "none"
    };
    let prefix = format!("{name}:");
    let mut own: Vec<String> = models.iter().filter_map(|m| m.strip_prefix(&prefix)).map(str::to_string).collect();
    if own.is_empty() {
        own.clone_from(&entry.models);
    }
    let more = own.len().saturating_sub(SHOWN_MODELS);
    own.truncate(SHOWN_MODELS);
    let mut v = json!({"name": name, "kind": kind, "url": url, "key": key, "models": own, "more": more});
    if let Some(var) = key_env(name) {
        v["key_env"] = json!(var);
    }
    v
}

/// The `providers` message: every provider kept, the known ones, and whether they may be used.
pub fn listed(conn: &Connection, models: &[String], allowed: bool) -> Value {
    let items: Vec<Value> = conn.providers.iter().map(|(name, e)| item(name, e, models)).collect();
    let known: Vec<Value> = providers::KNOWN
        .iter()
        .filter(|k| k.name != "ollama")
        .map(|k| json!({"name": k.name, "kind": kind_name(k.kind), "url": k.url, "key_env": k.key_env, "about": k.about}))
        .collect();
    json!({"type": "providers", "allowed": allowed, "items": items, "known": known})
}

/// Whether Arena's admins let the person's own providers be used: as the session asked when it began
/// (`codeArenaProviders`; not said, or no answer: they do). Without Arena there is no one to ask.
pub async fn allowed(edition: ca_settings::Edition, arena: Option<ca_arena::ArenaClient>) -> bool {
    if edition != ca_settings::Edition::Managed {
        return true;
    }
    let Some(a) = arena else { return true };
    tokio::time::timeout(Duration::from_secs(8), a.config())
        .await
        .ok()
        .and_then(|c| c.ok())
        .and_then(|c| c.get("codeArenaProviders").and_then(Value::as_bool))
        .unwrap_or(true)
}

/// What is asked for a provider, as `provider_add` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    pub name: String,
    pub url: Option<String>,
    pub kind: Option<String>,
    pub key: String,
}

impl Asked {
    pub fn of(m: &Value) -> Asked {
        let s = |k: &str| m[k].as_str().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
        Asked {
            name: s("name").unwrap_or_default().to_lowercase(),
            url: s("url"),
            kind: s("kind"),
            key: m["api_key"].as_str().unwrap_or("").trim().to_string(),
        }
    }
}

/// The entry to keep and the provider it is, from what was asked and what is kept already; or why not.
pub fn entry_for(asked: &Asked, had: Option<&ProviderEntry>) -> Result<(ProviderEntry, providers::Provider), String> {
    if !providers::valid_name(&asked.name) {
        return Err(format!(
            "'{}' is not a provider's name: lower-case letters, digits and '-' only",
            asked.name
        ));
    }
    let mut entry = had.cloned().unwrap_or_default();
    // What was there only for the environment's key is the person's own from now on.
    entry.from_env = false;
    if asked.url.is_some() {
        entry.url.clone_from(&asked.url);
    }
    if asked.kind.is_some() {
        entry.kind.clone_from(&asked.kind);
    }
    let provider = providers::Provider::parse(&asked.name, &entry.settings())?;
    Ok((entry, provider))
}

/// The key to use: the one given, else a known provider's variable's (`true`: from the environment); a provider
/// on this machine or the network the person named may have none.
pub fn key_for(
    asked: &Asked,
    provider: &providers::Provider,
    env: &ca_settings::Env,
) -> Result<(Secret, bool), String> {
    if !asked.key.is_empty() {
        return Ok((Secret::new(asked.key.clone()), false));
    }
    let var = key_env(&asked.name);
    if let Some(k) = var.and_then(|v| env.get(v)) {
        return Ok((Secret::new(k), true));
    }
    if asked.name == "ollama" || provider.url.starts_with("http://") {
        return Ok((Secret::new(""), false));
    }
    Err(match var {
        Some(v) => format!("{} needs a key: give it, or set {v}", asked.name),
        None => format!("{} needs a key", asked.name),
    })
}

/// A reply with the key of the request it answers.
fn keyed(mut v: Value, m: &Value) -> Value {
    if m["key"].is_string() || m["key"].is_number() {
        v["key"] = m["key"].clone();
    }
    v
}

/// `providers`, answered from a task: Arena may be asked whether they are allowed.
pub fn shared(sh: &Shared, m: &Value) -> bool {
    if kind(m) != "providers" {
        return false;
    }
    let (out, paths, env, models) = (sh.out.clone(), sh.paths.clone(), sh.settings.env().clone(), sh.models.clone());
    let (edition, arena, m) = (sh.edition, sh.arena.clone(), m.clone());
    let Ok(rt) = tokio::runtime::Handle::try_current() else { return true };
    rt.spawn(async move {
        let ok = allowed(edition, arena).await;
        match Connection::load(&paths.credentials_file(), &env) {
            Ok(conn) => send(&out, keyed(listed(&conn, &models, ok), &m)),
            Err(e) => crate::ide_ext::say(&out, &format!("The providers could not be read: {e}")),
        }
    });
    true
}

/// `provider_add` and `provider_remove`, between turns.
pub async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
    match kind(m) {
        "provider_add" => {
            let asked = Asked::of(m);
            let reply = match add(ctx, &asked).await {
                Ok(n) => json!({"type": "provider_added", "name": asked.name, "ok": true, "models": n}),
                Err(e) => {
                    ctx.sh.say(&format!("{} was not added: {e}", asked.name));
                    json!({"type": "provider_added", "name": asked.name, "ok": false, "error": e})
                }
            };
            send(&ctx.sh.out, keyed(reply, m));
            true
        }
        "provider_remove" => {
            let name = m["name"].as_str().unwrap_or("").to_string();
            let reply = match remove(ctx, &name).await {
                Ok(()) => json!({"type": "provider_removed", "name": name, "ok": true}),
                Err(e) => {
                    ctx.sh.say(&e);
                    json!({"type": "provider_removed", "name": name, "ok": false, "error": e})
                }
            };
            send(&ctx.sh.out, keyed(reply, m));
            true
        }
        _ => false,
    }
}

/// Lists its models with the key to check it, keeps it, and has the session use it: how many models it has.
async fn add(ctx: &mut Ctx<'_>, asked: &Asked) -> Result<usize, String> {
    let ok = allowed(ctx.sh.edition, ctx.sh.arena.clone()).await;
    if !ok {
        return Err(BANNED.to_string());
    }
    let file = ctx.app.paths.credentials_file();
    let mut conn = Connection::load(&file, &ca_settings::Env::default()).map_err(|e| e.to_string())?;
    let (mut entry, provider) = entry_for(asked, conn.providers.get(&asked.name))?;
    let (key, from_env) = key_for(asked, &provider, ctx.app.settings.env())?;
    let g = ca_llm::Gateway::standalone(vec![(provider, key.clone())], ctx.app.ca()).map_err(|e| e.to_string())?;
    let models = match tokio::time::timeout(Duration::from_secs(20), g.models()).await {
        Ok(Ok(ms)) => ms,
        Ok(Err(e)) => return Err(format!("it did not list its models with that key: {e}")),
        Err(_) => return Err("it did not list its models within 20 seconds".into()),
    };
    entry.api_key = if from_env || key.is_empty() { None } else { Some(key) };
    entry.key_from_env = false;
    conn.providers.insert(asked.name.clone(), entry);
    conn.save(&file).map_err(|e| e.to_string())?;
    let shown: Vec<&str> = models.iter().take(SHOWN_MODELS).map(|m| m.id.as_str()).collect();
    let more = if models.len() > shown.len() { ", …" } else { "" };
    refresh(ctx, ok).await;
    ctx.sh.say(&format!(
        "Kept {}: {} models ({}{more}), named {}:MODEL in the model picker.",
        asked.name,
        models.len(),
        shown.join(", "),
        asked.name
    ));
    Ok(models.len())
}

/// Forgets it and its key, and has the session no longer offer its models.
async fn remove(ctx: &mut Ctx<'_>, name: &str) -> Result<(), String> {
    let file = ctx.app.paths.credentials_file();
    let mut conn = Connection::load(&file, &ca_settings::Env::default()).map_err(|e| e.to_string())?;
    if conn.providers.remove(name).is_none() {
        return Err(match ctx.app.connection.providers.get(name) {
            Some(e) if e.from_env => format!(
                "{name} is there for its key in {}: unset it to remove {name}.",
                key_env(name).unwrap_or("the environment")
            ),
            _ => format!("No provider {name} is kept here."),
        });
    }
    conn.save(&file).map_err(|e| e.to_string())?;
    let ok = allowed(ctx.sh.edition, ctx.sh.arena.clone()).await;
    refresh(ctx, ok).await;
    ctx.sh.say(&format!("{name} and its key are forgotten here."));
    Ok(())
}

/// The providers read again, the session's gateway given them (unless Arena's admins keep them out), its models
/// asked again and the faces told: `settings_changed` and `providers`.
async fn refresh(ctx: &mut Ctx<'_>, allowed: bool) {
    match Connection::load(&ctx.app.paths.credentials_file(), ctx.app.settings.env()) {
        Ok(c) => ctx.app.connection = c,
        Err(e) => ctx.sh.say(&format!("The providers could not be read again: {e}")),
    }
    let own = if allowed { ctx.app.providers() } else { Vec::new() };
    ctx.p.agent.gateway.set_providers(own);
    if let Ok(Ok(models)) = tokio::time::timeout(Duration::from_secs(15), ctx.p.agent.gateway.models()).await {
        ctx.drv.models = models.iter().map(|m| m.id.clone()).collect();
        ctx.sh.kept.of::<crate::ide_chat::pickers::Infos>().set(models);
    }
    let mut now = ctx.sh.clone();
    now.models = Arc::new(ctx.drv.models.clone());
    let mut v = json!({});
    crate::ide_chat::pickers::state(&now, &mut v);
    send(&ctx.sh.out, json!({"type": "settings_changed", "models": now.models.as_slice(), "model_info": v["model_info"]}));
    send(&ctx.sh.out, listed(&ctx.app.connection, &ctx.drv.models, allowed));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: Option<&str>, from_env: bool) -> ProviderEntry {
        ProviderEntry { api_key: key.map(Secret::new), key_from_env: from_env, ..Default::default() }
    }

    #[test]
    fn a_row_says_where_the_key_comes_from_never_the_key() {
        let models: Vec<String> = ["anthropic:a", "anthropic:b", "anthropic:c", "anthropic:d", "anthropic:e", "x"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let here = item("anthropic", &entry(Some("fake-key-123"), false), &models);
        assert_eq!(here["key"], "here");
        assert_eq!(here["kind"], "anthropic");
        assert_eq!(here["url"], "https://api.anthropic.com");
        assert_eq!(here["models"], json!(["a", "b", "c", "d"]));
        assert_eq!(here["more"], 1);
        assert_eq!(here["key_env"], "ANTHROPIC_API_KEY");
        assert!(!here.to_string().contains("fake-key-123"));
        assert_eq!(item("openai", &entry(Some("fake"), true), &[])["key"], "env");
        let gpu = ProviderEntry {
            kind: Some("openai".into()),
            url: Some("http://gpu:8000".into()),
            models: vec!["qwen3".into()],
            ..Default::default()
        };
        let row = item("gpu", &gpu, &[]);
        assert_eq!((row["key"].as_str(), row["models"].clone()), (Some("none"), json!(["qwen3"])));
        assert!(row.get("key_env").is_none());
    }

    #[test]
    fn the_list_has_every_provider_and_the_known_ones_without_a_key() {
        let mut conn = Connection::default();
        conn.providers.insert("deepseek".into(), entry(Some("fake-secret"), false));
        let v = listed(&conn, &[], false);
        assert_eq!(v["allowed"], false);
        assert_eq!(v["items"].as_array().map(Vec::len), Some(1));
        let known: Vec<&str> = v["known"].as_array().into_iter().flatten().filter_map(|k| k["name"].as_str()).collect();
        assert_eq!(known, ["anthropic", "openai", "deepseek", "openrouter", "mistral"]);
        assert!(!v.to_string().contains("fake-secret"));
    }

    #[test]
    fn what_is_asked_must_be_a_provider() {
        let asked = |m: Value| Asked::of(&m);
        let bad = asked(json!({"name": "My API", "url": "https://x"}));
        assert!(entry_for(&bad, None).is_err());
        let other = asked(json!({"name": "gpu", "url": " https://gpu.example/ ", "kind": "anthropic", "api_key": " k "}));
        assert_eq!(other.key, "k");
        let (e, p) = entry_for(&other, None).expect("an API of one's own");
        assert_eq!((p.kind, e.url.as_deref()), (Kind::Anthropic, Some("https://gpu.example/")));
        assert!(entry_for(&asked(json!({"name": "nowhere"})), None).is_err(), "an unknown one needs its address");
        let (_, known) = entry_for(&asked(json!({"name": "mistral"})), None).expect("known by name");
        assert_eq!(known.url, "https://api.mistral.ai");
    }

    #[test]
    fn a_key_is_the_one_given_else_its_variables_else_none_only_nearby() {
        let env = ca_settings::Env::from_pairs([("OPENAI_API_KEY", "fake-env-key")]);
        let ask = |m: Value| {
            let a = Asked::of(&m);
            let (_, p) = entry_for(&a, None).expect("a provider");
            key_for(&a, &p, &env)
        };
        let given = ask(json!({"name": "openai", "api_key": "fake-typed"})).expect("given");
        assert_eq!((given.0.expose(), given.1), ("fake-typed", false));
        let from_env = ask(json!({"name": "openai"})).expect("from its variable");
        assert_eq!((from_env.0.expose(), from_env.1), ("fake-env-key", true));
        assert!(ask(json!({"name": "mistral"})).is_err_and(|e| e.contains("MISTRAL_API_KEY")));
        assert!(ask(json!({"name": "local", "url": "http://127.0.0.1:8000"})).is_ok_and(|k| k.0.is_empty()));
    }
}
