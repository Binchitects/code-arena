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
use floem::views::editor::text::{Document, Styling};
use floem::views::editor::text_document::TextDocument;
use serde_json::Value;
use syntect::highlighting::{
    FontStyle, HighlightState, Highlighter, RangedHighlightIterator, ScopeSelectors, StyleModifier, Theme, ThemeItem,
    ThemeSettings,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

use crate::widgets::MONO_FONT;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

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

/// The parse after each line (`after[n]` is the state at the start of line n + 1).
struct Parsed {
    after: Vec<(ParseState, HighlightState)>,
}

/// The editor's styling: VS Code's font and sizes, and each line's colours from its grammar and the theme.
pub struct Colours {
    doc: Rc<TextDocument>,
    syntax: Option<&'static SyntaxReference>,
    theme: RefCell<Theme>,
    parsed: RefCell<Parsed>,
    id: Cell<u64>,
    family: Vec<FamilyOwned>,
}

impl Colours {
    pub fn new(doc: Rc<TextDocument>, path: &str) -> Rc<Colours> {
        let first = doc.rope_text().line_content(0).to_string();
        Rc::new(Colours {
            doc,
            syntax: syntax_for(path, &first),
            theme: RefCell::new(Theme::default()),
            parsed: RefCell::new(Parsed { after: Vec::new() }),
            id: Cell::new(1),
            family: FamilyOwned::parse_list(&MONO_FONT).collect(),
        })
    }

    /// The file's language as VS Code's status bar names it.
    pub fn language(&self) -> String {
        self.syntax.map(|s| s.name.clone()).unwrap_or_else(|| "Plain Text".into())
    }

    /// Another theme: every line coloured again.
    pub fn set_theme(&self, theme: Theme) {
        *self.theme.borrow_mut() = theme;
        self.parsed.borrow_mut().after.clear();
        self.id.set(self.id.get() + 1);
    }

    /// The text changed from `line` on: what was parsed before it stands.
    pub fn changed_from(&self, line: usize) {
        self.parsed.borrow_mut().after.truncate(line);
        self.id.set(self.id.get() + 1);
    }

    /// The runs of a line with their style: parsed from the last line kept up to it.
    fn runs(&self, line: usize) -> Vec<(std::ops::Range<usize>, syntect::highlighting::Style)> {
        let Some(syntax) = self.syntax else { return Vec::new() };
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
        14
    }

    fn line_height(&self, _: EditorId, _: usize) -> f32 {
        19.0
    }

    fn font_family(&self, _: EditorId, _: usize) -> Cow<'_, [FamilyOwned]> {
        Cow::Borrowed(&self.family)
    }

    fn weight(&self, _: EditorId, _: usize) -> Weight {
        Weight::NORMAL
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

    fn apply_attr_styles(&self, _: EditorId, _: &EditorStyle, line: usize, default: Attrs, attrs: &mut AttrsList) {
        let runs = self.runs(line);
        if runs.is_empty() {
            // Plain text: the editor's own foreground.
            if let Some(f) = self.theme.borrow().settings.foreground {
                let len = self.doc.rope_text().line_content(line).len();
                attrs.add_span(0..len, default.color(Color::rgba8(f.r, f.g, f.b, f.a)));
            }
            return;
        }
        for (range, style) in runs {
            let f = style.foreground;
            let mut a = default.color(Color::rgba8(f.r, f.g, f.b, f.a));
            if style.font_style.contains(FontStyle::BOLD) {
                a = a.weight(Weight::BOLD);
            }
            if style.font_style.contains(FontStyle::ITALIC) {
                a = a.style(FontSlant::Italic);
            }
            attrs.add_span(range, a);
        }
    }
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
