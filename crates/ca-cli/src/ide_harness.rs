//! The harness as the IDE shows it: the hooks, the permission rules, the sandbox, the plugins, the output
//! styles and doctor, as `/hooks`, `/permissions`, `/sandbox`, `/output-style`, `code-arena plugin` and
//! `code-arena doctor` say them in the terminal.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn).
//!
//! Messages (each reply carries the `key` it answers, when one was given):
//! - `hooks{key?}` [T]: `hooks{items: [{event, command, from}], trusted, waiting: [{event, command, from}]}`. `items`
//!   are the hooks that run; `trusted` is whether the project's own hooks run (null: the project has none);
//!   `waiting` are the project's hooks that do not run until trusted.
//! - `hooks_trust` [Q]: the project's hooks trusted as they are, and the session prepared again so that they run
//!   from the next message: a `notice{text, of, error?}`, then `hooks` again.
//! - `permissions{key?}` [T]: `permissions{rules: [{rule, allow}]}`, the allowed first.
//! - `permission_set{rule, allow}` [Q]: the rule allowed (true) or refused (false) for this session, or taken away
//!   (null): a `notice`, then `permissions` again.
//! - `sandbox{key?}` [T]: `sandbox{on, text: [lines]}`.
//! - `plugins{key?}` [T]: `plugins{items: [{name, version, description, parts, on, yours}]}`. `on`: enabled in the
//!   settings in force; `yours`: in your own `plugins.enabled`.
//! - `plugin_set{name, on}`, `plugin_install{source}` (a folder or a git URL), `plugin_remove{name}` [T]: a
//!   `notice`, then `plugins` again. The work runs off the driver's loop (a clone may take a while).
//! - `output_styles{key?}` [T]: `output_styles{items: [{name, description, from}], current}`.
//! - `output_style_set{name}` [R]: kept where the style in force was given, and the session prepared again: a
//!   `notice`, then `output_styles` again.
//! - `doctor{key?}` [Q]: `doctor{text}`, what `code-arena doctor` says (asked of the network off the loop).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ide_ext::{Area, Ctx, Rule, Shared, kind, send};
use crate::session::Resume;

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("hooks", Rule::Turn),
        ("hooks_trust", Rule::Queue),
        ("permissions", Rule::Turn),
        ("permission_set", Rule::Queue),
        ("sandbox", Rule::Turn),
        ("plugins", Rule::Turn),
        ("plugin_set", Rule::Turn),
        ("plugin_install", Rule::Turn),
        ("plugin_remove", Rule::Turn),
        ("output_styles", Rule::Turn),
        ("output_style_set", Rule::Refuse("change the output style")),
        ("doctor", Rule::Queue),
    ];

    fn shared(sh: &Shared, m: &Value) -> bool {
        let key = &m["key"];
        match kind(m) {
            "hooks" => {
                send(&sh.out, keyed(hooks_reply(&sh.was.hooks, &sh.paths.data, sh.workspace.root(), &agents(sh)), key))
            }
            "permissions" => send(&sh.out, keyed(permissions_reply(&sh.was.rules), key)),
            "sandbox" => send(&sh.out, keyed(sandbox_reply(sh.was.sandbox.as_deref()), key)),
            "output_styles" => {
                let current =
                    sh.settings.string(&ca_settings::schema::OUTPUT_STYLE).unwrap_or_else(|| "default".into());
                send(&sh.out, keyed(styles_reply(&sh.sources, &current), key));
            }
            "plugins" => {
                let (sh, key) = (sh.clone(), key.clone());
                tokio::task::spawn_blocking(move || send(&sh.out, keyed(plugins_reply(&sh), &key)));
            }
            "plugin_set" | "plugin_install" | "plugin_remove" => {
                let (sh, m) = (sh.clone(), m.clone());
                tokio::task::spawn_blocking(move || {
                    let of = kind(&m).to_string();
                    send(&sh.out, notice(&of, plugin_work(&sh, &m)));
                    send(&sh.out, plugins_reply(&sh));
                });
            }
            _ => return false,
        }
        true
    }

    async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
        match kind(m) {
            "hooks_trust" => {
                let said = trust_hooks(ctx).await;
                send(&ctx.sh.out, notice("hooks_trust", said));
                let hooks: Vec<_> = ctx
                    .p
                    .agent
                    .hooks
                    .list
                    .iter()
                    .map(|h| (h.event.clone(), h.command.clone(), h.from.clone()))
                    .collect();
                let root = ctx.p.agent.ctx.workspace.root().to_path_buf();
                send(&ctx.sh.out, hooks_reply(&hooks, &ctx.app.paths.data, &root, &ctx.app.agents()));
            }
            "permission_set" => {
                let rule = m["rule"].as_str().unwrap_or("").trim();
                let said = set_rule(&mut ctx.p.agent.policy.rules, rule, m["allow"].as_bool());
                send(&ctx.sh.out, notice("permission_set", said));
                send(&ctx.sh.out, permissions_reply(&ctx.p.agent.policy.rules.written()));
            }
            "output_style_set" => {
                let name = m["name"].as_str().unwrap_or("").trim().to_string();
                let said = set_style(ctx, &name).await;
                let current =
                    ctx.app.settings.string(&ca_settings::schema::OUTPUT_STYLE).unwrap_or_else(|| "default".into());
                send(&ctx.sh.out, notice("output_style_set", said));
                send(&ctx.sh.out, styles_reply(&ctx.p.sources, &current));
            }
            "doctor" => {
                let (app, out, key) = (ctx.app.clone(), ctx.sh.out.clone(), m["key"].clone());
                tokio::spawn(async move {
                    let mut lines = Vec::new();
                    let ui = crate::ui::Ui::new(false, true);
                    if let Err(e) = crate::account::doctor_to(&app, ui, &mut |l| lines.push(l)).await {
                        lines.push(format!("\n{e}"));
                    }
                    send(&out, keyed(json!({"type": "doctor", "text": lines.join("\n")}), &key));
                });
            }
            _ => return false,
        }
        true
    }
}

/// The other agents whose files are read too, as the settings name them.
fn agents(sh: &Shared) -> ca_core::compat::Agents {
    ca_core::compat::Agents::new(&sh.settings.strings(&ca_settings::schema::COMPAT_AGENTS))
}

/// A reply with the key of the request it answers.
fn keyed(mut v: Value, key: &Value) -> Value {
    if !key.is_null() {
        v["key"] = key.clone();
    }
    v
}

/// The notice that answers a request: what was done, or why not.
fn notice(of: &str, said: Result<String, String>) -> Value {
    match said {
        Ok(text) => json!({"type": "notice", "text": text, "of": of}),
        Err(text) => json!({"type": "notice", "text": text, "of": of, "error": true}),
    }
}

/// A hook as the replies say it.
fn hook(event: &str, command: &str, from: &str) -> Value {
    json!({"event": event, "command": command, "from": from})
}

/// The `hooks` reply: the hooks that run, and the project's own while they are not trusted.
fn hooks_reply(
    running: &[(String, String, String)],
    data: &Path,
    root: &Path,
    agents: &ca_core::compat::Agents,
) -> Value {
    let items: Vec<Value> = running.iter().map(|(e, c, f)| hook(e, c, f)).collect();
    let project = crate::session::project_hooks(root, agents);
    let has = project.as_object().is_some_and(|o| !o.is_empty());
    let trusted = has && ca_agent::hooks::trusted(data, root, &project);
    let waiting: Vec<Value> = if has && !trusted { waiting_hooks(&project) } else { Vec::new() };
    json!({"type": "hooks", "items": items, "trusted": if has { json!(trusted) } else { Value::Null }, "waiting": waiting})
}

/// The project's hooks, file by file, as rows.
fn waiting_hooks(project: &Value) -> Vec<Value> {
    project
        .as_object()
        .into_iter()
        .flatten()
        .flat_map(|(file, v)| {
            let (list, _) = ca_agent::hooks::parse(v, &format!("the project's {file}"));
            list.into_iter().map(|h| hook(&h.event, &h.command, &h.from)).collect::<Vec<_>>()
        })
        .collect()
}

/// The `permissions` reply.
fn permissions_reply((allow, deny): &(Vec<String>, Vec<String>)) -> Value {
    let rules: Vec<Value> = allow
        .iter()
        .map(|r| json!({"rule": r, "allow": true}))
        .chain(deny.iter().map(|r| json!({"rule": r, "allow": false})))
        .collect();
    json!({"type": "permissions", "rules": rules})
}

/// A rule allowed, refused or taken away, for this session: what is said of it.
fn set_rule(rules: &mut ca_agent::policy::Rules, rule: &str, allow: Option<bool>) -> Result<String, String> {
    if ca_agent::policy::Rule::parse(rule).is_none() {
        return Err(format!(
            "\u{201c}{rule}\u{201d} is not a rule: a tool's name, with a pattern if you like, as run_shell(npm test*) or read_file(.env*)."
        ));
    }
    let had = rules.forget(rule);
    match allow {
        Some(true) => {
            rules.allow(rule);
            Ok(format!("Allowed for this session: {rule} (add it to permissions.allow in settings.json to keep it)."))
        }
        Some(false) => {
            rules.deny(rule);
            Ok(format!("Refused for this session: {rule} (add it to permissions.deny in settings.json to keep it)."))
        }
        None if had => Ok(format!("{rule} is no longer a rule in this session.")),
        None => Err(format!("{rule} is not one of the rules.")),
    }
}

/// The `sandbox` reply.
fn sandbox_reply(lines: Option<&[String]>) -> Value {
    match lines {
        Some(l) => json!({"type": "sandbox", "on": true, "text": l}),
        None => {
            let why = ca_tools::sandbox::find().err().unwrap_or_else(|| "tools.sandbox is off".into());
            json!({"type": "sandbox", "on": false, "text": [
                format!("No sandbox: commands run as you, and the mode is what guards them ({why})."),
            ]})
        }
    }
}

/// The `output_styles` reply.
fn styles_reply(sources: &[ca_agent::library::Source], current: &str) -> Value {
    let items: Vec<Value> = ca_agent::output_style::all(sources)
        .into_iter()
        .map(|s| json!({"name": s.name, "description": s.description, "from": s.from}))
        .collect();
    json!({"type": "output_styles", "items": items, "current": current})
}

/// The project's settings file, and the person's.
fn files(sh_paths: &ca_core::Paths, root: &Path) -> (PathBuf, PathBuf) {
    (sh_paths.settings_file(), root.join(".code-arena").join("settings.json"))
}

/// Your own `plugins.enabled`, read now (a project's list is its own).
fn yours(user: &Path) -> Vec<String> {
    ca_settings::settings::Settings::load(user, None, Default::default()).strings(&ca_settings::schema::PLUGINS)
}

/// The `plugins` reply, read from the plugins' folders and the settings files now.
fn plugins_reply(sh: &Shared) -> Value {
    let (user, project) = files(&sh.paths, sh.workspace.root());
    let now = sh.settings.again(&user, Some(&project)).strings(&ca_settings::schema::PLUGINS);
    let mine = yours(&user);
    let items: Vec<Value> = crate::plugins::installed(&sh.paths.data, &agents(sh))
        .into_iter()
        .map(|p| {
            json!({
                "name": p.name,
                "version": p.version,
                "description": p.description,
                "parts": crate::plugins::parts(&p.root),
                "on": now.contains(&p.name),
                "yours": mine.contains(&p.name),
            })
        })
        .collect();
    json!({"type": "plugins", "items": items})
}

/// A plugin turned on or off, installed or removed: what is said of it.
fn plugin_work(sh: &Shared, m: &Value) -> Result<String, String> {
    let (user, _) = files(&sh.paths, sh.workspace.root());
    let data = &sh.paths.data;
    let keep = |names: Vec<String>| {
        ca_settings::settings::keep(&user, ca_settings::schema::PLUGINS.key, &json!(names)).map_err(|e| e.to_string())
    };
    let name = m["name"].as_str().unwrap_or("").trim();
    match kind(m) {
        "plugin_set" => {
            let on = m["on"].as_bool().unwrap_or(false);
            keep(crate::plugins::turned(data, name, on, &yours(&user)).map_err(|e| e.to_string())?)?;
            Ok(if on {
                format!("{name} is enabled for you: a new session uses it.")
            } else {
                format!("{name} is disabled for you: a new session goes without it.")
            })
        }
        "plugin_remove" => {
            keep(crate::plugins::remove(data, name, &yours(&user)).map_err(|e| e.to_string())?)?;
            Ok(format!("Removed {name}."))
        }
        _ => {
            let source = m["source"].as_str().unwrap_or("").trim();
            if source.is_empty() {
                return Err("Say where the plugin is: a folder or a git URL.".into());
            }
            let source = match (source.strip_prefix("~/"), &sh.home) {
                (Some(rest), Some(h)) => h.join(rest).to_string_lossy().into_owned(),
                _ => source.to_string(),
            };
            let (p, parts) = crate::plugins::install(data, &source, &agents(sh)).map_err(|e| format!("{e:#}"))?;
            Ok(format!(
                "Installed {} ({}). Enable it to use it.",
                p.name,
                if parts.is_empty() { "nothing in it Code Arena reads".to_string() } else { parts.join(", ") }
            ))
        }
    }
}

/// The session prepared again as it is (its rules for this session kept), so that what was changed is in force
/// from the next message.
async fn again(ctx: &mut Ctx<'_>) -> Result<(), String> {
    let (rules, thinking) = (ctx.p.agent.policy.rules.clone(), ctx.p.agent.thinking.clone());
    let id = ctx.p.session.clone();
    let stored = ctx.p.store.lock().ok().and_then(|s| s.summary(&id).ok()).is_some();
    crate::ide_sessions::switch_session(ctx, if stored { Resume::Id(id) } else { Resume::New }).await?;
    ctx.p.agent.policy.rules = rules;
    ctx.p.agent.thinking = thinking;
    Ok(())
}

/// The settings read again from their files, as one was just written.
fn reread(ctx: &mut Ctx<'_>) {
    let (user, project) = files(&ctx.app.paths, ctx.p.agent.ctx.workspace.root());
    ctx.app.settings = ctx.app.settings.again(&user, Some(&project));
}

/// Trusts the project's hooks as they are, and prepares the session again so that they run.
async fn trust_hooks(ctx: &mut Ctx<'_>) -> Result<String, String> {
    let root = ctx.p.agent.ctx.workspace.root().to_path_buf();
    let project = crate::session::project_hooks(&root, &ctx.app.agents());
    if project.as_object().is_none_or(|o| o.is_empty()) {
        return Err("This project has no hooks.".into());
    }
    ca_agent::hooks::trust(&ctx.app.paths.data, &root, &project).map_err(|e| e.to_string())?;
    again(ctx).await?;
    Ok("The project's hooks run from the next message; a change to them asks again.".into())
}

/// Uses an output style from the next message: kept where the style in force was given (the project's settings
/// when they name one, else yours).
async fn set_style(ctx: &mut Ctx<'_>, name: &str) -> Result<String, String> {
    let Some(style) = ca_agent::output_style::find(&ctx.p.sources, name) else {
        return Err(format!("There is no output style {name}."));
    };
    let file = match ctx.app.settings.resolve(&ca_settings::schema::OUTPUT_STYLE).1 {
        ca_settings::settings::Source::Project(f) => f,
        _ => ctx.app.paths.settings_file(),
    };
    ca_settings::settings::keep(&file, ca_settings::schema::OUTPUT_STYLE.key, &json!(style.name))
        .map_err(|e| e.to_string())?;
    reread(ctx);
    again(ctx).await?;
    Ok(format!("The output style is {} from the next message.", style.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_is_allowed_refused_and_taken_away_for_the_session() {
        let mut rules = ca_agent::policy::Rules::default();
        assert!(set_rule(&mut rules, "run_shell(npm test*)", Some(true)).is_ok());
        assert_eq!(rules.written(), (vec!["run_shell(npm test*)".to_string()], vec![]));
        // Refusing an allowed rule moves it: a rule is in one list only.
        assert!(set_rule(&mut rules, "run_shell(npm test*)", Some(false)).expect("made").starts_with("Refused"));
        assert_eq!(rules.written(), (vec![], vec!["run_shell(npm test*)".to_string()]));
        assert!(set_rule(&mut rules, "run_shell(npm test*)", None).is_ok());
        assert_eq!(rules.written(), (vec![], vec![]));
        assert!(set_rule(&mut rules, "run_shell(npm test*)", None).is_err());
        assert!(set_rule(&mut rules, "", Some(true)).is_err());
    }

    #[test]
    fn the_rules_are_said_allowed_first() {
        let v = permissions_reply(&(vec!["read_file".into()], vec!["run_shell(rm*)".into()]));
        assert_eq!(v["type"], "permissions");
        assert_eq!(v["rules"][0], json!({"rule": "read_file", "allow": true}));
        assert_eq!(v["rules"][1], json!({"rule": "run_shell(rm*)", "allow": false}));
    }

    #[test]
    fn the_project_s_hooks_wait_until_trusted_and_run_once_they_are() {
        let d = tempfile::tempdir().expect("made");
        let (data, root) = (d.path().join("data"), d.path().join("project"));
        std::fs::create_dir_all(root.join(".code-arena")).expect("made");
        let agents = ca_core::compat::Agents::new(&[]);
        let none = hooks_reply(&[], &data, &root, &agents);
        assert_eq!((none["trusted"].clone(), none["waiting"].clone()), (Value::Null, json!([])));
        std::fs::write(
            root.join(".code-arena/settings.json"),
            r#"{"hooks": {"PostToolUse": [{"matcher": "edit_file", "hooks": [{"type": "command", "command": "cargo fmt"}]}]}}"#,
        )
        .expect("made");
        let running = [("SessionStart".to_string(), "echo hi".to_string(), "your settings.json".to_string())];
        let v = hooks_reply(&running, &data, &root, &agents);
        assert_eq!(v["items"][0], json!({"event": "SessionStart", "command": "echo hi", "from": "your settings.json"}));
        assert_eq!(v["trusted"], false);
        assert_eq!(v["waiting"][0]["command"], "cargo fmt");
        assert_eq!(v["waiting"][0]["event"], "PostToolUse");
        let project = crate::session::project_hooks(&root, &agents);
        ca_agent::hooks::trust(&data, &root, &project).expect("made");
        let v = hooks_reply(&[], &data, &root, &agents);
        assert_eq!((v["trusted"].clone(), v["waiting"].clone()), (json!(true), json!([])));
    }

    #[test]
    fn the_sandbox_says_what_it_lets_commands_do_or_that_there_is_none() {
        let v = sandbox_reply(Some(&["They cannot reach the network.".to_string()]));
        assert_eq!((v["on"].clone(), v["text"][0].clone()), (json!(true), json!("They cannot reach the network.")));
        let v = sandbox_reply(None);
        assert_eq!(v["on"], false);
        assert!(v["text"][0].as_str().expect("made").starts_with("No sandbox:"));
    }

    #[test]
    fn a_plugin_is_installed_from_a_folder_turned_on_and_off_and_removed() {
        let d = tempfile::tempdir().expect("made");
        let (data, src) = (d.path().join("data"), d.path().join("mine"));
        std::fs::create_dir_all(src.join(".code-arena-plugin")).expect("made");
        std::fs::create_dir_all(src.join("commands")).expect("made");
        std::fs::write(src.join(".code-arena-plugin/plugin.json"), r#"{"name": "mine", "version": "1.0"}"#)
            .expect("made");
        let agents = ca_core::compat::Agents::new(&[]);
        let (p, parts) = crate::plugins::install(&data, &src.to_string_lossy(), &agents).expect("made");
        assert_eq!((p.name.as_str(), parts), ("mine", vec!["commands"]));
        assert!(crate::plugins::turned(&data, "other", true, &[]).is_err());
        let on = crate::plugins::turned(&data, "mine", true, &["a".into()]).expect("made");
        assert_eq!(on, ["a", "mine"]);
        assert_eq!(crate::plugins::turned(&data, "mine", false, &on).expect("made"), ["a"]);
        // A name that is not a folder's is never taken as a path.
        assert!(crate::plugins::remove(&data, "../data", &on).is_err());
        assert_eq!(crate::plugins::remove(&data, "mine", &on).expect("made"), ["a"]);
        assert!(crate::plugins::installed(&data, &agents).is_empty());
    }

    #[test]
    fn a_reply_carries_the_key_it_answers() {
        assert_eq!(keyed(json!({"type": "hooks"}), &json!(7))["key"], 7);
        assert!(keyed(json!({"type": "hooks"}), &Value::Null).get("key").is_none());
    }
}
