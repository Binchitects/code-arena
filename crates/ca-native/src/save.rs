//! Saving, as VS Code's: Save, Save All, Save without Formatting and Revert File; the question when a file changed
//! on disk since it was read (Overwrite, Compare); what is done to a file as it is saved (trailing whitespace, the
//! final newlines); auto save (`files.autoSave`); and hot exit (`files.hotExit`): the text of a tab not saved is kept
//! aside by the session a second after typing stops, dropped when it is saved or let go of, and put back into its
//! tab at the next start, as one edit over what is on disk.
//!
//! A save sends `save{path, text, modified}` with the time the file was read, so the session refuses it with
//! `saved{error:"changed"}` when the disk moved since; Overwrite sends it again with `force`. A tab is marked saved
//! only when its text is still what was sent (keys typed while a save is on its way keep it unsaved).

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::Duration;

use floem::event::EventListener;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{Decorators, empty};
use floem::{AnyView, IntoView};
use serde_json::{Value, json};

use crate::layers::{self, Place};
use crate::settings;
use crate::state::{OnDisk, Send, Tab, Ui};
use crate::widgets::{dialog, toast};

pub const SAVE_ALL: &str = "workbench.action.files.saveAll";
pub const SAVE_WITHOUT_FORMATTING: &str = "workbench.action.files.saveWithoutFormatting";
pub const REVERT: &str = "workbench.action.files.revert";

/// How a tab is saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// Asked for (Save, Save All): what is done to a file as it is saved is done first.
    Asked,
    /// Save without Formatting: as it is.
    AsItIs,
    /// Auto save: as it is, and a file changed on disk pauses it rather than asking.
    Auto,
    /// Overwrite: over whatever is on disk now.
    Overwrite,
}

/// What saving keeps while the window is open.
#[derive(Clone, Copy)]
pub struct Saves {
    /// Each file's text as last sent to be saved (by its hash), and whether it was auto save's.
    sent: RwSignal<HashMap<String, (u64, bool)>>,
    /// Each file's text as last kept aside by hot exit (by its hash).
    backed: RwSignal<HashMap<String, u64>>,
    /// The texts kept aside at the last exit for files whose tabs are not open yet, put back when they come.
    pending: RwSignal<HashMap<String, String>>,
    /// Whether the kept texts were asked for (once a window).
    asked: RwSignal<bool>,
    /// Whether their answer is still to come.
    waiting: RwSignal<bool>,
    /// Counted up at each edit: a timer that finds it moved on is of an edit before the last.
    typed: RwSignal<u64>,
    /// The files auto save was paused for, said once each.
    paused: RwSignal<HashSet<String>>,
}

impl crate::ext::State for Saves {
    fn new(ui: Ui) -> Self {
        Saves {
            sent: ui.scope.create_rw_signal(HashMap::new()),
            backed: ui.scope.create_rw_signal(HashMap::new()),
            pending: ui.scope.create_rw_signal(HashMap::new()),
            asked: ui.scope.create_rw_signal(false),
            waiting: ui.scope.create_rw_signal(false),
            typed: ui.scope.create_rw_signal(0),
            paused: ui.scope.create_rw_signal(HashSet::new()),
        }
    }
}

fn hash_of(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

fn name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn tell(ui: Ui, what: impl FnOnce(&Send)) {
    if let Some(send) = ui.out.get_untracked() {
        what(&send);
    }
}

/// The lines' spaces and tabs at their ends taken away (a line's `\r` stays).
pub fn trim_trailing_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let (body, cr) = match line.strip_suffix('\r') {
            Some(b) => (b, "\r"),
            None => (line, ""),
        };
        out.push_str(body.trim_end_matches([' ', '\t']));
        out.push_str(cr);
    }
    out
}

/// The line ending a file uses: `\r\n` when its first line ends so, else `\n`.
fn eol_of(text: &str) -> &'static str {
    match text.find('\n') {
        Some(at) if at > 0 && text.as_bytes()[at - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

/// A file that is not empty made to end with a newline.
pub fn insert_final_newline(text: &str) -> String {
    if text.is_empty() || text.ends_with('\n') {
        return text.to_string();
    }
    format!("{text}{}", eol_of(text))
}

/// The newlines after the last one at the end of a file taken away.
pub fn trim_final_newlines(text: &str) -> String {
    let body = text.trim_end_matches(['\n', '\r']);
    if body.len() == text.len() {
        return text.to_string();
    }
    format!("{body}{}", eol_of(text))
}

/// One of what is done to a file as it is saved: its text before, its text after.
type Step = fn(&str) -> String;

/// What is done to a file as it is saved, in VS Code's order, each by its setting (a language's own value wins).
fn participants(ui: Ui, tab: &Tab) {
    let lang = crate::language::of(ui, &tab.file);
    let steps: [(bool, Step); 3] = [
        (settings::now(ui, "files.trimTrailingWhitespace", lang) == true, trim_trailing_whitespace),
        (settings::now(ui, "files.insertFinalNewline", lang) == true, insert_final_newline),
        (settings::now(ui, "files.trimFinalNewlines", lang) == true, trim_final_newlines),
    ];
    for (on, step) in steps {
        if on {
            // One edit a step: each is undone by itself.
            let old = tab.text();
            let new = step(&old);
            if new != old {
                ui.edit_to(tab, &new);
            }
        }
    }
}

/// A tab saved: a file's text sent with when it was read, a tab of a kind its kind's way.
pub fn save(ui: Ui, path: &str, send: &Send, how: How) {
    let Some(tab) = ui.tab(path) else { return };
    if tab.kind.is_some() {
        crate::editor::kinds::save(ui, send, &tab);
        return;
    }
    if !tab.is_file() {
        return;
    }
    if how == How::Asked {
        participants(ui, &tab);
    }
    let text = tab.text();
    ui.state::<Saves>().sent.update(|s| {
        s.insert(path.to_string(), (hash_of(&text), how == How::Auto));
    });
    // Written with the file's line ending (`indent.rs`), which its document cannot change.
    let mut m = json!({"type": "save", "path": path, "text": crate::indent::written(ui, path, text.clone())});
    if let Some(at) = ui.modified.with_untracked(|k| k.get(path).copied()) {
        m["modified"] = json!(at);
    }
    if how == How::Overwrite {
        m["force"] = json!(true);
    }
    send(m);
}

/// The paths of the file tabs with changes not saved.
fn dirty_files(ui: Ui) -> Vec<String> {
    ui.tabs.with_untracked(|t| {
        t.iter().filter(|t| t.is_file() && t.dirty.get_untracked()).map(|t| t.path.clone()).collect()
    })
}

/// Save All: every tab with changes not saved, files and tabs of a kind.
pub fn save_all(ui: Ui, send: &Send) {
    let all: Vec<String> =
        ui.tabs.with_untracked(|t| t.iter().filter(|t| t.dirty.get_untracked()).map(|t| t.path.clone()).collect());
    for p in all {
        save(ui, &p, send, How::Asked);
    }
}

/// Auto save of these files; one changed on disk is left (and said once) until the person chooses.
fn auto_save(ui: Ui, paths: Vec<String>) {
    let s = ui.state::<Saves>();
    tell(ui, |send| {
        for p in paths {
            if ui.on_disk.with_untracked(|d| d.contains_key(&p)) {
                if !s.paused.with_untracked(|x| x.contains(&p)) {
                    s.paused.update(|x| {
                        x.insert(p.clone());
                    });
                    toast::warning(ui, &format!("Auto save is paused for {}: it changed on disk.", name_of(&p)));
                }
                continue;
            }
            save(ui, &p, send, How::Auto);
        }
    });
}

/// What the person let go of in a tab (Don't Save, Revert): what hot exit kept of it goes too.
pub fn discarded(ui: Ui, path: &str, send: &Send) {
    let s = ui.state::<Saves>();
    if s.backed.with_untracked(|b| b.contains_key(path)) {
        s.backed.update(|b| {
            b.remove(path);
        });
    }
    send(json!({"type": "backup_drop", "path": path}));
}

/// Hot exit: the text of each tab not saved that changed since it was last kept aside, sent to be kept.
fn back_up(ui: Ui) {
    if settings::now(ui, "files.hotExit", None) == "off" {
        return;
    }
    let s = ui.state::<Saves>();
    tell(ui, |send| {
        for p in dirty_files(ui) {
            let Some(tab) = ui.tab(&p) else { continue };
            let text = tab.text();
            let h = hash_of(&text);
            if s.backed.with_untracked(|b| b.get(&p) != Some(&h)) {
                s.backed.update(|b| {
                    b.insert(p.clone(), h);
                });
                send(json!({"type": "backup", "path": p, "text": text}));
            }
        }
    });
}

/// A text kept at the last exit put back into its tab, as one edit over what is on disk.
fn put_back(ui: Ui, tab: &Tab, text: &str, send: &Send) {
    if tab.text() == text {
        send(json!({"type": "backup_drop", "path": tab.path}));
        return;
    }
    ui.edit_to(tab, text);
    ui.state::<Saves>().backed.update(|b| {
        b.insert(tab.path.clone(), hash_of(text));
    });
}

/// The open files asked how they are on disk now: those changed are read again or said to have changed.
fn stat_open(ui: Ui) {
    let paths: Vec<String> =
        ui.tabs.with_untracked(|t| t.iter().filter(|t| t.is_file()).map(|t| t.path.clone()).collect());
    if !paths.is_empty() {
        tell(ui, |send| send(json!({"type": "stat", "paths": paths})));
    }
}

/// A view that takes no room: it hears the window go behind (auto save, hot exit at once), come back (the open
/// files looked at on disk) and close (what is not kept aside yet sent, as far as it goes before the exit).
fn watcher(ui: Ui, _send: Send) -> AnyView {
    empty()
        .on_event_cont(EventListener::WindowLostFocus, move |_| {
            back_up(ui);
            let mode = settings::now(ui, "files.autoSave", None);
            if mode == "onWindowChange" || mode == "onFocusChange" {
                auto_save(ui, dirty_files(ui));
            }
        })
        .on_event_cont(EventListener::WindowGotFocus, move |_| stat_open(ui))
        .on_event_cont(EventListener::WindowClosed, move |_| back_up(ui))
        .style(|s| s.absolute().width(0.0).height(0.0))
        .into_any()
}

/// Exit: with changes not saved and hot exit off, asked first; else what is not kept aside yet is sent, then the
/// window goes.
fn exit(ui: Ui, send: &Send) {
    let dirty = dirty_files(ui);
    if dirty.is_empty() {
        floem::quit_app();
        return;
    }
    if settings::now(ui, "files.hotExit", None) != "off" {
        back_up(ui);
        floem::action::exec_after(Duration::from_millis(300), |_| floem::quit_app());
        return;
    }
    let send = send.clone();
    let what = match dirty.as_slice() {
        [one] => name_of(one).to_string(),
        _ => format!("the following {} files", dirty.len()),
    };
    let detail = match dirty.len() {
        1 => "Your changes will be lost if you don't save them.".to_string(),
        _ => {
            let names: Vec<&str> = dirty.iter().map(|p| name_of(p)).collect();
            format!("{}\n\nYour changes will be lost if you don't save them.", names.join("\n"))
        }
    };
    dialog::ask(&format!("Do you want to save the changes you made to {what}?"))
        .detail(&detail)
        .button(if dirty.len() == 1 { "Save" } else { "Save All" })
        .button("Don't Save")
        .open(ui, move |a| match a.button {
            Some(0) => {
                for p in &dirty {
                    save(ui, p, &send, How::Asked);
                }
                floem::action::exec_after(Duration::from_millis(300), |_| floem::quit_app());
            }
            Some(1) => {
                for p in &dirty {
                    discarded(ui, p, &send);
                }
                floem::action::exec_after(Duration::from_millis(100), |_| floem::quit_app());
            }
            _ => {}
        });
}

/// VS Code's question when a save found the file changed on disk since it was read.
fn ask_conflict(ui: Ui, path: &str) {
    let path = path.to_string();
    dialog::ask(&format!("Failed to save '{}': The content of the file is newer.", name_of(&path)))
        .detail("Please compare your version with the file contents or overwrite the content of the file with your changes.")
        .button("Overwrite")
        .button("Compare")
        .open(ui, move |a| {
            // A close waiting for this save goes on only with Overwrite.
            if a.button != Some(0) && ui.close_saved.with_untracked(|c| c.as_deref() == Some(path.as_str())) {
                ui.close_saved.set(None);
            }
            match a.button {
                Some(0) => tell(ui, |send| save(ui, &path, send, How::Overwrite)),
                Some(1) => tell(ui, |send| {
                    use crate::compare::{Source, open};
                    open(ui, send, Source::Saved(path.clone()), Source::Live(path.clone()));
                }),
                _ => {}
            }
        });
}

/// What it adds to the window's registries, once at start: the commands, the File menu's items, Exit asking
/// first, what hears the window, and the timers of hot exit and auto save.
pub fn register(ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::menus::{Menu, add as menu, item};
    add(command(SAVE_ALL, "File", "Save All", save_all).keys("Ctrl+K S"));
    add(command(SAVE_WITHOUT_FORMATTING, "File", "Save without Formatting", |ui, send| {
        if let Some(p) = ui.active.get_untracked() {
            save(ui, &p, send, How::AsItIs);
        }
    })
    .keys("Ctrl+K Ctrl+Shift+S"));
    add(command(REVERT, "File", "Revert File", |ui, send| {
        if let Some(p) = ui.active.get_untracked().filter(|p| ui.tab(p).is_some_and(|t| t.is_file())) {
            ui.take_disk(&p, send);
        }
    }));
    add(command(crate::workbench::QUIT, "File", "Exit", exit).hidden());
    let bar = Menu::TitleBar;
    menu(bar(crate::menus::FILE), item(SAVE_ALL).group("1_save"));
    menu(bar(crate::menus::FILE), item(REVERT).group("2_close"));
    layers::add(Place::Over, "save.watch", 0, watcher);
    let s = ui.state::<Saves>();
    // Each edit counted; a second after the last one the tabs not saved are kept aside, and after files.autoSaveDelay
    // auto save ("afterDelay") saves them. Nothing more is done per key.
    ui.scope.create_effect(move |first: Option<()>| {
        ui.edited.with(|_| ());
        if first.is_none() {
            return;
        }
        let n = s.typed.get_untracked() + 1;
        s.typed.set(n);
        floem::action::exec_after(Duration::from_secs(1), move |_| {
            if s.typed.get_untracked() == n {
                back_up(ui);
            }
        });
        if settings::now(ui, "files.autoSave", None) == "afterDelay" {
            let wait = settings::now(ui, "files.autoSaveDelay", None).as_u64().unwrap_or(1000);
            floem::action::exec_after(Duration::from_millis(wait), move |_| {
                if s.typed.get_untracked() == n && settings::now(ui, "files.autoSave", None) == "afterDelay" {
                    auto_save(ui, dirty_files(ui));
                }
            });
        }
    });
    // "onFocusChange": the editor left is saved.
    ui.scope.create_effect(move |before: Option<Option<String>>| {
        let now = ui.active.get();
        if let Some(Some(left)) = before
            && now.as_ref() != Some(&left)
            && settings::now(ui, "files.autoSave", None) == "onFocusChange"
            && ui.tab(&left).is_some_and(|t| t.is_file() && t.dirty.get_untracked())
        {
            auto_save(ui, vec![left]);
        }
        now
    });
    // Once the folder's tabs are asked for again, so are the texts kept at the last exit.
    crate::ext::after("save", after);
}

/// After the window took a message in: the kept texts asked for once the session said its state, and one put back
/// once its file's tab is there.
fn after(ui: Ui, m: &Value, send: &Send) {
    let s = ui.state::<Saves>();
    match m["type"].as_str().unwrap_or("") {
        "state" if !s.asked.get_untracked() => {
            s.asked.set(true);
            if settings::now(ui, "files.hotExit", None) != "off" {
                s.waiting.set(true);
                send(json!({"type": "backups"}));
            }
        }
        "file" if m["error"].is_null() => {
            let path = m["path"].as_str().unwrap_or("");
            if let Some(text) = s.pending.with_untracked(|p| p.get(path).cloned()) {
                s.pending.update(|p| {
                    p.remove(path);
                });
                if let Some(tab) = ui.tab(path) {
                    put_back(ui, &tab, &text, send);
                }
            }
        }
        _ => {}
    }
}

/// The session's messages of saving taken in: a save's answer, the texts kept at the last exit, an agent's edit
/// (the open files looked at on disk at once). A kept text's file that cannot be read is taken away from the rest
/// of the window (it is not opened as a tab that says why); nothing else is.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let s = ui.state::<Saves>();
    let text = |k: &str| m[k].as_str().unwrap_or("").to_string();
    match m["type"].as_str().unwrap_or("") {
        "saved" => saved(ui, m),
        // Only the answer to this window's own asking, once: another would put older texts over newer typing.
        "backups" if s.waiting.get_untracked() => {
            s.waiting.set(false);
            for it in m["items"].as_array().into_iter().flatten() {
                let (Some(path), Some(kept)) = (it["path"].as_str(), it["text"].as_str()) else { continue };
                match ui.tab(path).filter(|t| t.is_file()) {
                    Some(tab) => put_back(ui, &tab, kept, send),
                    None => {
                        s.pending.update(|p| {
                            p.insert(path.to_string(), kept.to_string());
                        });
                        send(json!({"type": "open", "path": path}));
                    }
                }
            }
        }
        "file" if m["error"].is_string() => {
            let path = text("path");
            if s.pending.with_untracked(|p| p.contains_key(&path)) {
                s.pending.update(|p| {
                    p.remove(&path);
                });
                toast::warning(ui, &format!("Could not restore your changes to {path}: {}", text("error")));
                return true;
            }
        }
        // The agent wrote a file: an open one is read again now, not at the next look.
        "tool_end" if m["ok"] == true && matches!(m["name"].as_str(), Some("edit_file" | "write_file")) => {
            stat_open(ui)
        }
        _ => {}
    }
    false
}

/// A save's answer: the tab marked saved when its text is still what was sent; a file changed on disk since it was
/// read asked about (or auto save paused for it); another error said.
fn saved(ui: Ui, m: &Value) {
    let s = ui.state::<Saves>();
    let path = m["path"].as_str().unwrap_or("").to_string();
    let sent = s.sent.with_untracked(|x| x.get(&path).copied());
    if sent.is_some() {
        s.sent.update(|x| {
            x.remove(&path);
        });
    }
    let closing = ui.close_saved.with_untracked(|c| c.as_deref() == Some(path.as_str()));
    match m["error"].as_str() {
        Some("changed") => {
            ui.on_disk.update(|d| {
                d.insert(path.clone(), OnDisk::Changed);
            });
            match sent {
                Some((_, true)) => auto_save(ui, vec![path]),
                _ => ask_conflict(ui, &path),
            }
        }
        Some(e) => {
            if closing {
                ui.close_saved.set(None);
            }
            ui.message.set(format!("Could not save {path}: {e}"))
        }
        None => {
            let tab = ui.tab(&path);
            // Keys typed while the save was on its way: still not saved.
            let same = match (&tab, sent) {
                (Some(t), Some((h, _))) => hash_of(&t.text()) == h,
                _ => true,
            };
            if let Some(t) = &tab
                && same
            {
                t.dirty.set(false);
            }
            if let Some(at) = m["modified"].as_u64() {
                ui.modified.update(|k| {
                    k.insert(path.clone(), at);
                });
            }
            ui.forget_on_disk(&path);
            if s.paused.with_untracked(|x| x.contains(&path)) {
                s.paused.update(|x| {
                    x.remove(&path);
                });
            }
            // The session dropped what hot exit kept of it.
            if s.backed.with_untracked(|b| b.contains_key(&path)) {
                s.backed.update(|b| {
                    b.remove(&path);
                });
            }
            ui.message.set(String::new());
            if closing && same {
                ui.close_saved.set(None);
                ui.close_now(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floem::views::editor::core::editor::EditType;
    use floem::views::editor::core::selection::Selection;
    use floem::views::editor::text::Document;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn window() -> (Ui, Send, Rc<RefCell<Vec<Value>>>) {
        let ui = Ui::new();
        let asked = Rc::new(RefCell::new(Vec::new()));
        let to = asked.clone();
        let send: Send = Rc::new(move |v: Value| to.borrow_mut().push(v));
        ui.out.set(Some(send.clone()));
        (ui, send, asked)
    }

    fn of(asked: &Rc<RefCell<Vec<Value>>>, kind: &str) -> Vec<Value> {
        asked.borrow().iter().filter(|m| m["type"] == kind).cloned().collect()
    }

    fn type_in(ui: Ui, path: &str, text: &str) {
        ui.tab(path).expect("open").doc.edit_single(Selection::caret(0), text, EditType::InsertChars);
    }

    #[test]
    fn a_save_says_when_the_file_was_read_and_a_refused_one_is_asked_about_then_overwritten() {
        let (ui, send, asked) = window();
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "one\n", "modified": 5}), &send);
        type_in(ui, "a.rs", "mine ");
        // Read again meanwhile (asked for once more): its time is not taken while the tab has its own changes.
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "theirs\n", "modified": 9}), &send);
        save(ui, "a.rs", &send, How::AsItIs);
        assert_eq!(of(&asked, "save").last().expect("sent")["modified"], 5);
        ui.take(&json!({"type": "saved", "path": "a.rs", "error": "changed", "modified": 9}), &send);
        assert!(ui.tab("a.rs").expect("open").dirty.get_untracked(), "not saved");
        assert_eq!(ui.on_disk.get_untracked().get("a.rs"), Some(&OnDisk::Changed));
        assert!(dialog::is_open(ui), "asked: Overwrite or Compare");
        dialog::cancel(ui);
        save(ui, "a.rs", &send, How::Overwrite);
        assert_eq!(of(&asked, "save").last().expect("sent")["force"], true);
        ui.take(&json!({"type": "saved", "path": "a.rs", "modified": 10}), &send);
        assert!(!ui.tab("a.rs").expect("open").dirty.get_untracked());
        assert!(ui.on_disk.get_untracked().is_empty());
        assert_eq!(ui.modified.get_untracked().get("a.rs"), Some(&10));
    }

    #[test]
    fn keys_typed_while_a_save_is_on_its_way_keep_the_tab_unsaved() {
        let (ui, send, _) = window();
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "one\n", "modified": 5}), &send);
        type_in(ui, "a.rs", "x");
        save(ui, "a.rs", &send, How::Auto);
        type_in(ui, "a.rs", "y");
        ui.take(&json!({"type": "saved", "path": "a.rs", "modified": 6}), &send);
        assert!(ui.tab("a.rs").expect("open").dirty.get_untracked());
        save(ui, "a.rs", &send, How::Auto);
        ui.take(&json!({"type": "saved", "path": "a.rs", "modified": 7}), &send);
        assert!(!ui.tab("a.rs").expect("open").dirty.get_untracked());
    }

    #[test]
    fn auto_save_refused_for_a_file_changed_on_disk_pauses_without_asking() {
        let (ui, send, asked) = window();
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "one\n", "modified": 5}), &send);
        type_in(ui, "a.rs", "x");
        save(ui, "a.rs", &send, How::Auto);
        ui.take(&json!({"type": "saved", "path": "a.rs", "error": "changed", "modified": 9}), &send);
        assert!(!dialog::is_open(ui));
        assert_eq!(ui.on_disk.get_untracked().get("a.rs"), Some(&OnDisk::Changed));
        let n = of(&asked, "save").len();
        auto_save(ui, vec!["a.rs".into()]);
        assert_eq!(of(&asked, "save").len(), n, "paused while the file differs on disk");
    }

    #[test]
    fn a_text_kept_at_the_last_exit_comes_back_into_its_tab_as_one_undo() {
        let (ui, send, asked) = window();
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "one\n", "modified": 5}), &send);
        after(ui, &json!({"type": "state"}), &send);
        assert_eq!(of(&asked, "backups").len(), 1, "asked for once");
        after(ui, &json!({"type": "state"}), &send);
        assert_eq!(of(&asked, "backups").len(), 1);
        ui.take(
            &json!({"type": "backups", "items": [
                {"path": "a.rs", "text": "one, changed\n"},
                {"path": "b.rs", "text": "b, changed\n"},
                {"path": "gone.rs", "text": "lost\n"},
            ]}),
            &send,
        );
        let a = ui.tab("a.rs").expect("open");
        assert_eq!(a.text(), "one, changed\n");
        assert!(a.dirty.get_untracked());
        // Another window's answer, later: not put over what is typed since.
        type_in(ui, "a.rs", "new ");
        ui.take(&json!({"type": "backups", "items": [{"path": "a.rs", "text": "old\n"}]}), &send);
        assert_eq!(a.text(), "new one, changed\n");
        // The others are opened, and put back once they come.
        assert!(of(&asked, "open").iter().any(|m| m["path"] == "b.rs"));
        let b = json!({"type": "file", "path": "b.rs", "text": "b\n", "modified": 5});
        ui.take(&b, &send);
        after(ui, &b, &send);
        let tab = ui.tab("b.rs").expect("open");
        assert_eq!(tab.text(), "b, changed\n");
        assert!(tab.dirty.get_untracked());
        // One that cannot be read is said, not opened as a tab.
        ui.take(&json!({"type": "file", "path": "gone.rs", "error": "not found"}), &send);
        assert!(ui.tab("gone.rs").is_none());
        // Let go of (Don't Save): what was kept goes too.
        ui.close_now("a.rs");
        assert!(of(&asked, "backup_drop").iter().any(|m| m["path"] == "a.rs"));
    }

    #[test]
    fn what_is_kept_aside_is_each_text_not_saved_once() {
        let (ui, send, asked) = window();
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "one\n", "modified": 5}), &send);
        ui.take(&json!({"type": "file", "path": "b.rs", "text": "b\n", "modified": 5}), &send);
        type_in(ui, "a.rs", "x");
        back_up(ui);
        back_up(ui);
        let kept = of(&asked, "backup");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0], json!({"type": "backup", "path": "a.rs", "text": "xone\n"}));
        type_in(ui, "a.rs", "y");
        back_up(ui);
        assert_eq!(of(&asked, "backup").len(), 2);
    }

    #[test]
    fn save_all_saves_every_tab_not_saved() {
        let (ui, send, asked) = window();
        for p in ["a.rs", "b.rs", "c.rs"] {
            ui.take(&json!({"type": "file", "path": p, "text": "x\n", "modified": 5}), &send);
        }
        type_in(ui, "a.rs", "1");
        type_in(ui, "c.rs", "3");
        save_all(ui, &send);
        let saved: Vec<Value> = of(&asked, "save").iter().map(|m| m["path"].clone()).collect();
        assert_eq!(saved, vec![json!("a.rs"), json!("c.rs")]);
    }

    #[test]
    fn trailing_whitespace_goes_from_each_line_and_line_ends_stay() {
        assert_eq!(trim_trailing_whitespace("a  \nb\t\n  c \n"), "a\nb\n  c\n");
        assert_eq!(trim_trailing_whitespace("a \r\nb\t \r\n"), "a\r\nb\r\n");
        assert_eq!(trim_trailing_whitespace("x  "), "x");
        assert_eq!(trim_trailing_whitespace(""), "");
    }

    #[test]
    fn a_final_newline_is_put_in_the_file_s_own_line_ending() {
        assert_eq!(insert_final_newline("a\nb"), "a\nb\n");
        assert_eq!(insert_final_newline("a\r\nb"), "a\r\nb\r\n");
        assert_eq!(insert_final_newline("a\n"), "a\n");
        assert_eq!(insert_final_newline(""), "", "an empty file stays empty");
    }

    #[test]
    fn the_newlines_after_the_last_one_go() {
        assert_eq!(trim_final_newlines("a\n\n\n"), "a\n");
        assert_eq!(trim_final_newlines("a\r\n\r\n"), "a\r\n");
        assert_eq!(trim_final_newlines("a\n"), "a\n");
        assert_eq!(trim_final_newlines("a"), "a");
    }
}
