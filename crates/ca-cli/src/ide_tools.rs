//! The tools the model has: a group turned on or off, each tool's own settings, the MCP servers and how the
//! model calls them.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn).
//!
//! - `tools{key?}` [T] → `tools{key, groups:[{id, title, description, on, locked, ask_first, functions, loaded}]}`:
//!   the session's tools by group, as `/tools` lists them: Code Arena's own (always on), the Arena project's, and
//!   each MCP server's. A server that connected since the last turn is listed with `loaded: false` (its tools
//!   join with the next message). Answered from what the session was when it last stood still.
//! - `tools_set{group, on, key?}` [Q] → `tools{key: group, group, ...}`, or `refused{of, key: group, reason}` for a
//!   group that is always on or not there. A group turned off stays off for the session, its tools that join later
//!   too, as `/tools off NAME` does.
//! - `servers{key?}` [T] → `servers{key, items:[{name, title, url, state, tools, error, next_try}]}`: each MCP
//!   server, `state` one of connecting, connected, unavailable, failed; `next_try` in ms since 1970 while it is
//!   tried again by itself. Said again by itself whenever a server's state changes.
//! - `servers_retry{name?}` [T] → `servers{...}`: tries one server again now (none: every one not connected).
//!   Answered at once, a turn running or not: it only wakes the server's connecting.
//!
//! A server's `url` is shown without its user, password and query, and a server run as a program shows its command
//! only: what may hold a secret is never said.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use ca_tools::Kind;
use serde_json::{Value, json};

use crate::ide_ext::{Area, Ctx, Rule, Shared, TurnEnd, kind, refuse, send};
use crate::servers::{Servers, State};
use crate::session::Prepared;

/// The group of Code Arena's own tools: always on.
const OWN: &str = "Code Arena's own";

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] =
        &[("tools", Rule::Turn), ("tools_set", Rule::Queue), ("servers", Rule::Turn), ("servers_retry", Rule::Turn)];

    fn start(sh: &Shared) {
        watch(sh);
    }

    fn shared(sh: &Shared, m: &Value) -> bool {
        match kind(m) {
            "tools" => send(&sh.out, tools_message(sh, &m["key"])),
            "servers" => send(&sh.out, servers_message(&sh.servers, &m["key"])),
            "servers_retry" => {
                let name = m["name"].as_str();
                for s in sh.servers.iter().filter(|s| name.is_none_or(|n| n == s.name)) {
                    s.retry();
                }
                send(&sh.out, servers_message(&sh.servers, &m["key"]));
            }
            _ => return false,
        }
        true
    }

    async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
        if kind(m) != "tools_set" {
            return false;
        }
        let group = m["group"].as_str().unwrap_or("").to_string();
        let on = m["on"].as_bool().unwrap_or(true);
        let asked = json!({"type": "tools_set", "key": group});
        ctx.p.agent.take_joined();
        let rows = rows_of(ctx.p);
        let had = rows.iter().any(|r| r.group == group) || ctx.sh.servers.iter().any(|s| s.name == group);
        if !had {
            refuse(&ctx.sh.out, &asked, &format!("No tool group {group}."));
            return true;
        }
        if group == OWN {
            refuse(&ctx.sh.out, &asked, &format!("{OWN} tools are always on."));
            return true;
        }
        let off = ctx.sh.kept.of::<Off>();
        off.set(&ctx.sh.session, &group, on);
        if on {
            let (back, still): (Vec<_>, Vec<_>) = ctx.p.disabled.drain(..).partition(|t| t.group() == group);
            ctx.p.disabled = still;
            ctx.p.agent.tools.extend(back);
        } else {
            apply_off(ctx.p, &off.of(&ctx.sh.session));
        }
        keep(ctx.sh, ctx.p);
        let mut v = tools_message(ctx.sh, &json!(group));
        v["group"] = json!(group);
        send(&ctx.sh.out, v);
        true
    }

    async fn before_turn(ctx: &mut Ctx<'_>, _message: &str) {
        // What joined since is taken in first, so that a group turned off keeps its late tools off too.
        ctx.p.agent.take_joined();
        apply_off(ctx.p, &ctx.sh.kept.of::<Off>().of(&ctx.sh.session));
        keep(ctx.sh, ctx.p);
    }

    async fn after_turn(ctx: &mut Ctx<'_>, _end: &TurnEnd) {
        keep(ctx.sh, ctx.p);
    }

    fn state(sh: &Shared, p: Option<&Prepared>, _v: &mut Value) {
        if let Some(p) = p {
            keep(sh, p);
        }
    }

    fn status(sh: &Shared, p: Option<&Prepared>, _v: &mut Value) {
        if let Some(p) = p {
            keep(sh, p);
        }
    }
}

/// Where one tool is: sent whole, listed and loaded when needed, or turned off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Where {
    On,
    Listed,
    Off,
}

/// One tool as the groups are made of it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    group: String,
    name: String,
    at: Where,
    /// It reads only (it never asks).
    reads: bool,
}

/// The session's tools, each with its group.
fn rows_of(p: &Prepared) -> Vec<Row> {
    let row = |t: &std::sync::Arc<dyn ca_tools::Tool>, at| Row {
        group: t.group(),
        name: t.def().name,
        at,
        reads: t.kind() == Kind::Read,
    };
    let mut rows: Vec<Row> = p.agent.tools.iter().map(|t| row(t, Where::On)).collect();
    rows.extend(p.agent.deferred.iter().map(|t| row(t, Where::Listed)));
    rows.extend(p.disabled.iter().map(|t| row(t, Where::Off)));
    rows
}

/// The rows as the session last stood still, kept for `tools` while a turn holds it.
#[derive(Default)]
struct Kept(Mutex<(String, Vec<Row>)>);

fn keep(sh: &Shared, p: &Prepared) {
    let rows = rows_of(p);
    if let Ok(mut g) = sh.kept.of::<Kept>().0.lock() {
        *g = (sh.session.clone(), rows);
    }
}

/// The groups the person turned off, for the session they were turned off in.
#[derive(Default)]
struct Off(Mutex<(String, BTreeSet<String>)>);

impl Off {
    fn set(&self, session: &str, group: &str, on: bool) {
        if let Ok(mut g) = self.0.lock() {
            if g.0 != session {
                *g = (session.to_string(), BTreeSet::new());
            }
            if on {
                g.1.remove(group);
            } else {
                g.1.insert(group.to_string());
            }
        }
    }

    fn of(&self, session: &str) -> BTreeSet<String> {
        self.0.lock().ok().filter(|g| g.0 == session).map(|g| g.1.clone()).unwrap_or_default()
    }
}

/// Moves the tools of the groups turned off out of what the model is sent.
fn apply_off(p: &mut Prepared, off: &BTreeSet<String>) {
    if off.is_empty() {
        return;
    }
    let (gone, kept): (Vec<_>, Vec<_>) = p.agent.tools.drain(..).partition(|t| off.contains(&t.group()));
    p.agent.tools = kept;
    let (gone2, kept2): (Vec<_>, Vec<_>) = p.agent.deferred.drain(..).partition(|t| off.contains(&t.group()));
    p.agent.deferred = kept2;
    p.disabled.extend(gone.into_iter().chain(gone2));
}

/// A server as the groups need it: its name, title, whether it is trusted, and its tools if connected.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ServerRow {
    name: String,
    title: String,
    trusted: bool,
    connected: Option<usize>,
}

/// What a group is for, in a line.
fn about(group: &str, server: Option<&ServerRow>) -> String {
    match (group, server) {
        (OWN, _) => "Reads, searches and edits the folder's files, runs commands, git and the plan.".into(),
        ("Arena's project", _) => "Reads and writes the files of the Arena project this session is in.".into(),
        ("arena", _) => "Arena's own tools, served to Code Arena: what Arena's chat can call.".into(),
        ("code-intel", _) => "Farzan Graph's code tools: what calls what, and where a name is used.".into(),
        (g, _) => format!("The tools of the MCP server {g}."),
    }
}

/// The groups, made of the tools and the servers: Code Arena's own first, then by name; a connected server whose
/// tools have not joined yet is a group not loaded.
fn groups(rows: &[Row], servers: &[ServerRow], off: &BTreeSet<String>) -> Vec<Value> {
    let mut by: BTreeMap<String, Vec<&Row>> = BTreeMap::new();
    for r in rows {
        by.entry(r.group.clone()).or_default().push(r);
    }
    let mut out = Vec::new();
    let mut order: Vec<&String> = by.keys().collect();
    order.sort_by_key(|g| (g.as_str() != OWN, g.to_lowercase()));
    for g in order {
        let tools = &by[g];
        let server = servers.iter().find(|s| &s.name == g);
        let locked = g == OWN;
        let asks = !locked && !server.is_some_and(|s| s.trusted) && tools.iter().any(|r| !r.reads);
        let on = locked || tools.iter().any(|r| r.at != Where::Off);
        out.push(json!({
            "id": g,
            "title": server.map(|s| s.title.clone()).unwrap_or_else(|| g.clone()),
            "description": about(g, server),
            "on": on,
            "locked": locked,
            "ask_first": asks,
            "functions": tools.iter().map(|r| r.name.clone()).collect::<Vec<_>>(),
            "loaded": true,
        }));
    }
    for s in servers.iter().filter(|s| s.connected.is_some() && !by.contains_key(&s.name)) {
        out.push(json!({
            "id": s.name,
            "title": s.title,
            "description": about(&s.name, Some(s)),
            "on": !off.contains(&s.name),
            "locked": false,
            "ask_first": !s.trusted,
            "functions": [],
            "loaded": false,
        }));
    }
    out
}

fn server_rows(servers: &Servers) -> Vec<ServerRow> {
    servers
        .iter()
        .map(|s| ServerRow {
            name: s.name.clone(),
            title: s.title.clone(),
            trusted: s.trusted,
            connected: match s.state() {
                State::Connected { tools } => Some(tools),
                _ => None,
            },
        })
        .collect()
}

/// `tools`: the groups as the session last stood still.
fn tools_message(sh: &Shared, key: &Value) -> Value {
    let rows = sh.kept.of::<Kept>().0.lock().ok().filter(|g| g.0 == sh.session).map(|g| g.1.clone());
    let off = sh.kept.of::<Off>().of(&sh.session);
    let groups = groups(&rows.unwrap_or_default(), &server_rows(&sh.servers), &off);
    json!({"type": "tools", "key": key, "groups": groups})
}

/// One server's row of `servers`.
fn server_item(name: &str, title: &str, url: &str, state: &State, next_try: Option<u64>) -> Value {
    let (state, tools, error) = match state {
        State::Connecting => ("connecting", 0, None),
        State::Connected { tools } => ("connected", *tools, None),
        State::Unavailable(e) => ("unavailable", 0, Some(e.clone())),
        State::Failed(e) => ("failed", 0, Some(e.clone())),
    };
    json!({
        "name": name, "title": title, "url": url, "state": state, "tools": tools, "error": error, "next_try": next_try,
    })
}

fn servers_items(servers: &Servers) -> Vec<Value> {
    servers
        .iter()
        .map(|s| {
            let next = s.next_try().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64);
            server_item(&s.name, &s.title, &s.shown_url(), &s.state(), next)
        })
        .collect()
}

/// `servers`: each server and where it stands.
fn servers_message(servers: &Servers, key: &Value) -> Value {
    json!({"type": "servers", "key": key, "items": servers_items(servers)})
}

/// Which watching is the current one: a newer start ends the one before.
#[derive(Default)]
struct Watching(AtomicU64);

/// Says `servers` again whenever a server's state changes, until the session is another.
fn watch(sh: &Shared) {
    let me = sh.kept.of::<Watching>().0.fetch_add(1, Ordering::SeqCst) + 1;
    let (kept, servers, out) = (sh.kept.of::<Watching>(), sh.servers.clone(), sh.out.clone());
    if servers.is_empty() {
        return;
    }
    tokio::spawn(async move {
        let mut was = servers_items(&servers);
        while kept.0.load(Ordering::SeqCst) == me {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let now = servers_items(&servers);
            if now != was {
                send(&out, json!({"type": "servers", "key": null, "items": now}));
                was = now;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(group: &str, name: &str, at: Where, reads: bool) -> Row {
        Row { group: group.into(), name: name.into(), at, reads }
    }

    fn server(name: &str, trusted: bool, connected: Option<usize>) -> ServerRow {
        ServerRow { name: name.into(), title: format!("{name} title"), trusted, connected }
    }

    #[test]
    fn own_tools_come_first_always_on_and_servers_by_name() {
        let rows = vec![
            row("zeta", "z1", Where::On, true),
            row(OWN, "read_file", Where::On, true),
            row(OWN, "run_shell", Where::On, false),
            row("alpha", "a1", Where::Off, false),
        ];
        let servers = vec![server("zeta", false, Some(1)), server("alpha", true, Some(1))];
        let g = groups(&rows, &servers, &BTreeSet::new());
        let ids: Vec<_> = g.iter().map(|g| g["id"].as_str().unwrap_or("")).collect();
        assert_eq!(ids, [OWN, "alpha", "zeta"]);
        assert_eq!(g[0]["locked"], true);
        assert_eq!(g[0]["on"], true);
        assert_eq!(g[0]["ask_first"], false);
        assert_eq!(g[0]["functions"], json!(["read_file", "run_shell"]));
        // A trusted server never asks; its tools are all off, so it is off.
        assert_eq!(g[1]["title"], "alpha title");
        assert_eq!(g[1]["on"], false);
        assert_eq!(g[1]["ask_first"], false);
        // An untrusted server that only reads does not ask either.
        assert_eq!(g[2]["ask_first"], false);
    }

    #[test]
    fn an_untrusted_server_that_changes_things_asks_first() {
        let rows = vec![row("own", "w", Where::Listed, false)];
        let g = groups(&rows, &[server("own", false, Some(1))], &BTreeSet::new());
        assert_eq!(g[0]["ask_first"], true);
        assert_eq!(g[0]["on"], true);
        assert_eq!(g[0]["loaded"], true);
    }

    #[test]
    fn a_server_connected_since_the_last_turn_is_listed_not_loaded() {
        let servers = vec![server("late", false, Some(3)), server("down", false, None)];
        let off: BTreeSet<String> = ["late".to_string()].into();
        let g = groups(&[], &servers, &off);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0]["id"], "late");
        assert_eq!(g[0]["loaded"], false);
        assert_eq!(g[0]["on"], false);
    }

    #[test]
    fn a_server_row_says_its_state_and_why_not() {
        let v = server_item("x", "X", "https://h.test/mcp", &State::Failed("refused".into()), Some(5));
        assert_eq!(v["state"], "failed");
        assert_eq!(v["error"], "refused");
        assert_eq!(v["next_try"], 5);
        let v = server_item("x", "X", "", &State::Connected { tools: 4 }, None);
        assert_eq!(v["state"], "connected");
        assert_eq!(v["tools"], 4);
        assert!(v["error"].is_null() && v["next_try"].is_null());
    }

    #[test]
    fn groups_turned_off_are_kept_for_their_session_only() {
        let off = Off::default();
        off.set("s1", "a", false);
        off.set("s1", "b", false);
        off.set("s1", "a", true);
        assert_eq!(off.of("s1"), ["b".to_string()].into());
        assert!(off.of("s2").is_empty());
        off.set("s2", "c", false);
        assert!(off.of("s1").is_empty());
    }
}
