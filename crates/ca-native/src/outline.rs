//! The Outline view, as VS Code's: the shown file's symbols (its functions, types, modules, constants, a document's
//! headings) as a tree by their nesting, each 22 px row with its symbol icon in the icon's own colour; a click goes
//! to the symbol's line. The symbols are the names the file's grammar marks as defined (`entity.name.*`), found off
//! the window's thread, a moment after the last edit.

use std::collections::HashSet;
use std::rc::Rc;

use floem::IntoView;
use floem::event::EventListener;
use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_memo, create_rw_signal};
use floem::style::CursorStyle;
use floem::views::editor::text::Document;
use floem::views::{
    Decorators, VirtualDirection, VirtualItemSize, container, empty, h_stack, label, scroll, text_input, tooltip,
    virtual_stack,
};
use syntect::parsing::{ParseState, Scope, ScopeStack};

use crate::colors::Colors;
use crate::highlight::{SYNTAXES, syntax_for};
use crate::icons::icon;
use crate::layout::View;
use crate::state::{Send, Ui};

/// What a symbol is, as VS Code's outline tells them apart by icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Interface,
    Type,
    Module,
    Constant,
    Variable,
    Field,
    Heading,
}

impl Kind {
    /// By the part of the grammar's scope after `entity.name` (`entity.name.function.rust`: `function`); none for
    /// the names that are not definitions of the file's own (a tag, a label, a lifetime).
    fn of(part: &str) -> Option<Kind> {
        Some(match part {
            "function" | "macro" => Kind::Function,
            "method" => Kind::Method,
            "class" | "impl" => Kind::Class,
            "struct" | "union" => Kind::Struct,
            "enum" => Kind::Enum,
            "trait" | "interface" | "protocol" => Kind::Interface,
            "type" => Kind::Type,
            "module" | "namespace" | "package" => Kind::Module,
            "constant" => Kind::Constant,
            "variable" => Kind::Variable,
            "field" | "property" => Kind::Field,
            "section" => Kind::Heading,
            _ => return None,
        })
    }

    /// VS Code's codicon for it.
    pub fn icon(self) -> &'static str {
        match self {
            Kind::Function | Kind::Method => "symbol-method",
            Kind::Class => "symbol-class",
            Kind::Struct | Kind::Type => "symbol-struct",
            Kind::Enum => "symbol-enum",
            Kind::Interface => "symbol-interface",
            Kind::Module => "symbol-namespace",
            Kind::Constant => "symbol-constant",
            Kind::Variable => "symbol-variable",
            Kind::Field => "symbol-field",
            Kind::Heading => "symbol-string",
        }
    }

    /// Its name, as a list of symbols tells it beside each (the breadcrumbs' picker).
    pub fn word(self) -> &'static str {
        match self {
            Kind::Function => "function",
            Kind::Method => "method",
            Kind::Class => "class",
            Kind::Struct => "struct",
            Kind::Enum => "enum",
            Kind::Interface => "interface",
            Kind::Type => "type",
            Kind::Module => "module",
            Kind::Constant => "constant",
            Kind::Variable => "variable",
            Kind::Field => "field",
            Kind::Heading => "heading",
        }
    }

    /// Its icon's colour (VS Code's `symbolIcon.*Foreground`).
    pub fn colour(self, c: &Colors) -> floem::peniko::Color {
        match self {
            Kind::Function => c.symbol_function,
            Kind::Method => c.symbol_method,
            Kind::Class => c.symbol_class,
            Kind::Struct | Kind::Type => c.symbol_struct,
            Kind::Enum => c.symbol_enum,
            Kind::Interface => c.symbol_interface,
            Kind::Module => c.symbol_namespace,
            Kind::Constant => c.symbol_constant,
            Kind::Variable => c.symbol_variable,
            Kind::Field => c.symbol_field,
            Kind::Heading => c.symbol_string,
        }
    }

    /// Whether what is defined inside it is its member (a function in it is a method).
    fn holds_members(self) -> bool {
        matches!(self, Kind::Class | Kind::Struct | Kind::Enum | Kind::Interface)
    }
}

/// A symbol of a file: its name, its line (from 1) and how deep it is nested (by its indentation, a heading by its
/// level).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Symbol {
    pub name: String,
    pub kind: Kind,
    pub line: usize,
    pub depth: usize,
    /// The line it is defined on, trimmed and cut short (its signature, shown when the pointer rests on its row).
    pub detail: String,
}

/// Files bigger than this get no outline (a bundle, a data file: not what one finds one's way in by symbol).
const MOST: usize = 2_000_000;

/// The file's symbols in order: the names its grammar marks as defined. Nothing for plain text.
pub fn symbols(path: &str, text: &str) -> Vec<Symbol> {
    let first = text.lines().next().unwrap_or("");
    let Some(syntax) = syntax_for(path, first) else { return Vec::new() };
    if text.len() > MOST {
        return Vec::new();
    }
    let entity = Scope::new("entity.name").unwrap_or_default();
    let heading = Scope::new("markup.heading").unwrap_or_default();
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out: Vec<Symbol> = Vec::new();
    // The symbols this one is inside of: each one's indentation and kind.
    let mut inside: Vec<(usize, Kind)> = Vec::new();
    for (n, line) in text.split_inclusive('\n').enumerate() {
        let Ok(ops) = state.parse_line(line, &SYNTAXES) else { break };
        // The runs of the line that name something, joined when they touch (a heading with a word in italics).
        let mut named: Vec<(Kind, usize, usize, usize)> = Vec::new();
        let mut note = |stack: &ScopeStack, from: usize, to: usize| {
            if from >= to {
                return;
            }
            let scopes = stack.as_slice();
            let Some(kind) = scopes.iter().rev().find(|s| entity.is_prefix_of(**s)).and_then(|s| {
                let name = s.build_string();
                Kind::of(name.split('.').nth(2).unwrap_or(""))
            }) else {
                return;
            };
            let level = scopes
                .iter()
                .find(|s| heading.is_prefix_of(**s))
                .and_then(|s| s.build_string().split('.').nth(2).and_then(|l| l.parse::<usize>().ok()))
                .unwrap_or(1);
            match named.last_mut() {
                Some((k, _, end, _)) if *k == kind && *end == from => *end = to,
                _ => named.push((kind, from, to, level)),
            }
        };
        let mut at = 0;
        for (pos, op) in &ops {
            note(&stack, at, *pos);
            if stack.apply(op).is_err() {
                return out;
            }
            at = *pos;
        }
        note(&stack, at, line.len());
        let indent = line.len() - line.trim_start().len();
        if named.is_empty()
            && let Some(name) = rust_constant(line, &stack)
        {
            named.push((Kind::Constant, name.0, name.1, 1));
        }
        // A heading's level by its marks (the grammar numbers the first two only).
        let marks = line.trim_start().bytes().take_while(|b| *b == b'#').count();
        for (kind, from, to, level) in named {
            let name = line.get(from..to).unwrap_or("").trim().trim_end_matches('#').trim_end();
            if name.is_empty() {
                continue;
            }
            let (kind, depth) = if kind == Kind::Heading {
                (kind, if marks > 0 { marks } else { level }.saturating_sub(1))
            } else {
                while inside.last().is_some_and(|(i, _)| *i >= indent) {
                    inside.pop();
                }
                let method = kind == Kind::Function && inside.last().is_some_and(|(_, k)| k.holds_members());
                let kind = if method { Kind::Method } else { kind };
                let depth = inside.len();
                inside.push((indent, kind));
                (kind, depth)
            };
            out.push(Symbol { name: name.to_string(), kind, line: n + 1, depth, detail: detail_of(line) });
        }
    }
    out
}

/// A definition's line as its tooltip shows it: trimmed, its opening brace or colon left off, at most 200 characters.
fn detail_of(line: &str) -> String {
    let line = line.trim();
    let line = line.strip_suffix('{').or_else(|| line.strip_suffix(':')).unwrap_or(line).trim_end();
    match line.char_indices().nth(200) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_string(),
    }
}

/// Rust's grammar does not mark a constant's name as defined: `const NAME:` and `static NAME:` are read here, in
/// code only (not in a comment or a string). Where the name is in the line.
fn rust_constant(line: &str, stack: &ScopeStack) -> Option<(usize, usize)> {
    let scopes = stack.as_slice();
    if !scopes.first().is_some_and(|s| s.build_string() == "source.rust")
        || scopes.iter().any(|s| {
            let name = s.build_string();
            name.starts_with("comment") || name.starts_with("string")
        })
    {
        return None;
    }
    let mut rest = line.trim_start();
    if let Some(after) = rest.strip_prefix("pub") {
        let after = after.trim_start();
        rest = match after.strip_prefix('(') {
            Some(inner) => inner.split_once(')')?.1.trim_start(),
            None => after,
        };
    }
    let rest = rest.strip_prefix("const ").or_else(|| rest.strip_prefix("static "))?.trim_start();
    let rest = rest.strip_prefix("mut ").unwrap_or(rest).trim_start();
    let len = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    if len == 0 || &rest[..len] == "_" || !rest[len..].trim_start().starts_with(':') {
        return None;
    }
    let from = line.len() - rest.len();
    Some((from, from + len))
}

/// A row of the tree: a symbol, whether others are nested in it, and whether it is open.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Row {
    symbol: Symbol,
    parent: bool,
    open: bool,
}

/// What names a symbol while the file is edited around it (its line moves).
type Name = (String, Kind, usize);

/// Each symbol's parent: the nearest one before it that is less deep (a heading's depth may go up by more than one).
pub fn parents(symbols: &[Symbol]) -> Vec<Option<usize>> {
    let mut out = Vec::with_capacity(symbols.len());
    let mut inside: Vec<usize> = Vec::new();
    for (i, s) in symbols.iter().enumerate() {
        while inside.last().is_some_and(|&j| symbols[j].depth >= s.depth) {
            inside.pop();
        }
        out.push(inside.last().copied());
        inside.push(i);
    }
    out
}

/// The symbols a line is in, outermost first: the last one at or before it, and the ones that one is nested in.
/// A symbol has no end line here, so a line after a function's end is still the function's until the next symbol.
pub fn chain(symbols: &[Symbol], line: usize) -> Vec<usize> {
    let before = symbols.iter().take_while(|s| s.line <= line).count();
    let Some(mut at) = before.checked_sub(1) else { return Vec::new() };
    let parents = parents(symbols);
    let mut out = vec![at];
    while let Some(p) = parents[at] {
        out.push(p);
        at = p;
    }
    out.reverse();
    out
}

/// The symbols beside one, itself among them, in order: those with its parent.
pub fn siblings(symbols: &[Symbol], i: usize) -> Vec<usize> {
    let parents = parents(symbols);
    let Some(&mine) = parents.get(i) else { return Vec::new() };
    (0..symbols.len()).filter(|&j| parents[j] == mine).collect()
}

/// Whether a name matches what is typed in a filter, as VS Code's outline filters: the letters typed in their order,
/// any case, others between them.
pub fn matches(name: &str, typed: &str) -> bool {
    let mut name = name.chars().flat_map(char::to_lowercase);
    typed.chars().flat_map(char::to_lowercase).filter(|c| !c.is_whitespace()).all(|t| name.any(|c| c == t))
}

/// The rows shown: every symbol but those under a closed one; with a filter, the symbols that match it and those
/// they are nested in, all open.
fn rows(symbols: &[Symbol], closed: &HashSet<Name>, filter: &str) -> im::Vector<Row> {
    let mut out = im::Vector::new();
    if !filter.trim().is_empty() {
        let parents = parents(symbols);
        let mut keep = vec![false; symbols.len()];
        for (i, s) in symbols.iter().enumerate() {
            if !keep[i] && matches(&s.name, filter) {
                let mut at = Some(i);
                while let Some(j) = at.filter(|&j| !keep[j]) {
                    keep[j] = true;
                    at = parents[j];
                }
            }
        }
        let kept: Vec<usize> = (0..symbols.len()).filter(|&i| keep[i]).collect();
        for (n, &i) in kept.iter().enumerate() {
            // The ones kept are whole from the top: a symbol's first kept one after it is nested in it, if any is.
            let parent = kept.get(n + 1).is_some_and(|&next| symbols[next].depth > symbols[i].depth);
            out.push_back(Row { symbol: symbols[i].clone(), parent, open: true });
        }
        return out;
    }
    // Hidden while deeper than this (the closed symbol's depth).
    let mut hidden: Option<usize> = None;
    for (n, s) in symbols.iter().enumerate() {
        if let Some(d) = hidden {
            if s.depth > d {
                continue;
            }
            hidden = None;
        }
        let parent = symbols.get(n + 1).is_some_and(|next| next.depth > s.depth);
        let open = !(parent && closed.contains(&(s.name.clone(), s.kind, s.depth)));
        if !open {
            hidden = Some(s.depth);
        }
        out.push_back(Row { symbol: s.clone(), parent, open });
    }
    out
}

/// What Collapse All closes: every symbol others are nested in.
fn all_parents(symbols: &[Symbol]) -> HashSet<Name> {
    symbols.windows(2).filter(|w| w[1].depth > w[0].depth).map(|w| (w[0].name.clone(), w[0].kind, w[0].depth)).collect()
}

/// The symbol the caret is in (its line): the last one at or before its line.
fn current(symbols: &[Symbol], line: usize) -> Option<usize> {
    symbols.iter().take_while(|s| s.line <= line).last().map(|s| s.line)
}

/// The Outline's own state, kept while its view is built again: the symbols closed, the filter, and how many
/// Outline views are built (the symbols are found while one is, or while the breadcrumbs show them).
#[derive(Clone, Copy)]
struct Outline {
    closed: RwSignal<HashSet<Name>>,
    filter: RwSignal<String>,
    shown: RwSignal<usize>,
}

impl crate::ext::State for Outline {
    fn new(ui: Ui) -> Outline {
        Outline {
            closed: ui.scope.create_rw_signal(HashSet::new()),
            filter: ui.scope.create_rw_signal(String::new()),
            shown: ui.scope.create_rw_signal(0),
        }
    }
}

/// Finds the shown file's symbols again: at once when another file is shown, a moment after the last edit else;
/// off the window's thread, one file at a time (what was typed meanwhile is read when it is done). Only while an
/// Outline view is built or the breadcrumbs show the symbols. Made once, at start.
fn follow(ui: Ui) {
    // Counted at each change (a wait for the typing to stop is for the count it began at), whether a file is being
    // read, whether more was asked meanwhile, and a count of the reads done.
    let generation = ui.scope.create_rw_signal(0u64);
    let busy = ui.scope.create_rw_signal(false);
    let wanted = ui.scope.create_rw_signal(false);
    let finished = ui.scope.create_rw_signal(0u64);
    let outline = ui.state::<Outline>();
    let find = move || {
        if busy.get_untracked() {
            return wanted.set(true);
        }
        let Some(tab) = ui.active.get_untracked().and_then(|p| ui.tab(&p)).filter(|t| t.is_file()) else {
            if ui.symbols.with_untracked(|(p, s)| !p.is_empty() || !s.is_empty()) {
                ui.symbols.set((String::new(), Rc::new(Vec::new())));
            }
            return;
        };
        busy.set(true);
        let (path, rope) = (tab.path.clone(), tab.doc.text());
        let file = path.clone();
        let done = floem::ext_event::create_ext_action(ui.scope, move |found: Vec<Symbol>| {
            if ui.active.with_untracked(|a| a.as_deref() == Some(file.as_str()))
                && ui.symbols.with_untracked(|(p, s)| *p != file || **s != found)
            {
                ui.symbols.set((file, Rc::new(found)));
            }
            busy.set(false);
            finished.update(|n| *n += 1);
        });
        std::thread::spawn(move || done(symbols(&path, &rope.to_string())));
    };
    ui.scope.create_effect(move |shown: Option<Option<Option<String>>>| {
        let active = ui.active.get();
        ui.edited.track();
        let want = outline.shown.get() > 0 || crate::editor::breadcrumbs::shows_symbols(ui);
        generation.update(|g| *g += 1);
        if !want {
            // Found again once they are wanted.
            return None;
        }
        let g = generation.get_untracked();
        if shown.flatten().as_ref() != Some(&active) {
            if ui.symbols.with_untracked(|(p, _)| Some(p) != active.as_ref()) {
                ui.symbols.set((active.clone().unwrap_or_default(), Rc::new(Vec::new())));
            }
            find();
        } else {
            floem::action::exec_after(std::time::Duration::from_millis(250), move |_| {
                if generation.try_get_untracked() == Some(g) {
                    find();
                }
            });
        }
        Some(active)
    });
    // What was asked while the last was being read.
    ui.scope.create_effect(move |last: Option<u64>| {
        let n = finished.get();
        if last.is_some() && wanted.get_untracked() {
            wanted.set(false);
            find();
        }
        n
    });
}

// The workbench shows it for `View::Outline`.
pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let o = ui.state::<Outline>();
    o.shown.update(|n| *n += 1);
    let (closed, filter) = (o.closed, o.filter);
    // The symbol the caret is in (its line), lit as VS Code's outline follows the caret.
    let at = create_memo(move |_| ui.symbols.with(|(_, s)| current(s, ui.caret.get().0)));
    let shown = create_memo(move |_| ui.symbols.with(|(_, s)| closed.with(|c| filter.with(|f| rows(s, c, f)))));
    let list = virtual_stack(
        VirtualDirection::Vertical,
        VirtualItemSize::Fixed(Box::new(|| 22.0)),
        move || shown.get(),
        |r: &Row| r.clone(),
        move |r: Row| {
            let send = send.clone();
            let Row { symbol, parent, open } = r;
            let (kind, line, depth) = (symbol.kind, symbol.line, symbol.depth);
            let name: Name = (symbol.name.clone(), kind, depth);
            let twistie = if parent {
                container(icon(if open { "chevron-down" } else { "chevron-right" }, 16.0))
                    .on_click_stop(move |_| {
                        closed.update(|c| {
                            if !c.remove(&name) {
                                c.insert(name.clone());
                            }
                        })
                    })
                    .into_any()
            } else {
                empty().style(|s| s.width(16.0)).into_any()
            };
            let detail = symbol.detail.clone();
            let row = h_stack((
                empty().style(move |s| s.width(8.0 * depth as f64)),
                twistie,
                icon(kind.icon(), 16.0).style(move |s| s.margin_left(2.0).color(kind.colour(&t.get()))),
                label(move || symbol.name.clone()).style(|s| s.padding_left(6.0).text_ellipsis()),
            ))
            .on_click_stop(move |_| {
                if let Some(path) = ui.active.get_untracked() {
                    ui.open_at(&path, line, &send);
                }
            })
            .style(move |s| {
                let c = t.get();
                let here = at.get() == Some(line);
                s.height(22.0)
                    .width_full()
                    .items_center()
                    .padding_left(8.0)
                    .padding_right(12.0)
                    .font_size(13.0)
                    .color(c.side_fg)
                    .cursor(CursorStyle::Pointer)
                    .selectable(false)
                    .hover(|s| s.background(c.list_hover))
                    .apply_if(here, |s| s.background(c.list_inactive))
            });
            // The symbol's signature, as VS Code's outline tells it on hover.
            tooltip(row, move || {
                let detail = detail.clone();
                label(move || detail.clone()).style(move |s| {
                    let c = t.get();
                    s.padding_horiz(8.0)
                        .padding_vert(4.0)
                        .font_size(12.0)
                        .max_width(600.0)
                        .color(c.hover_fg)
                        .background(c.hover_bg)
                        .border(1.0)
                        .border_color(c.hover_border)
                        .border_radius(3.0)
                })
            })
            .style(|s| s.width_full())
        },
    )
    .style(|s| s.flex_col().width_full());
    let none = label(move || {
        if ui.active.with(|a| a.is_none()) {
            "The active editor cannot provide outline information."
        } else if filter.with(|f| !f.trim().is_empty()) && ui.symbols.with(|(_, s)| !s.is_empty()) {
            "No symbols match the filter."
        } else {
            "No symbols found in this document."
        }
    })
    .style(move |s| {
        let some = shown.with(|r| !r.is_empty());
        s.padding_horiz(20.0).padding_vert(8.0).font_size(13.0).color(t.get().desc).apply_if(some, |s| s.hide())
    });
    floem::views::v_stack((
        filter_box(ui, filter),
        none,
        scroll(list).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full()),
    ))
    .on_cleanup(move || o.shown.update(|n| *n = n.saturating_sub(1)))
    .style(|s| s.size_full().min_height(0.0))
}

/// The box at the Outline's top that filters its symbols by name; Escape empties it.
fn filter_box(ui: Ui, filter: RwSignal<String>) -> impl IntoView {
    let t = ui.colors;
    let focused = create_rw_signal(false);
    let input = text_input(filter)
        .placeholder("Filter")
        .on_event_cont(EventListener::FocusGained, move |_| focused.set(true))
        .on_event_cont(EventListener::FocusLost, move |_| focused.set(false))
        .on_event_cont(EventListener::KeyDown, move |e| {
            if let floem::event::Event::KeyDown(k) = e
                && k.key.logical_key == Key::Named(NamedKey::Escape)
                && filter.with_untracked(|f| !f.is_empty())
            {
                filter.set(String::new());
            }
        })
        .style(move |s| {
            let c = t.get();
            let s = crate::widgets::steady_input(s, floem::peniko::Color::TRANSPARENT, None);
            s.flex_grow(1.0_f32)
                .min_width(0.0)
                .padding_horiz(4.0)
                .border(0.0)
                .background(floem::peniko::Color::TRANSPARENT)
                .color(c.input_fg)
                .cursor_color(floem::peniko::Brush::Solid(c.cursor))
        });
    container(input).style(move |s| {
        let c = t.get();
        s.height(24.0)
            .margin_horiz(12.0)
            .margin_vert(4.0)
            .items_center()
            .font_size(13.0)
            .border(1.0)
            .border_radius(2.0)
            .border_color(if focused.get() { c.focus } else { c.input_border })
            .background(c.input_bg)
    })
}

/// VS Code's Outline title action.
pub const COLLAPSE: &str = "outline.collapse";

/// What it adds to the window's registries, once at start: Collapse All in the Outline's title, and the following
/// of the shown file's symbols.
pub fn register(ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::menus::{Menu, item};
    add(command(COLLAPSE, "Outline", "Collapse All", |ui, _| {
        let o = ui.state::<Outline>();
        let all = ui.symbols.with_untracked(|(_, s)| all_parents(s));
        if o.closed.with_untracked(|c| !all.is_subset(c)) {
            o.closed.update(|c| c.extend(all));
        }
    }));
    crate::menus::add(Menu::ViewTitle(View::Outline), item(COLLAPSE).icon("collapse-all").group("navigation"));
    follow(ui);
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &serde_json::Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(path: &str, text: &str) -> Vec<(String, Kind, usize, usize)> {
        symbols(path, text).into_iter().map(|s| (s.name, s.kind, s.line, s.depth)).collect()
    }

    #[test]
    fn rusts_symbols_by_its_grammar_nested_by_indentation() {
        let text = "pub struct Cart {\n    items: Vec<u32>,\n}\n\nimpl Cart {\n    pub fn total(&self) -> u32 {\n        0\n    }\n}\n\nenum Kind { A, B }\ntrait Priced {\n    fn price(&self) -> u32;\n}\ntype Id = u64;\nmod tests {\n    fn it_adds() {}\n}\npub const MOST: usize = 5;\nstatic mut NAME: &str = \"x\";\n// const NOT: u8 = 1;\nfn main<'a>() { 'outer: loop {} }\n";
        assert_eq!(
            said("src/cart.rs", text),
            [
                ("Cart".to_string(), Kind::Struct, 1, 0),
                ("Cart".to_string(), Kind::Class, 5, 0),
                ("total".to_string(), Kind::Method, 6, 1),
                ("Kind".to_string(), Kind::Enum, 11, 0),
                ("Priced".to_string(), Kind::Interface, 12, 0),
                ("price".to_string(), Kind::Method, 13, 1),
                ("Id".to_string(), Kind::Type, 15, 0),
                ("tests".to_string(), Kind::Module, 16, 0),
                ("it_adds".to_string(), Kind::Function, 17, 1),
                ("MOST".to_string(), Kind::Constant, 19, 0),
                ("NAME".to_string(), Kind::Constant, 20, 0),
                // Its lifetime and its label are not symbols.
                ("main".to_string(), Kind::Function, 22, 0),
            ]
        );
    }

    #[test]
    fn pythons_classes_methods_and_functions() {
        let text = "import os\n\nclass Cart:\n    def total(self):\n        def inner():\n            pass\n        return 0\n\nasync def main():\n    pass\n";
        assert_eq!(
            said("cart.py", text),
            [
                ("Cart".to_string(), Kind::Class, 3, 0),
                ("total".to_string(), Kind::Method, 4, 1),
                ("inner".to_string(), Kind::Function, 5, 2),
                ("main".to_string(), Kind::Function, 9, 0),
            ]
        );
    }

    #[test]
    fn a_documents_headings_by_their_level_and_none_in_a_code_block() {
        let text = "# Title\n\ntext\n\n## Part *one* here\n\n```rust\n# not one\nfn x() {}\n```\n\n### Deep ###\n";
        assert_eq!(
            said("README.md", text),
            [
                ("Title".to_string(), Kind::Heading, 1, 0),
                ("Part *one* here".to_string(), Kind::Heading, 5, 1),
                ("Deep".to_string(), Kind::Heading, 12, 2),
            ]
        );
    }

    #[test]
    fn other_grammars_give_theirs_and_plain_text_none() {
        let js = said("a.js", "class A extends B {\n  m() {}\n}\nfunction f() {}\n");
        assert_eq!(
            js.iter().map(|s| (s.0.as_str(), s.1)).collect::<Vec<_>>(),
            [("A", Kind::Class), ("m", Kind::Method), ("f", Kind::Function)]
        );
        let c = said("a.c", "struct s { int a; };\nint main(void) { return 0; }\n#define X 1\n");
        assert_eq!(
            c.iter().map(|s| (s.0.as_str(), s.1)).collect::<Vec<_>>(),
            [("s", Kind::Struct), ("main", Kind::Function), ("X", Kind::Constant)]
        );
        assert!(said("notes.txt", "fn main() {}\n# Title\n").is_empty());
    }

    #[test]
    fn the_rows_under_a_closed_symbol_are_not_shown() {
        let all =
            symbols("cart.py", "class A:\n    def a(self):\n        pass\nclass B:\n    def b(self):\n        pass\n");
        let names =
            |closed: &HashSet<Name>| rows(&all, closed, "").iter().map(|r| r.symbol.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&HashSet::new()), ["A", "a", "B", "b"]);
        let closed = HashSet::from([("A".to_string(), Kind::Class, 0)]);
        assert_eq!(names(&closed), ["A", "B", "b"]);
        let shown = rows(&all, &closed, "");
        assert!(shown[0].parent && !shown[0].open && shown[1].open);
        // The caret's symbol: the last at or before its line.
        assert_eq!(current(&all, 3), Some(2));
        assert_eq!(current(&all, 4), Some(4));
    }

    #[test]
    fn a_filter_keeps_the_symbols_that_match_and_those_they_are_in() {
        let all = symbols(
            "cart.py",
            "class Cart:\n    def total(self):\n        pass\n    def add_item(self):\n        pass\nclass Shop:\n    def open(self):\n        pass\n",
        );
        let names = |f: &str| {
            rows(&all, &HashSet::new(), f).iter().map(|r| (r.symbol.name.clone(), r.parent)).collect::<Vec<_>>()
        };
        assert_eq!(names("ADI"), [("Cart".to_string(), true), ("add_item".to_string(), false)]);
        assert_eq!(names("op"), [("Shop".to_string(), true), ("open".to_string(), false)]);
        assert!(names("zz").is_empty());
        // A closed symbol is opened while a filter is typed.
        let closed = HashSet::from([("Cart".to_string(), Kind::Class, 0)]);
        assert_eq!(rows(&all, &closed, "total").len(), 2);
        assert!(matches("add_item", "aim") && !matches("add_item", "mia") && matches("x", ""));
    }

    #[test]
    fn collapse_all_closes_every_symbol_others_are_in() {
        let all = symbols("doc.md", "# A\n\n### Deep\n\n# B\n\n## C\n");
        assert_eq!(parents(&all), [None, Some(0), None, Some(2)], "a heading two levels down is the first's");
        let closed = all_parents(&all);
        assert_eq!(closed.len(), 2);
        let names = rows(&all, &closed, "").iter().map(|r| r.symbol.name.clone()).collect::<Vec<_>>();
        assert_eq!(names, ["A", "B"]);
    }

    #[test]
    fn a_symbols_detail_is_its_line() {
        let all = symbols("a.rs", "    pub fn total(&self, n: u32) -> u32 {\n        0\n    }\n");
        assert_eq!(all[0].detail, "pub fn total(&self, n: u32) -> u32");
        assert_eq!(detail_of("class A:"), "class A");
        assert_eq!(detail_of(&"x".repeat(300)).chars().count(), 201);
    }
}
