//! Syntax colours as VS Code's: the file's grammar (TextMate's, as VS Code's are, through syntect) gives each run of
//! text its scopes, and the theme's `tokenColors` colour them by the very same rules. The parse is kept a line at a
//! time, so an edit parses again from its line only.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::str::FromStr;
use std::sync::LazyLock;

use floem::peniko::Color;
use floem::text::{Attrs, AttrsList, FamilyOwned, Stretch, Style as FontSlant, Weight};
use floem::views::editor::EditorStyle;
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::id::EditorId;
use floem::views::editor::layout::{LineExtraStyle, TextLayoutLine};
use floem::views::editor::text::{Document, Styling};
use floem::views::editor::text_document::TextDocument;
use serde_json::Value;
use syntect::highlighting::{
    FontStyle, HighlightState, Highlighter, RangedHighlightIterator, ScopeSelectors, StyleModifier, Theme, ThemeItem,
    ThemeSettings,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxDefinition, SyntaxReference, SyntaxSet};

use crate::widgets::MONO_FONT;

/// The grammars the window adds to syntect's own, written for it (`assets/syntaxes`, NOTICE says so): the languages
/// VS Code colours that syntect's set leaves plain.
const GRAMMARS: &[(&str, &str)] = &[
    ("TypeScript", include_str!("../assets/syntaxes/TypeScript.sublime-syntax")),
    ("TypeScript JSX", include_str!("../assets/syntaxes/TypeScriptReact.sublime-syntax")),
    ("JavaScript JSX", include_str!("../assets/syntaxes/JavaScriptReact.sublime-syntax")),
    ("TOML", include_str!("../assets/syntaxes/TOML.sublime-syntax")),
    ("Dockerfile", include_str!("../assets/syntaxes/Dockerfile.sublime-syntax")),
    ("Vue", include_str!("../assets/syntaxes/Vue.sublime-syntax")),
    ("Svelte", include_str!("../assets/syntaxes/Svelte.sublime-syntax")),
    ("Kotlin", include_str!("../assets/syntaxes/Kotlin.sublime-syntax")),
    ("PowerShell", include_str!("../assets/syntaxes/PowerShell.sublime-syntax")),
    ("Terraform", include_str!("../assets/syntaxes/Terraform.sublime-syntax")),
    ("GraphQL", include_str!("../assets/syntaxes/GraphQL.sublime-syntax")),
];

/// Every grammar: syntect's, then the window's (found first by a file ending both have). Linked once, the first
/// time one is wanted; the window starts that off its own thread when it opens.
pub static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(|| {
    let mut set = SyntaxSet::load_defaults_newlines().into_builder();
    for (_, text) in GRAMMARS {
        // One that does not load is left out (a test loads each).
        if let Ok(grammar) = SyntaxDefinition::load_from_str(text, true, None) {
            set.add(grammar);
        }
    }
    set.build()
});

/// A theme's `tokenColors` as syntect's theme (the rules without a scope are the editor's own colours).
pub fn theme_of(json: &Value) -> Theme {
    let hex = |v: &Value| v.as_str().and_then(crate::colors::hex).map(to_syntect);
    // What no rule colours is the editor's foreground, as in VS Code.
    let fg = json["colors"]["editor.foreground"].as_str().or(json["colors"]["foreground"].as_str());
    let dark = !matches!(json["type"].as_str(), Some("light" | "hcLight"));
    let fallback = if dark { "#D4D4D4" } else { "#000000" };
    let settings = ThemeSettings {
        foreground: fg.and_then(crate::colors::hex).or(crate::colors::hex(fallback)).map(to_syntect),
        ..ThemeSettings::default()
    };
    let mut theme = Theme { settings, ..Theme::default() };
    for rule in json["tokenColors"].as_array().into_iter().flatten() {
        let s = &rule["settings"];
        let font_style = s["fontStyle"].as_str().map(|f| {
            let mut out = FontStyle::empty();
            for w in f.split_whitespace() {
                match w {
                    "bold" => out |= FontStyle::BOLD,
                    "italic" => out |= FontStyle::ITALIC,
                    "underline" => out |= FontStyle::UNDERLINE,
                    _ => {}
                }
            }
            out
        });
        let style = StyleModifier { foreground: hex(&s["foreground"]), background: hex(&s["background"]), font_style };
        let scopes = match &rule["scope"] {
            Value::String(s) => s.clone(),
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
            _ => {
                if style.foreground.is_some() {
                    theme.settings.foreground = style.foreground;
                }
                continue;
            }
        };
        if let Ok(scope) = ScopeSelectors::from_str(&scopes) {
            theme.scopes.push(ThemeItem { scope, style });
        }
    }
    theme
}

fn to_syntect(c: Color) -> syntect::highlighting::Color {
    syntect::highlighting::Color { r: c.r, g: c.g, b: c.b, a: c.a }
}

/// The grammar for a file, by its name (else its first line, as `#!/bin/sh`); none for plain text.
pub fn syntax_for(path: &str, first_line: &str) -> Option<&'static SyntaxReference> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or(name);
    let s = SYNTAXES.find_syntax_by_extension(ext).or_else(|| SYNTAXES.find_syntax_by_first_line(first_line))?;
    (s.name != "Plain Text").then_some(s)
}

/// The grammar a fence names: a language's name or a file ending (`rust`, `rs`, `ts`), or what answers often call it.
pub fn syntax_named(lang: &str) -> Option<&'static SyntaxReference> {
    let lang = match lang {
        "" => return None,
        "typescriptreact" => "tsx",
        "javascriptreact" => "jsx",
        "docker" | "containerfile" => "dockerfile",
        "pwsh" | "ps" | "posh" => "ps1",
        "hcl" | "terraform" => "tf",
        "shell" | "shellscript" | "zsh" | "console" => "sh",
        "golang" => "go",
        other => other,
    };
    let s = SYNTAXES.find_syntax_by_token(lang).or_else(|| syntax_for(&format!("a.{lang}"), ""))?;
    (s.name != "Plain Text").then_some(s)
}

/// A run of a fenced block's line in its colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub range: std::ops::Range<usize>,
    pub colour: Color,
    pub bold: bool,
    pub italic: bool,
}

/// A line of a fenced block: its text, its runs, and a number of its own, another each time the line is parsed
/// (so what draws it knows a line it has drawn from one that changed).
#[derive(Debug, PartialEq)]
pub struct Line {
    pub id: u64,
    pub text: String,
    pub runs: Vec<Run>,
}

/// A line's minimap blocks, and whether they are of the line as it is now.
type MiniLine = (bool, Rc<[Block]>);

/// A run of a line's characters as the minimap draws it: its first column and the column after its last (a tab
/// counted as the columns it is drawn as), and its colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    pub from: u16,
    pub to: u16,
    pub colour: Color,
}

/// A line's runs as the minimap draws them, as VS Code's: each character that is not white space by its column (a
/// tab goes to its next stop), in its run's colour (`plain` outside the runs), those side by side in one colour as
/// one block, and nothing from column `most` on.
pub fn blocks(
    text: &str,
    runs: &[(std::ops::Range<usize>, Color)],
    tab: usize,
    most: usize,
    plain: Color,
) -> Vec<Block> {
    let tab = tab.max(1);
    let (mut out, mut col, mut r) = (Vec::<Block>::new(), 0usize, 0usize);
    for (i, ch) in text.char_indices() {
        if col >= most || ch == '\n' || ch == '\r' {
            break;
        }
        if ch == '\t' {
            col += tab - col % tab;
            continue;
        }
        if ch.is_whitespace() {
            col += 1;
            continue;
        }
        while r < runs.len() && runs[r].0.end <= i {
            r += 1;
        }
        let colour = runs.get(r).filter(|run| run.0.start <= i).map_or(plain, |run| run.1);
        match out.last_mut() {
            Some(b) if usize::from(b.to) == col && b.colour == colour => b.to += 1,
            _ => out.push(Block { from: col as u16, to: col as u16 + 1, colour }),
        }
        col += 1;
    }
    out
}

/// A line as it was parsed: whether a line follows it, and the parse after it.
struct Kept {
    line: Rc<Line>,
    ended: bool,
    after: Option<(ParseState, HighlightState)>,
}

/// A fenced block's lines in their colours, kept a line at a time: while the block is being written, only the
/// lines that changed (its last) are parsed again, and the others are the very lines given before.
pub struct Snippet {
    syntax: Option<&'static SyntaxReference>,
    theme: Option<Rc<Theme>>,
    lines: Vec<Kept>,
    next: u64,
}

impl Snippet {
    pub fn new(lang: &str) -> Snippet {
        Snippet { syntax: syntax_named(lang), theme: None, lines: Vec::new(), next: 0 }
    }

    /// The text's lines with their runs in the theme's colours (no runs for a language not known: drawn plain).
    pub fn lines(&mut self, text: &str, theme: &Rc<Theme>) -> Vec<Rc<Line>> {
        if !self.theme.as_ref().is_some_and(|t| Rc::ptr_eq(t, theme)) {
            self.theme = Some(theme.clone());
            self.lines.clear();
        }
        let highlighter = Highlighter::new(theme);
        let mut count = 0;
        for (n, raw) in text.split_inclusive('\n').enumerate() {
            count = n + 1;
            let ended = raw.ends_with('\n');
            let shown = raw.strip_suffix('\n').unwrap_or(raw);
            if self.lines.get(n).is_some_and(|k| k.ended == ended && k.line.text == shown) {
                continue;
            }
            self.lines.truncate(n);
            let (mut runs, mut after) = (Vec::new(), None);
            if let Some(syntax) = self.syntax {
                let (mut ps, mut hs) = match n.checked_sub(1).and_then(|p| self.lines[p].after.clone()) {
                    Some(before) => before,
                    None => (ParseState::new(syntax), HighlightState::new(&highlighter, ScopeStack::new())),
                };
                if let Ok(ops) = ps.parse_line(raw, &SYNTAXES) {
                    for (s, _, range) in RangedHighlightIterator::new(&mut hs, &ops, raw, &highlighter) {
                        // The line's end is not drawn.
                        let range = range.start.min(shown.len())..range.end.min(shown.len());
                        if !range.is_empty() {
                            runs.push(Run {
                                range,
                                colour: Color::rgba8(s.foreground.r, s.foreground.g, s.foreground.b, s.foreground.a),
                                bold: s.font_style.contains(FontStyle::BOLD),
                                italic: s.font_style.contains(FontStyle::ITALIC),
                            });
                        }
                    }
                }
                after = Some((ps, hs));
            }
            self.next += 1;
            let line = Rc::new(Line { id: self.next, text: shown.to_string(), runs });
            self.lines.push(Kept { line, ended, after });
        }
        self.lines.truncate(count);
        self.lines.iter().map(|k| k.line.clone()).collect()
    }
}

/// The parse after each line (`after[n]` is the state at the start of line n + 1).
struct Parsed {
    after: Vec<(ParseState, HighlightState)>,
}

/// The font the editor's lines are laid out with, and where its rulers stand (in pixels from the text's start).
struct Look {
    family: Vec<FamilyOwned>,
    size: f32,
    weight: Weight,
    line_height: f32,
    rulers: Vec<f64>,
    ruler: Color,
}

impl Default for Look {
    fn default() -> Self {
        Look {
            family: FamilyOwned::parse_list(&MONO_FONT).collect(),
            size: 14.0,
            weight: Weight::NORMAL,
            line_height: 19.0,
            rulers: Vec::new(),
            ruler: Color::TRANSPARENT,
        }
    }
}

/// The editor's styling: VS Code's font and sizes, and each line's colours from its grammar and the theme.
pub struct Colours {
    doc: Rc<TextDocument>,
    syntax: Cell<Option<&'static SyntaxReference>>,
    theme: RefCell<Theme>,
    parsed: RefCell<Parsed>,
    id: Cell<u64>,
    look: RefCell<Look>,
    /// How many columns a tab is drawn as: the file's indentation's (`indent.rs`).
    tab: Cell<usize>,
    /// The file's path, by which its decorations are found (`decor.rs`).
    path: String,
    /// The minimap's runs of each line worked out, and whether they are of the line as it is now (`minimap`).
    mini: RefCell<Vec<Option<MiniLine>>>,
}

impl Colours {
    pub fn new(doc: Rc<TextDocument>, path: &str) -> Rc<Colours> {
        let first = doc.rope_text().line_content(0).to_string();
        Rc::new(Colours {
            doc,
            syntax: Cell::new(syntax_for(path, &first)),
            theme: RefCell::new(Theme::default()),
            parsed: RefCell::new(Parsed { after: Vec::new() }),
            id: Cell::new(1),
            look: RefCell::new(Look::default()),
            tab: Cell::new(4),
            path: path.to_string(),
            mini: RefCell::new(Vec::new()),
        })
    }

    /// Another grammar (its language picked, or found again): every line coloured again. False: it was this one.
    pub fn set_syntax(&self, syntax: Option<&'static SyntaxReference>) -> bool {
        if self.syntax.get().map(std::ptr::from_ref) == syntax.map(std::ptr::from_ref) {
            return false;
        }
        self.syntax.set(syntax);
        self.parsed.borrow_mut().after.clear();
        self.mini_stale(0);
        self.id.set(self.id.get() + 1);
        true
    }

    /// Another theme: every line coloured again.
    pub fn set_theme(&self, theme: Theme) {
        *self.theme.borrow_mut() = theme;
        self.parsed.borrow_mut().after.clear();
        self.mini_stale(0);
        self.id.set(self.id.get() + 1);
    }

    /// The editor's font and rulers (`editor/options.rs`): every line laid out again with them.
    pub fn set_look(&self, font: &crate::editor::options::Font, rulers: &[usize], ruler: Color) {
        let width = font.char_width();
        *self.look.borrow_mut() = Look {
            family: font.families(),
            size: font.size,
            weight: Weight(font.weight),
            line_height: font.line_height,
            rulers: rulers.iter().map(|c| *c as f64 * width).collect(),
            ruler,
        };
        self.id.set(self.id.get() + 1);
    }

    /// A tab drawn `columns` wide; the editor drawn again with `update_styling`.
    pub fn set_tab_width(&self, columns: usize) {
        self.tab.set(columns.max(1));
        self.mini_stale(0);
        self.id.set(self.id.get() + 1);
    }

    pub fn tab_width_is(&self, columns: usize) -> bool {
        self.tab.get() == columns.max(1)
    }

    /// The text changed from `line` on: what was parsed before it stands.
    pub fn changed_from(&self, line: usize) {
        self.parsed.borrow_mut().after.truncate(line);
        self.mini_stale(line);
        self.id.set(self.id.get() + 1);
    }

    /// The minimap's runs of the lines from `line` on are no longer those of the text. They are still drawn until
    /// they are worked out again, so the minimap does not blink while typing.
    fn mini_stale(&self, line: usize) {
        for kept in self.mini.borrow_mut().iter_mut().skip(line).flatten() {
            kept.0 = false;
        }
    }

    /// The minimap's runs of a line (`blocks`, at most `most` columns), kept until the line changes. They are worked
    /// out only when `work` is true; otherwise the ones kept are given, even from before the last edit (false then:
    /// not of the line as it is now), or none.
    pub fn minimap(&self, line: usize, most: usize, work: bool) -> (Option<Rc<[Block]>>, bool) {
        let kept = self.mini.borrow().get(line).cloned().flatten();
        if let Some((fresh, kept)) = kept
            && (fresh || !work)
        {
            return (Some(kept), fresh);
        }
        if !work {
            return (None, false);
        }
        let colour = |c: syntect::highlighting::Color| Color::rgba8(c.r, c.g, c.b, c.a);
        let plain = self.theme.borrow().settings.foreground.map_or(Color::rgb8(0xD4, 0xD4, 0xD4), colour);
        let runs: Vec<_> = self.runs(line).into_iter().map(|(r, s)| (r, colour(s.foreground))).collect();
        let text = self.doc.rope_text();
        let made: Rc<[Block]> = blocks(&text.line_content(line), &runs, self.tab.get(), most, plain).into();
        let mut mini = self.mini.borrow_mut();
        if mini.len() <= line {
            mini.resize(line + 1, None);
        }
        mini[line] = Some((true, made.clone()));
        (Some(made), true)
    }

    /// The runs of a line with their style: parsed from the last line kept up to it.
    fn runs(&self, line: usize) -> Vec<(std::ops::Range<usize>, syntect::highlighting::Style)> {
        let Some(syntax) = self.syntax.get() else { return Vec::new() };
        let theme = self.theme.borrow();
        let highlighter = Highlighter::new(&theme);
        let text = self.doc.rope_text();
        let mut parsed = self.parsed.borrow_mut();
        let start = |p: &Parsed, n: usize| match n {
            0 => (ParseState::new(syntax), HighlightState::new(&highlighter, ScopeStack::new())),
            n => p.after[n - 1].clone(),
        };
        while parsed.after.len() < line {
            let n = parsed.after.len();
            let (mut ps, mut hs) = start(&parsed, n);
            let content = text.line_content(n);
            if let Ok(ops) = ps.parse_line(&content, &SYNTAXES) {
                for _ in RangedHighlightIterator::new(&mut hs, &ops, &content, &highlighter) {}
            }
            parsed.after.push((ps, hs));
        }
        let (mut ps, mut hs) = start(&parsed, line);
        let content = text.line_content(line);
        let Ok(ops) = ps.parse_line(&content, &SYNTAXES) else { return Vec::new() };
        let runs =
            RangedHighlightIterator::new(&mut hs, &ops, &content, &highlighter).map(|(s, _, r)| (r, s)).collect();
        if parsed.after.len() == line {
            parsed.after.push((ps, hs));
        }
        runs
    }
}

impl Styling for Colours {
    fn id(&self) -> u64 {
        self.id.get()
    }

    fn font_size(&self, _: EditorId, _: usize) -> usize {
        self.look.borrow().size.round() as usize
    }

    fn line_height(&self, _: EditorId, _: usize) -> f32 {
        self.look.borrow().line_height
    }

    fn font_family(&self, _: EditorId, _: usize) -> Cow<'_, [FamilyOwned]> {
        Cow::Owned(self.look.borrow().family.clone())
    }

    fn weight(&self, _: EditorId, _: usize) -> Weight {
        self.look.borrow().weight
    }

    fn italic_style(&self, _: EditorId, _: usize) -> FontSlant {
        FontSlant::Normal
    }

    fn stretch(&self, _: EditorId, _: usize) -> Stretch {
        Stretch::Normal
    }

    fn tab_width(&self, _: EditorId, _: usize) -> usize {
        self.tab.get()
    }

    fn apply_attr_styles(&self, edid: EditorId, es: &EditorStyle, line: usize, default: Attrs, attrs: &mut AttrsList) {
        let phantom = crate::decor::phantom_line(&self.doc, &self.path, edid, es, line);
        let runs = self.runs(line);
        let mut spans = Vec::with_capacity(runs.len().max(1));
        if runs.is_empty() {
            // Plain text: the editor's own foreground.
            if let Some(f) = self.theme.borrow().settings.foreground {
                let len = self.doc.rope_text().line_content(line).len();
                spans.push((0..len, default.color(Color::rgba8(f.r, f.g, f.b, f.a))));
            }
        }
        for (range, style) in runs {
            let f = style.foreground;
            let mut a = default.color(Color::rgba8(f.r, f.g, f.b, f.a));
            if style.font_style.contains(FontStyle::BOLD) {
                a = a.weight(Weight::BOLD);
            }
            if style.font_style.contains(FontStyle::ITALIC) && *crate::widgets::ITALIC {
                a = a.style(FontSlant::Italic);
            }
            spans.push((range, a));
        }
        crate::decor::attrs(&self.path, line, &phantom, spans, default, attrs);
    }

    fn apply_layout_styles(&self, edid: EditorId, _: &EditorStyle, line: usize, layout: &mut TextLayoutLine) {
        let len = self.doc.rope_text().line_end_col(line, true);
        let height = self.line_height(edid, line) as f64;
        crate::decor::layout(&self.path, line, len, height, layout);
        let look = self.look.borrow();
        // Down every row of a wrapped line too.
        let rows = layout.text.size().height.max(height);
        for x in &look.rulers {
            layout.extra_style.push(LineExtraStyle {
                x: *x,
                y: 0.0,
                width: Some(1.0),
                height: rows,
                bg_color: Some(look.ruler),
                under_line: None,
                wave_line: None,
            });
        }
    }
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: crate::state::Ui, _send: &crate::state::Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: crate::state::Ui, _m: &serde_json::Value, _send: &crate::state::Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_minimaps_blocks_are_the_runs_without_white_space_by_column() {
        let (red, blue, plain) = (Color::rgb8(255, 0, 0), Color::rgb8(0, 0, 255), Color::rgb8(9, 9, 9));
        let block = |from, to, colour| Block { from, to, colour };
        // "fn ab" with "fn" red and "ab" blue: two blocks, the space between them none.
        assert_eq!(
            blocks("fn ab\n", &[(0..2, red), (2..5, blue)], 4, 120, plain),
            [block(0, 2, red), block(3, 5, blue)]
        );
        // A tab goes to its stop; outside the runs the plain colour; runs of one colour side by side join.
        assert_eq!(blocks("\tx", &[], 4, 120, plain), [block(4, 5, plain)]);
        assert_eq!(blocks("ab", &[(0..1, red), (1..2, red)], 4, 120, plain), [block(0, 2, red)]);
        // Nothing past the most columns.
        assert_eq!(blocks("abcdef", &[], 4, 3, plain), [block(0, 3, plain)]);
    }

    #[test]
    fn the_minimap_keeps_a_lines_blocks_until_an_edit_reaches_it() {
        let scope = floem::reactive::Scope::new();
        let doc = Rc::new(TextDocument::new(scope, "a\nb\nc\n"));
        let colours = Colours::new(doc, "x.txt");
        assert_eq!(colours.minimap(1, 120, false), (None, false), "not worked out without work");
        let (made, fresh) = colours.minimap(1, 120, true);
        assert!(fresh && made.is_some());
        assert!(colours.minimap(1, 120, false).1, "kept");
        colours.changed_from(2);
        assert!(colours.minimap(1, 120, false).1, "an edit below leaves it");
        colours.changed_from(0);
        let (kept, fresh) = colours.minimap(1, 120, false);
        assert!(kept.is_some() && !fresh, "an edit above: drawn as it was until worked out again");
        assert!(colours.minimap(1, 120, true).1);
    }

    #[test]
    fn a_file_gets_its_grammar_by_name_or_first_line() {
        assert_eq!(syntax_for("src/main.rs", "").map(|s| s.name.as_str()), Some("Rust"));
        assert_eq!(syntax_for("run", "#!/bin/bash").map(|s| s.name.as_str()), Some("Bourne Again Shell (bash)"));
        assert!(syntax_for("notes.txt", "hello").is_none());
    }

    #[test]
    fn a_fence_names_its_grammar_as_answers_do() {
        let name = |lang| syntax_named(lang).map(|s| s.name.as_str());
        assert_eq!(name("rust"), Some("Rust"));
        assert_eq!(name("rs"), Some("Rust"));
        assert_eq!(name("python"), Some("Python"));
        assert_eq!(name("ts"), Some("TypeScript"));
        assert_eq!(name("typescript"), Some("TypeScript"));
        assert_eq!(name("tsx"), Some("TypeScript JSX"));
        assert_eq!(name("typescriptreact"), Some("TypeScript JSX"));
        assert_eq!(name("jsx"), Some("JavaScript JSX"));
        assert_eq!(name("toml"), Some("TOML"));
        assert_eq!(name("docker"), Some("Dockerfile"));
        assert_eq!(name("dockerfile"), Some("Dockerfile"));
        assert_eq!(name("kotlin"), Some("Kotlin"));
        assert_eq!(name("kt"), Some("Kotlin"));
        assert_eq!(name("powershell"), Some("PowerShell"));
        assert_eq!(name("pwsh"), Some("PowerShell"));
        assert_eq!(name("hcl"), Some("Terraform"));
        assert_eq!(name("terraform"), Some("Terraform"));
        assert_eq!(name("graphql"), Some("GraphQL"));
        assert_eq!(name("gql"), Some("GraphQL"));
        assert_eq!(name("vue"), Some("Vue"));
        assert_eq!(name("svelte"), Some("Svelte"));
        assert_eq!(name("bash"), Some("Bourne Again Shell (bash)"));
        assert_eq!(name("shell"), Some("Bourne Again Shell (bash)"));
        assert_eq!(name("json"), Some("JSON"));
        assert!(name("yaml").is_some() && name("go").is_some() && name("golang").is_some());
        assert!(name("").is_none() && name("no-such-language").is_none());
    }

    #[test]
    fn a_fenced_block_is_coloured_and_only_its_new_lines_parsed_again() {
        let all = ca_themes::built_in();
        let theme = Rc::new(theme_of(&ca_themes::chosen(&all, "dark-modern", true).json));
        let text = "// a note\nfn main() {\n\n    let a = 1;\n}";
        let whole = Snippet::new("rust").lines(text, &theme);
        let texts: Vec<&str> = whole.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["// a note", "fn main() {", "", "    let a = 1;", "}"]);
        let colour = |l: &Line, at: usize| l.runs.iter().find(|r| r.range.contains(&at)).map(|r| r.colour);
        // Dark Modern's comments are green, #6A9955; a keyword is not.
        assert_eq!(colour(&whole[0], 0), Some(Color::rgb8(0x6A, 0x99, 0x55)));
        assert_ne!(colour(&whole[1], 0), colour(&whole[0], 0));
        assert!(whole.iter().all(|l| l.runs.iter().all(|r| r.range.end <= l.text.len())));
        // Written a piece at a time, it is coloured the same, and the lines before the last are the lines given
        // before: nothing draws them again.
        let mut grown = Snippet::new("rust");
        let mut before = Vec::new();
        for end in 1..=text.len() {
            let now = grown.lines(&text[..end], &theme);
            for (old, new) in before.iter().zip(&now).take(now.len().saturating_sub(2)) {
                assert!(Rc::ptr_eq(old, new));
            }
            before = now;
        }
        let same = |a: &[Rc<Line>], b: &[Rc<Line>]| {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.text == b.text && a.runs == b.runs)
        };
        assert!(same(&before, &whole));
        // A language not known is drawn plain, a line at a time all the same; another theme colours all again.
        let plain = Snippet::new("").lines(text, &theme);
        assert!(plain.len() == 5 && plain.iter().all(|l| l.runs.is_empty()));
        let other = Rc::new((*theme).clone());
        let again = grown.lines(text, &other);
        assert!(again.iter().zip(&before).all(|(a, b)| a.id != b.id));
    }

    #[test]
    fn the_themes_token_colours_colour_the_scopes_as_vs_code_does() {
        let all = ca_themes::built_in();
        let theme = theme_of(&ca_themes::chosen(&all, "dark-modern", true).json);
        let h = Highlighter::new(&theme);
        let scope = |s: &str| ScopeStack::from_str(s).expect("scope");
        let comment = h.style_for_stack(scope("source.rust comment.line.double-slash.rust").as_slice());
        let keyword = h.style_for_stack(scope("source.rust keyword.control.rust").as_slice());
        assert_ne!(comment.foreground, keyword.foreground);
        // Dark Modern's comments are green, #6A9955.
        assert_eq!((comment.foreground.r, comment.foreground.g, comment.foreground.b), (0x6A, 0x99, 0x55));
        // A macro (println!) is coloured as a function is, #DCDCAA.
        let mac = h.style_for_stack(scope("source.rust support.macro.rust").as_slice());
        assert_eq!((mac.foreground.r, mac.foreground.g, mac.foreground.b), (0xDC, 0xDC, 0xAA));
    }

    /// A line's runs and their colours.
    type Spans = Vec<(std::ops::Range<usize>, u32)>;

    /// A file's lines as its grammar colours them under Dark Modern (every line parses): the colour of a piece of a
    /// line, by its first place in it.
    fn coloured(path: &str, text: &str) -> impl Fn(usize, &str) -> u32 {
        let all = ca_themes::built_in();
        let theme = theme_of(&ca_themes::chosen(&all, "dark-modern", true).json);
        let syntax = syntax_for(path, text.lines().next().unwrap_or("")).unwrap_or_else(|| panic!("{path}"));
        let mut lines = syntect::easy::HighlightLines::new(syntax, &theme);
        let mut out: Vec<(String, Spans)> = Vec::new();
        for line in text.split_inclusive('\n') {
            let runs = lines.highlight_line(line, &SYNTAXES).unwrap_or_else(|e| panic!("{path}: {line}: {e}"));
            let mut at = 0;
            let mut spans = Vec::new();
            for (style, piece) in runs {
                let c = style.foreground;
                spans.push((at..at + piece.len(), u32::from_be_bytes([0, c.r, c.g, c.b])));
                at += piece.len();
            }
            out.push((line.to_string(), spans));
        }
        let path = path.to_string();
        move |line, needle| {
            let (text, spans) = &out[line];
            let at = text.find(needle).unwrap_or_else(|| panic!("{path}:{line} has no {needle}"));
            spans.iter().find(|(r, _)| r.contains(&at)).map(|(_, c)| *c).expect("coloured")
        }
    }

    // Dark Modern's colours, as VS Code's.
    const COMMENT: u32 = 0x6A9955;
    const STRING: u32 = 0xCE9178;
    const REGEXP: u32 = 0xD16969;
    const NUMBER: u32 = 0xB5CEA8;
    const STORAGE: u32 = 0x569CD6;
    const CONTROL: u32 = 0xC586C0;
    const TYPE: u32 = 0x4EC9B0;
    const FUNCTION: u32 = 0xDCDCAA;
    const VARIABLE: u32 = 0x9CDCFE;
    const CONSTANT: u32 = 0x4FC1FF;

    #[test]
    fn the_windows_own_grammars_load_and_link() {
        for (name, text) in GRAMMARS {
            let grammar = SyntaxDefinition::load_from_str(text, true, None);
            assert!(grammar.as_ref().is_ok_and(|g| g.name == *name), "{name}: {:?}", grammar.err());
            assert!(SYNTAXES.find_syntax_by_name(name).is_some(), "{name}");
        }
        // What they include (TypeScript's code, HTML, CSS) is found.
        let theirs = SyntaxSet::load_defaults_newlines().find_unlinked_contexts();
        let unlinked: Vec<String> = SYNTAXES.find_unlinked_contexts().difference(&theirs).cloned().collect();
        assert!(unlinked.is_empty(), "{unlinked:?}");
        for (path, name) in [
            ("src/a.ts", "TypeScript"),
            ("a.mts", "TypeScript"),
            ("App.tsx", "TypeScript JSX"),
            ("App.jsx", "JavaScript JSX"),
            ("Cargo.toml", "TOML"),
            ("Dockerfile", "Dockerfile"),
            ("app.dockerfile", "Dockerfile"),
            ("a.kts", "Kotlin"),
            ("a.psm1", "PowerShell"),
            ("main.tf", "Terraform"),
            ("a.hcl", "Terraform"),
            ("schema.gql", "GraphQL"),
            ("App.vue", "Vue"),
            ("App.svelte", "Svelte"),
        ] {
            assert_eq!(syntax_for(path, "").map(|s| s.name.as_str()), Some(name), "{path}");
        }
        assert_eq!(syntax_for("run", "#!/usr/bin/env -S deno run").map(|s| s.name.as_str()), Some("TypeScript"));
        assert_eq!(syntax_for("run", "#!/usr/bin/env pwsh").map(|s| s.name.as_str()), Some("PowerShell"));
    }

    #[test]
    fn typescript_is_coloured_as_vs_code_colours_it() {
        let c = coloured(
            "a.ts",
            concat!(
                "// note\n",
                "import { a } from \"./b\";\n",
                "/** Doc @param x */\n",
                "export interface Shape { area(): number }\n",
                "const re = /a[/]b/g, t = `x ${ {k: 1}.k } y`;\n",
                "class Box<T> extends Base implements Shape {\n",
                "  private size: number = 0x1F;\n",
                "  area(): number { return this.size * 2; }\n",
                "}\n",
                "type Id = string | number;\n",
                "const go = async (x: Id) => x;\n",
                "export default function make(): Box<string> { return new Box(); }\n",
            ),
        );
        assert_eq!(c(0, "note"), COMMENT);
        assert_eq!(c(1, "import"), CONTROL);
        assert_eq!(c(1, "./b"), STRING);
        assert_eq!(c(2, "Doc"), COMMENT);
        assert_eq!(c(3, "interface"), STORAGE);
        assert_eq!(c(3, "Shape"), TYPE);
        assert_eq!(c(4, "a[/]b"), REGEXP);
        assert_eq!(c(4, "1}"), NUMBER);
        // The template goes on after its `${…}`, an object in it and all.
        assert_eq!(c(4, " y`"), STRING);
        assert_eq!(c(5, "class"), STORAGE);
        assert_eq!(c(5, "Box"), TYPE);
        assert_eq!(c(5, "Base"), TYPE);
        assert_eq!(c(6, "0x1F"), NUMBER);
        assert_eq!(c(6, "number"), TYPE);
        assert_eq!(c(7, "area"), FUNCTION);
        assert_eq!(c(7, "return"), CONTROL);
        assert_eq!(c(9, "Id"), TYPE);
        assert_eq!(c(10, "go"), FUNCTION);
        assert_eq!(c(10, "=>"), STORAGE);
        assert_eq!(c(11, "function"), STORAGE);
        assert_eq!(c(11, "make"), FUNCTION);
        assert_eq!(c(11, "return"), CONTROL);
    }

    #[test]
    fn jsx_tags_are_coloured_among_the_code() {
        let c = coloured(
            "a.tsx",
            concat!(
                "const el = <div className=\"a\" onClick={() => go({x: 1})}>\n",
                "  Hi &amp; {name} <br/>\n",
                "  <Item />\n",
                "</div>;\n",
                "const n = a < b ? 1 : 2;\n",
                "export function App() { return null; }\n",
                "const id = <T extends object>(x: T) => x;\n",
                "export const k = 1;\n",
            ),
        );
        assert_eq!(c(0, "div"), STORAGE);
        assert_eq!(c(0, "className"), VARIABLE);
        assert_eq!(c(0, "\"a\""), STRING);
        assert_eq!(c(0, "go"), FUNCTION);
        assert_eq!(c(2, "Item"), STORAGE);
        assert_eq!(c(4, "const"), STORAGE);
        assert_eq!(c(4, "1"), NUMBER);
        assert_eq!(c(5, "function"), STORAGE);
        assert_eq!(c(5, "App"), FUNCTION);
        // A generic function's parameters are no tag.
        assert_eq!(c(6, "object"), TYPE);
        assert_eq!(c(7, "export"), CONTROL);
        let c = coloured("a.jsx", "const a = <A b=\"c\" />;\nexport default a;\n");
        assert_eq!(c(0, "A "), STORAGE);
        assert_eq!(c(1, "export"), CONTROL);
    }

    #[test]
    fn toml_and_dockerfiles_are_coloured() {
        let c = coloured(
            "Cargo.toml",
            concat!(
                "# c\n[package]\nname = \"ca\" # x\nversion = 1_000\n[[bin]]\ninline = { a = true, b = [1, 2] }\n",
                "text = \"\"\"\nmany\n\"\"\"\nlast = 'lit'\n",
            ),
        );
        assert_eq!(c(0, "c"), COMMENT);
        assert_eq!(c(1, "package"), VARIABLE);
        assert_eq!(c(2, "ca"), STRING);
        assert_eq!(c(2, "x"), COMMENT);
        assert_eq!(c(3, "1_000"), NUMBER);
        assert_eq!(c(5, "true"), STORAGE);
        assert_eq!(c(5, "2"), NUMBER);
        assert_eq!(c(7, "many"), STRING);
        assert_eq!(c(9, "last"), VARIABLE);
        assert_eq!(c(9, "lit"), STRING);
        let c = coloured(
            "Dockerfile",
            concat!(
                "# syntax=docker/dockerfile:1\nFROM rust:1 AS build\nRUN cargo build --release && \\\n",
                "    echo \"done $MODE\"\nCOPY --from=build /a /b\nCMD [\"/b\"]\n",
            ),
        );
        assert_eq!(c(1, "FROM"), STORAGE);
        assert_eq!(c(1, "AS"), CONTROL);
        assert_eq!(c(2, "release"), VARIABLE);
        assert_eq!(c(3, "done"), STRING);
        assert_eq!(c(4, "COPY"), STORAGE);
        assert_eq!(c(4, "from"), VARIABLE);
        assert_eq!(c(5, "/b"), STRING);
        // As a fenced block in an answer draws it: each instruction its own.
        let all = ca_themes::built_in();
        let theme = Rc::new(theme_of(&ca_themes::chosen(&all, "dark-modern", true).json));
        let lines = Snippet::new("docker").lines("FROM a\nRUN b\nCOPY c d", &theme);
        let at = |l: &Line| l.runs.iter().find(|r| r.range.contains(&0)).map(|r| r.colour);
        assert!(lines.iter().all(|l| at(l) == Some(Color::rgb8(0x56, 0x9C, 0xD6))), "{lines:?}");
    }

    #[test]
    fn kotlin_powershell_terraform_and_graphql_are_coloured() {
        let c = coloured(
            "a.kt",
            concat!(
                "package app.core\n/* a /* nested */ still */\ndata class User(val name: String)\n",
                "fun String.shout(): String = \"${this.uppercase()}!\"\n",
                "fun main() { if (true) println(\"hi $x\") }\n",
                "fun size(): Int {\n",
                "class A : Base { init { run { } } }\n",
            ),
        );
        assert_eq!(c(1, "still"), COMMENT);
        assert_eq!(c(2, "class"), STORAGE);
        assert_eq!(c(2, "User"), TYPE);
        assert_eq!(c(3, "shout"), FUNCTION);
        assert_eq!(c(3, "!"), STRING);
        assert_eq!(c(4, "if"), CONTROL);
        assert_eq!(c(4, "main"), FUNCTION);
        assert_eq!(c(4, "hi"), STRING);
        // A type before a `{` is a type, a lower-case name a call with a lambda.
        assert_eq!(c(5, "Int"), TYPE);
        assert_eq!(c(6, "Base"), TYPE);
        assert_eq!(c(6, "run"), FUNCTION);
        let c = coloured(
            "a.ps1",
            concat!(
                "<# .SYNOPSIS\n  Help #>\nfunction Get-Thing { param([string]$Name) }\n",
                "$x = \"a $($Name.Length) b\"\nif ($x) { Write-Host 'yes' }\n",
            ),
        );
        assert_eq!(c(1, "Help"), COMMENT);
        assert_eq!(c(2, "function"), STORAGE);
        assert_eq!(c(2, "Get-Thing"), FUNCTION);
        assert_eq!(c(2, "$Name"), VARIABLE);
        assert_eq!(c(3, " b"), STRING);
        assert_eq!(c(4, "if"), CONTROL);
        assert_eq!(c(4, "Write-Host"), FUNCTION);
        assert_eq!(c(4, "yes"), STRING);
        let c = coloured(
            "main.tf",
            concat!(
                "# c\nresource \"aws_s3_bucket\" \"logs\" {\n  bucket = \"x-${var.env}\"\n  count  = 2\n}\n",
                "output \"o\" {\n  value = <<EOT\nhi ${local.a}\nEOT\n}\nlocals { on = true }\n",
            ),
        );
        assert_eq!(c(0, "c"), COMMENT);
        assert_eq!(c(1, "resource"), STORAGE);
        assert_eq!(c(1, "logs"), STRING);
        assert_eq!(c(2, "bucket"), VARIABLE);
        assert_eq!(c(2, "x-"), STRING);
        assert_eq!(c(3, "2"), NUMBER);
        assert_eq!(c(7, "hi"), STRING);
        assert_eq!(c(10, "locals"), STORAGE);
        assert_eq!(c(10, "true"), STORAGE);
        let c = coloured(
            "a.graphql",
            concat!(
                "# c\nquery Get($id: ID!) { user(id: $id) { ...F @include(if: true) } }\n",
                "fragment F on User { name }\n\"\"\"Doc\"\"\"\ntype User implements Node { id: ID! name: String }\n",
            ),
        );
        assert_eq!(c(0, "c"), COMMENT);
        assert_eq!(c(1, "query"), STORAGE);
        assert_eq!(c(1, "Get"), FUNCTION);
        assert_eq!(c(1, "ID"), TYPE);
        assert_eq!(c(1, "$id"), VARIABLE);
        assert_eq!(c(2, "F"), FUNCTION);
        assert_eq!(c(3, "Doc"), STRING);
        assert_eq!(c(4, "User"), TYPE);
        assert_eq!(c(4, "String"), TYPE);
    }

    #[test]
    fn vue_and_svelte_colour_their_template_script_and_style() {
        let c = coloured(
            "App.vue",
            concat!(
                "<template>\n  <p class=\"a\">{{ count + 1 }}</p>\n</template>\n<script lang=\"ts\">\n",
                "const count: number = 1;\n</script>\n<style>\np { color: red; }\n</style>\n<div>end</div>\n",
            ),
        );
        assert_eq!(c(1, "p "), STORAGE);
        assert_eq!(c(1, "class"), VARIABLE);
        assert_eq!(c(1, "1"), NUMBER);
        assert_eq!(c(4, "const"), STORAGE);
        assert_eq!(c(4, "count"), CONSTANT);
        assert_eq!(c(4, "number"), TYPE);
        assert_eq!(c(7, "color"), VARIABLE);
        assert_eq!(c(9, "div"), STORAGE);
        let c = coloured(
            "App.svelte",
            concat!(
                "<script lang=\"ts\">\n  let n = 0;\n</script>\n{#if n > 1}\n  <b>{n}</b>\n{:else}\n  <i>none</i>\n",
                "{/if}\n<style>b { color: red; }</style>\n",
            ),
        );
        assert_eq!(c(1, "let"), STORAGE);
        assert_eq!(c(3, "if"), CONTROL);
        assert_eq!(c(3, "1"), NUMBER);
        assert_eq!(c(4, "b"), STORAGE);
        assert_eq!(c(5, "else"), CONTROL);
        assert_eq!(c(7, "if"), CONTROL);
        assert_eq!(c(8, "color"), VARIABLE);
    }
}
