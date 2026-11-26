//! The Outline view, as VS Code's: the shown file's symbols (its functions, types, modules, constants, a document's
//! headings) as a tree by their nesting, each 22 px row with its symbol icon in the icon's own colour; a click goes
//! to the symbol's line. The symbols are the names the file's grammar marks as defined (`entity.name.*`), found off
//! the window's thread, a moment after the last edit.

use std::collections::HashSet;
use std::rc::Rc;

use floem::IntoView;
use floem::reactive::{SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::style::CursorStyle;
use floem::views::editor::text::Document;
use floem::views::{
    Decorators, VirtualDirection, VirtualItemSize, container, empty, h_stack, label, scroll, virtual_stack,
};
use syntect::parsing::{ParseState, Scope, ScopeStack};

use crate::colors::Colors;
use crate::highlight::{SYNTAXES, syntax_for};
use crate::icons::icon;
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
            out.push(Symbol { name: name.to_string(), kind, line: n + 1, depth });
        }
    }
    out
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

/// The rows shown: every symbol but those under a closed one.
fn rows(symbols: &[Symbol], closed: &HashSet<Name>) -> im::Vector<Row> {
    let mut out = im::Vector::new();
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

/// The symbol the caret is in: the last one at or before its line.
fn current(symbols: &[Symbol], line: usize) -> Option<usize> {
    symbols.iter().take_while(|s| s.line <= line).last().map(|s| s.line)
}

/// Finds the shown file's symbols again: at once when another file is shown, a moment after the last edit else;
/// off the window's thread, one file at a time (what was typed meanwhile is read when it is done).
fn follow(ui: Ui) {
    // Counted at each change (a wait for the typing to stop is for the count it began at), whether a file is being
    // read, whether more was asked meanwhile, and a count of the reads done. They are the view's: a timer or a read
    // that ends after the view is gone finds them gone, and does nothing.
    let generation = create_rw_signal(0u64);
    let busy = create_rw_signal(false);
    let wanted = create_rw_signal(false);
    let finished = create_rw_signal(0u64);
    let find = move || {
        match busy.try_get_untracked() {
            None => return,
            Some(true) => return wanted.set(true),
            Some(false) => {}
        }
        let Some(tab) = ui.active.get_untracked().and_then(|p| ui.tab(&p)).filter(|t| t.diff.is_none()) else {
            ui.symbols.set((String::new(), Rc::new(Vec::new())));
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
    create_effect(move |shown: Option<Option<String>>| {
        let active = ui.active.get();
        ui.edited.track();
        generation.update(|g| *g += 1);
        let g = generation.get_untracked();
        if shown.as_ref() != Some(&active) {
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
        active
    });
    // What was asked while the last was being read.
    create_effect(move |last: Option<u64>| {
        let n = finished.get();
        if last.is_some() && wanted.get_untracked() {
            wanted.set(false);
            find();
        }
        n
    });
}

// The workbench shows it for `View::Outline`.
#[allow(dead_code)]
pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    follow(ui);
    let closed = create_rw_signal(HashSet::<Name>::new());
    // The symbol the caret is in (its line), lit as VS Code's outline follows the caret.
    let at = floem::reactive::create_memo(move |_| ui.symbols.with(|(_, s)| current(s, ui.caret.get().0)));
    let list = virtual_stack(
        VirtualDirection::Vertical,
        VirtualItemSize::Fixed(Box::new(|| 22.0)),
        move || ui.symbols.with(|(_, s)| closed.with(|c| rows(s, c))),
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
            h_stack((
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
            })
        },
    )
    .style(|s| s.flex_col().width_full());
    let none = label(move || {
        if ui.active.with(|a| a.is_none()) {
            "The active editor cannot provide outline information."
        } else {
            "No symbols found in this document."
        }
    })
    .style(move |s| {
        s.padding_horiz(20.0)
            .padding_vert(8.0)
            .font_size(13.0)
            .color(t.get().desc)
            .apply_if(ui.symbols.with(|(_, s)| !s.is_empty()), |s| s.hide())
    });
    floem::views::v_stack((none, scroll(list).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full())))
        .style(|s| s.size_full().min_height(0.0))
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
            |closed: &HashSet<Name>| rows(&all, closed).iter().map(|r| r.symbol.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&HashSet::new()), ["A", "a", "B", "b"]);
        let closed = HashSet::from([("A".to_string(), Kind::Class, 0)]);
        assert_eq!(names(&closed), ["A", "B", "b"]);
        let shown = rows(&all, &closed);
        assert!(shown[0].parent && !shown[0].open && shown[1].open);
        // The caret's symbol: the last at or before its line.
        assert_eq!(current(&all, 3), Some(2));
        assert_eq!(current(&all, 4), Some(4));
    }
}
