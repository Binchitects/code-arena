//! The IDE's settings as the window reads them, and the person's own settings files as editor tabs.
//!
//! # Reading a setting
//!
//! The session says every setting's value in force (`settings`: the default, then the person's `settings.json`,
//! then the project's `.code-arena/settings.json` and `.vscode/settings.json`, then the language's own block), and
//! says them again whenever a settings file is saved or changes on disk. A part reads one by its key, written as
//! a literal (a test of `ca-settings` reads the calls, and fails for a key that is not in the schema):
//!
//! ```ignore
//! let size = settings::number(ui, "editor.fontSize", None);          // followed: read again when it changes
//! let tab = settings::setting(ui, "editor.tabSize", Some("python")); // for a language (VS Code's id)
//! let wrap = settings::memo(ui, "editor.wordWrap", move || language.get()); // behind a memo, for what is built from it
//! ```
//!
//! `setting`, `flag`, `number` and `text` follow the settings where they are read (in an effect, a style, a
//! label): a change of any setting runs the reader again, so what is costly to make again (a `dyn_container`'s
//! child) reads through `memo`, which tells only when its own value is another. `now` reads without following (in
//! a command, a `take`). Before the session has said anything a setting is its default, so a reader never waits.
//!
//! # The person's files
//!
//! `open_user(ui, send, "settings.json")` opens one of the person's own files (`settings.json`,
//! `keybindings.json`, `snippets/NAME.json`) in a tab named "User › settings.json": a tab of the kind `user`
//! (`editor::kinds`), whose path is `user:` and the file's name. It is saved with Save as any tab, is kept with the
//! layout, and takes what is on disk when the file changes there and the tab has no changes of its own.
//!
//! What every part has (`register`, `take`) is said in `ext.rs`.

use std::collections::HashMap;
use std::rc::Rc;

use floem::AnyView;
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::views::editor::command::CommandExecuted;
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::mode::Mode;
use floem::views::editor::keypress::KeypressMap;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::editor::keypress::press::KeyPress;
use floem::views::editor::text::Document;
use floem::views::editor::text_document::TextDocument;
use floem::views::{Decorators, text_editor_keys};
use serde_json::{Map, Value, json};

use crate::commands;
use crate::editor::kinds;
use crate::highlight::Colours;
use crate::state::{Send, Tab, Ui};

pub mod editor;

/// VS Code's ids of the commands that open the settings files.
pub const OPEN_USER_JSON: &str = "workbench.action.openSettingsJson";
pub const OPEN_WORKSPACE_JSON: &str = "workbench.action.openWorkspaceSettingsFile";

/// The kind of tab one of the person's own files is in, and what its tabs' paths begin with.
pub const USER: &str = "user";

/// Something in a settings file that was not taken: its setting (empty: the whole file), its file, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub key: String,
    pub file: String,
    pub error: String,
}

/// The settings as the session said them last.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Said {
    /// Every setting's value in force, by its key.
    pub values: Map<String, Value>,
    /// Each language's own settings, by the language's id: they win over `values`.
    pub by_lang: Map<String, Value>,
    /// The file each setting that is not its default comes from (`env:NAME`, `flag`: not from a file).
    pub sources: Map<String, Value>,
    pub problems: Vec<Problem>,
    /// The settings files' paths: the person's, the project's own, VS Code's in the project.
    pub user_file: String,
    pub project_file: String,
    pub workspace_file: String,
}

impl Said {
    /// What a `settings` message says.
    pub fn from_json(m: &Value) -> Said {
        let map = |k: &str| m[k].as_object().cloned().unwrap_or_default();
        let text = |v: &Value| v.as_str().unwrap_or("").to_string();
        Said {
            values: map("values"),
            by_lang: map("by_lang"),
            sources: map("sources"),
            problems: m["problems"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| Problem { key: text(&p["key"]), file: text(&p["file"]), error: text(&p["error"]) })
                .collect(),
            user_file: text(&m["files"]["user"]),
            project_file: text(&m["files"]["project"]),
            workspace_file: text(&m["files"]["workspace"]),
        }
    }

    /// A setting's value: the language's own when it has one, else the value for every language, else (nothing
    /// said yet) the schema's default. Null for a key that is no setting.
    #[allow(dead_code)] // Read by the parts to come.
    pub fn value(&self, key: &str, language: Option<&str>) -> Value {
        if let Some(own) = language.and_then(|id| self.by_lang.get(id)).and_then(|own| own.get(key)) {
            return own.clone();
        }
        match self.values.get(key) {
            Some(v) => v.clone(),
            None => ca_settings::schema::find(key).map_or(Value::Null, |s| s.default_value()),
        }
    }
}

/// A tab's text, and the colours made for it.
type Coloured = (Rc<TextDocument>, Rc<Colours>);

/// This part's state: the settings as said last, and what the person's files' tabs need.
#[derive(Clone, Copy)]
pub struct Settings {
    /// Set only when what the session says differs from what is held.
    pub said: RwSignal<Rc<Said>>,
    /// The colours of each of the person's files open in a tab, by the tab's path.
    colours: RwSignal<HashMap<String, Coloured>>,
    /// This window asked for the project's settings file: it is opened when the session says it is there.
    asked: RwSignal<bool>,
}

impl crate::ext::State for Settings {
    fn new(ui: Ui) -> Settings {
        Settings {
            said: ui.scope.create_rw_signal(Rc::new(Said::default())),
            colours: ui.scope.create_rw_signal(HashMap::new()),
            asked: ui.scope.create_rw_signal(false),
        }
    }
}

/// A setting's value now, for a language when one is given (VS Code's id: `python`, `typescriptreact`). Followed:
/// what reads it is run again when the settings change.
#[allow(dead_code)] // Read by the parts to come.
pub fn setting(ui: Ui, key: &str, language: Option<&str>) -> Value {
    ui.state::<Settings>().said.with(|s| s.value(key, language))
}

/// As [`setting`], not followed: for a command, or a message being taken in.
#[allow(dead_code)] // Read by the parts to come.
pub fn now(ui: Ui, key: &str, language: Option<&str>) -> Value {
    ui.state::<Settings>().said.with_untracked(|s| s.value(key, language))
}

/// A setting that is true or false (false for one that is neither).
#[allow(dead_code)] // Read by the parts to come.
pub fn flag(ui: Ui, key: &str, language: Option<&str>) -> bool {
    setting(ui, key, language).as_bool().unwrap_or(false)
}

/// A setting that is a number (0 for one that is not).
#[allow(dead_code)] // Read by the parts to come.
pub fn number(ui: Ui, key: &str, language: Option<&str>) -> f64 {
    setting(ui, key, language).as_f64().unwrap_or(0.0)
}

/// A setting that is a word or a text (empty for one that is not).
#[allow(dead_code)] // Read by the parts to come.
pub fn text(ui: Ui, key: &str, language: Option<&str>) -> String {
    setting(ui, key, language).as_str().unwrap_or("").to_string()
}

/// A setting's value behind a memo of the window's own scope: it tells only when the value is another, so a
/// `dyn_container` built from it is not built again when some other setting changes. `language` is followed too
/// (the language of the file shown).
#[allow(dead_code)] // Read by the parts to come.
pub fn memo(ui: Ui, key: &'static str, language: impl Fn() -> Option<String> + 'static) -> Memo<Value> {
    ui.scope.create_memo(move |_| setting(ui, key, language().as_deref()))
}

/// What the settings files hold that was not taken, as the session said last. Followed.
#[allow(dead_code)] // Read by the parts to come.
pub fn problems(ui: Ui) -> Vec<Problem> {
    ui.state::<Settings>().said.with(|s| s.problems.clone())
}

/// The path of the tab one of the person's files is in.
pub fn user_tab(name: &str) -> String {
    format!("{USER}:{name}")
}

/// One of the person's own files (`settings.json`, `keybindings.json`, `snippets/NAME.json`) shown in its tab: the
/// tab is made and the file asked of the session when it is not open yet.
pub fn open_user(ui: Ui, send: &Send, name: &str) {
    let path = user_tab(name);
    let there = ui.tab(&path).is_some();
    kinds::open(ui, &path);
    if !there && let Some(tab) = ui.tab(&path) {
        colours_of(ui, &tab);
        send(json!({"type": "user_file", "name": name}));
    }
}

/// The project's own settings file (`.vscode/settings.json`) opened: the session makes it first, an empty object,
/// when it is not there.
pub fn open_workspace(ui: Ui, send: &Send) {
    let asked = ui.state::<Settings>().asked;
    if !asked.get_untracked() {
        asked.set(true);
    }
    send(json!({"type": "workspace_settings"}));
}

/// A tab's colours (JSON's, by its file's name), made the first time with what follows its text: an edit marks
/// the tab as changed and colours it again from the line edited.
fn colours_of(ui: Ui, tab: &Tab) -> Rc<Colours> {
    let held = ui.state::<Settings>().colours;
    // The same text as the tab's: a tab closed and opened again has another, and is hooked again.
    if let Some((of, colours)) = held.with_untracked(|h| h.get(&tab.path).cloned())
        && Rc::ptr_eq(&of, &tab.doc)
    {
        return colours;
    }
    let colours = Colours::new(tab.doc.clone(), &tab.file);
    let (dirty, edited, doc, to) = (tab.dirty, ui.edited, tab.doc.clone(), colours.clone());
    tab.doc.add_on_update(move |u| {
        if !dirty.get_untracked() {
            dirty.set(true);
        }
        edited.update(|n| *n += 1);
        let first = u.deltas().map(|x| x.summary().0.start()).min().unwrap_or(0);
        to.changed_from(doc.rope_text().line_of_offset(first));
    });
    held.update(|h| {
        h.insert(tab.path.clone(), (tab.doc.clone(), colours.clone()));
    });
    colours
}

thread_local! {
    /// The editor's own keymap, made once.
    static KEYMAP: Rc<KeypressMap> = Rc::new(KeypressMap::default());
}

/// The workbench's keys that are about the editor, taken by the editor itself as a file's editor takes them: Ctrl+F
/// finds, Ctrl+S saves, Ctrl+W closes the tab, Escape closes the find widget.
fn own_key(ui: Ui, send: &Send, press: &KeyPress, mods: Modifiers) -> bool {
    let KeyInput::Keyboard(key, _) = &press.key else { return false };
    let ctrl = mods.contains(Modifiers::CONTROL) && !mods.contains(Modifiers::ALT) && !mods.contains(Modifiers::SHIFT);
    match key {
        Key::Character(c) if ctrl => match c.to_lowercase().as_str() {
            "f" => crate::editor::find_show(ui),
            "s" => ui.save(send),
            "w" => {
                if let Some(p) = ui.active.get_untracked() {
                    ui.close(&p);
                }
            }
            _ => return false,
        },
        Key::Named(NamedKey::Escape) if ui.find.get_untracked() => ui.find.set(false),
        _ => return false,
    }
    true
}

/// The editor of one of the person's files, drawn as a file's is (the same font, colours and keys), on the tab's
/// own text.
fn user_editor(ui: Ui, send: Send, tab: &Tab) -> AnyView {
    let c = ui.colors.get_untracked();
    let colours = colours_of(ui, tab);
    colours.set_theme(crate::highlight::theme_of(&ui.theme_json()));
    let map = KEYMAP.with(Rc::clone);
    let path = tab.path.clone();
    let o = crate::editor::options::memo(ui, move || Some(path.clone()));
    let width = create_rw_signal(0.0);
    let ed = text_editor_keys("", move |editor, press, mods| {
        if let KeyInput::Keyboard(key, _) = &press.key
            && editor.with_untracked(|e| o.with_untracked(|o| crate::editor::options::key(ui, e, o, key, mods)))
        {
            return CommandExecuted::Yes;
        }
        if own_key(ui, &send, press, mods) {
            return CommandExecuted::Yes;
        }
        let command = map.keymaps.get(press).or_else(|| {
            let insert = editor.with_untracked(|e| e.cursor.with_untracked(|c| c.get_mode())) == Mode::Insert;
            let mut press = press.clone();
            press.mods.set(Modifiers::SHIFT, false);
            if insert { map.keymaps.get(&press) } else { None }
        });
        match command {
            Some(command) => editor.with_untracked(|e| e.doc().run_command(e, command, Some(1), mods)),
            // Not the editor's: the workbench's keys work while it has the focus.
            None => {
                if let KeyInput::Keyboard(key, _) = &press.key
                    && crate::keys_hook::editor(key, mods)
                {
                    crate::workbench::key(ui, &send, key, mods);
                }
                CommandExecuted::No
            }
        }
    })
    .use_doc(tab.doc.clone())
    .styling_rc(colours.clone())
    .editor_style(move |s| {
        let s = s
            .gutter_left_padding(18.0)
            .gutter_right_padding(22.0)
            .selection_color(c.selection)
            .cursor_color(c.cursor)
            .indent_guide_color(c.indent_guide)
            .visible_whitespace(c.whitespace)
            .phantom_color(c.desc)
            .placeholder_color(c.placeholder);
        o.with(|o| crate::editor::options::style(s, o, &c, width.get()))
    })
    .style(move |s| {
        s.size_full().background(c.editor_bg).set(floem::views::editor::Foreground, c.editor_fg).color(c.editor_fg)
    });
    // The editor on screen: the find widget looks in it, and a text read again keeps its caret's line.
    let editor = ed.editor().clone();
    let id = editor.id();
    let viewport = editor.viewport;
    create_effect(move |_| {
        let w = viewport.with(|v| v.width());
        if width.get_untracked() != w {
            width.set(w);
        }
    });
    ui.live.set(Some((tab.path.clone(), editor.clone())));
    let ed = ed.on_cleanup(move || {
        if ui.live.with_untracked(|l| l.as_ref().is_some_and(|(_, e)| e.id() == id)) {
            ui.live.set(None);
        }
    });
    crate::editor::options::follow(ui, ed, &editor, colours, o)
}

/// What it adds to the window's registries, once at start: the kind of tab the person's files are in, and the
/// commands that open the settings files. (The keys' file has its command with the keyboard shortcuts' part, which
/// opens it with `open_user(ui, send, "keybindings.json")`.)
pub fn register(_ui: Ui, _send: &Send) {
    kinds::add(
        kinds::kind(USER, |_ui, rest| format!("User › {rest}"), user_editor)
            .save(|_ui, send, tab| send(json!({"type": "save_user", "name": tab.file, "text": tab.text()})))
            .kept(|ui, send, path| {
                if let Some((_, name)) = kinds::split(path) {
                    open_user(ui, send, name);
                }
            }),
    );
    commands::add(commands::command(OPEN_USER_JSON, "Preferences", "Open User Settings (JSON)", |ui, send| {
        open_user(ui, send, "settings.json")
    }));
    commands::add(commands::command(
        OPEN_WORKSPACE_JSON,
        "Preferences",
        "Open Workspace Settings (JSON)",
        open_workspace,
    ));
}

/// The file's name at the end of a path.
fn name_of(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// What the status bar says of the settings files' problems: the first, and how many more there are.
fn said_of(problems: &[Problem]) -> String {
    match problems {
        [] => String::new(),
        [first, more @ ..] => {
            let rest = match more.len() {
                0 => String::new(),
                n => format!(" (and {n} more)"),
            };
            format!("{}: {}{rest}", name_of(&first.file), first.error)
        }
    }
}

/// The session's messages that are this part's, taken in: the settings as they are now, one of the person's files
/// read or saved, the project's settings file made. None is taken away from the parts after it (false).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    match m["type"].as_str().unwrap_or("") {
        // The session as it is (at start, and when it or its folder is another): its settings are asked for.
        "state" => send(json!({"type": "settings"})),
        "settings" => {
            let state = ui.state::<Settings>();
            let said = Said::from_json(m);
            let before = state.said.get_untracked();
            if *before != said {
                // What was not taken is said in the status bar when it is news, and unsaid when it is put right.
                if before.problems != said.problems {
                    let (was, is) = (said_of(&before.problems), said_of(&said.problems));
                    let shown = ui.message.get_untracked();
                    if shown != is && (!is.is_empty() || (!was.is_empty() && shown == was)) {
                        ui.message.set(is);
                    }
                }
                state.said.set(Rc::new(said));
            }
        }
        "user_file" => {
            let (name, path) = (s("name"), user_tab(&s("name")));
            // Said to every window, and unasked when the file changes on disk: taken only where its tab is open,
            // and has no changes of its own.
            let Some(tab) = ui.tab(&path) else { return false };
            if let Some(e) = m["error"].as_str() {
                ui.message.set(format!("Could not open {name}: {e}"));
            } else if !tab.dirty.get_untracked() {
                colours_of(ui, &tab);
                ui.replace(&tab, m["text"].as_str().unwrap_or(""));
            }
        }
        "saved_user" => {
            let (name, path) = (s("name"), user_tab(&s("name")));
            if ui.tab(&path).is_none() {
                return false;
            }
            match m["error"].as_str() {
                Some(e) => {
                    if ui.close_saved.with_untracked(|c| c.as_deref() == Some(path.as_str())) {
                        ui.close_saved.set(None);
                    }
                    ui.message.set(format!("Could not save {name}: {e}"));
                }
                None => kinds::saved(ui, &path),
            }
        }
        "workspace_settings" => {
            let asked = ui.state::<Settings>().asked;
            if asked.get_untracked() {
                asked.set(false);
                match m["error"].as_str() {
                    Some(e) => ui.message.set(format!("Could not open the workspace's settings: {e}")),
                    None => ui.open(&s("path"), send),
                }
            }
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use floem::reactive::create_effect;
    use floem::views::editor::core::editor::EditType;
    use floem::views::editor::core::selection::Selection;

    use super::*;

    /// A window with this part in it, and what it tells the session.
    fn window() -> (Ui, Send, Rc<RefCell<Vec<Value>>>) {
        let ui = Ui::new();
        let told = Rc::new(RefCell::new(Vec::new()));
        let send: Send = {
            let to = told.clone();
            Rc::new(move |v: Value| to.borrow_mut().push(v))
        };
        ui.out.set(Some(send.clone()));
        register(ui, &send);
        (ui, send, told)
    }

    fn settings(values: Value, by_lang: Value, problems: Value) -> Value {
        json!({
            "type": "settings",
            "values": values,
            "by_lang": by_lang,
            "sources": {"editor.tabSize": "/home/me/.config/code-arena/settings.json"},
            "problems": problems,
            "files": {"user": "/home/me/.config/code-arena/settings.json", "project": "/w/.code-arena/settings.json", "workspace": "/w/.vscode/settings.json"},
        })
    }

    #[test]
    fn a_setting_is_its_default_until_the_session_says_it_then_the_languages_own_over_the_rest() {
        let (ui, send, told) = window();
        // Nothing said yet: the schema's defaults, so no reader waits.
        assert_eq!(setting(ui, "editor.tabSize", None), json!(4));
        assert_eq!(number(ui, "editor.fontSize", Some("python")), 13.0);
        assert!(flag(ui, "editor.minimap.enabled", None) && !flag(ui, "editor.formatOnSave", None));
        assert_eq!(text(ui, "editor.wordWrap", None), "off");
        assert_eq!(now(ui, "not.a.setting", None), Value::Null);
        // The session's state has its settings asked for.
        take(ui, &json!({"type": "state"}), &send);
        assert_eq!(told.borrow().last(), Some(&json!({"type": "settings"})));
        // What it says is what is read: a language's own first, then the value for every language.
        let said = settings(
            json!({"editor.tabSize": 8, "editor.wordWrap": "on", "editor.fontSize": 15.5}),
            json!({"python": {"editor.tabSize": 2}}),
            json!([]),
        );
        take(ui, &said, &send);
        assert_eq!(
            (setting(ui, "editor.tabSize", None), setting(ui, "editor.tabSize", Some("python"))),
            (json!(8), json!(2))
        );
        assert_eq!(setting(ui, "editor.tabSize", Some("rust")), json!(8));
        assert_eq!(
            (number(ui, "editor.fontSize", None), text(ui, "editor.wordWrap", Some("python"))),
            (15.5, "on".into())
        );
        let state = ui.state::<Settings>();
        assert_eq!(state.said.get_untracked().workspace_file, "/w/.vscode/settings.json");
        assert_eq!(state.said.get_untracked().sources["editor.tabSize"], "/home/me/.config/code-arena/settings.json");
        // A reader is run again when the settings change, and not when the same is said again; a memo tells only
        // when its own setting is another.
        let (runs, tabs) = (Rc::new(RefCell::new(0)), Rc::new(RefCell::new(Vec::new())));
        {
            let runs = runs.clone();
            create_effect(move |_| {
                setting(ui, "editor.wordWrap", None);
                *runs.borrow_mut() += 1;
            });
            let (tabs, size) = (tabs.clone(), memo(ui, "editor.tabSize", || Some("python".to_string())));
            create_effect(move |_| tabs.borrow_mut().push(size.get()));
        }
        take(ui, &said, &send);
        assert_eq!((*runs.borrow(), tabs.borrow().clone()), (1, vec![json!(2)]));
        let other = settings(
            json!({"editor.tabSize": 8, "editor.wordWrap": "off"}),
            json!({"python": {"editor.tabSize": 2}}),
            json!([]),
        );
        take(ui, &other, &send);
        assert_eq!((*runs.borrow(), tabs.borrow().clone()), (2, vec![json!(2)]));
        take(ui, &settings(json!({"editor.tabSize": 8}), json!({"python": {"editor.tabSize": 3}}), json!([])), &send);
        assert_eq!((*runs.borrow(), tabs.borrow().clone()), (3, vec![json!(2), json!(3)]));
        // A setting the session no longer says (an older session) is its default again.
        assert_eq!(text(ui, "editor.wordWrap", None), "off");
    }

    #[test]
    fn what_was_not_taken_is_said_in_the_status_bar_and_unsaid_when_put_right() {
        let (ui, send, _told) = window();
        let wrong = json!([
            {"key": "editor.tabSize", "file": "/home/me/.config/code-arena/settings.json", "error": "\"editor.tabSize\" takes a whole number. Its default holds."},
            {"key": "", "file": "/w/.vscode/settings.json", "error": "not read: expected value at line 1"},
        ]);
        take(ui, &settings(json!({}), json!({}), wrong.clone()), &send);
        let said = "settings.json: \"editor.tabSize\" takes a whole number. Its default holds. (and 1 more)";
        assert_eq!(ui.message.get_untracked(), said);
        assert_eq!(problems(ui).len(), 2);
        assert_eq!(problems(ui)[1].file, "/w/.vscode/settings.json");
        // Put right: the line goes, unless something else is said there meanwhile.
        take(ui, &settings(json!({}), json!({}), json!([])), &send);
        assert_eq!(ui.message.get_untracked(), "");
        take(ui, &settings(json!({}), json!({}), wrong), &send);
        ui.message.set("Saved.".into());
        take(ui, &settings(json!({}), json!({}), json!([])), &send);
        assert_eq!(ui.message.get_untracked(), "Saved.");
    }

    #[test]
    fn the_persons_file_is_opened_in_its_tab_edited_saved_and_follows_the_disk() {
        crate::icons::load();
        let (ui, send, told) = window();
        let last = || told.borrow().last().cloned();
        // The command opens the tab and asks the session for the file.
        assert!(commands::run(ui, &send, OPEN_USER_JSON));
        let tab = ui.tab("user:settings.json").expect("its tab");
        assert_eq!(last(), Some(json!({"type": "user_file", "name": "settings.json"})));
        assert_eq!(
            (kinds::title(ui, &tab), tab.kind, tab.is_file()),
            ("User › settings.json".to_string(), Some("user"), false)
        );
        assert_eq!(ui.active.get_untracked().as_deref(), Some("user:settings.json"));
        // Asked for again, it is only shown: the file is not read a second time.
        let asked = told.borrow().len();
        assert!(commands::run(ui, &send, OPEN_USER_JSON));
        assert_eq!(told.borrow().len(), asked);
        // The file comes: its text, with no changes to save.
        let file = |text: &str| json!({"type": "user_file", "name": "settings.json", "text": text, "modified": 5, "path": "/home/me/.config/code-arena/settings.json"});
        take(ui, &file("{\n  \"editor.tabSize\": 2\n}\n"), &send);
        assert_eq!((tab.text().as_str(), tab.dirty.get_untracked()), ("{\n  \"editor.tabSize\": 2\n}\n", false));
        // It changes on disk while the tab has no changes: the tab takes it.
        take(ui, &file("{\n  \"editor.tabSize\": 4\n}\n"), &send);
        assert_eq!((tab.text().as_str(), tab.dirty.get_untracked()), ("{\n  \"editor.tabSize\": 4\n}\n", false));
        // Edited: it has changes, and what is on disk no longer takes their place.
        let edits = ui.edited.get_untracked();
        tab.doc.edit_single(Selection::caret(0), "// mine\n", EditType::InsertChars);
        assert!(tab.dirty.get_untracked() && ui.edited.get_untracked() == edits + 1);
        take(ui, &file("{}"), &send);
        assert!(tab.text().starts_with("// mine\n{") && tab.dirty.get_untracked());
        // Save sends the text under the file's name; saved, the tab has no changes.
        ui.save(&send);
        assert_eq!(
            last(),
            Some(
                json!({"type": "save_user", "name": "settings.json", "text": "// mine\n{\n  \"editor.tabSize\": 4\n}\n"})
            )
        );
        take(ui, &json!({"type": "saved_user", "name": "settings.json", "error": "read-only"}), &send);
        assert!(tab.dirty.get_untracked());
        assert_eq!(ui.message.get_untracked(), "Could not save settings.json: read-only");
        take(ui, &json!({"type": "saved_user", "name": "settings.json", "modified": 9}), &send);
        assert!(!tab.dirty.get_untracked());
        // Another of the person's files, and one whose reply says it cannot be opened.
        open_user(ui, &send, "keybindings.json");
        assert_eq!(last(), Some(json!({"type": "user_file", "name": "keybindings.json"})));
        take(ui, &json!({"type": "user_file", "name": "keybindings.json", "error": "no"}), &send);
        assert_eq!(ui.message.get_untracked(), "Could not open keybindings.json: no");
        // A file said for a tab that is not open (another window's, or one closed meanwhile) is nobody's.
        take(ui, &json!({"type": "user_file", "name": "snippets/rust.json", "text": "{}"}), &send);
        assert!(ui.tab("user:snippets/rust.json").is_none());
        // The tabs are kept with the layout, and made again from it (the files asked for again).
        let kept = ui.view_state();
        assert_eq!(kept["kind_tabs"], json!(["user:settings.json", "user:keybindings.json"]));
        ui.close_now("user:settings.json");
        ui.close_now("user:keybindings.json");
        told.borrow_mut().clear();
        kinds::restore_all(ui, &send, &kept);
        let again = ui.tab("user:settings.json").expect("made again");
        assert_eq!(told.borrow().iter().filter(|m| m["type"] == "user_file").count(), 2);
        // The tab made again has a text of its own, followed as the first was.
        take(ui, &file("{}"), &send);
        again.doc.edit_single(Selection::caret(0), " ", EditType::InsertChars);
        assert!(again.dirty.get_untracked() && again.text() == " {}");
        // The editor is built for the tab shown.
        let _view = kinds::draw(ui, &again);
    }

    #[test]
    fn the_projects_settings_file_is_opened_once_the_session_has_it() {
        let (ui, send, told) = window();
        // Said for another window's asking: nothing opens here.
        take(ui, &json!({"type": "workspace_settings", "path": ".vscode/settings.json", "made": false}), &send);
        assert!(told.borrow().is_empty());
        assert!(commands::run(ui, &send, OPEN_WORKSPACE_JSON));
        assert_eq!(told.borrow().last(), Some(&json!({"type": "workspace_settings"})));
        // There (made or not): opened as the project's file it is.
        take(ui, &json!({"type": "workspace_settings", "path": ".vscode/settings.json", "made": true}), &send);
        assert_eq!(told.borrow().last(), Some(&json!({"type": "open", "path": ".vscode/settings.json"})));
        // Asked once, opened once.
        take(ui, &json!({"type": "workspace_settings", "path": ".vscode/settings.json", "made": false}), &send);
        assert_eq!(told.borrow().iter().filter(|m| m["type"] == "open").count(), 1);
        assert!(commands::run(ui, &send, OPEN_WORKSPACE_JSON));
        take(
            ui,
            &json!({"type": "workspace_settings", "path": ".vscode/settings.json", "error": "it leads out of the project"}),
            &send,
        );
        assert_eq!(ui.message.get_untracked(), "Could not open the workspace's settings: it leads out of the project");
        // The commands are in the palette under VS Code's names.
        let label = |id: &str| commands::find(id).map(|c| c.label());
        assert_eq!(label(OPEN_USER_JSON).as_deref(), Some("Preferences: Open User Settings (JSON)"));
        assert_eq!(label(OPEN_WORKSPACE_JSON).as_deref(), Some("Preferences: Open Workspace Settings (JSON)"));
    }
}
