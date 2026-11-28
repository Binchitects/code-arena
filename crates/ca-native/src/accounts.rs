//! The Accounts menu of the activity bar: who is signed in, to which Arena, signing in and out, and feedback.
//! What every part has (`register`, `take`) is said in `ext.rs`.
//!
//! The button runs `workbench.actions.accounts`, which opens the menu beside it: whose the key is "on HOST", Your
//! Account in Arena, People (Admin) for those who may manage people, Sign in as Someone Else..., Send Feedback...
//! and Sign Out (asked first); signed out, Sign In with User Name and Password.... The button has a dot while no
//! key is kept or Arena refused it. Signing in asks the address, the user name, the password and then, when the
//! account has one, the second factor's code; the driver then takes the session up again with the new key.
//!
//! The driver's messages (`ide_account.rs`): `account` asked on every `state`, answered with `account{signed_in,
//! name, user, host, url, admin, problem}`; `sign_in{url, user, password}` and `sign_in_code{code}` answered with
//! `sign_in{status, error, url, user}`; `sign_out` with `signed_out{revoked, error}`; `feedback{kind, text, id}`
//! with `feedback{id, status, error}`.

use floem::IntoView;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{Decorators, empty};
use serde_json::{Value, json};

use crate::state::{Send, Ui};
use crate::widgets::popup::{self, At, Item, entry, line};
use crate::widgets::{dialog, quick, toast};

/// The commands, under VS Code's id for the button and Code Arena's own for the rest.
pub const MENU: &str = crate::activity_bar::ACCOUNTS;
pub const SIGN_IN: &str = "codeArena.accounts.signIn";
pub const SIGN_OUT: &str = "codeArena.accounts.signOut";
pub const FEEDBACK: &str = "codeArena.sendFeedback";

/// Whose the key is, as the driver said it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Account {
    /// A key is kept (it may still be refused: `problem`).
    pub signed_in: bool,
    pub name: String,
    pub user: String,
    /// The Arena's address without its scheme, and with it.
    pub host: String,
    pub url: String,
    /// The person may manage people (Admin → People).
    pub admin: bool,
    /// What went wrong asking Arena whose the key is.
    pub problem: Option<String>,
}

impl Account {
    pub fn of(m: &Value) -> Account {
        let text = |k: &str| m[k].as_str().unwrap_or("").to_string();
        Account {
            signed_in: m["signed_in"] == true,
            name: text("name"),
            user: text("user"),
            host: text("host"),
            url: text("url"),
            admin: m["admin"] == true,
            problem: m["problem"].as_str().filter(|p| !p.is_empty()).map(str::to_string),
        }
    }

    /// The button's dot: no key kept, or Arena refused it.
    pub fn warns(&self) -> bool {
        !self.signed_in || self.problem.is_some()
    }

    /// Who, as the menu's first row says it: "Sara Ahmadi (sara) on arena.example".
    pub fn who(&self) -> String {
        let on = if self.host.is_empty() { String::new() } else { format!(" on {}", self.host) };
        if !self.signed_in {
            return format!(
                "Not signed in{}",
                if self.host.is_empty() { String::new() } else { format!(" to {}", self.host) }
            );
        }
        match (self.name.as_str(), self.user.as_str()) {
            ("", "") => format!("Signed in{on}"),
            (n, u) if n == u || u.is_empty() => format!("{n}{on}"),
            ("", u) => format!("{u}{on}"),
            (n, u) => format!("{n} ({u}){on}"),
        }
    }

    /// A page of this Arena: `/account`, `/admin/people`.
    fn page(&self, path: &str) -> String {
        let base = if self.url.is_empty() { format!("https://{}", self.host) } else { self.url.clone() };
        format!("{}{path}", base.trim_end_matches('/'))
    }
}

/// What a row of the menu does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    YourAccount,
    People,
    SignIn,
    Feedback,
    SignOut,
}

/// A row of the menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// Words only, dimmed.
    Note(String),
    Line,
    Do(String, Act, bool),
}

/// The menu as it is for this account (none said yet: signed out, until the driver answers).
pub fn rows(a: &Account) -> Vec<Row> {
    let mut rows = vec![Row::Note(a.who())];
    if let Some(p) = &a.problem {
        rows.push(Row::Note(p.clone()));
    }
    rows.push(Row::Line);
    if a.signed_in {
        rows.push(Row::Do("Your Account in Arena".into(), Act::YourAccount, true));
        if a.admin {
            rows.push(Row::Do("People (Admin)".into(), Act::People, true));
        }
        rows.push(Row::Line);
        rows.push(Row::Do("Sign in as Someone Else...".into(), Act::SignIn, true));
        rows.push(Row::Do("Send Feedback...".into(), Act::Feedback, a.problem.is_none()));
        rows.push(Row::Line);
        rows.push(Row::Do("Sign Out".into(), Act::SignOut, true));
    } else {
        rows.push(Row::Do("Sign In with User Name and Password...".into(), Act::SignIn, true));
        rows.push(Row::Do("Send Feedback...".into(), Act::Feedback, false));
    }
    rows
}

/// This part's state in a window.
#[derive(Clone, Copy)]
struct Accounts {
    account: RwSignal<Account>,
    /// The user name this window signs in as, while it waits for the driver's answer.
    signing_in: RwSignal<Option<String>>,
    /// The toast that says a sign-in or a sign-out is under way.
    working: RwSignal<Option<u64>>,
    /// The feedback this window sent last, by its number.
    feedback: RwSignal<u64>,
    /// The driver has said whose the key is (until then the button has no dot).
    known: RwSignal<bool>,
}

impl crate::ext::State for Accounts {
    fn new(ui: Ui) -> Accounts {
        Accounts {
            known: ui.scope.create_rw_signal(false),
            account: ui.scope.create_rw_signal(Account::default()),
            signing_in: ui.scope.create_rw_signal(None),
            working: ui.scope.create_rw_signal(None),
            feedback: ui.scope.create_rw_signal(0),
        }
    }
}

fn state(ui: Ui) -> Accounts {
    ui.state::<Accounts>()
}

/// The dot drawn over the Accounts button while no key is kept or it was refused, as VS Code's warning badge.
/// It sits at the icon's lower right, where the count of a view's badge is (`top`, `right` from the icon's box).
pub fn dot(ui: Ui, shown: bool, top: f64, right: f64) -> impl IntoView {
    let t = ui.colors;
    let me = state(ui);
    empty().style(move |s| {
        let c = t.get();
        let on = shown && me.known.get() && me.account.with(Account::warns);
        s.absolute()
            .inset_top(top)
            .inset_right(right)
            .size(8.0, 8.0)
            .border_radius(4.0)
            .background(c.warning_badge_bg)
            .apply_if(!on, |s| s.hide())
    })
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    let signed_in = |ui: Ui| state(ui).account.with_untracked(|a| a.signed_in);
    add(command(MENU, "Accounts", "Accounts", open_menu).hidden());
    add(command(SIGN_IN, "Accounts", "Sign In with User Name and Password...", sign_in));
    add(command(SIGN_OUT, "Accounts", "Sign Out", sign_out).enabled(signed_in));
    add(command(FEEDBACK, "Help", "Send Feedback...", feedback).enabled(signed_in));
    use crate::menus::{Menu, add as put, item};
    put(Menu::Manage, item(FEEDBACK).title("Send Feedback...").group("4_feedback").order(1));
}

/// The menu, beside the button clicked (the palette's: under the button of the bar it is on).
fn open_menu(ui: Ui, send: &Send) {
    let a = state(ui).account.get_untracked();
    let items: Vec<Item> = rows(&a)
        .into_iter()
        .map(|r| match r {
            Row::Line => line(),
            Row::Note(text) => entry(&text, || {}).enabled(false).into(),
            Row::Do(label, act, enabled) => {
                let (send, a) = (send.clone(), a.clone());
                entry(&label, move || run(ui, &send, &a, act)).enabled(enabled).into()
            }
        })
        .collect();
    let at = crate::activity_bar::foot_button().map(At::beside);
    match at {
        Some(at) => popup::open(ui, at, items),
        None => as_pick(ui, send, &a),
    }
}

/// The menu as a quick pick, when no button was clicked (the command run from the palette).
fn as_pick(ui: Ui, send: &Send, a: &Account) {
    let mut items = Vec::new();
    for r in rows(a) {
        if let Row::Do(label, act, true) = r {
            items.push(quick::item(&format!("{act:?}"), &label));
        }
    }
    let (send, a) = (send.clone(), a.clone());
    quick::pick("Accounts").placeholder(&a.who()).items(items).open(ui, move |chosen| {
        let Some(id) = chosen.map(|c| c.id) else { return };
        let acts = [Act::YourAccount, Act::People, Act::SignIn, Act::Feedback, Act::SignOut];
        if let Some(act) = acts.into_iter().find(|x| format!("{x:?}") == id) {
            run(ui, &send, &a, act);
        }
    });
}

fn run(ui: Ui, send: &Send, a: &Account, act: Act) {
    match act {
        Act::YourAccount => crate::citations::browse(&a.page("/account")),
        Act::People => crate::citations::browse(&a.page("/admin/people")),
        Act::SignIn => sign_in(ui, send),
        Act::Feedback => feedback(ui, send),
        Act::SignOut => sign_out(ui, send),
    }
}

const SIGN_IN_TITLE: &str = "Sign In to Arena";

/// The address, the user name, the password; then the driver signs in.
fn sign_in(ui: Ui, send: &Send) {
    let a = state(ui).account.get_untracked();
    let send = send.clone();
    let needed = |what: &'static str| move |v: &str| v.trim().is_empty().then(|| format!("{what} is needed."));
    quick::input(SIGN_IN_TITLE)
        .placeholder("https://arena.example.com")
        .prompt("The Arena's address (Enter to go on)")
        .value(if a.url.is_empty() { &a.host } else { &a.url })
        .validate(needed("The address"))
        .open(ui, move |url| {
            let Some(url) = url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()) else { return };
            quick::input(SIGN_IN_TITLE)
                .placeholder("User name")
                .prompt(&format!("Your user name on {url}"))
                .validate(needed("A user name"))
                .open(ui, move |user| {
                    let Some(user) = user.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()) else { return };
                    quick::input(SIGN_IN_TITLE)
                        .placeholder("Password")
                        .prompt(&format!("{user}'s password"))
                        .password()
                        .open(ui, move |password| {
                            let Some(password) = password.filter(|p| !p.is_empty()) else { return };
                            let me = state(ui);
                            me.signing_in.set(Some(user.clone()));
                            working(ui, &format!("Signing in to {url}..."));
                            send(json!({"type": "sign_in", "url": url, "user": user, "password": password}));
                        });
                });
        });
}

/// The second factor's code, asked once the password was taken (again, with Arena's words, when one was refused).
fn ask_code(ui: Ui, send: &Send, user: String, error: Option<&str>) {
    let send = send.clone();
    let prompt = match error {
        Some(e) => format!("{e} Try another code, from your authenticator app or a recovery code."),
        None => "The code from your authenticator app, or a recovery code".to_string(),
    };
    quick::input(SIGN_IN_TITLE)
        .placeholder("Code")
        .prompt(&prompt)
        .validate(|v| v.trim().is_empty().then(|| "A code is needed.".into()))
        .open(ui, move |code| {
            let Some(code) = code.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()) else {
                state(ui).signing_in.set(None);
                return;
            };
            working(ui, "Signing in...");
            state(ui).signing_in.set(Some(user));
            send(json!({"type": "sign_in_code", "code": code}));
        });
}

/// Asked first, as VS Code asks before signing out.
fn sign_out(ui: Ui, send: &Send) {
    let a = state(ui).account.get_untracked();
    if !a.signed_in {
        toast::info(ui, "Not signed in.");
        return;
    }
    let send = send.clone();
    let host = if a.host.is_empty() { "Arena".to_string() } else { a.host.clone() };
    dialog::ask(&format!("Sign out of {host}?"))
        .detail(
            "The key this computer signed in with is revoked in Arena and forgotten here. The editor, the terminal \
             and git go on working; the agent has no model until you sign in again.",
        )
        .kind(dialog::Kind::Warning)
        .button("Sign Out")
        .open(ui, move |answer| {
            if answer.yes() {
                working(ui, &format!("Signing out of {host}..."));
                send(json!({"type": "sign_out"}));
            }
        });
}

/// What it is, then the words; sent to Arena's admins.
fn feedback(ui: Ui, send: &Send) {
    let a = state(ui).account.get_untracked();
    if !a.signed_in {
        toast::info(ui, "Sign in to an Arena to send feedback.");
        return;
    }
    let send = send.clone();
    let kinds = vec![
        quick::item("idea", "An Idea").icon("lightbulb"),
        quick::item("problem", "A Problem").icon("bug"),
        quick::item("praise", "Praise").icon("heart"),
        quick::item("other", "Other").icon("comment"),
    ];
    let host = a.host.clone();
    quick::pick("Send Feedback").placeholder("What it is").items(kinds).open(ui, move |kind| {
        let Some(kind) = kind.map(|k| k.id) else { return };
        quick::input("Send Feedback")
            .placeholder("What you want to say")
            .prompt(&format!("Sent to the admins of {host}, who may answer you there (Enter sends it)"))
            .validate(|v| match v.trim().chars().count() {
                0 => Some("Write what you want to say.".into()),
                n if n > 4000 => Some(format!("At most 4000 characters ({n} now).")),
                _ => None,
            })
            .open(ui, move |text| {
                let Some(text) = text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) else { return };
                let me = state(ui);
                let id = me.feedback.get_untracked() + 1;
                me.feedback.set(id);
                send(json!({"type": "feedback", "kind": kind, "text": text, "id": id}));
            });
    });
}

/// The toast that says something is under way, in place of the one before.
fn working(ui: Ui, text: &str) {
    done_working(ui);
    let id = toast::info_of(text).sticky().show_in(ui);
    state(ui).working.set(Some(id));
}

fn done_working(ui: Ui) {
    let me = state(ui);
    if let Some(id) = me.working.get_untracked() {
        toast::dismiss(ui, id);
        me.working.set(None);
    }
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let me = state(ui);
    match m["type"].as_str().unwrap_or("") {
        "state" => send(json!({"type": "account"})),
        "account" => {
            let a = Account::of(m);
            if me.account.with_untracked(|was| *was != a) {
                me.account.set(a);
            }
            if !me.known.get_untracked() {
                me.known.set(true);
            }
        }
        "sign_in" => {
            let user = m["user"].as_str().unwrap_or("");
            // The answer to another window's sign-in is that window's to show; one with no user (no sign-in
            // waited for the code) is the answer to whichever window waits.
            let mine = me.signing_in.with_untracked(|u| u.as_deref().is_some_and(|u| user.is_empty() || u == user));
            if !mine {
                return false;
            }
            let user = me.signing_in.get_untracked().unwrap_or_default();
            let user = user.as_str();
            done_working(ui);
            let error = m["error"].as_str().filter(|e| !e.is_empty());
            match m["status"].as_str().unwrap_or("") {
                "2fa" => ask_code(ui, send, user.to_string(), error),
                "ok" => {
                    me.signing_in.set(None);
                    toast::info(ui, &format!("Signed in to {} as {user}.", host_of(m["url"].as_str().unwrap_or(""))));
                }
                _ => {
                    me.signing_in.set(None);
                    let error = error.unwrap_or("Arena did not say why.").to_string();
                    toast::error_of(&format!("Could not sign in: {error}"))
                        .action("Try Again", sign_in)
                        .source("Accounts")
                        .show_in(ui);
                }
            }
        }
        "signed_out" => {
            done_working(ui);
            match m["error"].as_str().filter(|e| !e.is_empty()) {
                Some(e) => toast::warning(ui, e),
                None if m["revoked"] == true => toast::info(ui, "Signed out: the key was revoked in Arena."),
                None => toast::info(ui, "Signed out: the key is forgotten here."),
            };
        }
        "feedback" => {
            if m["id"].as_u64() != Some(me.feedback.get_untracked()) {
                return false;
            }
            match m["error"].as_str().filter(|e| !e.is_empty()) {
                Some(e) => toast::error_of(&format!("Feedback was not sent: {e}")).source("Accounts").show_in(ui),
                None => toast::info(ui, "Thank you: your feedback was sent."),
            };
        }
        "refused" if matches!(m["of"].as_str(), Some("sign_in" | "sign_in_code" | "sign_out")) => {
            done_working(ui);
            me.signing_in.set(None);
            let reason = m["reason"].as_str().unwrap_or("Wait for the answer, or stop it, first.");
            toast::warning(ui, reason);
        }
        _ => {}
    }
    false
}

fn host_of(url: &str) -> &str {
    url.split_once("://").map_or(url, |(_, r)| r).trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    fn signed(admin: bool, problem: Option<&str>) -> Account {
        Account::of(&json!({"type": "account", "signed_in": true, "name": "Sara Ahmadi", "user": "sara",
            "host": "arena.example", "url": "https://arena.example", "admin": admin, "problem": problem}))
    }

    fn shown(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Line => "-".to_string(),
                Row::Note(t) => format!("({t})"),
                Row::Do(t, _, true) => t.clone(),
                Row::Do(t, _, false) => format!("{t} (dimmed)"),
            })
            .collect()
    }

    #[test]
    fn signed_in_the_menu_says_whose_and_where_then_what_to_do() {
        assert_eq!(
            shown(&rows(&signed(true, None))),
            [
                "(Sara Ahmadi (sara) on arena.example)",
                "-",
                "Your Account in Arena",
                "People (Admin)",
                "-",
                "Sign in as Someone Else...",
                "Send Feedback...",
                "-",
                "Sign Out"
            ]
        );
        assert!(!rows(&signed(false, None)).iter().any(|r| matches!(r, Row::Do(_, Act::People, _))), "admins only");
        assert!(!signed(false, None).warns());
    }

    #[test]
    fn signed_out_or_refused_the_button_warns() {
        let out = Account::of(&json!({"type": "account", "signed_in": false, "host": "arena.example"}));
        assert!(out.warns());
        assert_eq!(
            shown(&rows(&out)),
            [
                "(Not signed in to arena.example)",
                "-",
                "Sign In with User Name and Password...",
                "Send Feedback... (dimmed)"
            ]
        );
        let refused = signed(false, Some("Arena refused the key: That key was revoked."));
        assert!(refused.warns());
        let rows = shown(&rows(&refused));
        assert_eq!(rows[1], "(Arena refused the key: That key was revoked.)");
        assert!(rows.contains(&"Sign Out".to_string()), "a refused key can still be signed out");
        assert_eq!(Account::default().who(), "Not signed in");
    }

    #[test]
    fn the_pages_of_this_arena() {
        assert_eq!(signed(true, None).page("/admin/people"), "https://arena.example/admin/people");
        let a = Account { host: "arena.example".into(), ..Default::default() };
        assert_eq!(a.page("/account"), "https://arena.example/account");
    }

    #[test]
    fn the_account_is_asked_on_state_and_kept_when_it_changes() {
        let ui = Ui::new();
        let sent = Rc::new(RefCell::new(Vec::new()));
        let s = sent.clone();
        let send: Send = Rc::new(move |m| s.borrow_mut().push(m));
        assert!(!take(ui, &json!({"type": "state"}), &send));
        assert_eq!(*sent.borrow(), [json!({"type": "account"})]);
        assert!(state(ui).account.get_untracked().warns(), "nothing said yet: signed out");
        take(ui, &json!({"type": "account", "signed_in": true, "name": "Sara", "user": "sara"}), &send);
        assert_eq!(state(ui).account.get_untracked().who(), "Sara (sara)");
        assert!(!state(ui).account.get_untracked().warns());
    }

    #[test]
    fn only_the_window_that_signs_in_hears_its_answer() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let me = state(ui);
        take(ui, &json!({"type": "sign_in", "status": "ok", "url": "https://a.example", "user": "sara"}), &send);
        assert_eq!(toast::count(ui), 0, "not this window's");
        me.signing_in.set(Some("sara".into()));
        take(ui, &json!({"type": "sign_in", "status": "error", "error": "Wrong password.", "user": "sara"}), &send);
        assert_eq!(me.signing_in.get_untracked(), None);
        assert_eq!(toast::count(ui), 1);
        // A code no sign-in waited for is answered with no user: the window that waits takes it.
        me.signing_in.set(Some("sara".into()));
        take(ui, &json!({"type": "sign_in", "status": "error", "error": "No sign-in waits.", "user": ""}), &send);
        assert_eq!(me.signing_in.get_untracked(), None);
        // A refusal mid-turn ends the wait too.
        me.signing_in.set(Some("sara".into()));
        take(
            ui,
            &json!({"type": "refused", "of": "sign_in", "reason": "Wait for the answer, or stop it, to sign in."}),
            &send,
        );
        assert_eq!(me.signing_in.get_untracked(), None);
    }

    #[test]
    fn feedback_answers_are_this_windows_by_their_number() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        state(ui).feedback.set(2);
        take(ui, &json!({"type": "feedback", "id": 1, "status": "sent"}), &send);
        assert_eq!(toast::count(ui), 0);
        take(ui, &json!({"type": "feedback", "id": 2, "status": "sent", "error": null}), &send);
        assert_eq!(toast::count(ui), 1);
    }
}
