//! A request's actions, under its bubble while the pointer is over it, as VS Code's chat has them on a request:
//!
//! - Copy: the words as they were typed (a check for 1.5 s once copied).
//! - Edit: the words in a box in the request's place, with the quote it replies to as a chip (taken away with ×),
//!   Cancel and Send. Sent, the conversation and the files go back to before it and it is asked in its new words.
//! - Answer again: a menu: the same, shorter, longer, with another model or other thinking (for that answer only),
//!   and with the files as they were (when its turn changed files). The files stay as they are otherwise.
//! - Rewind to here: "Rewind to here?" with what goes ("N messages go", the files put back or removed), and the
//!   choice of the conversation and the code, only the conversation or only the code (the code ones only when
//!   this run kept the files as they were). Once the conversation is back, the request's words (and its quote) are
//!   in the box again.
//!
//! Only a request the session numbered (`q`) can be edited, answered again or rewound to, and none while an answer
//! is written: "Wait for the answer, or stop it, to ask again." The session's side is in the driver's
//! `ide_chat/rewind.rs` (`rewind_preview`, `rewound`, `again`'s fields).

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use floem::action::exec_after;
use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, container, dyn_container, empty, h_stack, label, text_input, tooltip, v_stack};
use floem::{AnyView, IntoView, View, ViewId};
use serde_json::{Value, json};

use super::slots::{ItemSlot, add_item};
use crate::chat_bridge;
use crate::ext::State;
use crate::icons::icon;
use crate::model::{Item, Kind, Question};
use crate::state::{Send, Ui};
use crate::widgets::{self, dialog, popup, toast};

/// What is said when one is asked while an answer is written.
const BUSY: &str = "Wait for the answer, or stop it, to ask again.";

/// The words the menu and the dialog use.
const REWIND: &str = "Rewind to here";

/// A rewind asked about: the key its preview and its end are said with, and the words to give back to the box.
#[derive(Clone, Debug, PartialEq)]
struct Asked {
    key: String,
    q: u64,
    words: String,
    quote: String,
}

/// The actions' state in a window.
#[derive(Clone, Copy)]
struct Actions {
    /// The request the pointer is over.
    hovered: RwSignal<Option<u64>>,
    /// The request being edited, its words and the quote it keeps (empty: taken away).
    editing: RwSignal<Option<u64>>,
    words: RwSignal<String>,
    quote: RwSignal<String>,
    /// The request whose words were just copied.
    copied: RwSignal<Option<u64>>,
    /// The rewind waiting on the session.
    asked: RwSignal<Option<Asked>>,
    next: RwSignal<u64>,
}

impl State for Actions {
    fn new(ui: Ui) -> Self {
        Actions {
            hovered: ui.scope.create_rw_signal(None),
            editing: ui.scope.create_rw_signal(None),
            words: ui.scope.create_rw_signal(String::new()),
            quote: ui.scope.create_rw_signal(String::new()),
            copied: ui.scope.create_rw_signal(None),
            asked: ui.scope.create_rw_signal(None),
            next: ui.scope.create_rw_signal(1),
        }
    }
}

/// A way to answer again, from the menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Again {
    Same,
    Shorter,
    Longer,
    Model(String),
    Thinking(String),
    /// The same, the files put back as they were before it.
    Files,
}

/// What asks question `q` again that way.
pub fn again_message(q: u64, how: &Again) -> Value {
    let mut m = json!({"type": "again", "q": q, "files": false});
    match how {
        Again::Same => {}
        Again::Shorter => m["length"] = json!("shorter"),
        Again::Longer => m["length"] = json!("longer"),
        Again::Model(name) => m["model"] = json!(name),
        Again::Thinking(level) => m["thinking"] = json!(level),
        Again::Files => m["files"] = json!(true),
    }
    m
}

/// What asks question `q` again in new words, the quote kept or not (an empty one is taken away). None when there
/// are no words.
pub fn edit_message(q: u64, words: &str, quote: &str) -> Option<Value> {
    let words = words.trim();
    (!words.is_empty()).then(|| json!({"type": "again", "q": q, "text": words, "quote": quote, "files": true}))
}

/// The dialog's choices, by what each says to the session: the code ones only when its files were kept.
pub fn rewind_choices(checkpoint: bool) -> Vec<(&'static str, &'static str)> {
    let all = [("Conversation and code", "both"), ("Only the conversation", "chat"), ("Only the code", "code")];
    all.into_iter().filter(|(_, what)| checkpoint || *what == "chat").collect()
}

/// What the dialog says goes, from the session's `rewind_preview`.
pub fn preview_detail(m: &Value) -> String {
    let mut lines = Vec::new();
    let n = m["messages"].as_u64().unwrap_or(0);
    lines.push(match n {
        1 => "1 message goes: this question.".to_string(),
        n => format!("{n} messages go: this question and all after it."),
    });
    let files = m["files"].as_array().cloned().unwrap_or_default();
    if m["checkpoint"] != true {
        lines.push("The code cannot be put back: its files were not kept in this run.".into());
    } else if files.is_empty() {
        lines.push("No file would change.".into());
    } else {
        let one = |f: &Value| {
            let what = if f["removed"] == true { "removed" } else { "put back" };
            format!("{} ({what})", f["path"].as_str().unwrap_or(""))
        };
        let n = files.len();
        let names: Vec<String> = files.iter().take(8).map(one).collect();
        let more = if n > 8 { format!(" and {} more", n - 8) } else { String::new() };
        let s = if n == 1 { "" } else { "s" };
        lines.push(format!("{n} file{s} would change: {}{more}.", names.join(", ")));
    }
    let lost: Vec<&str> =
        m["lost"].as_array().map(|l| l.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if !lost.is_empty() {
        lines.push(format!("Too big to have been kept, so not put back: {}.", lost.join(", ")));
    }
    lines.push("What commands changed is not put back: git has it.".into());
    lines.join("\n")
}

/// Whether the turn of the request at `id` changed files: an edit of one of its answers did.
pub fn turn_changed_files(items: &[Item], id: u64) -> bool {
    let Some(at) = items.iter().position(|i| i.id == id) else { return false };
    items[at + 1..].iter().take_while(|i| !matches!(i.kind, Kind::User(_))).any(|i| match &i.kind {
        Kind::Assistant(a) => a.tools.with_untracked(|t| t.iter().any(|t| t.ok && t.diff.is_some())),
        _ => false,
    })
}

/// The request an item is, when it can have actions (a note has none).
fn request(item: &Item) -> Option<&Question> {
    match &item.kind {
        Kind::User(q) if !q.note => Some(q),
        _ => None,
    }
}

/// The words a request's bubble shows: as typed.
fn typed(q: &Question) -> String {
    if q.typed.is_empty() { q.text.clone() } else { q.typed.clone() }
}

/// A request of the thread as one its actions show over: the pointer coming in and going out of it.
pub(super) fn hovering(ui: Ui, id: u64, view: AnyView) -> AnyView {
    let a = ui.state::<Actions>();
    container(view)
        .on_event_cont(EventListener::PointerEnter, move |_| {
            if a.hovered.get_untracked() != Some(id) {
                a.hovered.set(Some(id));
            }
        })
        .on_event_cont(EventListener::PointerLeave, move |_| {
            if a.hovered.get_untracked() == Some(id) {
                a.hovered.set(None);
            }
        })
        .style(|s| s.width_full().min_width(0.0))
        .into_any()
}

/// What it adds to the window's registries, once at start: the actions under a request.
pub fn register(_ui: Ui, _send: &Send) {
    add_item(ItemSlot::QuestionFoot, "question.actions", 30, |ui, send, item| match request(item) {
        Some(q) => under(ui, send, item.id, q.clone()).into_any(),
        None => empty().style(|s| s.hide()).into_any(),
    });
}

/// Under a request: its actions, or the box it is edited in.
fn under(ui: Ui, send: Send, id: u64, q: Question) -> impl IntoView {
    let a = ui.state::<Actions>();
    let editing = create_memo(move |_| a.editing.get() == Some(id));
    dyn_container(
        move || editing.get(),
        move |on| if on { editor(ui, send.clone(), q.q).into_any() } else { row(ui, send.clone(), id, &q).into_any() },
    )
    .style(|s| s.width_full())
}

/// An action with its tooltip.
fn tipped(view: impl IntoView + 'static, tip: impl Fn() -> String + 'static) -> impl IntoView {
    let tip = Rc::new(tip);
    tooltip(view, move || {
        let tip = tip.clone();
        label(move || tip())
    })
}

/// The row of actions: at the right, shown while the pointer is over the request (its room always kept, so the
/// thread does not move).
fn row(ui: Ui, send: Send, id: u64, q: &Question) -> impl IntoView {
    let (a, t) = (ui.state::<Actions>(), ui.colors);
    let words = typed(q);
    let copied = create_memo(move |_| a.copied.get() == Some(id));
    let copy = {
        let words = words.clone();
        widgets::action_lit(
            t,
            move || if copied.get() { "check" } else { "copy" },
            move || {
                let _ = floem::Clipboard::set_contents(words.clone());
                a.copied.set(Some(id));
                exec_after(Duration::from_millis(1500), move |_| {
                    if a.copied.try_get_untracked() == Some(Some(id)) {
                        a.copied.set(None);
                    }
                });
            },
        )
    };
    let copy = tipped(copy, move || if copied.get_untracked() { "Copied".into() } else { "Copy".into() });
    let n = q.q;
    let numbered = n > 0;
    let busy = move || ui.running.get();
    // A question a compaction covers is not asked again: Edit and Answer again say so.
    let covered = move |asks: bool| asks && super::compaction::covers(ui, n);
    let covered_now = move |asks: bool| asks && super::compaction::covers_now(ui, n);
    // Each of the others, dimmed while an answer is written (and saying why when pressed).
    let gated = move |name: &'static str, tip: &'static str, asks: bool, run: Rc<dyn Fn()>| {
        let button = widgets::action(t, name, move || {
            if ui.running.get_untracked() {
                busy_said(ui)
            } else if covered_now(asks) {
                toast::info(ui, super::compaction::COVERED);
            } else {
                run()
            }
        });
        let dim = move || busy() || covered(asks);
        tipped(container(button).style(move |s| s.apply_if(dim(), |s| s.color(t.get().desc))), move || {
            if ui.running.get_untracked() {
                BUSY.into()
            } else if covered_now(asks) {
                super::compaction::COVERED.into()
            } else {
                tip.into()
            }
        })
        .style(move |s| s.apply_if(!numbered, |s| s.hide()))
    };
    let (quote, edit_words) = (q.quote.clone(), words.clone());
    let edit = gated(
        "edit",
        "Edit: ask it again in new words (what came after it goes, and the files are put back)",
        true,
        Rc::new(move || {
            a.words.set(edit_words.clone());
            a.quote.set(quote.clone());
            a.editing.set(Some(id));
        }),
    );
    let place: Rc<Cell<Option<ViewId>>> = Rc::new(Cell::new(None));
    let again = {
        let (send, place) = (send.clone(), place.clone());
        gated(
            "refresh",
            "Answer again",
            true,
            Rc::new(move || {
                if let Some(at) = place.get() {
                    again_menu(ui, send.clone(), id, n, at);
                }
            }),
        )
    };
    let again = container(again);
    place.set(Some(again.id()));
    let (quote, rewind_words) = (q.quote.clone(), words);
    let rewind =
        gated("discard", REWIND, false, Rc::new(move || ask_rewind(ui, &send, n, rewind_words.clone(), quote.clone())));
    let shown = create_memo(move |_| a.hovered.get() == Some(id));
    container(h_stack((copy, edit, again, rewind)).style(move |s| s.gap(2.0).apply_if(!shown.get(), |s| s.hide())))
        .style(|s| s.width_full().height(24.0).margin_top(2.0).flex_row().justify_end().items_center())
}

fn busy_said(ui: Ui) {
    toast::info(ui, BUSY);
}

/// The Answer again menu, under its button.
fn again_menu(ui: Ui, send: Send, id: u64, q: u64, at: ViewId) {
    let go = |how: Again| {
        let send = send.clone();
        move || send(again_message(q, &how))
    };
    let models: Vec<popup::Item> = ui
        .models
        .get_untracked()
        .into_iter()
        .map(|m| popup::entry(&m, go(Again::Model(m.clone()))).radio(m == ui.model.get_untracked()).into())
        .collect();
    let now = ui.thinking.get_untracked();
    let levels: Vec<popup::Item> = super::pickers::THINKING
        .iter()
        .map(|l| popup::entry(&super::pickers::capital(l), go(Again::Thinking(l.to_string()))).radio(*l == now).into())
        .collect();
    let changed = ui.items.with_untracked(|l| turn_changed_files(l, id));
    let mut items: Vec<popup::Item> = vec![
        popup::entry("Answer Again", go(Again::Same)).into(),
        popup::entry("Shorter", go(Again::Shorter)).into(),
        popup::entry("Longer", go(Again::Longer)).into(),
        popup::line(),
    ];
    if !models.is_empty() {
        items.push(popup::submenu("With Another Model", models).into());
    }
    items.push(popup::submenu("With Thinking", levels).into());
    items.push(popup::line());
    items.push(popup::entry("With the Files as They Were", go(Again::Files)).enabled(changed).into());
    popup::open(ui, popup::At::under(at), items);
}

/// Asks the session what going back to before question `q` would do; the dialog opens with its answer.
fn ask_rewind(ui: Ui, send: &Send, q: u64, words: String, quote: String) {
    let a = ui.state::<Actions>();
    let n = a.next.get_untracked();
    a.next.set(n + 1);
    let key = format!("rewind-{n}");
    a.asked.set(Some(Asked { key: key.clone(), q, words, quote }));
    send(json!({"type": "rewind_preview", "q": q, "key": key}));
}

/// "Rewind to here?" with what goes, and the choice of what goes back.
fn rewind_dialog(ui: Ui, send: Send, m: &Value, asked: Asked) {
    let choices = rewind_choices(m["checkpoint"] == true);
    let mut d = dialog::ask(&format!("{REWIND}?")).detail(&preview_detail(m)).kind(dialog::Kind::Warning);
    for (label, _) in &choices {
        d = d.button(label);
    }
    let a = ui.state::<Actions>();
    d.open(ui, move |answer| {
        let Some((_, what)) = answer.button.and_then(|b| choices.get(b)) else {
            a.asked.set(None);
            return;
        };
        send(json!({"type": "rewind", "q": asked.q, "what": what, "key": asked.key}));
    });
}

/// The editor in a request's place: its words in a box, the quote as a chip, Cancel and Send.
fn editor(ui: Ui, send: Send, q: u64) -> impl IntoView {
    let (a, t) = (ui.state::<Actions>(), ui.colors);
    let cancel = move || a.editing.set(None);
    let go = {
        let send = send.clone();
        Rc::new(move || {
            if ui.running.get_untracked() {
                busy_said(ui);
                return;
            }
            if let Some(m) = edit_message(q, &a.words.get_untracked(), &a.quote.get_untracked()) {
                a.editing.set(None);
                send(m);
            }
        })
    };
    let chip = h_stack((
        icon("quote", 14.0).style(move |s| s.color(t.get().icon).flex_shrink(0.0_f32)),
        label(move || a.quote.get().lines().next().unwrap_or("").chars().take(80).collect::<String>())
            .style(|s| s.font_size(12.0).text_ellipsis().min_width(0.0).flex_grow(1.0_f32)),
        widgets::action(t, "close", move || a.quote.set(String::new())),
    ))
    .style(move |s| {
        let c = t.get();
        s.gap(6.0)
            .items_center()
            .padding_left(6.0)
            .border(1.0)
            .border_radius(4.0)
            .border_color(c.widget_border)
            .color(c.desc)
            .max_width_full()
            .apply_if(a.quote.with(String::is_empty), |s| s.hide())
    });
    let input = text_input(a.words);
    let input_id = input.id();
    exec_after(Duration::from_millis(0), move |_| input_id.request_focus());
    let enter = go.clone();
    let input = input
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            match &k.key.logical_key {
                Key::Named(NamedKey::Enter) if !k.modifiers.shift() => enter(),
                Key::Named(NamedKey::Escape) => cancel(),
                _ => {}
            }
            // No key goes on to the workbench's own while the words are edited.
            EventPropagation::Stop
        })
        .style(move |s| {
            let c = t.get();
            let s = s
                .width_full()
                .padding_horiz(8.0)
                .padding_vert(6.0)
                .font_size(13.0)
                .border(1.0)
                .border_radius(4.0)
                .border_color(c.focus)
                .background(c.input_bg)
                .color(c.input_fg);
            crate::widgets::steady_input(s, c.input_bg, Some(c.focus))
        });
    let buttons = h_stack((
        label(|| "A new version of the question: what came after it goes.").style(move |s| {
            s.font_size(11.0).color(t.get().desc).flex_grow(1.0_f32).text_overflow(TextOverflow::Wrap).min_width(0.0)
        }),
        widgets::button(t, "Cancel", false, cancel),
        widgets::button(t, "Send", true, move || go()),
    ))
    .style(|s| s.gap(8.0).items_center().width_full());
    v_stack((chip, input, buttons))
        .style(move |s| {
            let c = t.get();
            s.gap(6.0).width_full().padding(8.0).border_radius(8.0).background(c.chat_request).cursor(CursorStyle::Text)
        })
        .on_event_stop(EventListener::PointerDown, |_| {})
}

/// The session's messages for the actions: the preview of a rewind asked from here, its end, and an again or a
/// rewind refused. Never taken away from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let a = ui.state::<Actions>();
    let key = m["key"].as_str().unwrap_or("");
    let mine = || a.asked.with_untracked(|w| w.as_ref().filter(|w| !key.is_empty() && w.key == key).cloned());
    match m["type"].as_str().unwrap_or("") {
        "rewind_preview" => {
            if let Some(asked) = mine() {
                rewind_dialog(ui, send.clone(), m, asked);
            }
        }
        "rewound" => {
            if let Some(asked) = mine() {
                a.asked.set(None);
                if m["chat"] == true {
                    if ui.draft.with_untracked(|d| d.trim().is_empty()) {
                        chat_bridge::put_in_box(ui, &asked.words);
                    }
                    if !asked.quote.is_empty() {
                        chat_bridge::came_back(ui, &json!({"quote": asked.quote}));
                    }
                    chat_bridge::focus(ui);
                    toast::info(ui, "Rewound: the question is back in the box, to send again or change.");
                } else {
                    toast::info(ui, "The files are as they were before the question.");
                }
            }
        }
        "refused" if matches!(m["of"].as_str(), Some("again" | "rewind")) => {
            a.asked.set(None);
            toast::warning(ui, m["reason"].as_str().filter(|r| !r.is_empty()).unwrap_or(BUSY));
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_way_of_answering_again_says_so_to_the_session() {
        assert_eq!(again_message(3, &Again::Same), json!({"type": "again", "q": 3, "files": false}));
        assert_eq!(again_message(3, &Again::Shorter)["length"], "shorter");
        assert_eq!(again_message(3, &Again::Longer)["length"], "longer");
        assert_eq!(again_message(3, &Again::Model("big".into()))["model"], "big");
        assert_eq!(again_message(3, &Again::Thinking("high".into()))["thinking"], "high");
        assert_eq!(again_message(3, &Again::Files)["files"], true);
        assert_eq!(
            edit_message(2, "  new words ", ""),
            Some(json!({"type": "again", "q": 2, "text": "new words", "quote": "", "files": true}))
        );
        assert_eq!(edit_message(2, "  ", "the part"), None);
    }

    #[test]
    fn the_code_can_be_put_back_only_when_its_files_were_kept() {
        let whats = |c| rewind_choices(c).into_iter().map(|(_, w)| w).collect::<Vec<_>>();
        assert_eq!(whats(true), ["both", "chat", "code"]);
        assert_eq!(whats(false), ["chat"]);
    }

    #[test]
    fn the_rewind_says_what_goes_and_what_the_files_become() {
        let m = json!({"messages": 4, "checkpoint": true, "lost": [],
            "files": [{"path": "src/a.rs", "removed": false}, {"path": "new.rs", "removed": true}]});
        let d = preview_detail(&m);
        assert!(d.starts_with("4 messages go: this question and all after it.\n"), "{d}");
        assert!(d.contains("2 files would change: src/a.rs (put back), new.rs (removed)."), "{d}");
        assert!(d.ends_with("What commands changed is not put back: git has it."));
        let d = preview_detail(&json!({"messages": 1, "checkpoint": false, "files": [], "lost": ["big.bin"]}));
        assert!(d.contains("1 message goes") && d.contains("cannot be put back") && d.contains("big.bin"), "{d}");
        assert!(
            preview_detail(&json!({"messages": 2, "checkpoint": true, "files": []})).contains("No file would change")
        );
    }

    #[test]
    fn a_turn_changed_files_when_an_edit_of_its_answer_did() {
        let ui = Ui::new();
        let kind = Kind::assistant(ui.scope);
        let Kind::Assistant(answer) = kind.clone() else { unreachable!() };
        let items = vec![
            Item { id: 1, kind: Kind::User(Question::of("first")) },
            Item { id: 2, kind },
            Item { id: 3, kind: Kind::User(Question::of("second")) },
        ];
        assert!(!turn_changed_files(&items, 1));
        answer.tools.update(|t| {
            t.push(crate::model::Tool { ok: true, diff: Some(Rc::new(Default::default())), ..Default::default() })
        });
        assert!(turn_changed_files(&items, 1));
        assert!(!turn_changed_files(&items, 3), "the next turn changed none");
    }
}
