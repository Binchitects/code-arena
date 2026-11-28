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
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

use crate::widgets::MONO_FONT;

pub static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

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
    let s = SYNTAXES
        .find_syntax_by_extension(ext)
        .or_else(|| match ext {
            "ts" | "tsx" | "mts" | "cts" => SYNTAXES.find_syntax_by_name("JavaScript"),
            "toml" => SYNTAXES.find_syntax_by_extension("ini"),
            _ => None,
        })
        .or_else(|| SYNTAXES.find_syntax_by_first_line(first_line))?;
    (s.name != "Plain Text").then_some(s)
}

/// The grammar a fence names: a language's name or a file ending (`rust`, `rs`, `ts`), or what answers often call it.
pub fn syntax_named(lang: &str) -> Option<&'static SyntaxReference> {
    let lang = match lang {
        "" => return None,
        "typescript" | "tsx" | "jsx" | "javascriptreact" | "typescriptreact" => "js",
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
    /// The file's path, by which its decorations are found (`decor.rs`).
    path: String,
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
            path: path.to_string(),
        })
    }

    /// Another grammar (its language picked, or found again): every line coloured again. False: it was this one.
    pub fn set_syntax(&self, syntax: Option<&'static SyntaxReference>) -> bool {
        if self.syntax.get().map(std::ptr::from_ref) == syntax.map(std::ptr::from_ref) {
            return false;
        }
        self.syntax.set(syntax);
        self.parsed.borrow_mut().after.clear();
        self.id.set(self.id.get() + 1);
        true
    }

    /// Another theme: every line coloured again.
    pub fn set_theme(&self, theme: Theme) {
        *self.theme.borrow_mut() = theme;
        self.parsed.borrow_mut().after.clear();
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

    /// The text changed from `line` on: what was parsed before it stands.
    pub fn changed_from(&self, line: usize) {
        self.parsed.borrow_mut().after.truncate(line);
        self.id.set(self.id.get() + 1);
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
        4
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
        assert_eq!(name("ts"), Some("JavaScript"));
        assert_eq!(name("typescript"), Some("JavaScript"));
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
}
