//! The editor's highlights, as VS Code's: the matching bracket (`editor.matchBrackets`), brackets coloured by how
//! deep they are (`editor.bracketPairColorization.enabled`, `editorBracketHighlight.foreground1..6`), the other
//! places of the word at the caret (`editor.occurrencesHighlight`) and of the selected text
//! (`editor.selectionHighlight`), every match of the find widget (the current one apart), characters that look like
//! others or are not seen (`editor.unicodeHighlight.ambiguousCharacters`), and links: an address or a project's file
//! held Ctrl over is underlined, and Ctrl+click opens it (the address in the system's browser, the file in the
//! editor).
//!
//! The file's brackets and comments are read off the window's thread after each edit (strings and comments are
//! skipped, as VS Code's tokens skip them); the rest is looked for on the lines on screen and a screen above and
//! below them only, and the marks are given to `decor.rs` only when they are other than before (each new set lays
//! the lines on screen out again).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::LazyLock;

use floem::event::{Event, EventListener};
use floem::peniko::Color;
use floem::peniko::kurbo::Point;
use floem::reactive::{SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::views::editor::Editor;
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::mode::Mode;
use floem::views::editor::core::xi_rope::Rope;
use floem::views::editor::text::Document;
use floem::views::editor::text_document::TextDocument;
use floem::views::{Decorators, container};
use floem::{IntoView, ViewId};
use regex::Regex;
use serde_json::Value;

use crate::colors::Colors;
use crate::decor::{Look, Mark, Tint};
use crate::settings;
use crate::state::{Send, Ui};

/// The owner name the marks are given under.
const OWNER: &str = "highlights";
/// The largest file whose brackets are read (VS Code too stops colouring very large files).
const MOST: usize = 8 << 20;
/// The most find matches put in the overview ruler.
const RULER_MOST: usize = 2000;

/// A bracket of the text: where (a byte offset), which pair (`()` 0, `[]` 1, `{}` 2), opening or closing, how deep it
/// is (its pair's level), and the index of the bracket it pairs with (none: it pairs with none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bracket {
    pub at: usize,
    pub kind: u8,
    pub open: bool,
    pub level: usize,
    pub partner: Option<usize>,
}

/// What reading a text found: its brackets and its comments (from, to: byte offsets), both in order.
#[derive(Debug, Default, PartialEq)]
pub struct Scan {
    pub brackets: Vec<Bracket>,
    pub comments: Vec<(usize, usize)>,
}

/// How a language writes its comments and strings, for the brackets in them to be passed over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Syntax {
    line: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    quotes: &'static [u8],
    /// `'` starts a character (`'{'`) when one follows, else it is no quote (a Rust lifetime).
    chars: bool,
    /// Three quotes start a string to three quotes (Python's).
    triple: bool,
    /// Prose (Markdown, plain text): its Unicode is not looked at, as VS Code's defaults leave it.
    prose: bool,
}

const C_LIKE: Syntax =
    Syntax { line: &["//"], block: Some(("/*", "*/")), quotes: b"\"'", chars: true, triple: false, prose: false };
const JS: Syntax =
    Syntax { line: &["//"], block: Some(("/*", "*/")), quotes: b"\"'`", chars: false, triple: false, prose: false };
const HASH: Syntax = Syntax { line: &["#"], block: None, quotes: b"\"'", chars: false, triple: false, prose: false };
const PLAIN: Syntax = Syntax { line: &[], block: None, quotes: b"", chars: false, triple: false, prose: false };

/// The comments and strings of a file's language, by its name's ending.
pub fn syntax_of(path: &str) -> Syntax {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "rs" | "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "cs" | "java" | "kt" | "kts" | "swift" | "scala"
        | "dart" | "zig" => C_LIKE,
        "go" => Syntax { quotes: b"\"'`", ..C_LIKE },
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "json" | "jsonc" | "vue" | "svelte" => JS,
        "css" => Syntax { line: &[], quotes: b"\"'", ..JS },
        "scss" | "less" => Syntax { quotes: b"\"'", ..JS },
        "py" | "pyw" | "pyi" => Syntax { triple: true, ..HASH },
        "sh" | "bash" | "zsh" | "fish" | "yaml" | "yml" | "toml" | "rb" | "r" | "pl" | "ps1" | "cmake" | "mk"
        | "dockerfile" | "nix" => HASH,
        "lua" | "sql" | "hs" => Syntax { line: &["--"], ..HASH },
        "md" | "markdown" | "txt" | "" => Syntax { prose: true, ..PLAIN },
        _ if name == "Makefile" || name == "Dockerfile" => HASH,
        _ => PLAIN,
    }
}

/// Where a character literal that starts at `i` (a `'`) ends; none when it is not one (a lifetime, an apostrophe).
fn char_end(b: &[u8], i: usize) -> Option<usize> {
    let next = *b.get(i + 1)?;
    if next == b'\\' {
        // An escape: `'\n'`, `'\''`, `'\u{1F600}'`.
        let end = (i + 3..(i + 14).min(b.len())).find(|&j| b[j] == b'\'' || b[j] == b'\n')?;
        return (b[end] == b'\'').then_some(end + 1);
    }
    let width = match next {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    };
    (next != b'\'' && b.get(i + 1 + width) == Some(&b'\'')).then_some(i + 2 + width)
}

/// A text's brackets, paired as VS Code pairs them (a closing bracket closes the nearest open one of its kind, those
/// open inside it left unpaired; one with none open is unexpected), and its comments; strings and comments are passed
/// over.
pub fn scan(text: &str, syn: Syntax) -> Scan {
    let b = text.as_bytes();
    let mut out = Scan::default();
    let mut open: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphanumeric() || c == b' ' || c == b'\n' {
            i += 1;
            continue;
        }
        let rest = &b[i..];
        if syn.line.iter().any(|l| rest.starts_with(l.as_bytes())) {
            let end = rest.iter().position(|&x| x == b'\n').map_or(b.len(), |n| i + n);
            out.comments.push((i, end));
            i = end;
            continue;
        }
        if let Some((from, to)) = syn.block
            && rest.starts_with(from.as_bytes())
        {
            let body = i + from.len();
            let end = text[body..].find(to).map_or(b.len(), |n| body + n + to.len());
            out.comments.push((i, end));
            i = end;
            continue;
        }
        if syn.quotes.contains(&c) {
            if c == b'\'' && syn.chars {
                i = char_end(b, i).unwrap_or(i + 1);
                continue;
            }
            if syn.triple && rest.starts_with(&[c, c, c]) {
                let close = [c, c, c];
                i = b[i + 3..].windows(3).position(|w| w == close).map_or(b.len(), |n| i + 3 + n + 3);
                continue;
            }
            // To the quote that closes it; a line's end closes all but a template string.
            let mut j = i + 1;
            while j < b.len() {
                match b[j] {
                    b'\\' => j += 1,
                    x if x == c => {
                        j += 1;
                        break;
                    }
                    b'\n' if c != b'`' => break,
                    _ => {}
                }
                j += 1;
            }
            i = j.min(b.len());
            continue;
        }
        let kind = match c {
            b'(' | b')' => 0,
            b'[' | b']' => 1,
            b'{' | b'}' => 2,
            _ => {
                i += 1;
                continue;
            }
        };
        let index = out.brackets.len();
        if matches!(c, b'(' | b'[' | b'{') {
            out.brackets.push(Bracket { at: i, kind, open: true, level: open.len(), partner: None });
            open.push(index);
        } else {
            match open.iter().rposition(|&o| out.brackets[o].kind == kind) {
                Some(p) => {
                    let o = open[p];
                    open.truncate(p);
                    out.brackets[o].partner = Some(index);
                    let level = out.brackets[o].level;
                    out.brackets.push(Bracket { at: i, kind, open: false, level, partner: Some(o) });
                }
                None => out.brackets.push(Bracket { at: i, kind, open: false, level: 0, partner: None }),
            }
        }
        i += 1;
    }
    out
}

/// What is odd about a character, as VS Code's Unicode highlighting says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Odd {
    /// It is not seen (a zero-width space, a direction mark).
    Invisible,
    /// It looks like this more common ASCII character.
    Like(char),
}

/// Whether a character is one VS Code marks: an invisible one, or one that could be taken for an ASCII one.
pub fn odd(c: char) -> Option<Odd> {
    if c.is_ascii() {
        return None;
    }
    let like = match c {
        '\u{200B}'
        | '\u{200C}'
        | '\u{200E}'
        | '\u{200F}'
        | '\u{2060}'..='\u{2064}'
        | '\u{FEFF}'
        | '\u{00AD}'
        | '\u{180E}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}' => return Some(Odd::Invisible),
        '\u{00A0}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
        '\u{2018}' | '\u{2019}' | '\u{02BC}' | '\u{2032}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{2033}' => '"',
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2212}' => '-',
        '\u{037E}' => ';',
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or('?'),
        'а' => 'a',
        'е' => 'e',
        'о' | 'ο' => 'o',
        'р' => 'p',
        'с' => 'c',
        'у' => 'y',
        'х' => 'x',
        'і' => 'i',
        'ј' => 'j',
        'ѕ' => 's',
        'ν' => 'v',
        'А' | 'Α' => 'A',
        'В' | 'Β' => 'B',
        'Е' | 'Ε' => 'E',
        'Ζ' => 'Z',
        'Н' | 'Η' => 'H',
        'І' | 'Ι' => 'I',
        'К' | 'Κ' => 'K',
        'М' | 'Μ' => 'M',
        'Ν' => 'N',
        'О' | 'Ο' => 'O',
        'Р' | 'Ρ' => 'P',
        'С' => 'C',
        'Т' | 'Τ' => 'T',
        'Υ' => 'Y',
        'Х' | 'Χ' => 'X',
        _ => return None,
    };
    Some(Odd::Like(like))
}

/// What VS Code's hover says of an odd character.
pub fn odd_text(c: char, odd: Odd) -> String {
    let name = |c: char| format!("U+{:04X}", c as u32);
    match odd {
        Odd::Invisible => format!("The character {} is invisible.", name(c)),
        Odd::Like(a) => format!(
            "The character {} \"{c}\" could be confused with the ASCII character {} \"{a}\", which is more common in source code.",
            name(c),
            name(a)
        ),
    }
}

/// How a highlighted range looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    /// A bracket in the colour of its level (the index among the theme's colours that are not see-through).
    Bracket(usize),
    Unexpected,
    Match,
    Word,
    Selected,
    Find,
    FindNow,
    Unicode(char, Odd),
    /// A link: with Ctrl held over it, underlined in the link colour.
    Link(bool),
}

/// A range highlighted: from, to (byte offsets in the text), and its look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hl {
    pub from: usize,
    pub to: usize,
    pub style: Style,
}

/// VS Code's `editor.matchBrackets`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchBrackets {
    Never,
    /// Only the bracket at the caret and its pair.
    Near,
    /// Those, else the pair the caret is inside.
    Always,
}

/// What is on: the settings, and what the find widget asks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opts {
    pub match_brackets: MatchBrackets,
    pub colorize: bool,
    pub occurrences: bool,
    pub selection: bool,
    pub unicode: bool,
    /// The find widget's Match Case: the selection's other places are then looked for with its case.
    pub case: bool,
    /// How many of the theme's bracket colours are not see-through (VS Code cycles through those).
    pub colours: usize,
}

/// What highlights are looked for in: a piece of the text (`text`, from `base` in the whole), the caret, the
/// selection, the file's brackets and comments, the find widget's matches, and the link held Ctrl over.
pub struct Ask<'a> {
    pub text: &'a str,
    pub base: usize,
    pub caret: usize,
    pub selection: Option<(usize, usize)>,
    pub scan: &'a Scan,
    pub find: &'a [(usize, usize)],
    pub link: Option<(usize, usize)>,
    pub syntax: Syntax,
    pub opts: Opts,
}

/// VS Code's `editor.wordSeparators`, and space: what a word stops at.
fn word_char(c: char) -> bool {
    !c.is_whitespace() && !"`~!@#$%^&*()-=+[{]}\\|;:'\",.<>/?".contains(c)
}

/// The word the caret is in or at the end of, as from, to in `text`.
pub fn word_at(text: &str, at: usize) -> Option<(usize, usize)> {
    if at > text.len() || !text.is_char_boundary(at) {
        return None;
    }
    let from = text[..at].char_indices().rev().take_while(|(_, c)| word_char(*c)).last().map_or(at, |(i, _)| i);
    let to = text[at..].char_indices().find(|(_, c)| !word_char(*c)).map_or(text.len(), |(i, _)| at + i);
    (from < to).then_some((from, to))
}

/// Where a whole word is in a text.
fn whole_words(text: &str, word: &str) -> Vec<(usize, usize)> {
    text.match_indices(word)
        .filter(|(i, _)| {
            let before = text[..*i].chars().next_back();
            let after = text[i + word.len()..].chars().next();
            !before.is_some_and(word_char) && !after.is_some_and(word_char)
        })
        .map(|(i, _)| (i, i + word.len()))
        .collect()
}

/// The ranges where `a`'s highlights are in its piece of the text, in the order they are drawn (backgrounds first).
pub fn highlights(a: &Ask) -> Vec<Hl> {
    let (base, end) = (a.base, a.base + a.text.len());
    let inside = |from: usize, to: usize| from >= base && to <= end;
    let mut out = Vec::new();
    let selected = a.selection.filter(|(f, t)| f < t);
    // The find widget's matches, the one selected apart.
    let first = a.find.partition_point(|h| h.1 < base);
    for &(from, to) in a.find[first..].iter().take_while(|h| h.0 <= end) {
        let style = if selected == Some((from, to)) { Style::FindNow } else { Style::Find };
        out.push(Hl { from, to, style });
    }
    // The word at the caret's other places, or the selection's.
    match selected {
        None if a.opts.occurrences && a.caret >= base && a.caret <= end => {
            if let Some((f, t)) = word_at(a.text, a.caret - base) {
                for (from, to) in whole_words(a.text, &a.text[f..t]) {
                    out.push(Hl { from: base + from, to: base + to, style: Style::Word });
                }
            }
        }
        Some((from, to)) if a.opts.selection && inside(from, to) => {
            let needle = &a.text[from - base..to - base];
            if !needle.contains('\n') && !needle.trim().is_empty() {
                let found: Vec<usize> = if a.opts.case {
                    a.text.match_indices(needle).map(|m| m.0).collect()
                } else {
                    let (hay, needle) = (a.text.to_ascii_lowercase(), needle.to_ascii_lowercase());
                    hay.match_indices(&needle).map(|m| m.0).collect()
                };
                for at in found.into_iter().map(|i| base + i).filter(|i| *i != from) {
                    out.push(Hl { from: at, to: at + (to - from), style: Style::Selected });
                }
            }
        }
        _ => {}
    }
    let brackets = &a.scan.brackets;
    // The brackets' colours.
    let first = brackets.partition_point(|b| b.at < base);
    let shown = &brackets[first..first + brackets[first..].partition_point(|b| b.at < end)];
    if a.opts.colorize && a.opts.colours > 0 {
        for b in shown {
            let style = match b.partner {
                Some(_) => Style::Bracket(b.level % a.opts.colours),
                None => Style::Unexpected,
            };
            out.push(Hl { from: b.at, to: b.at + 1, style });
        }
    }
    // The bracket at the caret (before it first) and its pair; with none, the pair the caret is in.
    if a.opts.match_brackets != MatchBrackets::Never {
        let at = |o: usize| brackets.binary_search_by_key(&o, |b| b.at).ok().filter(|&i| brackets[i].partner.is_some());
        let near = a.caret.checked_sub(1).and_then(at).or_else(|| at(a.caret));
        let pair = near.map(|i| (i, brackets[i].partner.unwrap_or(i))).or_else(|| {
            (a.opts.match_brackets == MatchBrackets::Always).then(|| enclosing(brackets, a.caret)).flatten()
        });
        if let Some((x, y)) = pair {
            for i in [x, y] {
                out.push(Hl { from: brackets[i].at, to: brackets[i].at + 1, style: Style::Match });
            }
        }
    }
    // Characters that look like others, but not in comments.
    if a.opts.unicode && !a.syntax.prose && !a.text.is_ascii() {
        let comments = &a.scan.comments;
        for (i, c) in a.text.char_indices() {
            let Some(o) = odd(c) else { continue };
            let at = base + i;
            let k = comments.partition_point(|x| x.1 <= at);
            if comments.get(k).is_some_and(|x| x.0 <= at) {
                continue;
            }
            out.push(Hl { from: at, to: at + c.len_utf8(), style: Style::Unicode(c, o) });
        }
    }
    // Links, for their hover; the one held Ctrl over underlined.
    for m in URL.find_iter(a.text) {
        let (from, to) = (base + m.start(), base + m.start() + url_len(m.as_str()));
        out.push(Hl { from, to, style: Style::Link(a.link == Some((from, to))) });
    }
    if let Some((from, to)) = a.link.filter(|l| !out.iter().any(|h| (h.from, h.to) == *l)) {
        out.push(Hl { from, to, style: Style::Link(true) });
    }
    out
}

/// The innermost pair of brackets the caret is between.
fn enclosing(brackets: &[Bracket], caret: usize) -> Option<(usize, usize)> {
    let mut i = brackets.partition_point(|b| b.at < caret);
    while i > 0 {
        i -= 1;
        let b = brackets[i];
        match (b.open, b.partner) {
            (true, Some(p)) if brackets[p].at >= caret => return Some((i, p)),
            // A pair that closes before the caret: passed over whole.
            (false, Some(p)) => i = p,
            _ => {}
        }
    }
    None
}

static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s<>"'`)\]}]+"#).expect("an address"));

/// An address's length, without the sentence's punctuation after it.
fn url_len(m: &str) -> usize {
    m.trim_end_matches(['.', ',', ';', ':', '!', '?']).len()
}

/// Where a link is in a line (from, to) at a column, and where it leads: an address, or a file of the project.
pub fn link_at(
    line: &str,
    col: usize,
    files: &std::collections::HashSet<String>,
) -> Option<(usize, usize, crate::citations::Target)> {
    let has = |from: usize, to: usize| from <= col && col < to;
    if let Some(m) = URL.find_iter(line).find(|m| has(m.start(), m.start() + url_len(m.as_str()))) {
        let address = &m.as_str()[..url_len(m.as_str())];
        return Some((m.start(), m.start() + address.len(), crate::citations::Target::Web(address.to_string())));
    }
    crate::citations::find_refs(line, files)
        .into_iter()
        .find(|(r, _)| has(r.start, r.end))
        .map(|(r, to)| (r.start, r.end, crate::citations::Target::File(to)))
}

const BRACKETS: [fn(&Colors) -> Color; 6] =
    [|c| c.bracket1, |c| c.bracket2, |c| c.bracket3, |c| c.bracket4, |c| c.bracket5, |c| c.bracket6];

/// The theme's bracket colours that are not see-through, as indexes into `BRACKETS`.
fn bracket_colours(c: &Colors) -> Vec<usize> {
    (0..BRACKETS.len()).filter(|&i| BRACKETS[i](c).a > 0).collect()
}

/// A highlight's look and hover.
fn look(style: Style, colours: &[usize]) -> (Look, Option<String>) {
    let bg = |f: fn(&Colors) -> Color| Look { bg: Some(Tint::Theme(f)), ..Look::default() };
    match style {
        Style::Bracket(i) => (Look { fg: Some(Tint::Theme(BRACKETS[colours[i]])), ..Look::default() }, None),
        Style::Unexpected => (Look { fg: Some(Tint::Theme(|c| c.bracket_unexpected)), ..Look::default() }, None),
        Style::Match => (
            Look {
                bg: Some(Tint::Theme(|c| c.bracket_match_bg)),
                underline: Some(Tint::Theme(|c| c.bracket_match_border)),
                ..Look::default()
            },
            None,
        ),
        Style::Word => (bg(|c| c.word_highlight), None),
        Style::Selected => (bg(|c| c.selection_highlight), None),
        Style::Find => (bg(|c| c.find_match_highlight), None),
        Style::FindNow => (bg(|c| c.find_match), None),
        Style::Unicode(c, o) => {
            (Look { underline: Some(Tint::Theme(|c| c.unicode_border)), ..Look::default() }, Some(odd_text(c, o)))
        }
        Style::Link(held) => {
            let hover =
                Some(format!("Follow link ({} + click)", if cfg!(target_os = "macos") { "cmd" } else { "ctrl" }));
            if held {
                let t = Tint::Theme(|c| c.link_active);
                (Look { fg: Some(t), underline: Some(t), ..Look::default() }, hover)
            } else {
                (Look::default(), hover)
            }
        }
    }
}

/// The marks for highlights, with the overview ruler's marks of the find widget's matches.
fn marks(text: &impl RopeText, hls: &[Hl], find: &[(usize, usize)], c: &Colors) -> Vec<Mark> {
    let colours = bracket_colours(c);
    let mut out: Vec<Mark> = hls
        .iter()
        .filter(|h| !matches!(h.style, Style::Bracket(i) if i >= colours.len()))
        .map(|h| {
            let (look, hover) = look(h.style, &colours);
            Mark::Range { from: text.offset_to_line_col(h.from), to: text.offset_to_line_col(h.to), look, hover }
        })
        .collect();
    let mut last = None;
    for &(from, _) in find.iter().take(RULER_MOST) {
        let line = text.line_of_offset(from);
        if last != Some(line) {
            out.push(Mark::Ruler { line, tint: Tint::Theme(|c| c.ruler_find) });
            last = Some(line);
        }
    }
    out
}

thread_local! {
    /// The highlights given last: by which editor, of which file, and what they were (with the find widget's
    /// matches they were made with), so the same are not given again.
    static GIVEN: RefCell<Option<(ViewId, String, Vec<Hl>, Rc<Vec<(usize, usize)>>)>> = const { RefCell::new(None) };
}

/// The file's brackets and comments as read last, and the text they were read from.
#[derive(Clone)]
struct Read {
    text: Rope,
    scan: Rc<Scan>,
}

/// The highlights of an editor of a file followed while it is on screen: its text read again after each edit (off
/// the window's thread), its highlights looked for again when the caret, the lines on screen, the find widget's
/// matches, the theme or the settings change. Its view is the editor with Ctrl+click on links.
pub fn follow<V: IntoView + 'static>(
    ui: Ui,
    view: V,
    editor: &Editor,
    doc: Rc<TextDocument>,
    path: &str,
) -> impl IntoView + use<V> {
    let syntax = syntax_of(path);
    let read = create_rw_signal(None::<Read>);
    let link = create_rw_signal(None::<(usize, usize)>);
    let generation = create_rw_signal(0u64);
    // The text read again after each edit (and once at first).
    let reading = doc.clone();
    create_effect(move |_| {
        ui.edited.track();
        let text = reading.text();
        if read.with_untracked(|r| r.as_ref().is_some_and(|r| r.text.ptr_eq(&text))) {
            return;
        }
        generation.update(|g| *g += 1);
        let g = generation.get_untracked();
        let back = text.clone();
        let done = floem::ext_event::create_ext_action(ui.scope, move |scan: Scan| {
            if generation.try_get_untracked() == Some(g) {
                read.set(Some(Read { text: back, scan: Rc::new(scan) }));
            }
        });
        std::thread::spawn(move || {
            let scan = if text.len() > MOST { Scan::default() } else { scan(&text.to_string(), syntax) };
            done(scan)
        });
    });
    // The lines looked at: those on screen and a screen above and below, again only when those on screen leave them.
    let lines = create_rw_signal((0usize, 0usize));
    let (viewport, shown) = (editor.viewport, editor.clone());
    create_effect(move |_| {
        let v = viewport.get();
        let height = (shown.line_height(0) as f64).max(1.0);
        let (top, bottom) = ((v.y0 / height).floor() as usize, (v.y1 / height).ceil() as usize);
        let (from, to) = lines.get_untracked();
        if top < from || bottom > to || to == 0 {
            let page = bottom.saturating_sub(top).max(20);
            lines.set((top.saturating_sub(page), bottom + page));
        }
    });
    let (path_now, ed) = (path.to_string(), editor.clone());
    let view_id = ViewId::new();
    create_effect(move |_| {
        let c = ui.colors.get();
        let (from, to) = lines.get();
        let cursor = ed.cursor.get();
        let find = if ui.find.get() { ui.find_hits.get() } else { Rc::new(Vec::new()) };
        let held = link.get();
        let opts = Opts {
            match_brackets: match settings::text(ui, "editor.matchBrackets", None).as_str() {
                "never" => MatchBrackets::Never,
                "near" => MatchBrackets::Near,
                _ => MatchBrackets::Always,
            },
            colorize: settings::flag(ui, "editor.bracketPairColorization.enabled", None),
            occurrences: settings::text(ui, "editor.occurrencesHighlight", None) != "off",
            selection: settings::flag(ui, "editor.selectionHighlight", None),
            unicode: settings::flag(ui, "editor.unicodeHighlight.ambiguousCharacters", None),
            case: ui.find_case.get(),
            colours: bracket_colours(&c).len(),
        };
        let Some(r) = read.get() else { return };
        let text = ed.rope_text();
        // An edit not read yet: the highlights wait for it (the brackets' places are those of the text before).
        if !r.text.ptr_eq(text.text()) {
            return;
        }
        let last = text.last_line();
        let base = text.offset_of_line(from.min(last));
        let end = text.offset_of_line((to + 1).min(last + 1)).max(base);
        let piece = text.slice_to_cow(base..end);
        let selection = cursor.get_selection().map(|(a, b)| (a.min(b), a.max(b)));
        let ask = Ask {
            text: &piece,
            base,
            caret: cursor.offset(),
            selection,
            scan: &r.scan,
            find: &find,
            link: held,
            syntax,
            opts,
        };
        let hls = highlights(&ask);
        let same = GIVEN.with(|g| {
            g.borrow()
                .as_ref()
                .is_some_and(|(v, p, h, f)| *v == view_id && *p == path_now && *h == hls && Rc::ptr_eq(f, &find))
        });
        if same {
            return;
        }
        crate::decor::set(ui, OWNER, &path_now, marks(&text, &hls, &find, &c));
        GIVEN.with(|g| *g.borrow_mut() = Some((view_id, path_now.clone(), hls, find)));
    });
    let origin = create_rw_signal(Point::ZERO);
    let (pressed, moved, path) = (editor.clone(), editor.clone(), path.to_string());
    let gone = path.clone();
    container(view)
        .on_move(move |p| origin.set(p))
        .on_event_cont(EventListener::PointerDown, move |e| {
            let Event::PointerDown(p) = e else { return };
            if !held(&p.modifiers) {
                return;
            }
            if let Some((_, _, to)) = link_under(ui, &pressed, p.pos + origin.get_untracked().to_vec2()) {
                crate::citations::open(ui, &to);
            }
        })
        .on_event_cont(EventListener::PointerMove, move |e| {
            let Event::PointerMove(p) = e else { return };
            let now = if held(&p.modifiers) {
                link_under(ui, &moved, p.pos + origin.get_untracked().to_vec2()).map(|(a, b, _)| (a, b))
            } else {
                None
            };
            if link.get_untracked() != now {
                link.set(now);
            }
        })
        .on_event_cont(EventListener::PointerLeave, move |_| {
            if link.get_untracked().is_some() {
                link.set(None);
            }
        })
        // Gone from the screen: its highlights with it, unless the next editor (drawn before this one goes) gave its own.
        .on_cleanup(move || {
            let mine = GIVEN.with(|g| g.borrow().as_ref().is_some_and(|(v, _, _, _)| *v == view_id));
            if mine {
                GIVEN.with(|g| *g.borrow_mut() = None);
                crate::decor::clear(ui, OWNER, &gone);
            }
        })
        .style(|s| s.size_full())
}

/// Ctrl (Cmd on macOS) held: a link is followed by a click.
fn held(m: &floem::keyboard::Modifiers) -> bool {
    if cfg!(target_os = "macos") { m.meta() } else { m.control() }
}

/// The link under a point of the window over an editor: from, to (offsets in the text), and where it leads.
fn link_under(ui: Ui, editor: &Editor, win: Point) -> Option<(usize, usize, crate::citations::Target)> {
    let (o, v) = (editor.window_origin.get_untracked(), editor.viewport.get_untracked());
    let at = Point::new(win.x - o.x + v.x0, win.y - o.y + v.y0);
    let ((line, col), inside) = editor.line_col_of_point(Mode::Insert, at);
    if !inside || win.x < o.x {
        return None;
    }
    let text = editor.rope_text();
    let start = text.offset_of_line(line);
    let content = text.slice_to_cow(start..text.line_end_offset(line, true));
    let files = ui.state::<crate::citations::Citations>().files.get_untracked();
    link_at(&content, col, &files).map(|(a, b, to)| (start + a, start + b, to))
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: Opts = Opts {
        match_brackets: MatchBrackets::Always,
        colorize: true,
        occurrences: true,
        selection: true,
        unicode: true,
        case: false,
        colours: 3,
    };

    fn ask<'a>(text: &'a str, scan: &'a Scan, caret: usize, selection: Option<(usize, usize)>) -> Ask<'a> {
        Ask { text, base: 0, caret, selection, scan, find: &[], link: None, syntax: C_LIKE, opts: ON }
    }

    fn styled(hls: &[Hl], f: impl Fn(Style) -> bool) -> Vec<(usize, usize)> {
        hls.iter().filter(|h| f(h.style)).map(|h| (h.from, h.to)).collect()
    }

    #[test]
    fn brackets_pair_by_kind_and_level_and_skip_strings_and_comments() {
        let s = scan("f(a[0], { b: \"(\" }) // )\n/* ] */ '{' 'a x)", C_LIKE);
        let at: Vec<(usize, usize, Option<usize>)> = s.brackets.iter().map(|b| (b.at, b.level, b.partner)).collect();
        // ( [ ] { } ) and the last ), which closes nothing.
        assert_eq!(
            at,
            vec![
                (1, 0, Some(5)),
                (3, 1, Some(2)),
                (5, 1, Some(1)),
                (8, 1, Some(4)),
                (17, 1, Some(3)),
                (18, 0, Some(0)),
                (41, 0, None)
            ]
        );
        assert_eq!(s.comments, vec![(20, 24), (25, 32)]);
        // A closing bracket closes the nearest open one of its kind; those open inside it pair with none.
        let s = scan("{ ( }", PLAIN);
        assert_eq!(s.brackets[0].partner, Some(2));
        assert_eq!(s.brackets[1].partner, None);
        // Python's three quotes, and a template string over lines.
        assert!(scan("'''(\n'''", syntax_of("a.py")).brackets.is_empty());
        assert!(scan("`(\n${x}`", syntax_of("a.ts")).brackets.is_empty());
    }

    #[test]
    fn the_caret_matches_its_bracket_or_the_pair_it_is_in() {
        let text = "a(bb{c}dd)e";
        let s = scan(text, C_LIKE);
        let at = |caret, how| {
            let mut a = ask(text, &s, caret, None);
            a.opts.match_brackets = how;
            styled(&highlights(&a), |s| s == Style::Match)
        };
        // After a bracket, before one, inside a pair.
        assert_eq!(at(2, MatchBrackets::Near), vec![(1, 2), (9, 10)]);
        assert_eq!(at(4, MatchBrackets::Near), vec![(4, 5), (6, 7)]);
        assert_eq!(at(3, MatchBrackets::Always), vec![(1, 2), (9, 10)]);
        assert_eq!(at(3, MatchBrackets::Near), vec![]);
        assert_eq!(at(11, MatchBrackets::Always), vec![]);
        assert_eq!(at(2, MatchBrackets::Never), vec![]);
        // The colours by level, cycling through the theme's.
        let a = ask(text, &s, 0, None);
        let colours: Vec<Style> =
            highlights(&a).iter().map(|h| h.style).filter(|s| matches!(s, Style::Bracket(_))).collect();
        assert_eq!(colours, vec![Style::Bracket(0), Style::Bracket(1), Style::Bracket(1), Style::Bracket(0)]);
    }

    #[test]
    fn the_word_at_the_caret_and_the_selection_are_found_again() {
        let text = "let cart = cart_id + cart;\nCART";
        let s = Scan::default();
        assert_eq!(word_at(text, 6), Some((4, 8)));
        assert_eq!(word_at(text, 8), Some((4, 8)));
        assert_eq!(word_at(text, 9), None);
        let words = styled(&highlights(&ask(text, &s, 5, None)), |s| s == Style::Word);
        assert_eq!(words, vec![(4, 8), (21, 25)]);
        // The selection's other places, case aside unless the find widget asks for it; not across lines.
        let sel = styled(&highlights(&ask(text, &s, 8, Some((4, 8)))), |s| s == Style::Selected);
        assert_eq!(sel, vec![(11, 15), (21, 25), (27, 31)]);
        let mut a = ask(text, &s, 8, Some((4, 8)));
        a.opts.case = true;
        assert_eq!(styled(&highlights(&a), |s| s == Style::Selected), vec![(11, 15), (21, 25)]);
        assert!(styled(&highlights(&ask(text, &s, 8, Some((20, 28)))), |s| s == Style::Selected).is_empty());
        // Only in the piece looked at, its offsets the whole text's.
        let mut a = ask(&text[11..], &s, 22, None);
        a.base = 11;
        assert_eq!(styled(&highlights(&a), |s| s == Style::Word), vec![(21, 25)]);
    }

    #[test]
    fn find_matches_unicode_and_links_are_marked() {
        let text = "x = \"са\" // с\nsee https://example.com/a. ok";
        let s = scan(text, C_LIKE);
        let mut a = ask(text, &s, 0, Some((0, 1)));
        let find = [(0, 1), (4, 5)];
        a.find = &find;
        let hls = highlights(&a);
        assert_eq!(styled(&hls, |s| s == Style::FindNow), vec![(0, 1)]);
        assert_eq!(styled(&hls, |s| s == Style::Find), vec![(4, 5)]);
        // The two Cyrillic letters in the string, not the one in the comment.
        assert_eq!(styled(&hls, |s| matches!(s, Style::Unicode(..))), vec![(5, 7), (7, 9)]);
        let url = text.find("https").unwrap();
        assert_eq!(styled(&hls, |s| matches!(s, Style::Link(_))), vec![(url, url + 21)]);
        assert_eq!(odd('\u{200B}'), Some(Odd::Invisible));
        assert_eq!(odd('с'), Some(Odd::Like('c')));
        assert_eq!(odd('é'), None);
        assert_eq!(odd_text('\u{200B}', Odd::Invisible), "The character U+200B is invisible.");
        // Markdown's Unicode is its own.
        a.syntax = syntax_of("README.md");
        assert!(styled(&highlights(&a), |s| matches!(s, Style::Unicode(..))).is_empty());
    }

    #[test]
    fn a_link_under_a_column_leads_to_the_web_or_a_file() {
        let files: std::collections::HashSet<String> = ["src/app.rs".to_string()].into();
        let line = "see https://a.io/x, and src/app.rs:3 too";
        let web = link_at(line, 6, &files).map(|l| (l.0, l.1, l.2));
        assert!(matches!(web, Some((4, 18, crate::citations::Target::Web(ref a))) if a == "https://a.io/x"));
        let file = link_at(line, 26, &files);
        assert!(
            matches!(file, Some((24, 36, crate::citations::Target::File(ref r))) if r.path == "src/app.rs" && r.line == Some(3))
        );
        assert!(link_at(line, 1, &files).is_none());
    }
}
