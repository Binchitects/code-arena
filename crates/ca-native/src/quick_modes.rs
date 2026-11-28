//! Quick Open's other modes, as VS Code's: what is typed first says what the box is on. Nothing typed first is the
//! files (with ":12" or ":12:5" after a path to open it at a line), ">" the commands, ":" a line of the file shown
//! (Go to Line/Column, Ctrl+G), "@" the symbols of the file shown (Go to Symbol in Editor, Ctrl+Shift+O; "@:" has
//! them by kind) and "?" lists the modes. Quick Open with nothing typed has the files opened last first, and the
//! palette the commands run last ("recently used"), both kept with the folder's view state.

use std::rc::Rc;
use std::time::Duration;

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::text::Document;
use serde_json::{Value, json};

use crate::commands::Command;
use crate::outline::{Kind, Symbol};
use crate::palette::{Files, MOST, Mode, Pick, Query, Row, path_row, positions};
use crate::state::{Send, Ui};

pub const GOTO_LINE: &str = "workbench.action.gotoLine";
pub const GOTO_SYMBOL: &str = "workbench.action.gotoSymbol";

/// How many commands run last are kept (VS Code's `workbench.commandPalette.history`).
pub const RECENT_COMMANDS: usize = 20;
/// How many files opened last are kept.
pub const RECENT_FILES: usize = 50;
/// The most symbols listed: more than the files' fifty (one finds one's way in a long file by them).
const SYMBOLS: usize = 200;

/// A mode of the box: what is typed first, what it is for (as "?" lists it), and the mode.
pub struct Prefix {
    pub prefix: &'static str,
    pub help: &'static str,
    pub mode: Mode,
}

/// The modes, in the order "?" lists them (VS Code's).
pub const PREFIXES: &[Prefix] = &[
    Prefix { prefix: "", help: "Go to File", mode: Mode::Files },
    Prefix { prefix: ">", help: "Show and Run Commands", mode: Mode::Commands },
    Prefix { prefix: ":", help: "Go to Line/Column", mode: Mode::Line },
    Prefix { prefix: "@", help: "Go to Symbol in Editor", mode: Mode::Symbols },
    Prefix { prefix: "?", help: "Help", mode: Mode::Help },
];

/// The mode what is typed is in: the longest prefix it begins with, else the files.
pub fn mode_of(text: &str) -> Mode {
    PREFIXES
        .iter()
        .filter(|p| !p.prefix.is_empty() && text.starts_with(p.prefix))
        .max_by_key(|p| p.prefix.len())
        .map_or(Mode::Files, |p| p.mode)
}

/// What is typed after the mode's prefix.
pub fn rest_of(text: &str, mode: Mode) -> &str {
    let prefix = PREFIXES.iter().find(|p| p.mode == mode).map_or("", |p| p.prefix);
    text.strip_prefix(prefix).unwrap_or(text)
}

/// A line and maybe a column, from 1, as typed after ":" ("12", "12:5", "12,5"); none when it is not that.
pub fn line_target(text: &str) -> Option<(usize, Option<usize>)> {
    let text = text.trim();
    let (line, col) = match text.find([':', ',']) {
        Some(i) => (&text[..i], Some(text[i + 1..].trim())),
        None => (text, None),
    };
    let line: usize = line.trim().parse().ok().filter(|l| *l > 0)?;
    let col = match col {
        Some(c) => Some(c.parse::<usize>().ok().filter(|c| *c > 0)?),
        None => None,
    };
    Some((line, col))
}

/// A path typed with a line after it ("src/main.rs:12" or ":12:5"): the path, and the line and column.
pub fn path_line(text: &str) -> (&str, Option<(usize, Option<usize>)>) {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(i) = text.rfind(':') {
        let (head, tail) = (&text[..i], &text[i + 1..]);
        if digits(tail) {
            // "path:12:5": the column after the line.
            if let Some(j) = head.rfind(':')
                && digits(&head[j + 1..])
                && j > 0
            {
                return (&head[..j], line_target(&text[j + 1..]));
            }
            if !head.is_empty() {
                return (head, line_target(tail));
            }
        }
    }
    (text, None)
}

/// A row that is only something said (nothing is done when it is chosen).
fn said(text: &str) -> Row {
    Row::new(text, "", Vec::new(), "", Pick::Nothing)
}

/// Go to Line's row for what is typed after ":": where the caret is and how many lines there are while nothing is
/// typed, the line it goes to once one is, what may be typed when the number is not a line of the file.
pub fn line_rows(rest: &str, caret: (usize, usize), lines: Option<usize>) -> Vec<Row> {
    let Some(lines) = lines else { return vec![said("Open a text editor first to go to a line.")] };
    let range = format!("Type a line number between 1 and {lines} to navigate to.");
    if rest.trim().is_empty() {
        let (l, c) = caret;
        return vec![said(&format!("Current Line: {l}, Character: {c}. {range}"))];
    }
    match line_target(rest) {
        Some((line, col)) if line <= lines => {
            let text = match col {
                Some(c) => format!("Go to line {line} and character {c}."),
                None => format!("Go to line {line}."),
            };
            vec![Row::new(&text, "", Vec::new(), "", Pick::Line(line, col.unwrap_or(1)))]
        }
        _ => vec![said(&range)],
    }
}

/// The name of a kind of symbol as VS Code heads its group, many of them.
fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Function => "functions",
        Kind::Method => "methods",
        Kind::Class => "classes",
        Kind::Struct => "structs",
        Kind::Enum => "enumerations",
        Kind::Interface => "interfaces",
        Kind::Type => "type parameters",
        Kind::Module => "modules",
        Kind::Constant => "constants",
        Kind::Variable => "variables",
        Kind::Field => "fields",
        Kind::Heading => "strings",
    }
}

/// Rows in groups: the first row of each has the group's name (and how many it has) at its right, and the first
/// of each group after the first a line over it. Empty groups are left out.
pub fn grouped(groups: Vec<(String, Vec<Row>)>) -> Vec<Row> {
    let mut out = Vec::new();
    for (name, rows) in groups.into_iter().filter(|(_, r)| !r.is_empty()) {
        let line = !out.is_empty();
        for (n, row) in rows.into_iter().enumerate() {
            out.push(if n == 0 { row.heading(&name, line) } else { row });
        }
    }
    out
}

/// Go to Symbol's rows for what is typed after "@": the file's symbols that match, in their order under
/// "symbols (N)", or with ":" first by kind, each kind under its name. Choosing one goes to its line.
pub fn symbol_rows(symbols: Option<&[Symbol]>, rest: &str) -> Vec<Row> {
    let Some(symbols) = symbols else {
        return vec![said("To go to a symbol, first open a text editor with symbol information.")];
    };
    if symbols.is_empty() {
        return vec![said("The active text editor does not provide symbol information.")];
    }
    let (by_kind, typed) = match rest.strip_prefix(':') {
        Some(t) => (true, t),
        None => (false, rest),
    };
    let q = Query::of(typed);
    let names = Files::of(symbols.iter().map(|s| s.name.as_str()), false);
    let mut order = names.rank(&q, SYMBOLS);
    if q.is_empty() || by_kind {
        // In the file's order (the best are kept when there are more than are listed).
        order.sort_unstable();
    }
    let row = |i: usize| {
        let s = &symbols[i];
        let indent = "  ".repeat(s.depth.min(8));
        let lit = positions(&s.name, &q).into_iter().map(|r| r.start + indent.len()..r.end + indent.len()).collect();
        Row::new(&format!("{indent}{}", s.name), "", lit, "", Pick::Line(s.line, 1)).kind(s.kind)
    };
    if !by_kind {
        let rows: Vec<Row> = order.into_iter().map(row).collect();
        let name = format!("symbols ({})", rows.len());
        return grouped(vec![(name, rows)]);
    }
    let mut kinds: Vec<Kind> = Vec::new();
    for i in &order {
        if !kinds.contains(&symbols[*i].kind) {
            kinds.push(symbols[*i].kind);
        }
    }
    grouped(
        kinds
            .into_iter()
            .map(|k| {
                let rows: Vec<Row> = order.iter().copied().filter(|i| symbols[*i].kind == k).map(row).collect();
                (format!("{} ({})", kind_name(k), rows.len()), rows)
            })
            .collect(),
    )
}

/// "?": the modes, each with what it is for; choosing one types its prefix.
pub fn help_rows(rest: &str) -> Vec<Row> {
    let q = Query::of(rest);
    let mut found = Files::of(PREFIXES.iter().map(|p| p.help), false).rank(&q, PREFIXES.len());
    found.sort_unstable();
    found
        .into_iter()
        .map(|i| &PREFIXES[i])
        .map(|p| {
            let name = if p.prefix.is_empty() { "…" } else { p.prefix };
            Row::new(name, p.help, Vec::new(), "", Pick::Prefix(p.prefix.to_string()))
        })
        .collect()
}

/// Quick Open's rows for what is typed: the files opened last that match first ("recently opened"), then the
/// others ("file results"); a line typed after the path is where the chosen file opens.
pub fn file_rows(files: &Files, text: &str, recent: &[String]) -> Vec<Row> {
    let (path, at) = path_line(text);
    let q = Query::of(path);
    let pick = |p: &str| match at {
        Some((line, col)) => Pick::FileAt(p.to_string(), line, col.unwrap_or(1)),
        None => Pick::File(p.to_string()),
    };
    let row = |p: &str| {
        let mut r = path_row(p, &q);
        r.pick = pick(p);
        r
    };
    let mine = Files::of(recent.iter().map(String::as_str), false);
    let first: Vec<Row> = mine.rank(&q, MOST).into_iter().map(|i| row(mine.path(i))).collect();
    let others: Vec<Row> = files
        .rank(&q, MOST + first.len())
        .into_iter()
        .map(|i| files.path(i))
        .filter(|p| !recent.iter().any(|r| r == p))
        .take(MOST - first.len().min(MOST))
        .map(row)
        .collect();
    if first.is_empty() {
        return others;
    }
    grouped(vec![("recently opened".into(), first), ("file results".into(), others)])
}

/// The row Quick Open chooses first: with nothing typed, the file opened before the one shown when the one shown
/// heads the list (VS Code's Ctrl+P then Enter goes back to it), else the first.
pub fn first_chosen(rows: &[Row], text: &str, shown: Option<&str>) -> usize {
    let heads = matches!((rows.first().map(|r| &r.pick), shown), (Some(Pick::File(p)), Some(s)) if p == s);
    usize::from(text.trim().is_empty() && heads && rows.len() > 1)
}

/// Whether a command matches what is typed by its id ("workbench.action.files.save"), or by its words in any order
/// ("save file" for "File: Save").
fn matches_otherwise(c: &Command, typed: &str) -> bool {
    let typed = typed.trim().to_lowercase();
    if typed.is_empty() {
        return false;
    }
    if !typed.contains(' ') && c.id.to_lowercase().contains(&typed) {
        return true;
    }
    let label = c.label().to_lowercase();
    typed.split_whitespace().count() > 1 && typed.split_whitespace().all(|w| label.contains(w))
}

/// The palette's rows for what is typed after ">": the commands run last that match ("recently used", the last
/// first), then the others that match ("other commands") by their letters, then by their id or their words.
pub fn command_rows(all: &[Rc<Command>], text: &str, recent: &[String]) -> Vec<Row> {
    let q = Query::of(text);
    let names: Vec<String> = all.iter().map(|c| c.label()).collect();
    let labels = Files::of(names.iter().map(String::as_str), false);
    let mut order = labels.rank(&q, MOST);
    if !q.is_empty() {
        for (i, c) in all.iter().enumerate() {
            if order.len() >= MOST {
                break;
            }
            if !order.contains(&i) && matches_otherwise(c, text) {
                order.push(i);
            }
        }
    }
    let row = |i: usize| {
        let (c, label) = (&all[i], &names[i]);
        Row::new(label, "", positions(label, &q), &crate::commands::keys_of(&c.id), Pick::Command(c.id.clone()))
    };
    // The commands run last that are listed now, the last first (one not there any more is passed over).
    let place = |id: &str| all.iter().position(|c| c.id == id);
    let used: Vec<usize> = if q.is_empty() {
        recent.iter().filter_map(|id| place(id)).collect()
    } else {
        recent.iter().filter_map(|id| place(id)).filter(|i| order.contains(i)).collect()
    };
    if used.is_empty() {
        return order.into_iter().map(row).collect();
    }
    let others: Vec<Row> = order.into_iter().filter(|i| !used.contains(i)).take(MOST).map(row).collect();
    grouped(vec![("recently used".into(), used.into_iter().map(row).collect()), ("other commands".into(), others)])
}

/// A name put first in a list kept to `most`, once.
pub fn put_first(list: &mut Vec<String>, name: &str, most: usize) {
    list.retain(|n| n != name);
    list.insert(0, name.to_string());
    list.truncate(most);
}

/// The files opened last and the commands run last, the last first.
#[derive(Clone, Copy)]
pub struct Recent {
    pub files: RwSignal<Vec<String>>,
    pub commands: RwSignal<Vec<String>>,
    /// The symbols asked for, by the file they are of (read off the window's thread once per file shown).
    asked: RwSignal<Option<String>>,
    /// The editor drawn has the keys now, and had them when the box was opened (they go back to it after a command).
    editor_keys: RwSignal<bool>,
    pub back_to_editor: RwSignal<bool>,
    restored: RwSignal<bool>,
}

impl crate::ext::State for Recent {
    fn new(ui: Ui) -> Self {
        let s = ui.scope;
        Recent {
            files: s.create_rw_signal(Vec::new()),
            commands: s.create_rw_signal(Vec::new()),
            asked: s.create_rw_signal(None),
            editor_keys: s.create_rw_signal(false),
            back_to_editor: s.create_rw_signal(false),
            restored: s.create_rw_signal(false),
        }
    }
}

pub fn recent(ui: Ui) -> Recent {
    ui.state::<Recent>()
}

/// The box opened: whether the keys go back to the editor drawn after a command (they were in it).
pub fn opened(ui: Ui) {
    let st = recent(ui);
    st.back_to_editor.set(st.editor_keys.get_untracked());
}

/// The keys given back to the editor drawn, when they were in it as the box was opened.
pub fn give_back(ui: Ui) {
    let st = recent(ui);
    if st.back_to_editor.get_untracked()
        && let Some(id) = ui.live.with_untracked(|l| l.as_ref().and_then(|(_, e)| e.editor_view_id.get_untracked()))
    {
        id.request_focus();
    }
}

/// A command run from the palette: first among those run last, and kept.
pub fn ran(ui: Ui, send: &Send, id: &str) {
    recent(ui).commands.update(|c| put_first(c, id, RECENT_COMMANDS));
    if ui.loaded.get_untracked() && !ui.running.get_untracked() {
        send(json!({"type": "keep_view_state", "state": ui.view_state()}));
    }
}

/// The symbols of the file shown, for "@": the Outline's when they are of it, else read off the window's thread
/// (once for the file; the answer is the Outline's then too). None while no file is shown; none inside while they
/// are being read.
pub fn symbols_shown(ui: Ui) -> Option<Option<Rc<Vec<Symbol>>>> {
    let active = ui.active.get()?;
    let tab = ui.tab(&active).filter(|t| t.is_file())?;
    // Read (and so watched by the effect asking) before the reading is begun: an effect made while another runs
    // (the answer's) leaves what that one reads after it unwatched.
    let shown = ui.symbols.with(|(p, s)| (*p == active).then(|| s.clone()));
    let st = recent(ui);
    // Read again each time "@" is opened (the Outline's may be of the text before an edit, when it is not shown).
    if st.asked.with_untracked(|a| a.as_deref() != Some(active.as_str())) {
        st.asked.set(Some(active.clone()));
        let (path, rope) = (active.clone(), tab.doc.text());
        let done = floem::ext_event::create_ext_action(ui.scope, move |found: Vec<Symbol>| {
            // Unless another file is shown meanwhile, or they are the ones there.
            if ui.active.with_untracked(|a| a.as_deref() == Some(path.as_str()))
                && ui.symbols.with_untracked(|(p, s)| *p != path || **s != found)
            {
                ui.symbols.set((path, Rc::new(found)));
            }
        });
        let file = active.clone();
        std::thread::spawn(move || done(crate::outline::symbols(&file, &rope.to_string())));
    }
    Some(shown)
}

/// The symbols read again the next time "@" is shown.
pub fn forget_symbols(ui: Ui) {
    recent(ui).asked.set(None);
}

/// The lines of the file shown, when its editor is the one drawn.
pub fn lines_shown(ui: Ui) -> Option<usize> {
    let active = ui.active.get()?;
    ui.live.with(|l| l.as_ref().filter(|(p, _)| *p == active).map(|(_, e)| e.last_line() + 1))
}

/// The caret put at a line and a column (from 1) of the editor drawn, when it shows the file, in the middle of it,
/// and the editor given the keys. False when another file is drawn.
pub fn put_caret(ui: Ui, path: &str, line: usize, col: usize) -> bool {
    let Some(editor) = ui.live.with_untracked(|l| l.as_ref().filter(|(p, _)| p == path).map(|(_, e)| e.clone())) else {
        return false;
    };
    let line = line.saturating_sub(1).min(editor.last_line());
    let start = editor.offset_of_line(line);
    // Not past the line's end (its line break).
    let rope = editor.rope_text();
    let end = rope.line_end_offset(line, true).max(start);
    // The column counts characters, as VS Code's does (never inside one written in several bytes).
    let text = rope.slice_to_cow(start..end);
    let at = start + text.char_indices().nth(col.saturating_sub(1)).map_or(text.len(), |(i, _)| i);
    editor.cursor.update(|c| c.set_insert(floem::views::editor::core::selection::Selection::caret(at)));
    if let Some(id) = editor.editor_view_id.get_untracked() {
        id.request_focus();
    }
    let (e, path) = (editor.clone(), path.to_string());
    floem::action::exec_after(Duration::from_millis(30), move |_| {
        if e.cursor.try_get_untracked().is_some() {
            crate::editor::folding::center(&e, &path);
        }
    });
    true
}

/// A file opened at a line and a column: at once when it is drawn, else once its editor is (asked a few times
/// while the file comes; given up after a second).
pub fn open_at(ui: Ui, send: &Send, path: &str, line: usize, col: usize) {
    if put_caret(ui, path, line, col) {
        return;
    }
    ui.open_at(path, line, send);
    let path = path.to_string();
    fn retry(ui: Ui, path: String, line: usize, col: usize, left: u32) {
        floem::action::exec_after(Duration::from_millis(50), move |_| {
            if !put_caret(ui, &path, line, col) && left > 0 {
                retry(ui, path, line, col, left - 1);
            }
        });
    }
    retry(ui, path, line, col, 20);
}

/// What is kept with the folder's view state (`Ui::view_state`).
pub fn kept(ui: Ui) -> Value {
    let st = recent(ui);
    json!({"files": st.files.get_untracked(), "commands": st.commands.get_untracked()})
}

fn restore(ui: Ui, v: &Value) {
    let st = recent(ui);
    // The first state only (a session switched keeps the window's own).
    if st.restored.get_untracked() {
        return;
    }
    st.restored.set(true);
    let list = |k: &str, most: usize| -> Vec<String> {
        v[k].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).take(most).collect()
    };
    st.files.set(list("files", RECENT_FILES));
    st.commands.set(list("commands", RECENT_COMMANDS));
}

/// What it adds to the window's registries, once at start: Go to Line/Column and Go to Symbol in Editor, in the
/// palette and the Go menu; the files opened, followed.
pub fn register(ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::menus::{GO, Menu, item};
    use crate::palette::open;
    add(command(GOTO_LINE, "", "Go to Line/Column...", |ui, send| open(ui, send, Mode::Line)).keys("Ctrl+G"));
    add(command(GOTO_SYMBOL, "", "Go to Symbol in Editor...", |ui, send| open(ui, send, Mode::Symbols))
        .keys("Ctrl+Shift+O"));
    crate::menus::add(Menu::TitleBar(GO), item(GOTO_SYMBOL).group("4_symbol_nav").order(2));
    crate::menus::add(Menu::TitleBar(GO), item(GOTO_LINE).group("5_infile_nav").order(1));
    let st = recent(ui);
    // Whether the editor drawn has the keys: followed on its own focus signals, in a scope of its own that goes when
    // another editor is drawn.
    floem::reactive::create_effect(move |was: Option<Option<floem::reactive::Scope>>| {
        if let Some(Some(old)) = was {
            old.dispose();
        }
        let (gained, lost) =
            ui.live.with(|l| l.as_ref().map(|(_, e)| (e.editor_view_focused, e.editor_view_focus_lost)))?;
        let cx = ui.scope.create_child();
        floem::reactive::with_scope(cx, || {
            floem::reactive::create_effect(move |first: Option<()>| {
                gained.track();
                if first.is_some() {
                    st.editor_keys.set(true);
                }
            });
            floem::reactive::create_effect(move |first: Option<()>| {
                lost.track();
                if first.is_some() {
                    st.editor_keys.set(false);
                }
            });
        });
        Some(cx)
    });
    floem::reactive::create_effect(move |_| {
        let Some(path) = ui.active.get() else { return };
        if ui.tab(&path).is_some_and(|t| t.is_file())
            && st.files.with_untracked(|f| f.first().map(String::as_str) != Some(path.as_str()))
        {
            st.files.update(|f| put_first(f, &path, RECENT_FILES));
        }
    });
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "state" {
        restore(ui, &m["view_state"]["quick"]);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| r.name.clone()).collect()
    }

    #[test]
    fn what_is_typed_first_says_the_mode() {
        assert_eq!(mode_of(""), Mode::Files);
        assert_eq!(mode_of("src/main"), Mode::Files);
        assert_eq!(mode_of(">save"), Mode::Commands);
        assert_eq!(mode_of(":12"), Mode::Line);
        assert_eq!(mode_of("@total"), Mode::Symbols);
        assert_eq!(mode_of("?"), Mode::Help);
        assert_eq!(rest_of(":12:5", Mode::Line), "12:5");
        assert_eq!(rest_of("main", Mode::Files), "main");
    }

    #[test]
    fn a_line_and_a_column_are_read_as_vs_code_reads_them() {
        assert_eq!(line_target("12"), Some((12, None)));
        assert_eq!(line_target(" 12:5 "), Some((12, Some(5))));
        assert_eq!(line_target("12,5"), Some((12, Some(5))));
        assert_eq!(line_target("0"), None);
        assert_eq!(line_target("x"), None);
        assert_eq!(line_target("12:x"), None);
        assert_eq!(path_line("src/main.rs:12"), ("src/main.rs", Some((12, None))));
        assert_eq!(path_line("src/main.rs:12:5"), ("src/main.rs", Some((12, Some(5)))));
        assert_eq!(path_line("src/main.rs"), ("src/main.rs", None));
        assert_eq!(path_line("main:"), ("main:", None));
    }

    #[test]
    fn go_to_line_says_where_the_caret_is_then_the_line_it_goes_to() {
        let rows = line_rows("", (3, 7), Some(40));
        assert_eq!(
            names(&rows),
            ["Current Line: 3, Character: 7. Type a line number between 1 and 40 to navigate to."]
        );
        assert!(matches!(rows[0].pick, Pick::Nothing));
        let rows = line_rows("12", (3, 7), Some(40));
        assert_eq!(names(&rows), ["Go to line 12."]);
        assert!(matches!(rows[0].pick, Pick::Line(12, 1)));
        let rows = line_rows("12:5", (3, 7), Some(40));
        assert_eq!(names(&rows), ["Go to line 12 and character 5."]);
        assert!(matches!(rows[0].pick, Pick::Line(12, 5)));
        assert!(matches!(line_rows("41", (1, 1), Some(40))[0].pick, Pick::Nothing));
        assert_eq!(names(&line_rows("4", (1, 1), None)), ["Open a text editor first to go to a line."]);
    }

    fn sym(name: &str, kind: Kind, line: usize, depth: usize) -> Symbol {
        Symbol { name: name.into(), kind, line, depth, detail: String::new() }
    }

    #[test]
    fn the_symbols_are_listed_in_their_order_or_by_kind() {
        let all = vec![
            sym("Cart", Kind::Struct, 1, 0),
            sym("total", Kind::Method, 5, 1),
            sym("Kind", Kind::Enum, 9, 0),
            sym("tax", Kind::Method, 12, 1),
        ];
        let rows = symbol_rows(Some(&all), "");
        assert_eq!(names(&rows), ["Cart", "  total", "Kind", "  tax"]);
        assert_eq!(rows[0].group, "symbols (4)");
        assert!(rows[1..].iter().all(|r| r.group.is_empty()));
        assert!(matches!(rows[1].pick, Pick::Line(5, 1)));
        // Typed: the ones that match, best first, the letters lit past the indent.
        let rows = symbol_rows(Some(&all), "ta");
        assert_eq!(names(&rows)[0], "  tax");
        assert_eq!(rows[0].lit, vec![2..3, 3..4]);
        // By kind: each kind under its name, in the order its first symbol comes, a line between the kinds.
        let rows = symbol_rows(Some(&all), ":");
        assert_eq!(names(&rows), ["Cart", "  total", "  tax", "Kind"]);
        let heads: Vec<(&str, bool)> = rows.iter().map(|r| (r.group.as_str(), r.line)).collect();
        assert_eq!(heads, [("structs (1)", false), ("methods (2)", true), ("", false), ("enumerations (1)", true)]);
        assert_eq!(rows[0].kind, Some(Kind::Struct));
        // No editor, no symbols.
        assert!(matches!(symbol_rows(None, "")[0].pick, Pick::Nothing));
        assert_eq!(names(&symbol_rows(Some(&[]), "")), ["The active text editor does not provide symbol information."]);
    }

    #[test]
    fn the_help_lists_the_modes_and_types_the_one_chosen() {
        let rows = help_rows("");
        assert_eq!(names(&rows), ["…", ">", ":", "@", "?"]);
        assert_eq!(rows[2].detail, "Go to Line/Column");
        assert!(matches!(&rows[3].pick, Pick::Prefix(p) if p == "@"));
        assert_eq!(names(&help_rows("symbol")), ["@"]);
    }

    #[test]
    fn quick_open_with_nothing_typed_chooses_the_file_before_the_one_shown() {
        let files = Files::of(["a.rs", "b.rs"].into_iter(), false);
        let recent = vec!["b.rs".to_string(), "a.rs".to_string()];
        let rows = file_rows(&files, "", &recent);
        assert_eq!(first_chosen(&rows, "", Some("b.rs")), 1);
        assert_eq!(first_chosen(&rows, "", Some("a.rs")), 0, "the one shown is not first");
        assert_eq!(first_chosen(&rows, "", None), 0);
        assert_eq!(first_chosen(&file_rows(&files, "b", &recent), "b", Some("b.rs")), 0, "something typed");
    }

    #[test]
    fn the_files_opened_last_come_first_and_a_line_typed_opens_there() {
        let files = Files::of(["src/a.rs", "src/b.rs", "src/main.rs"].into_iter(), false);
        let recent = vec!["src/main.rs".to_string()];
        let rows = file_rows(&files, "", &recent);
        assert_eq!(names(&rows), ["main.rs", "a.rs", "b.rs"]);
        assert_eq!((rows[0].group.as_str(), rows[1].group.as_str()), ("recently opened", "file results"));
        assert!(rows[1].line);
        // None opened yet: no headings.
        assert!(file_rows(&files, "", &[]).iter().all(|r| r.group.is_empty()));
        let rows = file_rows(&files, "b.rs:12:5", &recent);
        assert_eq!(names(&rows), ["b.rs"]);
        assert!(matches!(&rows[0].pick, Pick::FileAt(p, 12, 5) if p == "src/b.rs"));
    }

    #[test]
    fn the_box_opens_on_a_mode_by_its_command_and_its_key() {
        use crate::palette::{accept, open, refresh, shown};
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        register(ui, &send);
        crate::workbench::register(ui, &send);
        open(ui, &send, Mode::Line);
        assert_eq!(shown(ui), Some(Mode::Line));
        refresh(ui);
        assert_eq!(ui.quick_rows.with_untracked(|r| names(r)), ["Open a text editor first to go to a line."]);
        open(ui, &send, Mode::Symbols);
        assert_eq!(shown(ui), Some(Mode::Symbols));
        // "?" then a mode chosen: its prefix typed.
        ui.quick_text.set("?".into());
        refresh(ui);
        accept(ui, &send, 2);
        assert_eq!(ui.quick_text.get_untracked(), ":");
        crate::palette::close(ui);
        let press = |k: floem::keyboard::Key, m| crate::commands::press(ui, &send, &k, m);
        use floem::keyboard::{Key, Modifiers, NamedKey};
        press(Key::Character("g".into()), Modifiers::CONTROL);
        assert_eq!(shown(ui), Some(Mode::Line));
        press(Key::Character("O".into()), Modifiers::CONTROL | Modifiers::SHIFT);
        assert_eq!(shown(ui), Some(Mode::Symbols));
        press(Key::Named(NamedKey::F1), Modifiers::empty());
        assert_eq!(shown(ui), Some(Mode::Commands));
    }

    #[test]
    fn a_name_put_first_is_there_once_and_the_list_is_kept_short() {
        let mut l = vec!["a".to_string(), "b".into(), "c".into()];
        put_first(&mut l, "c", 3);
        assert_eq!(l, ["c", "a", "b"]);
        put_first(&mut l, "d", 3);
        assert_eq!(l, ["d", "c", "a"]);
    }

    #[test]
    fn the_commands_run_last_are_first_and_kept_with_the_folder() {
        let ui = Ui::new();
        let all = [("a.one", "View", "Show One"), ("b.two", "File", "Save"), ("c.three", "View", "Show Three")]
            .map(|(id, cat, t)| Rc::new(crate::commands::command(id, cat, t, |_, _| {})));
        let rows = command_rows(&all, "", &["c.three".into(), "gone".into()]);
        assert_eq!(names(&rows), ["View: Show Three", "View: Show One", "File: Save"]);
        assert_eq!((rows[0].group.as_str(), rows[1].group.as_str()), ("recently used", "other commands"));
        // Typed: the ones run last that match, then the others; by the id, by the words in any order.
        let rows = command_rows(&all, "show", &["c.three".into()]);
        assert_eq!(names(&rows), ["View: Show Three", "View: Show One"]);
        assert_eq!(names(&command_rows(&all, "b.two", &[])), ["File: Save"]);
        assert_eq!(names(&command_rows(&all, "three show", &[])), ["View: Show Three"]);
        // Kept and read back; a second state leaves them.
        let send: Send = Rc::new(|_| {});
        ran(ui, &send, "b.two");
        let kept = ui.view_state();
        assert_eq!(kept["quick"]["commands"], json!(["b.two"]));
        let again = Ui::new();
        take(again, &json!({"type": "state", "view_state": kept}), &send);
        assert_eq!(recent(again).commands.get_untracked(), ["b.two"]);
        take(again, &json!({"type": "state", "view_state": {"quick": {"commands": ["x"]}}}), &send);
        assert_eq!(recent(again).commands.get_untracked(), ["b.two"]);
    }
}
