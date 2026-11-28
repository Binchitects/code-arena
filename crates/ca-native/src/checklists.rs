//! Checklists, in the Plan view: the checklist on in the session's chat in Arena, "N/M done", and its items as boxes
//! to tick (the model ticks them with `checklist_check` as it goes through them, the person with a click), each with
//! how it was done or checked; Turn off. With none on: "Turn a checklist on", the person's own (Yours, each with
//! Edit and Delete) and the repository's (`.code-arena/checklists`), New Checklist (its name, its items one per
//! line), and why there are none when the session is not kept in Arena. The session's side is the driver's
//! `ide_plan.rs`.

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::editor::Editor;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::{
    Decorators, container, dyn_container, dyn_stack, empty, h_stack, label, scroll, text_editor, tooltip, v_stack,
};
use floem::{AnyView, IntoView};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::keymap::{Focus, TakesKeys};
use crate::layers::{self, Place};
use crate::state::{Send, Ui};
use crate::widgets::form::{self, Form};
use crate::widgets::{Theme, action, bold, button, caps, dialog, toast};

/// An item of the checklist on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub number: u64,
    pub text: String,
    pub done: bool,
    pub note: Option<String>,
    /// "model" or "person".
    pub by: Option<String>,
}

/// The checklist on in the session's chat.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct On {
    pub name: String,
    pub done: u64,
    pub total: u64,
    pub items: Vec<Item>,
}

/// One of the person's checklists, or the repository's (no id).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct List {
    pub id: Option<String>,
    pub name: String,
    pub items: Vec<String>,
}

/// What the session last said of the checklists.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listing {
    pub on: Option<On>,
    pub mine: Vec<List>,
    pub repo: Vec<List>,
    pub problem: Option<String>,
    /// Whether Arena makes, changes and deletes them.
    pub edit: bool,
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn lists(v: &Value) -> Vec<List> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|c| List {
            id: text(&c["id"]),
            name: text(&c["name"]).unwrap_or_default(),
            items: c["items"].as_array().into_iter().flatten().filter_map(text).collect(),
        })
        .collect()
}

/// `checklists{on, mine, repo, problem, edit}` read.
pub fn listing_of(m: &Value) -> Listing {
    let on = m["on"].is_object().then(|| On {
        name: text(&m["on"]["name"]).unwrap_or_default(),
        done: m["on"]["done"].as_u64().unwrap_or(0),
        total: m["on"]["total"].as_u64().unwrap_or(0),
        items: m["on"]["items"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|i| Item {
                number: i["number"].as_u64().unwrap_or(0),
                text: i["text"].as_str().unwrap_or("").to_string(),
                done: i["done"].as_bool().unwrap_or(false),
                note: text(&i["note"]),
                by: text(&i["by"]),
            })
            .collect(),
    });
    Listing {
        on,
        mine: lists(&m["mine"]),
        repo: lists(&m["repo"]),
        problem: text(&m["problem"]),
        edit: m["edit"].as_bool().unwrap_or(false),
    }
}

/// "1/2 done".
pub fn progress(on: &On) -> String {
    format!("{}/{} done", on.done, on.total)
}

/// What the box of an item says it does: "Tick 1: the tests pass", or "Untick 1: ..." once it is ticked.
pub fn tick_words(i: &Item) -> String {
    format!("{} {}: {}", if i.done { "Untick" } else { "Tick" }, i.number, i.text)
}

/// The line under an item: how it was done or checked, and by whom.
pub fn note_line(i: &Item) -> Option<String> {
    let by = match i.by.as_deref() {
        Some("model") => Some("by the model"),
        Some("person") => Some("by you"),
        _ => None,
    };
    match (&i.note, by.filter(|_| i.done)) {
        (Some(n), Some(b)) => Some(format!("{n} · {b}")),
        (Some(n), None) => Some(n.clone()),
        (None, Some(b)) => Some(format!("Ticked {b}")),
        (None, None) => None,
    }
}

/// An item ticked (or not) at once, before Arena says so; the count follows.
pub fn ticked(on: &mut On, n: u64, done: bool) {
    for i in on.items.iter_mut().filter(|i| i.number == n) {
        i.done = done;
        i.by = Some("person".into());
    }
    on.done = on.items.iter().filter(|i| i.done).count() as u64;
}

/// A checklist's items as typed, one per line: trimmed, a leading "- " or "[ ] " taken off, the empty ones left out.
pub fn items_of(lines: &str) -> Vec<String> {
    lines
        .lines()
        .map(|l| {
            let l = l.trim();
            let l = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")).unwrap_or(l).trim_start();
            l.strip_prefix("[ ] ").or_else(|| l.strip_prefix("[x] ")).unwrap_or(l).trim().to_string()
        })
        .filter(|l| !l.is_empty())
        .collect()
}

/// The part's state in a window.
#[derive(Clone, Copy)]
pub struct Checklists {
    /// None until the session answered.
    list: RwSignal<Option<Listing>>,
    /// Whether the view asked: it is asked again after each turn.
    asked: RwSignal<bool>,
    form: Form,
    /// The checklist the form changes (none: a new one).
    editing: RwSignal<Option<String>>,
    name: RwSignal<String>,
    /// The form's box of items, once it is drawn.
    lines: RwSignal<Option<Editor>>,
}

impl crate::ext::State for Checklists {
    fn new(ui: Ui) -> Self {
        Checklists {
            list: ui.scope.create_rw_signal(None),
            asked: ui.scope.create_rw_signal(false),
            form: Form::new(ui),
            editing: ui.scope.create_rw_signal(None),
            name: ui.scope.create_rw_signal(String::new()),
            lines: ui.scope.create_rw_signal(None),
        }
    }
}

fn state(ui: Ui) -> Checklists {
    ui.state::<Checklists>()
}

fn ask(send: &Send) {
    send(json!({"type": "checklists"}));
}

/// The card at the head of the Plan view: the checklist on, or the ones to turn on.
pub fn card(ui: Ui, send: Send) -> AnyView {
    let st = state(ui);
    if !st.asked.get_untracked() {
        st.asked.set(true);
    }
    ask(&send);
    let answered = create_memo(move |_| st.list.with(Option::is_some));
    let is_on = create_memo(move |_| st.list.with(|l| l.as_ref().is_some_and(|l| l.on.is_some())));
    let body = dyn_container(
        move || (answered.get(), is_on.get()),
        move |(answered, on)| match (answered, on) {
            (false, _) => empty().into_any(),
            (true, true) => on_card(ui, send.clone()).into_any(),
            (true, false) => off_card(ui, send.clone()).into_any(),
        },
    )
    .style(|s| s.width_full());
    scroll(body)
        .style(move |s| {
            s.width_full()
                .max_height_pct(45.0)
                .flex_shrink(0.0_f32)
                .border_bottom(1.0)
                .border_color(ui.colors.get().section_border)
        })
        .into_any()
}

/// Wrapped words, in the description's colour.
fn desc(t: Theme, words: impl Fn() -> String + 'static) -> impl IntoView {
    label(words).style(move |s| {
        s.font_size(12.0).line_height(1.4).color(t.get().desc).text_overflow(TextOverflow::Wrap).min_width(0.0)
    })
}

fn link(t: Theme, words: &'static str, on: impl Fn() + 'static) -> impl IntoView {
    label(move || words).on_click_stop(move |_| on()).style(move |s| {
        let c = t.get();
        s.font_size(12.0)
            .color(c.link)
            .cursor(CursorStyle::Pointer)
            .flex_shrink(0.0_f32)
            .hover(|s| s.color(c.link_active))
    })
}

/// The checklist on: its name, how much is done, Turn off, and its items.
fn on_card(ui: Ui, send: Send) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let on = create_memo(move |_| st.list.with(|l| l.as_ref().and_then(|l| l.on.clone())));
    let off = send.clone();
    let head = h_stack((
        label(|| "Checklist").style(move |s| bold(s).font_size(13.0).color(t.get().fg).flex_shrink(0.0_f32)),
        label(move || on.with(|o| o.as_ref().map(|o| o.name.clone()).unwrap_or_default())).style(move |s| {
            s.font_size(12.0)
                .color(t.get().desc)
                .flex_grow(1.0_f32)
                .min_width(0.0)
                .text_overflow(TextOverflow::Ellipsis)
        }),
        label(move || on.with(|o| o.as_ref().map(progress).unwrap_or_default()))
            .style(move |s| s.font_size(12.0).color(t.get().desc).flex_shrink(0.0_f32)),
        link(t, "Turn off", move || off(json!({"type": "checklist_off"}))),
    ))
    .style(|s| s.gap(8.0).width_full().items_center().padding_horiz(20.0).padding_vert(8.0));
    let items = dyn_stack(
        move || on.with(|o| o.as_ref().map(|o| o.items.clone()).unwrap_or_default()),
        |i| format!("{i:?}"),
        move |i| item_row(ui, send.clone(), i),
    )
    .style(|s| s.flex_col().width_full().padding_bottom(8.0));
    v_stack((head, items)).style(|s| s.width_full())
}

/// An item: its box (ticked by a click), its words, and how it was done.
fn item_row(ui: Ui, send: Send, i: Item) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let (n, done) = (i.number, i.done);
    let mark = container(icon("check", 14.0).style(move |s| s.apply_if(!done, |s| s.hide()))).style(move |s| {
        let c = t.get();
        s.width(18.0)
            .height(18.0)
            .flex_shrink(0.0_f32)
            .items_center()
            .justify_center()
            .border(1.0)
            .border_radius(3.0)
            .border_color(c.checkbox_border)
            .background(c.checkbox_bg)
            .color(c.checkbox_fg)
            .cursor(CursorStyle::Pointer)
    });
    let words = tick_words(&i);
    let mark = tooltip(mark, move || {
        let w = words.clone();
        label(move || w.clone())
    })
    .on_click_stop(move |_| {
        st.list.update(|l| {
            if let Some(on) = l.as_mut().and_then(|l| l.on.as_mut()) {
                ticked(on, n, !done);
            }
        });
        send(json!({"type": "checklist_tick", "n": n, "done": !done}));
    });
    let note = note_line(&i);
    let shown = note.is_some();
    let words = v_stack((
        label(move || i.text.clone()).style(move |s| {
            s.font_size(13.0).line_height(1.4).color(t.get().fg).text_overflow(TextOverflow::Wrap).min_width(0.0)
        }),
        desc(t, move || note.clone().unwrap_or_default()).style(move |s| s.apply_if(!shown, |s| s.hide())),
    ))
    .style(|s| s.gap(2.0).flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0));
    h_stack((mark, words)).style(move |s| {
        s.gap(8.0)
            .width_full()
            .items_start()
            .padding_horiz(20.0)
            .padding_vert(4.0)
            .hover(|s| s.background(t.get().list_hover))
    })
}

/// None on: the ones to turn on, by whose they are, and New Checklist.
fn off_card(ui: Ui, send: Send) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let problem = create_memo(move |_| st.list.with(|l| l.as_ref().and_then(|l| l.problem.clone())));
    let mine = create_memo(move |_| st.list.with(|l| l.as_ref().map(|l| l.mine.clone()).unwrap_or_default()));
    let repo = create_memo(move |_| st.list.with(|l| l.as_ref().map(|l| l.repo.clone()).unwrap_or_default()));
    let edit = create_memo(move |_| st.list.with(|l| l.as_ref().is_some_and(|l| l.edit)));
    let title = label(|| "Turn a checklist on")
        .style(move |s| bold(s).font_size(13.0).color(t.get().fg).padding_horiz(20.0).padding_top(8.0));
    let why = desc(t, move || problem.get().unwrap_or_default())
        .style(move |s| s.padding_horiz(20.0).apply_if(problem.with(Option::is_none), |s| s.hide()));
    let none_yet = desc(t, || "You have no checklists yet.".into()).style(move |s| {
        let hide = problem.with(Option::is_some) || mine.with(|m| !m.is_empty());
        s.padding_horiz(20.0).apply_if(hide, |s| s.hide())
    });
    let group = move |words: &'static str, of: floem::reactive::Memo<Vec<List>>| {
        label(move || words).style(move |s| {
            caps(s)
                .font_bold()
                .color(t.get().desc)
                .padding_horiz(20.0)
                .padding_top(6.0)
                .apply_if(of.with(Vec::is_empty), |s| s.hide())
        })
    };
    let (s1, s2) = (send.clone(), send.clone());
    let yours = dyn_stack(move || mine.get(), |c| format!("{c:?}"), move |c| list_row(ui, s1.clone(), c, edit))
        .style(|s| s.flex_col().width_full());
    let theirs = dyn_stack(move || repo.get(), |c| format!("{c:?}"), move |c| list_row(ui, s2.clone(), c, edit))
        .style(|s| s.flex_col().width_full());
    let new = container(button(t, "New Checklist", false, move || open_form(ui, None)))
        .style(move |s| s.padding_horiz(20.0).padding_vert(8.0).apply_if(!edit.get(), |s| s.hide()));
    v_stack((title, why, group("Yours", mine), none_yet, yours, group("The repository's", repo), theirs, new))
        .style(|s| s.gap(4.0).width_full().padding_bottom(6.0))
}

/// A checklist to turn on (a click), with how many items it has; the person's own with Edit and Delete.
fn list_row(ui: Ui, send: Send, c: List, edit: floem::reactive::Memo<bool>) -> impl IntoView {
    let t = ui.colors;
    let count = match c.items.len() {
        1 => "1 item".to_string(),
        n => format!("{n} items"),
    };
    let on = match &c.id {
        Some(id) => json!({"type": "checklist_on", "id": id}),
        None => json!({"type": "checklist_on", "repo": c.name}),
    };
    let mine = c.id.is_some();
    let (name, tip_name) = (c.name.clone(), c.name.clone());
    let turn = send.clone();
    let row_name = label(move || name.clone()).style(move |s| {
        s.font_size(13.0).color(t.get().fg).flex_grow(1.0_f32).min_width(0.0).text_overflow(TextOverflow::Ellipsis)
    });
    let (to_edit, to_delete) = (c.clone(), c);
    let actions = h_stack((
        tooltip(action(t, "edit", move || open_form(ui, Some(to_edit.clone()))), || label(|| "Edit")),
        tooltip(action(t, "trash", move || delete(ui, &send, &to_delete)), || label(|| "Delete")),
    ))
    .style(move |s| s.gap(2.0).apply_if(!mine || !edit.get(), |s| s.hide()));
    let row = h_stack((
        icon("checklist", 16.0).style(move |s| s.color(t.get().icon).flex_shrink(0.0_f32)),
        row_name,
        label(move || count.clone()).style(move |s| s.font_size(12.0).color(t.get().desc).flex_shrink(0.0_f32)),
        actions,
    ))
    .on_click_stop(move |_| turn(on.clone()))
    .style(move |s| {
        s.gap(6.0)
            .width_full()
            .height(22.0)
            .items_center()
            .padding_horiz(20.0)
            .cursor(CursorStyle::Pointer)
            .hover(|s| s.background(t.get().list_hover))
    });
    tooltip(row, move || {
        let w = format!("Turn on {tip_name}");
        label(move || w.clone())
    })
}

/// Delete, once the person said so.
fn delete(ui: Ui, send: &Send, c: &List) {
    let Some(id) = c.id.clone() else { return };
    let send = send.clone();
    dialog::ask(&format!("Delete the checklist '{}'?", c.name))
        .detail("It is deleted from Arena, for every session.")
        .kind(dialog::Kind::Warning)
        .button("Delete")
        .destructive()
        .open(ui, move |a| {
            if a.yes() {
                send(json!({"type": "checklist_delete", "id": id}));
            }
        });
}

/// The form opened: a new checklist (none), or one of the person's to change.
fn open_form(ui: Ui, c: Option<List>) {
    let st = state(ui);
    let (id, name, lines) = match c {
        Some(c) => (c.id, c.name, c.items.join("\n")),
        None => (None, String::new(), String::new()),
    };
    st.editing.set(id);
    st.name.set(name);
    if let Some(ed) = st.lines.get_untracked() {
        let len = ed.text().len();
        ed.doc().edit_single(Selection::region(0, len), &lines, EditType::Other);
        ed.cursor.update(|c| c.set_insert(Selection::caret(lines.len())));
    }
    st.form.show(None);
}

/// The form sent, when it has a name and an item; it closes.
fn save(ui: Ui, send: &Send) {
    let st = state(ui);
    let name = st.name.get_untracked().trim().to_string();
    let items = st.lines.get_untracked().map(|ed| items_of(&ed.text().to_string())).unwrap_or_default();
    if name.is_empty() || items.is_empty() {
        toast::info(ui, "A checklist has a name and at least one item.");
        return;
    }
    let mut m = json!({"type": "checklist_save", "name": name, "items": items});
    if let Some(id) = st.editing.get_untracked() {
        m["id"] = json!(id);
    }
    send(m);
    st.form.close();
}

/// The form over the window: the name, the items one per line, Cancel and Save.
fn host(ui: Ui, send: Send) -> AnyView {
    let (t, st) = (ui.colors, state(ui));
    let f = st.form;
    let enter = send.clone();
    let name = f.named(ui, "Name", f.input(ui, st.name, "Before a merge", move || save(ui, &enter)));
    let box_ = text_editor("")
        .placeholder("One item per line")
        .editor_style(move |s| {
            let c = t.get();
            s.hide_gutter(true).cursor_color(c.cursor).selection_color(c.selection).placeholder_color(c.placeholder)
        })
        .takes_keys(Focus::Input)
        .style(move |s| {
            let c = t.get();
            s.width_full()
                .height(120.0)
                .border(1.0)
                .border_radius(2.0)
                .border_color(c.input_border)
                .background(c.input_bg)
                .color(c.input_fg)
                .set(floem::views::editor::Foreground, c.input_fg)
        });
    st.lines.set(Some(box_.editor().clone()));
    let items = f.named(ui, "Items, one per line", box_.into_any());
    let buttons = form::buttons(vec![
        form::button(ui, || "Cancel".into(), false, || true, move || f.close()),
        form::button(
            ui,
            || "Save".into(),
            true,
            move || st.name.with(|n| !n.trim().is_empty()),
            move || save(ui, &send),
        ),
    ]);
    f.host(
        ui,
        move || if st.editing.with(Option::is_some) { "Edit Checklist".into() } else { "New Checklist".into() },
        || "Kept in Arena: turned on in a session, the model goes through it and ticks each item as it does it.".into(),
        vec![name, items],
        buttons,
    )
}

/// What it adds to the window's registries, once at start: the form's layer.
pub fn register(_ui: Ui, _send: &Send) {
    layers::add(Place::Over, "checklists.form", 30, |ui, send| host(ui, send));
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = state(ui);
    match m["type"].as_str().unwrap_or("") {
        "checklists" => {
            let l = Some(listing_of(m));
            // Set only when it changed: the card draws again on every set.
            if st.list.with_untracked(|now| *now != l) {
                st.list.set(l);
            }
            true
        }
        "checklist_on" | "checklist_off" | "checklist_tick" | "checklist_save" | "checklist_delete" => {
            if let Some(e) = m["error"].as_str() {
                toast::error(ui, e);
            }
            true
        }
        // The model ticked an item; a turn, or another session, may have changed what is on.
        "tool_end" if m["name"] == "checklist_check" => {
            ask(send);
            false
        }
        "turn_end" | "state" => {
            if st.asked.get_untracked() {
                ask(send);
            }
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said() -> Value {
        json!({"type": "checklists", "problem": null, "edit": true,
            "on": {"name": "Before a merge", "checklist_id": "k1", "done": 1, "total": 2, "items": [
                {"number": 1, "text": "the tests pass", "done": true, "note": "cargo test: 42 passed", "by": "model"},
                {"number": 2, "text": "the docs say what changed", "done": false, "note": null, "by": null}]},
            "mine": [{"id": "k1", "name": "Before a merge", "items": ["the tests pass", "the docs say what changed"]}],
            "repo": [{"name": "Release", "items": ["tag"]}]})
    }

    #[test]
    fn the_checklists_are_read_as_the_session_says_them() {
        let l = listing_of(&said());
        let on = l.on.clone().expect("one is on");
        assert_eq!((on.name.as_str(), progress(&on)), ("Before a merge", "1/2 done".to_string()));
        assert_eq!(on.items[1].note, None);
        assert_eq!(l.mine[0].id.as_deref(), Some("k1"));
        assert_eq!((l.repo[0].id.clone(), l.repo[0].items.clone()), (None, vec!["tag".to_string()]));
        assert!(l.edit && l.problem.is_none());
        let none = listing_of(&json!({"type": "checklists", "on": null, "mine": [], "repo": [], "problem": "sign in"}));
        assert_eq!((none.on, none.problem.as_deref(), none.edit), (None, Some("sign in"), false));
    }

    #[test]
    fn an_item_says_what_its_box_does_and_how_it_was_done() {
        let on = listing_of(&said()).on.expect("on");
        assert_eq!(tick_words(&on.items[0]), "Untick 1: the tests pass");
        assert_eq!(tick_words(&on.items[1]), "Tick 2: the docs say what changed");
        assert_eq!(note_line(&on.items[0]).as_deref(), Some("cargo test: 42 passed · by the model"));
        assert_eq!(note_line(&on.items[1]), None);
        let by_you = Item { done: true, by: Some("person".into()), ..Item::default() };
        assert_eq!(note_line(&by_you).as_deref(), Some("Ticked by you"));
    }

    #[test]
    fn a_tick_counts_at_once() {
        let mut on = listing_of(&said()).on.expect("on");
        ticked(&mut on, 2, true);
        assert_eq!((progress(&on), on.items[1].by.as_deref()), ("2/2 done".to_string(), Some("person")));
        ticked(&mut on, 1, false);
        assert_eq!(progress(&on), "1/2 done");
    }

    #[test]
    fn items_are_typed_one_per_line() {
        assert_eq!(
            items_of("  the tests pass \n\n- [ ] the docs say what changed\n* the tag\n   \n"),
            vec!["the tests pass".to_string(), "the docs say what changed".into(), "the tag".into()]
        );
        assert!(items_of("\n  \n").is_empty());
    }
}
