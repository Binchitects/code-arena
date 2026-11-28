//! The language servers: which are here, one started or stopped, and its messages between it and the window.
//!
//! The built-in servers are typescript-language-server, Pyright, basedpyright, the Python LSP Server, gopls,
//! rust-analyzer, clangd, csharp-ls, the JSON, CSS and HTML servers of VS Code, yaml-language-server and
//! bash-language-server; the `languageServers` setting adds the person's own and changes a built-in one
//! (`catalog`). `codeArena.languageServers.enabled` and `codeArena.languageServers.ID.enabled` turn them off.
//!
//! - `lsp_servers` [T] → `lsp_servers{items, trusted}`: every server, the setting's own first, each `{id, name,
//!   languages, found, path, version, reason, install, state, settings, client}`. `reason` says why it is not
//!   found or cannot run, `install` how to get it, `settings` what it is answered with (`workspace/configuration`).
//!   `state` is `running`, `stopped`, `failed` (it ended by itself and is not started again, or its last start was
//!   refused), `off`, `missing` or `restricted` (the folder is not trusted). `client` is the window it runs for.
//!   Versions are asked once for as long as the program is unchanged, never of a program in the folder.
//! - `lsp_start{server, client?}` [T] starts a server for a window, `lsp_restart{server, client?}` [T] stops it
//!   and starts it again (its count of ends forgotten), `lsp_stop{server, client?}` [T] stops it. Each is
//!   answered with `lsp_state{server, client, state, pid?, path?, sandboxed?, kind?, error?, install?}`; `kind` of
//!   a refusal is `not_trusted`, `missing`, `off`, `too_many` (six run at most) or `failed`. A start of a server
//!   that runs for the same window answers that it runs and starts nothing. One per server id runs: a start from
//!   another window (another `client`) takes it over, and the window that had it is told (`takenOver`).
//! - `lsp_send{server, client?, msg}` [T]: a JSON-RPC message for the server. Its file URIs are rewritten
//!   (`uris`), and an initialize is given the folder (`rootUri`, `rootPath`, `workspaceFolders`), the process id
//!   and the `initializationOptions` of the catalog and the setting. A request to a server that does not run (or
//!   runs for another window) is answered with an error (-32803).
//! - `lsp_msg{server, client, msg}`: a message of the server's, its URIs as the window's. The last of a run is the
//!   notification `codeArena/ended {reason, exitCode, takenOver, stopped, restart}`: `restart` when it ended by
//!   itself after it answered initialize, at most three times in three minutes; the window then starts it again
//!   (and initializes it again). Every end is also said as `lsp_state` (`stopped`, `failed` or `taken`).
//! - `lsp_log{server}` [T] → `lsp_log{server, lines, exit, refused, running}`: the last 200 lines of its
//!   standard error, of the one running else the last that ran; `refused` why its last start was refused.
//!
//! A server runs the folder's code (tsserver plugins, build scripts, go list), so none starts until the folder is
//! trusted, and all stop when the trust is taken back or another folder is opened. Each runs in the folder, with
//! none of the driver's keys in its environment, in the sandbox `tools.sandbox` allows (the folder and the build
//! tools' caches writable, the secrets hidden) and without the network: the catalog's servers are also told not
//! to fetch (no type packages, `GOPROXY=off`, `CARGO_NET_OFFLINE`, no schema store). A server stopping is sent
//! shutdown and exit; what is left of it two seconds later is killed.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver.

mod catalog;
mod relay;
mod uris;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use ca_settings::{Settings, schema};
use serde_json::{Value, json};

use crate::ide_ext::{Area, Rule, Shared, kind, send};
use relay::{Asked, Servers, Setup};

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("lsp_servers", Rule::Turn),
        ("lsp_start", Rule::Turn),
        ("lsp_stop", Rule::Turn),
        ("lsp_restart", Rule::Turn),
        ("lsp_send", Rule::Turn),
        ("lsp_log", Rule::Turn),
    ];

    fn shared(sh: &Shared, m: &Value) -> bool {
        let k = kind(m);
        if !k.starts_with("lsp_") {
            return false;
        }
        let servers = sh.kept.of::<Servers>();
        let id = m["server"].as_str().unwrap_or("");
        let client = m["client"].as_str().map(String::from);
        if k != "lsp_servers" && !named(id) {
            let error = format!("There is no language server \"{id}\".");
            let reply = if k == "lsp_send" || k == "lsp_log" { k } else { "lsp_state" };
            send(
                &sh.out,
                json!({"type": reply, "server": id, "client": client, "state": "missing", "kind": "missing", "error": error}),
            );
            return true;
        }
        match k {
            "lsp_servers" => {
                let sh = sh.clone();
                tokio::task::spawn_blocking(move || send(&sh.out, listing(&sh, &servers)));
            }
            "lsp_log" => {
                let (sh, id) = (sh.clone(), id.to_string());
                tokio::task::spawn_blocking(move || {
                    let known = catalog::entries(&setting(&current(&sh))).iter().any(|e| e.id == id);
                    let mut v = servers.log(&id);
                    if !known {
                        v["error"] = json!(format!("There is no language server \"{id}\"."));
                    }
                    v["type"] = json!("lsp_log");
                    v["server"] = json!(id);
                    send(&sh.out, v);
                });
            }
            "lsp_start" => servers.start(id, client, setup(sh), &sh.out),
            "lsp_restart" => servers.restart(id, client, setup(sh), &sh.out),
            "lsp_stop" => servers.stop(id, client, &sh.out),
            "lsp_send" => servers.relay(id, client, m["msg"].clone(), &sh.out),
            _ => return false,
        }
        true
    }

    /// The servers stop when the folder's trust is taken back, and those of another folder when one is opened.
    fn start(sh: &Shared) {
        crate::ide_trust::on_revoke(&sh.kept, "lsp", |sh: &Shared| {
            sh.kept.of::<Servers>().stop_all("The folder is no longer trusted: its language servers stopped.", None);
        });
        sh.kept.of::<Servers>().stop_all("Another folder was opened.", Some(sh.workspace.root()));
    }
}

/// A server id as the catalog and the setting name them.
fn named(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn current(sh: &Shared) -> Arc<Settings> {
    crate::ide_settings::current(sh)
}

/// The `languageServers` setting.
fn setting(s: &Settings) -> Value {
    s.ide("languageServers", None)
}

/// The ids the `codeArena.languageServers.*` settings turn off.
fn off(s: &Settings, setting: &Value) -> Vec<String> {
    let all = s.ide("codeArena.languageServers.enabled", None).as_bool() == Some(false);
    catalog::entries(setting)
        .into_iter()
        .filter(|e| all || s.ide(&format!("codeArena.languageServers.{}.enabled", e.id), None).as_bool() == Some(false))
        .map(|e| e.id)
        .collect()
}

/// The environment's names a server is given: none of the driver's keys.
fn passed() -> Vec<(OsString, OsString)> {
    const MORE: &[&str] = &["GOBIN", "GOMODCACHE", "GOFLAGS", "PYTHONPATH", "JAVA_HOME", "NVM_DIR"];
    let mut env = ca_tools::formatters::passed_env(std::env::vars_os());
    env.extend(std::env::vars_os().filter(|(k, _)| k.to_str().is_some_and(|k| MORE.contains(&k))));
    env
}

/// The folder the servers are for, and how a start asks what it needs off the loop.
fn setup(sh: &Shared) -> Setup {
    let asking = sh.clone();
    Setup {
        root: sh.workspace.root().to_path_buf(),
        data: sh.paths.data.clone(),
        home: dirs::home_dir(),
        env: passed(),
        ask: Arc::new(move || asked(&asking)),
    }
}

fn asked(sh: &Shared) -> Asked {
    let settings = current(sh);
    let setting = setting(&settings);
    Asked {
        off: off(&settings, &setting),
        setting,
        trusted: crate::ide_trust::trusted(&sh.paths.data, sh.workspace.root()),
        sandbox: sandbox(sh, &settings),
    }
}

/// The sandbox the agent's commands run in, without the network.
fn sandbox(sh: &Shared, settings: &Settings) -> Result<Option<ca_tools::sandbox::Sandbox>, String> {
    let mode = settings.string(&schema::SANDBOX).unwrap_or_else(|| "auto".into());
    if mode == "off" {
        return Ok(None);
    }
    match ca_tools::sandbox::find() {
        Ok(kind) => {
            let extra: Vec<PathBuf> =
                settings.strings(&schema::SANDBOX_WRITABLE).into_iter().map(PathBuf::from).collect();
            let own = [sh.paths.config.clone(), sh.paths.data.clone()];
            let runtime = settings.env().get("XDG_RUNTIME_DIR").map(PathBuf::from);
            Ok(Some(ca_tools::sandbox::Sandbox::new(
                kind,
                false,
                sh.workspace.allowed(),
                &extra,
                dirs::home_dir().as_deref(),
                &own,
                runtime.as_deref(),
            )))
        }
        Err(why) if mode == "on" => Err(format!("tools.sandbox is on, but {why}.")),
        Err(_) => Ok(None),
    }
}

/// The `lsp_servers` reply.
fn listing(sh: &Shared, servers: &Servers) -> Value {
    let settings = current(sh);
    let setting = setting(&settings);
    let off = off(&settings, &setting);
    let trusted = crate::ide_trust::trusted(&sh.paths.data, sh.workspace.root());
    let setup = setup(sh);
    let items = list(servers, &setup, &setting, &off, trusted);
    json!({"type": "lsp_servers", "items": items, "trusted": trusted})
}

/// Every server as the window lists them, their versions asked at once.
fn list(servers: &Servers, setup: &Setup, setting: &Value, off: &[String], trusted: bool) -> Vec<Value> {
    let entries = catalog::entries(setting);
    let look = catalog::Look { root: &setup.root, home: setup.home.as_deref(), env: &setup.env };
    std::thread::scope(|s| {
        let asked: Vec<_> = entries
            .iter()
            .map(|e| {
                let look = &look;
                s.spawn(move || {
                    let found = catalog::find(e, look).and_then(|f| {
                        servers.versions.of(&f, e.kind, &setup.root, &setup.data, &setup.env).map(|v| (f, v))
                    });
                    item(e, found, servers.seen(&e.id), off.contains(&e.id), trusted)
                })
            })
            .collect();
        asked.into_iter().filter_map(|h| h.join().ok()).collect()
    })
}

/// One server as the window lists it.
fn item(
    e: &catalog::Entry,
    found: Result<(catalog::Found, Option<String>), String>,
    seen: relay::Seen,
    off: bool,
    trusted: bool,
) -> Value {
    let state = match (&found, seen.state) {
        (_, "running") => "running",
        _ if off => "off",
        (Err(_), _) => "missing",
        _ if !trusted => "restricted",
        (_, state) => state,
    };
    let disabled = e.own.as_ref().is_some_and(|o| o.disabled);
    let (path, version, settings, reason) = match &found {
        Ok((f, v)) => {
            (Some(f.program.display().to_string()), v.clone(), Value::Object(f.settings.clone()), seen.failed.clone())
        }
        Err(why) => (None, None, Value::Null, Some(why.clone())),
    };
    json!({
        "id": e.id,
        "name": e.name,
        "languages": e.languages,
        "found": found.is_ok(),
        "path": path,
        "version": version,
        "reason": reason,
        "install": (found.is_err() && !disabled).then(|| catalog::install(&e.id)),
        "state": state,
        "settings": settings,
        "client": seen.client,
    })
}

#[cfg(test)]
mod tests;
