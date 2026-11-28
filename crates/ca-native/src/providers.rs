//! The Other providers dialog, as the old Code Arena's: the providers of the person's own keys, whose models the
//! model picker offers as `provider:model` beside Arena's. Opened by the plug beside the pickers and by the chat's
//! More menu ("Other Providers…").
//!
//! Its list: each provider's name, how its API is spoken ("Messages API" or "Chat completions"), its address, where
//! its key comes from (kept here, from its variable, or no key, in red), up to four of its models, and a button to
//! remove it. Under it, a provider to add: Anthropic, OpenAI, DeepSeek, OpenRouter, Mistral, or "Another API…" (its
//! lower-case name, its address and what it speaks), its key in a password field (empty: from its variable), and
//! "Add and list its models", which checks the key by listing them; what failed is said in the dialog, which stays
//! open. While Arena's admins keep other providers out, the dialog says so and adds none.
//!
//! Messages: `providers`, `provider_add{name, url?, kind?, api_key?}` and `provider_remove{name}` out;
//! `providers{allowed, items, known}`, `provider_added{name, ok, error?}`, `provider_removed{name, ok}` and
//! `refused{of: "provider_add"}` in (the driver's `ide_account/providers.rs`). The key is cleared as soon as it is
//! sent, and when the dialog closes.

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect};
use floem::style::{FlexWrap, TextOverflow};
use floem::views::{Decorators, dyn_stack, h_stack, label, scroll, tooltip, v_stack};
use floem::{AnyView, IntoView};
use serde_json::{Value, json};

use crate::agent::slots::{self, Slot};
use crate::commands::{self, command};
use crate::layers::{self, Place};
use crate::menus::{self, Menu};
use crate::state::{Send, Ui};
use crate::widgets::form::{self, Form};
use crate::widgets::{action, bold};

/// The dialog's command, in the chat's More menu and the palette.
pub const OPEN: &str = "codeArena.chat.otherProviders";

/// How many of a provider's models its row names.
const SHOWN_MODELS: usize = 4;

/// A provider kept, as the driver lists it (never its key).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Row {
    pub name: String,
    /// `anthropic` or `openai`.
    pub kind: String,
    pub url: String,
    /// `here`, `env` or `none`.
    pub key: String,
    pub key_env: String,
    pub models: Vec<String>,
    pub more: u64,
}

impl Row {
    pub fn of(v: &Value) -> Option<Row> {
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        Some(Row {
            name: v["name"].as_str().filter(|n| !n.is_empty())?.to_string(),
            kind: s("kind"),
            url: s("url"),
            key: s("key"),
            key_env: s("key_env"),
            models: v["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            more: v["more"].as_u64().unwrap_or(0),
        })
    }

    /// How its API is spoken, in the dialog's words.
    pub fn speaks(&self) -> &'static str {
        if self.kind == "anthropic" { "Messages API" } else { "Chat completions" }
    }

    /// Where its key comes from; `true`: there is none (said in red).
    pub fn key_words(&self) -> (String, bool) {
        match self.key.as_str() {
            "here" => ("Key kept here".into(), false),
            "env" if !self.key_env.is_empty() => (format!("Key from {}", self.key_env), false),
            "env" => ("Key from the environment".into(), false),
            // Kept without one only where none is needed (as the driver takes them): not a fault.
            _ if self.name == "ollama" || self.url.starts_with("http://") => ("No key needed".into(), false),
            _ => ("No key".into(), true),
        }
    }

    /// Its models as the row names them.
    pub fn models_words(&self) -> String {
        if self.models.is_empty() {
            return "No models listed".into();
        }
        let shown: Vec<&str> = self.models.iter().take(SHOWN_MODELS).map(String::as_str).collect();
        let more = self.more + self.models.len().saturating_sub(SHOWN_MODELS) as u64;
        if more > 0 { format!("{} and {more} more", shown.join(", ")) } else { shown.join(", ") }
    }
}

/// A provider to add: one known by name, or another API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Anthropic,
    OpenAi,
    DeepSeek,
    OpenRouter,
    Mistral,
    Other,
}

impl Choice {
    pub const ALL: [Choice; 6] =
        [Choice::Anthropic, Choice::OpenAi, Choice::DeepSeek, Choice::OpenRouter, Choice::Mistral, Choice::Other];

    /// Its name in the providers' list (none: the person names another API).
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Choice::Anthropic => "anthropic",
            Choice::OpenAi => "openai",
            Choice::DeepSeek => "deepseek",
            Choice::OpenRouter => "openrouter",
            Choice::Mistral => "mistral",
            Choice::Other => return None,
        })
    }

    pub fn words(self) -> &'static str {
        match self {
            Choice::Anthropic => "Anthropic",
            Choice::OpenAi => "OpenAI",
            Choice::DeepSeek => "DeepSeek",
            Choice::OpenRouter => "OpenRouter",
            Choice::Mistral => "Mistral",
            Choice::Other => "Another API…",
        }
    }

    /// The variable its key is read from when none is given.
    pub fn key_env(self) -> Option<&'static str> {
        Some(match self {
            Choice::Anthropic => "ANTHROPIC_API_KEY",
            Choice::OpenAi => "OPENAI_API_KEY",
            Choice::DeepSeek => "DEEPSEEK_API_KEY",
            Choice::OpenRouter => "OPENROUTER_API_KEY",
            Choice::Mistral => "MISTRAL_API_KEY",
            Choice::Other => return None,
        })
    }
}

/// What another API speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speaks {
    Chat,
    Messages,
}

/// A provider's name: lower-case letters, digits and `-`, not from a `-` (as the driver takes it).
pub fn valid_name(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && !name.starts_with('-')
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// What the form asks for, as `provider_add`; or why it cannot be asked yet.
pub fn add_message(choice: Choice, name: &str, url: &str, speaks: Speaks, key: &str) -> Result<Value, &'static str> {
    let mut v = match choice.name() {
        Some(known) => json!({"type": "provider_add", "name": known}),
        None => {
            let (name, url) = (name.trim(), url.trim());
            if !valid_name(name) {
                return Err("Its name: lower-case letters, digits and '-' only");
            }
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err("Its address begins with https:// or http://");
            }
            let kind = if speaks == Speaks::Messages { "anthropic" } else { "openai" };
            json!({"type": "provider_add", "name": name, "url": url, "kind": kind})
        }
    };
    if !key.trim().is_empty() {
        v["api_key"] = json!(key.trim());
    }
    Ok(v)
}

/// The words under the key's field.
pub fn key_hint(choice: Choice) -> String {
    match choice.key_env() {
        Some(var) => format!("From {var} (leave empty)"),
        None => "None for an API on this machine or your network (leave empty)".into(),
    }
}

/// The dialog's signals.
#[derive(Clone, Copy)]
struct Providers {
    form: Form,
    items: RwSignal<Vec<Row>>,
    allowed: RwSignal<bool>,
    choice: RwSignal<Choice>,
    name: RwSignal<String>,
    url: RwSignal<String>,
    speaks: RwSignal<Speaks>,
    key: RwSignal<String>,
    /// The provider being added, until the driver says how it went.
    busy: RwSignal<Option<String>>,
    /// What went wrong with the last one asked.
    error: RwSignal<String>,
}

impl crate::ext::State for Providers {
    fn new(ui: Ui) -> Providers {
        Providers {
            form: Form::new(ui),
            items: ui.scope.create_rw_signal(Vec::new()),
            allowed: ui.scope.create_rw_signal(true),
            choice: ui.scope.create_rw_signal(Choice::Anthropic),
            name: ui.scope.create_rw_signal(String::new()),
            url: ui.scope.create_rw_signal(String::new()),
            speaks: ui.scope.create_rw_signal(Speaks::Chat),
            key: ui.scope.create_rw_signal(String::new()),
            busy: ui.scope.create_rw_signal(None),
            error: ui.scope.create_rw_signal(String::new()),
        }
    }
}

fn set_if_new<T: PartialEq + 'static>(signal: RwSignal<T>, value: T) {
    if signal.with_untracked(|v| *v != value) {
        signal.set(value);
    }
}

/// Opens the dialog, its list asked for again.
pub fn open(ui: Ui, send: &Send) {
    let st = ui.state::<Providers>();
    send(json!({"type": "providers"}));
    set_if_new(st.error, String::new());
    st.form.show(None);
}

/// "Add and list its models" (or Enter): asked of the driver when the form holds a provider.
fn add(ui: Ui, send: &Send) {
    let st = ui.state::<Providers>();
    if st.busy.with_untracked(Option::is_some) || !st.allowed.get_untracked() {
        return;
    }
    let msg = st.key.with_untracked(|k| {
        add_message(
            st.choice.get_untracked(),
            &st.name.get_untracked(),
            &st.url.get_untracked(),
            st.speaks.get_untracked(),
            k,
        )
    });
    match msg {
        Ok(m) => {
            let name = m["name"].as_str().unwrap_or("").to_string();
            send(m);
            st.key.set(String::new());
            set_if_new(st.error, String::new());
            st.busy.set(Some(name));
        }
        Err(why) => set_if_new(st.error, why.to_string()),
    }
}

/// One provider's row: its name and API, its address, its key and models, and the button that removes it.
fn row(ui: Ui, send: Send, r: Row) -> AnyView {
    let t = ui.colors;
    let (key, missing) = r.key_words();
    let speaks = r.speaks();
    let head = h_stack((
        label({
            let n = r.name.clone();
            move || n.clone()
        })
        .style(move |s| bold(s).font_size(13.0)),
        label(move || speaks).style(move |s| s.font_size(12.0).color(t.get().desc)),
    ))
    .style(|s| s.gap(8.0).items_center());
    let url = label({
        let u = r.url.clone();
        move || u.clone()
    })
    .style(move |s| s.font_size(12.0).color(t.get().desc).text_overflow(TextOverflow::Ellipsis).min_width(0.0));
    let foot = h_stack((
        label(move || key.clone())
            .style(move |s| s.font_size(12.0).flex_shrink(0.0_f32).apply_if(missing, |s| s.color(t.get().error))),
        label({
            let m = r.models_words();
            move || m.clone()
        })
        .style(move |s| s.font_size(12.0).color(t.get().desc).text_overflow(TextOverflow::Ellipsis).min_width(0.0)),
    ))
    .style(|s| s.gap(12.0).min_width(0.0));
    let name = r.name.clone();
    let remove = tooltip(action(t, "trash", move || send(json!({"type": "provider_remove", "name": name}))), || {
        label(|| "Remove")
    });
    h_stack((v_stack((head, url, foot)).style(|s| s.gap(2.0).flex_grow(1.0_f32).min_width(0.0)), remove))
        .style(move |s| {
            let c = t.get();
            s.width_full()
                .padding_vert(6.0)
                .padding_horiz(8.0)
                .gap(8.0)
                .items_center()
                .border_bottom(1.0)
                .border_color(c.widget_border)
        })
        .into_any()
}

fn host(ui: Ui, send: Send) -> AnyView {
    let (t, st) = (ui.colors, ui.state::<Providers>());
    let f = st.form;
    // Closed: the key typed and not sent is forgotten.
    create_effect(move |_| {
        if !f.open.get() && st.key.with_untracked(|k| !k.is_empty()) {
            st.key.set(String::new());
        }
    });
    let list = {
        let send = send.clone();
        let rows = dyn_stack(move || st.items.get(), Clone::clone, move |r| row(ui, send.clone(), r))
            .style(|s| s.flex_col().width_full());
        let none = label(|| "No other providers yet.").style(move |s| {
            s.font_size(12.0).color(t.get().desc).padding(8.0).apply_if(st.items.with(|i| !i.is_empty()), |s| s.hide())
        });
        v_stack((scroll(rows).style(|s| s.max_height(200.0).width_full()), none))
            .style(move |s| {
                let c = t.get();
                s.width_full().border(1.0).border_radius(4.0).border_color(c.widget_border)
            })
            .into_any()
    };
    let banned = label(|| {
        "Arena's admins do not let Code Arena use other providers (Admin → Settings → Code Arena): only Arena's \
         models are offered."
    })
    .style(move |s| {
        s.font_size(12.0)
            .line_height(1.4)
            .color(t.get().error)
            .text_overflow(TextOverflow::Wrap)
            .apply_if(st.allowed.get(), |s| s.hide())
    })
    .into_any();
    let go = {
        let send = send.clone();
        move || add(ui, &send)
    };
    let choices = floem::views::h_stack_from_iter(Choice::ALL.map(|c| {
        floem::views::container(f.radio(ui, st.choice, c, c.words(), "")).style(|s| s.width_pct(33.0)).into_any()
    }))
    .style(|s| s.flex_wrap(FlexWrap::Wrap).row_gap(8.0).width_full());
    let choices = f.named(ui, "Add a provider", choices.into_any());
    let other = move || st.choice.get() == Choice::Other;
    let name = f
        .named(ui, "Name", f.input(ui, st.name, "Lower-case, as in name:model", go.clone()))
        .style(move |s| s.apply_if(!other(), |s| s.hide()))
        .into_any();
    let url = f
        .named(ui, "Address", f.input(ui, st.url, "https://…", go.clone()))
        .style(move |s| s.apply_if(!other(), |s| s.hide()))
        .into_any();
    let speaks = f
        .named(
            ui,
            "It speaks",
            h_stack((
                f.radio(ui, st.speaks, Speaks::Chat, "Chat completions", "OpenAI's API, as most do"),
                f.radio(ui, st.speaks, Speaks::Messages, "Messages API", "Anthropic's API"),
            ))
            .style(|s| s.gap(16.0).width_full())
            .into_any(),
        )
        .style(move |s| s.apply_if(!other(), |s| s.hide()))
        .into_any();
    let key = f.named(
        ui,
        "API key",
        v_stack((
            f.secret(ui, st.key, go.clone()),
            label(move || key_hint(st.choice.get())).style(move |s| s.font_size(12.0).color(t.get().desc)),
        ))
        .style(|s| s.gap(4.0).width_full())
        .into_any(),
    );
    let error = label(move || st.error.get())
        .style(move |s| {
            s.font_size(12.0)
                .line_height(1.4)
                .color(t.get().error)
                .text_overflow(TextOverflow::Wrap)
                .apply_if(st.error.with(String::is_empty), |s| s.hide())
        })
        .into_any();
    let able = move || st.allowed.get() && st.busy.with(Option::is_none);
    let buttons = form::buttons(vec![
        form::button(ui, || "Close".into(), false, || true, move || f.close()),
        form::button(
            ui,
            move || {
                if st.busy.with(Option::is_some) {
                    "Listing its models…".into()
                } else {
                    "Add and list its models".into()
                }
            },
            true,
            able,
            go,
        ),
    ]);
    f.host(
        ui,
        || "Other providers".into(),
        || "Models of your own keys, beside Arena's: the model picker offers them as provider:model.".into(),
        vec![list, banned, choices, name, url, speaks, key, error],
        buttons,
    )
}

/// The plug beside the pickers.
fn plug(ui: Ui, send: Send) -> AnyView {
    tooltip(action(ui.colors, "plug", move || open(ui, &send)), || label(|| "Other Providers…")).into_any()
}

/// Its command, its row in the chat's More menu, the plug and the dialog's layer, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    commands::add(command(OPEN, "Chat", "Other Providers…", open));
    menus::add(Menu::AgentMore, menus::item(OPEN).group("3_models").order(10));
    slots::add(Slot::BoxLeft, "providers", 20, plug);
    layers::add(Place::Over, "providers", 30, |ui, send| host(ui, send).into_any());
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let st = ui.state::<Providers>();
    match m["type"].as_str().unwrap_or("") {
        "providers" => {
            set_if_new(st.items, m["items"].as_array().into_iter().flatten().filter_map(Row::of).collect());
            set_if_new(st.allowed, m["allowed"] != false);
            true
        }
        "provider_added" => {
            let name = m["name"].as_str().unwrap_or("");
            if st.busy.with_untracked(|b| b.as_deref() == Some(name)) {
                st.busy.set(None);
                if m["ok"] == true {
                    set_if_new(st.name, String::new());
                    set_if_new(st.url, String::new());
                } else {
                    set_if_new(st.error, m["error"].as_str().unwrap_or("It was not added.").to_string());
                }
            }
            true
        }
        "provider_removed" => {
            if m["ok"] != true {
                set_if_new(st.error, m["error"].as_str().unwrap_or("").to_string());
            }
            true
        }
        "refused" if m["of"] == "provider_add" => {
            if st.busy.with_untracked(Option::is_some) {
                st.busy.set(None);
            }
            set_if_new(st.error, m["reason"].as_str().unwrap_or("").to_string());
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_says_its_api_its_key_and_four_models() {
        let r = Row::of(&json!({"name": "anthropic", "kind": "anthropic", "url": "https://api.anthropic.com",
            "key": "env", "key_env": "ANTHROPIC_API_KEY", "models": ["a", "b", "c", "d"], "more": 3}))
        .expect("a row");
        assert_eq!(r.speaks(), "Messages API");
        assert_eq!(r.key_words(), ("Key from ANTHROPIC_API_KEY".to_string(), false));
        assert_eq!(r.models_words(), "a, b, c, d and 3 more");
        let none = Row::of(&json!({"name": "gpu", "kind": "openai", "key": "none"})).expect("a row");
        assert_eq!(
            (none.speaks(), none.key_words().1, none.models_words()),
            ("Chat completions", true, "No models listed".into())
        );
        assert_eq!(
            Row::of(&json!({"name": "x", "key": "here"})).map(|r| r.key_words().0),
            Some("Key kept here".into())
        );
        let local = Row::of(&json!({"name": "local", "url": "http://127.0.0.1:8080", "key": "none"})).expect("a row");
        assert_eq!(local.key_words(), ("No key needed".to_string(), false), "an API nearby needs no key");
        assert!(Row::of(&json!({"kind": "openai"})).is_none(), "a row without its name is not one");
    }

    #[test]
    fn a_known_provider_is_asked_by_its_name_and_its_key_only_when_given() {
        let m = add_message(Choice::DeepSeek, "ignored", "", Speaks::Chat, "").expect("asked");
        assert_eq!(m, json!({"type": "provider_add", "name": "deepseek"}));
        let k = add_message(Choice::Anthropic, "", "", Speaks::Chat, " fake-key ").expect("asked");
        assert_eq!(k["api_key"], "fake-key");
        assert!(k.get("key").is_none(), "the key is never under `key`, which the driver says back");
        assert_eq!(key_hint(Choice::Mistral), "From MISTRAL_API_KEY (leave empty)");
    }

    #[test]
    fn another_api_needs_a_lower_case_name_and_an_address() {
        let ask = |n: &str, u: &str| add_message(Choice::Other, n, u, Speaks::Messages, "");
        assert!(ask("My API", "https://x").is_err());
        assert!(ask("gpu", "gpu:8000").is_err());
        let m = ask(" gpu ", " http://gpu:8000 ").expect("asked");
        assert_eq!(m, json!({"type": "provider_add", "name": "gpu", "url": "http://gpu:8000", "kind": "anthropic"}));
        let chat = add_message(Choice::Other, "vllm", "https://v.example", Speaks::Chat, "").expect("asked");
        assert_eq!(chat["kind"], "openai");
    }
}
