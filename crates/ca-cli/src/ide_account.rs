//! The person's accounts in the IDE: the Arena signed in to (sign in, sign out, feedback) and the providers of
//! their own.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn).
//!
//! - `account` [T] → `account{signed_in, name, user, host, url, admin, problem}`: whether a key is kept, whose it
//!   is (Arena's `GET /api/code-arena/me`) and on which Arena (`url`; `host`, without its scheme); `admin` when
//!   the person may manage people; `problem` what went wrong asking (the key refused, Arena not reached).
//! - `sign_in{url, user, password}` [R] → `sign_in{status, error, url, user}`: Arena's own sign-in, as
//!   `code-arena login` does it (its lockout, its directory and its second factor hold). `status` is `"2fa"` when
//!   the account has a second factor (`sign_in_code` next), `"error"` with Arena's words, `"ok"` once a key was
//!   made for this machine and kept. The password is never said back.
//! - `sign_in_code{code}` [R] → `sign_in{…}` as above: the authenticator's code (or a recovery code) for the sign-in
//!   waiting for one.
//! - `sign_out` [R] → `signed_out{revoked, error}`: Arena revokes the key when it is one Code Arena made
//!   (`revoked`), and it is forgotten here whatever Arena said.
//! - `feedback{kind, text, screenshot, id}` [T] → `feedback{id, status: "sent" | "error", error}`: sent to Arena
//!   (`POST /api/feedback`, kept for its admins); `kind` one of `idea`, `problem`, `praise`, `other`, `screenshot`
//!   a PNG in base64.
//!
//! Once signed in or out, the session is taken up again in place (the same session, or a new one when it had
//! nothing in it yet) so that its model is asked with the new key: the gateway's models are asked again, and the
//! faces get `state` and `tasks` whole, then `account`.

use std::sync::{Mutex, PoisonError};

use ca_arena::signin::{PasswordSignIn, Step};
use ca_arena::{ArenaClient, ArenaError, Me};
use ca_core::Secret;
use ca_settings::Connection;
use ca_settings::connection::normalize_url;
use serde_json::{Value, json};

use crate::ide_ext::{Area, Ctx, Rule, Shared, kind, send};
use crate::session::{self, Opts, Resume};

/// Where feedback says it was sent from.
const PAGE: &str = "Code Arena";

/// The most feedback says, in characters, as Arena takes it.
const MOST_TEXT: usize = 4000;

/// This area in the driver.
pub struct Here;

/// A password sign-in waiting for its second factor: its web session, the address and the user name.
#[derive(Default)]
struct Waiting(Mutex<Option<(PasswordSignIn, String, String)>>);

impl Waiting {
    fn put(&self, s: Option<(PasswordSignIn, String, String)>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = s;
    }

    fn take(&self) -> Option<(PasswordSignIn, String, String)> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[
        ("account", Rule::Turn),
        ("feedback", Rule::Turn),
        ("sign_in", Rule::Refuse("sign in")),
        ("sign_in_code", Rule::Refuse("sign in")),
        ("sign_out", Rule::Refuse("sign out")),
    ];

    fn shared(sh: &Shared, m: &Value) -> bool {
        match kind(m) {
            "account" => {
                tell_account(sh.out.clone(), sh.arena_url.clone(), sh.arena.clone());
                true
            }
            "feedback" => {
                let id = m["id"].clone();
                let out = sh.out.clone();
                let answer = move |error: Option<String>| {
                    let status = if error.is_some() { "error" } else { "sent" };
                    send(&out, json!({"type": "feedback", "id": id, "status": status, "error": error}));
                };
                match (checked_feedback(m), sh.arena.clone().filter(ArenaClient::has_key)) {
                    (Err(e), _) => answer(Some(e)),
                    (Ok(_), None) => answer(Some("Sign in to an Arena to send feedback.".into())),
                    (Ok((kind, text, shots)), Some(arena)) => {
                        tokio::spawn(async move {
                            answer(arena.feedback(&kind, &text, PAGE, &shots).await.err().map(|e| e.to_string()));
                        });
                    }
                }
                true
            }
            _ => false,
        }
    }

    async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
        match kind(m) {
            "sign_in" => sign_in(ctx, m).await,
            "sign_in_code" => sign_in_code(ctx, m).await,
            "sign_out" => sign_out(ctx).await,
            _ => return false,
        }
        true
    }
}

/// `account` asked of Arena off the loop, and said when it answers.
fn tell_account(out: tokio::sync::broadcast::Sender<String>, url: Option<String>, arena: Option<ArenaClient>) {
    let arena = arena.filter(ArenaClient::has_key);
    tokio::spawn(async move {
        let me = match arena {
            None => Ok(None),
            Some(a) => a.me().await.map(Some),
        };
        send(&out, said(url.as_deref(), me));
    });
}

/// The address as the menu shows it: `https://arena.example/` → `arena.example`.
fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.trim_end_matches('/').to_string()
}

/// What `account` says, from whose the key is (none kept: `Ok(None)`).
fn said(url: Option<&str>, me: Result<Option<Me>, ArenaError>) -> Value {
    let host = url.map(host_of);
    let mut v = json!({"type": "account", "signed_in": false, "name": null, "user": null, "host": host,
        "url": url, "admin": false, "problem": null});
    match me {
        Ok(None) => {}
        Ok(Some(me)) => {
            v["signed_in"] = json!(true);
            v["name"] = json!(me.display_name.clone().filter(|n| !n.trim().is_empty()).unwrap_or(me.user_name.clone()));
            v["user"] = json!(me.user_name);
            v["admin"] = json!(me.manages_people);
        }
        Err(e) => {
            // A key is kept, but it did not say whose it is: refused, or Arena not reached.
            v["signed_in"] = json!(true);
            v["problem"] = json!(match e {
                ArenaError::Key(m) => format!("Arena refused the key: {m}"),
                e => e.to_string(),
            });
        }
    }
    v
}

/// Feedback as Arena takes it: its kind, its words (trimmed, at most [`MOST_TEXT`]), its screenshot.
fn checked_feedback(m: &Value) -> Result<(String, String, Vec<String>), String> {
    let kind = m["kind"].as_str().unwrap_or("other");
    if !ArenaClient::FEEDBACK_KINDS.contains(&kind) {
        return Err(format!("Say what it is: {}.", ArenaClient::FEEDBACK_KINDS.join(", ")));
    }
    let text = m["text"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        return Err("Write what you want to say.".into());
    }
    if text.chars().count() > MOST_TEXT {
        return Err(format!("Feedback is at most {MOST_TEXT} characters."));
    }
    let shots = m["screenshot"].as_str().filter(|s| !s.is_empty()).map(|s| vec![s.to_string()]).unwrap_or_default();
    Ok((kind.to_string(), text.to_string(), shots))
}

/// `sign_in{status: "error"}` with the words, for the address and user asked.
fn refused(ctx: &Ctx<'_>, url: &str, user: &str, error: &str) {
    send(&ctx.drv.out, json!({"type": "sign_in", "status": "error", "error": error, "url": url, "user": user}));
}

async fn sign_in(ctx: &mut Ctx<'_>, m: &Value) {
    let typed = m["url"].as_str().unwrap_or("").trim().to_string();
    let user = m["user"].as_str().unwrap_or("").trim().to_string();
    let url = match normalize_url(&typed) {
        Ok(u) => u,
        Err(e) => return refused(ctx, &typed, &user, &e.to_string()),
    };
    if user.is_empty() {
        return refused(ctx, &url, &user, "A user name is needed.");
    }
    let password = Secret::new(m["password"].as_str().unwrap_or(""));
    let s = match PasswordSignIn::new(&url, ctx.app.ca()) {
        Ok(s) => s,
        Err(e) => return refused(ctx, &url, &user, &e.to_string()),
    };
    ctx.drv.kept.of::<Waiting>().put(None);
    match s.password(&user, &password).await {
        Err(e) => refused(ctx, &url, &user, &e.to_string()),
        Ok(Step::SecondFactor) => {
            send(&ctx.drv.out, json!({"type": "sign_in", "status": "2fa", "error": null, "url": url, "user": user}));
            ctx.drv.kept.of::<Waiting>().put(Some((s, url, user)));
        }
        Ok(Step::SignedIn) => signed_in(ctx, s, url, user).await,
    }
}

async fn sign_in_code(ctx: &mut Ctx<'_>, m: &Value) {
    let waiting = ctx.drv.kept.of::<Waiting>();
    let Some((s, url, user)) = waiting.take() else {
        return refused(ctx, "", "", "No sign-in waits for a code: sign in again.");
    };
    let code = Secret::new(m["code"].as_str().unwrap_or("").trim());
    match s.code(&code).await {
        Err(e) => {
            // Another code may be tried: the sign-in still waits.
            send(
                &ctx.drv.out,
                json!({"type": "sign_in", "status": "2fa", "error": e.to_string(), "url": url, "user": user}),
            );
            waiting.put(Some((s, url, user)));
        }
        Ok(()) => signed_in(ctx, s, url, user).await,
    }
}

/// Signed in: a key made for this machine and kept as `code-arena login` keeps it, then the session taken up with it.
async fn signed_in(ctx: &mut Ctx<'_>, s: PasswordSignIn, url: String, user: String) {
    let key = match s.make_key(&crate::account::machine_name()).await {
        Ok(k) => k,
        Err(e) => return refused(ctx, &url, &user, &format!("Signed in, but no key could be made: {e}")),
    };
    let before = &ctx.app.connection;
    let same = before.url.as_deref() == Some(url.as_str());
    let mut conn = Connection {
        url: Some(url.clone()),
        gateway: before.gateway.clone().filter(|_| same),
        ca: before.ca.clone(),
        providers: before.providers.clone(),
        api_key: Some(key.clone()),
        ..Default::default()
    };
    if conn.gateway_url().is_none() {
        return refused(ctx, &url, &user, &format!("{url} is an address, so its gateway cannot be guessed: sign in with code-arena login --url {url} --gateway URL"));
    }
    if let Ok(arena) = ArenaClient::new(&url, Some(key), conn.ca.as_deref())
        && let Ok(me) = arena.me().await
    {
        conn.account = Some(me.display_name.clone().unwrap_or_else(|| me.user_name.clone()));
    }
    if let Err(e) = conn.save(&ctx.app.paths.credentials_file()) {
        return refused(ctx, &url, &user, &format!("Signed in, but the key could not be kept: {e}"));
    }
    ctx.app.connection = conn;
    taken_up_again(ctx).await;
    send(&ctx.drv.out, json!({"type": "sign_in", "status": "ok", "error": null, "url": url, "user": user}));
    tell_account(ctx.drv.out.clone(), ctx.app.connection.url.clone(), ctx.drv.arena.clone());
}

/// Signed out as `code-arena logout` does: Arena told, the key forgotten here whatever it said.
async fn sign_out(ctx: &mut Ctx<'_>) {
    let mut error = None;
    let revoked = match ctx.drv.arena.clone().filter(ArenaClient::has_key) {
        None => false,
        Some(a) => match a.sign_out().await {
            Ok(()) => true,
            Err(ArenaError::Key(_)) => false,
            Err(e) => {
                error = Some(format!("Arena did not take the sign-out ({e}); the key is forgotten here all the same."));
                false
            }
        },
    };
    let file = ctx.app.paths.credentials_file();
    let kept = Connection::load(&file, &ca_settings::Env::default()).and_then(|mut c| {
        c.api_key = None;
        c.account = None;
        c.save(&file)
    });
    if let Err(e) = kept {
        error = Some(format!("The key could not be forgotten in {}: {e}", file.display()));
    }
    ctx.app.connection.api_key = None;
    ctx.app.connection.account = None;
    taken_up_again(ctx).await;
    send(&ctx.drv.out, json!({"type": "signed_out", "revoked": revoked, "error": error}));
    tell_account(ctx.drv.out.clone(), ctx.app.connection.url.clone(), ctx.drv.arena.clone());
}

/// The session taken up again with the connection as it is now: the same one, or a new one when it was never
/// kept (nothing was said in it yet); the gateway's models asked again; the faces told the whole of it.
async fn taken_up_again(ctx: &mut Ctx<'_>) {
    ctx.drv.arena = ctx.app.arena().ok();
    let id = ctx.p.session.clone();
    let kept = ctx.p.store.lock().ok().is_some_and(|s| s.summary(&id).is_ok());
    let resume = if kept { Resume::Id(id.clone()) } else { Resume::New };
    let opts = Opts {
        cwd: Some(ctx.p.agent.ctx.workspace.cwd().to_path_buf()),
        mode: Some(ctx.p.agent.policy.mode.name().into()),
        carry_plan: false,
        ..ctx.drv.opts.clone()
    };
    match session::prepare(ctx.app, &opts, ctx.drv.approver.clone(), resume).await {
        Ok(mut p) => {
            p.agent.thinking = ctx.p.agent.thinking.clone();
            p.agent.inbox = Some(ctx.drv.inbox.clone());
            *ctx.p = p;
        }
        Err(e) => {
            ctx.sh.say(&format!("The session could not be taken up with the new key: {e}. Open the IDE again."));
            return;
        }
    }
    let models = tokio::time::timeout(std::time::Duration::from_secs(15), ctx.p.agent.gateway.models()).await;
    match models {
        Ok(Ok(ms)) if !ms.is_empty() => {
            ctx.drv.models = ms.iter().map(|m| m.id.clone()).collect();
            ctx.drv.kept.of::<crate::ide_chat::pickers::Infos>().set(ms);
        }
        _ => ctx.drv.models = vec![ctx.p.agent.model.clone()].into_iter().filter(|m| !m.is_empty()).collect(),
    }
    // Another session is told whole by the driver itself; the same one is told here.
    if ctx.p.session == id {
        let sh = Shared::of(ctx.p, ctx.app, ctx.drv, ctx.side);
        send(&ctx.drv.out, crate::ide::state(ctx.p, ctx.app, &sh, false));
        send(&ctx.drv.out, crate::ide::tasks(&sh));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me(display: Option<&str>, admin: bool) -> Me {
        serde_json::from_value(json!({"userName": "sara", "displayName": display, "managesPeople": admin}))
            .expect("me")
    }

    #[test]
    fn whose_the_key_is_and_where() {
        let v = said(Some("https://arena.example/"), Ok(Some(me(Some("Sara Ahmadi"), true))));
        assert_eq!(v["type"], "account");
        assert_eq!(v["signed_in"], true);
        assert_eq!(v["name"], "Sara Ahmadi");
        assert_eq!(v["user"], "sara");
        assert_eq!(v["host"], "arena.example");
        assert_eq!(v["admin"], true);
        assert_eq!(v["problem"], Value::Null);
        // No display name: the user name is the name.
        assert_eq!(said(None, Ok(Some(me(None, false))))["name"], "sara");
    }

    #[test]
    fn no_key_is_signed_out_and_a_refused_key_is_a_problem() {
        let v = said(Some("https://arena.example"), Ok(None));
        assert_eq!((v["signed_in"].clone(), v["host"].clone()), (json!(false), json!("arena.example")));
        let v = said(Some("https://arena.example"), Err(ArenaError::Key("That key was revoked.".into())));
        assert_eq!(v["signed_in"], true, "a key is kept: Sign out stays in the menu");
        assert_eq!(v["problem"], "Arena refused the key: That key was revoked.");
        assert_eq!(said(None, Ok(None))["host"], Value::Null);
    }

    #[test]
    fn hosts_without_their_scheme() {
        assert_eq!(host_of("https://arena.example"), "arena.example");
        assert_eq!(host_of("http://10.0.0.5:8080/"), "10.0.0.5:8080");
        assert_eq!(host_of("arena.example"), "arena.example");
    }

    #[test]
    fn feedback_as_arena_takes_it() {
        let ok = checked_feedback(&json!({"kind": "idea", "text": "  More themes ", "screenshot": "iVBOR"}));
        assert_eq!(ok, Ok(("idea".into(), "More themes".into(), vec!["iVBOR".into()])));
        assert_eq!(checked_feedback(&json!({"text": "x"})).map(|f| f.0), Ok("other".into()), "no kind: other");
        assert!(checked_feedback(&json!({"kind": "rant", "text": "x"})).is_err());
        assert_eq!(checked_feedback(&json!({"kind": "idea", "text": "  "})), Err("Write what you want to say.".into()));
        assert!(checked_feedback(&json!({"kind": "idea", "text": "x".repeat(MOST_TEXT + 1)})).is_err());
        assert_eq!(checked_feedback(&json!({"kind": "praise", "text": "x", "screenshot": ""})).map(|f| f.2), Ok(vec![]));
    }

    #[test]
    fn the_password_step_waits_for_its_code_once() {
        let w = Waiting::default();
        let s = PasswordSignIn::new("https://arena.example", None).expect("sign-in");
        w.put(Some((s, "https://arena.example".into(), "sara".into())));
        assert_eq!(w.take().map(|(_, url, user)| (url, user)), Some(("https://arena.example".into(), "sara".into())));
        assert!(w.take().is_none(), "taken once");
    }
}
