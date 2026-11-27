//! A request in the thread: its words in a bubble on the right, as VS Code's chat draws the person's request, with
//! what it was sent with around it.
//!
//! Over the bubble: the part of an answer it replies to (two lines, "Show all" for the rest, and "Go to the quoted
//! answer", which brings that answer into view and flashes it), and "Sent while it worked: read at its next step"
//! for a message the running answer read. In the bubble: the words as they were typed, apart from what was
//! attached with @, and a card for the skill it ran. Under it: "Sent with it", a chip for each file and its lines
//! (opened at them). A note of Code Arena's own (a goal's next turn, a loop's report) is a centred line
//! "Code Arena: …" instead of a bubble.

use std::cell::RefCell;
use std::time::Duration;

use floem::action::exec_after;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect};
use floem::style::{CursorStyle, FlexWrap, TextOverflow};
use floem::views::{Decorators, container, empty, h_stack, h_stack_from_iter, label, v_stack};
use floem::{AnyView, IntoView, View};
use serde_json::Value;

use super::slots::{ItemSlot, add_item};
use crate::icons::icon;
use crate::model::{Item, Kind, Question};
use crate::state::{Send, Ui};

/// A request's bubble, or a note of Code Arena's as its line.
pub(super) fn bubble(ui: Ui, question: Question) -> AnyView {
    let t = ui.colors;
    if question.note {
        let words = note_line(&question.text);
        return container(label(move || words.clone()).style(move |s| {
            s.color(t.get().desc).font_size(12.0).text_overflow(TextOverflow::Wrap).max_width_pct(85.0)
        }))
        .style(|s| s.width_full().flex_row().justify_center())
        .into_any();
    }
    let words = shown(&question).to_string();
    let skill = question.skill.clone();
    let card = (!skill.is_empty()).then(|| skill_card(ui, skill));
    let inside = v_stack((
        label(move || words.clone())
            .style(move |s| s.color(t.get().fg).line_height(1.5).text_overflow(TextOverflow::Wrap)),
        match card {
            Some(c) => c.into_any(),
            None => empty().style(|s| s.hide()).into_any(),
        },
    ))
    .style(move |s| {
        let c = t.get();
        s.padding_horiz(12.0)
            .padding_vert(8.0)
            .border_radius(8.0)
            .background(c.chat_request)
            .gap(6.0)
            .max_width_pct(85.0)
    });
    h_stack((empty().style(|s| s.flex_grow(1.0_f32).min_width(40.0)), inside)).style(|s| s.width_full()).into_any()
}

/// The words a request's bubble shows: as typed, apart from what was attached with @ (the words the model read when
/// nothing was attached).
fn shown(q: &Question) -> &str {
    if q.typed.is_empty() { &q.text } else { &q.typed }
}

/// A note of Code Arena's as its line says it: "Code Arena: " and its first line ("…" when it goes on).
fn note_line(text: &str) -> String {
    let rest = text.strip_prefix("(Code Arena").unwrap_or(text);
    let rest = rest.strip_prefix(':').or_else(|| rest.strip_prefix(')')).unwrap_or(rest).trim_start();
    let mut lines = rest.lines();
    let first = lines.next().unwrap_or("").trim_end();
    let more = lines.any(|l| !l.trim().is_empty() && l.trim() != ")");
    let first = if more { first } else { first.strip_suffix(')').unwrap_or(first) };
    format!("Code Arena: {first}{}", if more { " …" } else { "" })
}

/// The card of the skill a request ran, in its bubble.
fn skill_card(ui: Ui, name: String) -> impl IntoView {
    let t = ui.colors;
    let words = format!("Used the skill {name}");
    h_stack((
        icon("sparkle", 14.0).style(move |s| s.color(t.get().icon)),
        label(move || words.clone()).style(|s| s.font_size(12.0).text_ellipsis()),
    ))
    .style(move |s| {
        let c = t.get();
        s.gap(6.0)
            .items_center()
            .padding_horiz(6.0)
            .padding_vert(2.0)
            .border(1.0)
            .border_color(c.widget_border)
            .border_radius(4.0)
            .color(c.desc)
            .max_width_full()
    })
}

/// The request an item of the thread is, when it is one (none for a note: nothing is drawn around it).
fn request(item: &Item) -> Option<&Question> {
    match &item.kind {
        Kind::User(q) | Kind::Queued { question: q, .. } if !q.note => Some(q),
        _ => None,
    }
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    add_item(ItemSlot::QuestionTop, "question.quote", 10, |ui, _send, item| match request(item) {
        Some(q) if !q.quote.is_empty() => quote(ui, item.id, q.quote.clone()).into_any(),
        _ => empty().style(|s| s.hide()).into_any(),
    });
    add_item(ItemSlot::QuestionTop, "question.taken_in", 20, |ui, _send, item| match request(item) {
        Some(q) if q.taken_in => {
            let t = ui.colors;
            label(|| "Sent while it worked: read at its next step")
                .style(move |s| s.font_size(11.0).color(t.get().desc).margin_bottom(4.0))
                .into_any()
        }
        _ => empty().style(|s| s.hide()).into_any(),
    });
    add_item(ItemSlot::QuestionFoot, "question.files", 10, |ui, send, item| match request(item) {
        Some(q) if !q.files.is_empty() => sent_with(ui, send, q).into_any(),
        _ => empty().style(|s| s.hide()).into_any(),
    });
}

/// The quote a request replies to, over its bubble: two lines of it, "Show all" for the rest, and the way to the
/// answer it is from.
fn quote(ui: Ui, at: u64, text: String) -> impl IntoView {
    let t = ui.colors;
    let all = ui.scope.create_rw_signal(false);
    let short = two_lines(&text);
    let long = short != text;
    let words = text.clone();
    let toggle = label(move || if all.get() { "Show less" } else { "Show all" })
        .on_click_stop(move |_| all.update(|a| *a = !*a))
        .style(move |s| {
            s.font_size(11.0).color(t.get().link).cursor(CursorStyle::Pointer).apply_if(!long, |s| s.hide())
        });
    let go = label(|| "Go to the quoted answer")
        .on_click_stop(move |_| go_to_quote(ui, at, &text))
        .style(move |s| s.font_size(11.0).color(t.get().link).cursor(CursorStyle::Pointer));
    v_stack((
        label(move || if all.get() { words.clone() } else { short.clone() })
            .style(move |s| s.font_size(12.0).line_height(1.5).color(t.get().desc).text_overflow(TextOverflow::Wrap)),
        h_stack((toggle, go)).style(|s| s.gap(12.0)),
    ))
    .style(move |s| {
        let c = t.get();
        s.gap(2.0)
            .padding_left(8.0)
            .border_left(2.0)
            .border_color(c.quote_border)
            .max_width_pct(85.0)
            .margin_bottom(4.0)
    })
}

/// A quote as its two lines show it: its first two lines, at most 140 characters, "…" where it was cut.
fn two_lines(text: &str) -> String {
    let lines: Vec<&str> = text.lines().take(2).collect();
    let mut short: String = lines.join("\n").chars().take(140).collect();
    if short.trim_end() != text.trim_end() {
        short = format!("{}…", short.trim_end());
    }
    short
}

/// "Sent with it" under a request: a chip for each file and its lines, opened at them.
fn sent_with(ui: Ui, send: Send, q: &Question) -> impl IntoView {
    let t = ui.colors;
    let chips: Vec<AnyView> = q
        .files
        .iter()
        .map(|f| {
            let (path, start) = (f.path.clone(), f.start);
            let words = piece_name(&f.path, f.start, f.end);
            let send = send.clone();
            h_stack((icon("file", 14.0), label(move || words.clone()).style(|s| s.font_size(12.0).text_ellipsis())))
                .on_click_stop(move |_| {
                    if start > 0 {
                        ui.open_at(&path, start, &send);
                    } else {
                        ui.open(&path, &send);
                    }
                })
                .style(move |s| {
                    let c = t.get();
                    s.gap(4.0)
                        .items_center()
                        .padding_horiz(6.0)
                        .padding_vert(1.0)
                        .border(1.0)
                        .border_color(c.widget_border)
                        .border_radius(4.0)
                        .color(c.link)
                        .cursor(CursorStyle::Pointer)
                        .max_width_full()
                        .hover(|s| s.background(c.list_hover))
                })
                .into_any()
        })
        .collect();
    v_stack((
        label(|| "Sent with it").style(move |s| s.font_size(11.0).color(t.get().desc)),
        h_stack_from_iter(chips).style(|s| s.gap(4.0).flex_wrap(FlexWrap::Wrap).justify_end()),
    ))
    .style(|s| s.items_end().gap(2.0).margin_top(4.0).max_width_pct(85.0))
}

/// A file and its lines as a chip says them: "parser.rs:10-20", "parser.rs:7", or the file's name alone.
fn piece_name(path: &str, start: usize, end: usize) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match (start, end) {
        (0, _) => name.to_string(),
        (s, e) if e > s => format!("{name}:{s}-{e}"),
        (s, _) => format!("{name}:{s}"),
    }
}

/// The answer a quote is from: the last answer before the request (item `at`) whose words have it, or the last
/// answer before the request when none has it word for word. Marks and spaces are not compared (a quote is taken
/// from the answer as drawn).
fn quoted_answer(items: &[Item], at: u64, quote: &str) -> Option<u64> {
    let end = items.iter().position(|i| i.id == at).unwrap_or(items.len());
    let answers: Vec<(u64, String)> = items[..end]
        .iter()
        .rev()
        .filter_map(|i| match &i.kind {
            Kind::Assistant(a) => Some((i.id, a.text.with_untracked(|t| bare(t)))),
            _ => None,
        })
        .collect();
    let wanted = bare(quote);
    answers.iter().find(|(_, t)| !wanted.is_empty() && t.contains(&wanted)).or(answers.first()).map(|(id, _)| *id)
}

/// Words with Markdown's marks taken out and their spaces made one.
fn bare(text: &str) -> String {
    let marks: String = text.chars().filter(|c| !matches!(c, '*' | '_' | '`' | '#' | '>' | '[' | ']')).collect();
    marks.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A window's thread (its items' signal) and the answer flashed in it.
type Flashed = (RwSignal<Vec<Item>>, RwSignal<Option<u64>>);

thread_local! {
    /// The answer flashed for a quote, by the window's thread it is in (its items' signal).
    static FLASH: RefCell<Vec<Flashed>> = const { RefCell::new(Vec::new()) };
}

/// The answer being flashed in this window's thread (none: none is).
fn flash(ui: Ui) -> RwSignal<Option<u64>> {
    FLASH.with(|f| {
        let mut all = f.borrow_mut();
        if let Some((_, s)) = all.iter().find(|(items, _)| *items == ui.items) {
            return *s;
        }
        let s = ui.scope.create_rw_signal(None);
        all.push((ui.items, s));
        s
    })
}

/// Brings the answer a request's quote is from into view, and flashes it for a moment.
fn go_to_quote(ui: Ui, at: u64, quote: &str) {
    let Some(answer) = ui.items.with_untracked(|l| quoted_answer(l, at, quote)) else { return };
    let f = flash(ui);
    f.set(Some(answer));
    exec_after(Duration::from_millis(1200), move |_| {
        if f.get_untracked() == Some(answer) {
            f.set(None);
        }
    });
}

/// An answer of the thread as one that can be gone to and flashed (`go_to_quote`).
pub(super) fn flashable(ui: Ui, id: u64, view: AnyView) -> AnyView {
    let t = ui.colors;
    let f = flash(ui);
    let held = container(view).style(move |s| {
        let lit = f.get() == Some(id);
        s.width_full().min_width(0.0).border_radius(6.0).apply_if(lit, |s| s.background(t.get().chat_flash))
    });
    let me = held.id();
    create_effect(move |_| {
        if f.get() == Some(id) {
            // Once it is laid out with the flash.
            exec_after(Duration::from_millis(0), move |_| me.scroll_to(None));
        }
    });
    held.into_any()
}

/// The session's messages that are a request's, taken in: one sent, and one sent while it worked that the running
/// answer read (its waiting bubble becomes the request, marked so). Never taken away from the rest of the window
/// (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    match m["type"].as_str().unwrap_or("") {
        "user" => ui.push(Kind::User(Question::from_json(m))),
        "notice" => {
            if let Some(text) = read_meanwhile(m["text"].as_str().unwrap_or("")) {
                taken_in(ui, text);
            }
        }
        _ => {}
    }
    false
}

/// The words of a message the running answer read at its next step, from the notice that says so.
fn read_meanwhile(notice: &str) -> Option<&str> {
    notice.strip_prefix("› ")?.strip_suffix("(sent while it worked)").map(str::trim_end)
}

/// A message read by the running answer: the bubble that waited with these words (the first of them) becomes the
/// request it is, marked as read meanwhile; with none waiting (sent from another window) it is added.
fn taken_in(ui: Ui, text: &str) {
    let id = ui.id();
    ui.items.update(|l| {
        let waiting = l
            .iter()
            .position(|i| matches!(&i.kind, Kind::Queued { question, later: false, .. } if question.text == text));
        let mut question = match waiting.map(|at| &l[at].kind) {
            Some(Kind::Queued { question, .. }) => question.clone(),
            _ => Question::of(text),
        };
        question.taken_in = true;
        let item = Item { id, kind: Kind::User(question) };
        match waiting {
            Some(at) => l[at] = item,
            None => l.push(item),
        }
    });
}

/// A request the session kept, as an item.
pub(super) fn kept(ui: Ui, msg: &Value) -> Option<Item> {
    Some(Item { id: ui.id(), kind: Kind::User(Question::from_json(msg)) })
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use serde_json::json;

    use super::*;

    #[test]
    fn a_long_quote_shows_two_lines_until_shown_all() {
        assert_eq!(two_lines("one line"), "one line");
        assert_eq!(two_lines("one\ntwo\nthree"), "one\ntwo…");
        assert_eq!(two_lines(&"a".repeat(150)).chars().count(), 141);
    }

    #[test]
    fn a_note_of_code_arenas_is_said_by_its_first_line() {
        assert_eq!(
            note_line("(Code Arena: loop 2's run 1 (tests) reports:\nall pass)"),
            "Code Arena: loop 2's run 1 (tests) reports: …"
        );
        assert_eq!(note_line("(Code Arena: the server docs joined.)"), "Code Arena: the server docs joined.");
        assert_eq!(note_line("(Code Arena) Goal: tests pass\n\nWork toward it"), "Code Arena: Goal: tests pass …");
    }

    #[test]
    fn a_bubble_shows_the_words_as_typed_and_a_chip_its_file_and_lines() {
        let q = Question::from_json(
            &json!({"text": "@src/a.rs\n```\nfn a() {}\n```\nexplain", "typed": "@src/a.rs explain"}),
        );
        assert_eq!(shown(&q), "@src/a.rs explain");
        assert_eq!(shown(&Question::of("hello")), "hello");
        assert_eq!(piece_name("src/parser.rs", 10, 20), "parser.rs:10-20");
        assert_eq!(piece_name("src\\parser.rs", 7, 7), "parser.rs:7");
        assert_eq!(piece_name("README.md", 0, 0), "README.md");
    }

    #[test]
    fn a_message_read_meanwhile_takes_the_place_of_its_waiting_bubble() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let take = |m: Value| ui.take(&m, &send);
        take(json!({"type": "user", "text": "first", "q": 1}));
        take(json!({"type": "queued", "text": "and the tests"}));
        take(json!({"type": "queued", "text": "/explain", "later": true}));
        let waited = ui.items.with_untracked(|l| l[1].id);
        take(json!({"type": "notice", "text": "› and the tests  (sent while it worked)"}));
        take(json!({"type": "notice", "text": "› from elsewhere  (sent while it worked)"}));
        let seen: Vec<(String, bool, bool)> = ui.items.with_untracked(|l| {
            l.iter()
                .filter_map(|i| match &i.kind {
                    Kind::User(q) => Some((q.text.clone(), q.taken_in, false)),
                    Kind::Queued { question, .. } => Some((question.text.clone(), false, true)),
                    _ => None,
                })
                .collect()
        });
        assert_eq!(
            seen,
            [
                ("first".into(), false, false),
                ("and the tests".into(), true, false),
                ("/explain".into(), false, true),
                ("from elsewhere".into(), true, false),
            ]
        );
        // Drawn again under a number of its own: the thread builds it as a request now.
        assert_ne!(ui.items.with_untracked(|l| l[1].id), waited);
    }

    #[test]
    fn a_quote_goes_to_the_answer_it_is_from() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let take = |m: Value| ui.take(&m, &send);
        take(json!({"type": "user", "text": "one", "q": 1}));
        take(json!({"type": "text", "delta": "The **parser** reads\nthe tokens."}));
        take(json!({"type": "turn_end", "ok": true}));
        take(json!({"type": "user", "text": "two", "q": 2}));
        take(json!({"type": "text", "delta": "Something else."}));
        take(json!({"type": "turn_end", "ok": true}));
        take(json!({"type": "user", "text": "why", "q": 3, "quote": "parser reads the tokens"}));
        let (first, second, asked) = ui.items.with_untracked(|l| {
            let answers: Vec<u64> = l.iter().filter(|i| matches!(i.kind, Kind::Assistant(_))).map(|i| i.id).collect();
            (answers[0], answers[1], l.last().map(|i| i.id).unwrap_or(0))
        });
        assert_eq!(ui.items.with_untracked(|l| quoted_answer(l, asked, "parser reads the tokens")), Some(first));
        // Not word for word in any: the last answer before the request.
        assert_eq!(ui.items.with_untracked(|l| quoted_answer(l, asked, "not there")), Some(second));
        assert_eq!(ui.items.with_untracked(|l| quoted_answer(l, first, "x")), None);
    }

    #[test]
    fn a_kept_note_is_kept_as_a_note() {
        let ui = Ui::new();
        let item = kept(ui, &json!({"role": "user", "text": "(Code Arena) Goal: x", "note": true})).expect("kept");
        assert!(matches!(item.kind, Kind::User(q) if q.note));
    }
}
