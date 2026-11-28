//! The compare editor's changes between two texts, as VS Code's diff editor finds them: the runs of lines that
//! differ (each a hunk: the original's lines and the modified's in their place), the words that differ inside a
//! hunk, and the empty rows (fillers) each side needs under a hunk for the lines after it to stand beside each
//! other. The lines are the editor's: the text split at each `\n` (a text ending in one has an empty last line),
//! a `\r` before it left out when lines are compared. With "ignore trim whitespace" (VS Code's
//! `diffEditor.ignoreTrimWhitespace`) lines that differ only in the spaces at their start and end are the same.
//!
//! What every part has (`register`, `take`) is said in `ext.rs`.

use std::ops::Range;
use std::time::{Duration, Instant};

use serde_json::Value;
use similar::{Algorithm, DiffTag, capture_diff_slices_deadline};

use crate::diff::{Line, Side};
use crate::state::{Send, Ui};

/// A run of lines that differ: the original's (left) and the modified's (right), by line (from 0, the end not in
/// it). An empty side is where the other's lines were taken away or put in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hunk {
    pub left: Range<usize>,
    pub right: Range<usize>,
}

impl Hunk {
    /// Whether it changed lines (both sides have some), not only took them away or put them in.
    pub fn is_change(&self) -> bool {
        !self.left.is_empty() && !self.right.is_empty()
    }
}

/// A part of a line that differs: its line and its bytes in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    pub line: usize,
    pub cols: Range<usize>,
}

/// Two texts compared.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Compared {
    pub hunks: Vec<Hunk>,
    /// The words that differ in the hunks that changed lines (not those that only took away or put in).
    pub left_words: Vec<Word>,
    pub right_words: Vec<Word>,
    /// Each side's lines.
    pub left_lines: usize,
    pub right_lines: usize,
    /// Whether the comparison stopped at its time (`diffEditor.maxComputationTime`): what is found then is still
    /// right, but coarser.
    pub cut: bool,
}

/// The longest a hunk may be on either side for its words to be compared (the work grows as their product).
const MOST_WORD_LINES: usize = 400;

/// A side's empty rows: each the line they are drawn above and how many.
pub type Fillers = Vec<(usize, usize)>;

/// The editor's lines of a text, each without its `\n`.
pub fn lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

fn key(line: &str, trim: bool) -> &str {
    let line = line.strip_suffix('\r').unwrap_or(line);
    if trim { line.trim() } else { line }
}

/// Two texts compared by line, then by word inside each hunk; `trim` ignores the spaces at the lines' ends, and
/// `most` is the longest the lines may take to compare (none: no end).
pub fn compare(left: &str, right: &str, trim: bool, most: Option<Duration>) -> Compared {
    let (a, b) = (lines(left), lines(right));
    let ka: Vec<&str> = a.iter().map(|l| key(l, trim)).collect();
    let kb: Vec<&str> = b.iter().map(|l| key(l, trim)).collect();
    let deadline = most.map(|d| Instant::now() + d);
    let ops = capture_diff_slices_deadline(Algorithm::Myers, &ka, &kb, deadline);
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut open: Option<Hunk> = None;
    for op in &ops {
        let (tag, l, r) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            hunks.extend(open.take());
            continue;
        }
        match open.as_mut() {
            Some(h) => {
                h.left.end = l.end;
                h.right.end = r.end;
            }
            None => open = Some(Hunk { left: l, right: r }),
        }
    }
    hunks.extend(open);
    let cut = deadline.is_some_and(|d| Instant::now() >= d);
    let mut out = Compared { left_lines: a.len(), right_lines: b.len(), cut, ..Compared::default() };
    for h in &hunks {
        if h.is_change() && h.left.len() <= MOST_WORD_LINES && h.right.len() <= MOST_WORD_LINES {
            let (l, r) = words(&a[h.left.clone()], &b[h.right.clone()], trim);
            out.left_words.extend(l.into_iter().map(|w| Word { line: w.line + h.left.start, ..w }));
            out.right_words.extend(r.into_iter().map(|w| Word { line: w.line + h.right.start, ..w }));
        }
    }
    out.hunks = hunks;
    out
}

/// A piece of a hunk's text: a word (letters, digits, `_`), a run of spaces, a line's end, or one other character;
/// with its line in the hunk and its bytes in that line.
#[derive(Clone, Debug)]
struct Token<'a> {
    text: &'a str,
    line: usize,
    cols: Range<usize>,
}

fn tokens<'a>(lines: &[&'a str]) -> Vec<Token<'a>> {
    let mut out = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let mut at = 0;
        let class = |c: char| {
            if c.is_alphanumeric() || c == '_' {
                0
            } else if c.is_whitespace() {
                1
            } else {
                2
            }
        };
        while let Some(c) = line[at..].chars().next() {
            let kind = class(c);
            let mut end = at + c.len_utf8();
            if kind != 2 {
                while let Some(d) = line[end..].chars().next().filter(|d| class(*d) == kind) {
                    end += d.len_utf8();
                }
            }
            out.push(Token { text: &line[at..end], line: n, cols: at..end });
            at = end;
        }
        if n + 1 < lines.len() {
            out.push(Token { text: "\n", line: n, cols: line.len()..line.len() });
        }
    }
    out
}

/// The words that differ between a hunk's two sides, each side's by its line in the hunk. Those next to each
/// other on a line are one; the spaces at the lines' ends are not marked when `trim` ignores them.
fn words(left: &[&str], right: &[&str], trim: bool) -> (Vec<Word>, Vec<Word>) {
    let (ta, tb) = (tokens(left), tokens(right));
    let ka: Vec<&str> = ta.iter().map(|t| t.text).collect();
    let kb: Vec<&str> = tb.iter().map(|t| t.text).collect();
    let ops = capture_diff_slices_deadline(Algorithm::Myers, &ka, &kb, None);
    let (mut l, mut r) = (Vec::new(), Vec::new());
    let mark = |out: &mut Vec<Word>, side: &[Token], range: Range<usize>, lines: &[&str]| {
        for t in &side[range] {
            if t.text == "\n" {
                continue;
            }
            let line = lines[t.line].strip_suffix('\r').unwrap_or(lines[t.line]);
            let blank_end = trim && t.text.trim().is_empty() && (t.cols.start == 0 || t.cols.end == line.len());
            if blank_end {
                continue;
            }
            match out.last_mut() {
                Some(w) if w.line == t.line && w.cols.end == t.cols.start => w.cols.end = t.cols.end,
                _ => out.push(Word { line: t.line, cols: t.cols.clone() }),
            }
        }
    };
    for op in &ops {
        let (tag, a, b) = op.as_tag_tuple();
        if tag != DiffTag::Equal {
            mark(&mut l, &ta, a, left);
            mark(&mut r, &tb, b, right);
        }
    }
    (l, r)
}

/// The empty rows each side has (left, right), each as the line they are drawn above and how many: under a hunk,
/// the side with fewer lines has as many rows as the other has more (the line "above" may be the line count, under
/// the last).
pub fn fillers(hunks: &[Hunk]) -> (Fillers, Fillers) {
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for h in hunks {
        let (a, b) = (h.left.len(), h.right.len());
        if a > b {
            r.push((h.right.end, a - b));
        } else if b > a {
            l.push((h.left.end, b - a));
        }
    }
    (l, r)
}

/// The lines added and taken away, as VS Code's toolbar counts them (the modified side's lines in hunks, and the
/// original's).
pub fn counts(hunks: &[Hunk]) -> (usize, usize) {
    hunks.iter().fold((0, 0), |(a, r), h| (a + h.right.len(), r + h.left.len()))
}

/// The hunk to go to from a line of the modified side: the next one after it, or the one before it, round the end
/// as VS Code's Next and Previous Change go.
pub fn next(hunks: &[Hunk], line: usize, forward: bool) -> Option<usize> {
    if hunks.is_empty() {
        return None;
    }
    if forward {
        Some(hunks.iter().position(|h| h.right.start > line).unwrap_or(0))
    } else {
        Some(hunks.iter().rposition(|h| h.right.start < line).unwrap_or(hunks.len() - 1))
    }
}

/// A hunk reverted: the modified text's bytes to replace and the original's lines to put there.
pub fn revert(left: &str, right: &str, h: &Hunk) -> (Range<usize>, String) {
    let (a, b) = (lines(left), lines(right));
    // Where each line of the modified side begins.
    let start_of = |n: usize| b[..n].iter().map(|l| l.len() + 1).sum::<usize>();
    if h.right.end == b.len() && h.left.end == a.len() {
        // At the end of both: the last line has no `\n` after it, so the one before the hunk is taken with it.
        if h.right.start == 0 {
            return (0..right.len(), a[h.left.clone()].join("\n"));
        }
        let from = start_of(h.right.start) - 1;
        let text: String = a[h.left.clone()].iter().map(|l| format!("\n{l}")).collect();
        return (from..right.len(), text);
    }
    let text: String = a[h.left.clone()].iter().map(|l| format!("{l}\n")).collect();
    (start_of(h.right.start)..start_of(h.right.end), text)
}

/// The two texts as one column, VS Code's inline view: the original's lines that went, then the modified's that
/// came, at each hunk, between the lines both have.
pub fn inline(left: &str, right: &str, c: &Compared) -> Vec<Line> {
    let (a, b) = (lines(left), lines(right));
    let mut out = Vec::with_capacity(a.len().max(b.len()));
    let (mut i, mut j) = (0, 0);
    let same = |i: usize, j: usize, out: &mut Vec<Line>| {
        out.push(Line { side: Side::Same, old: Some(i + 1), new: Some(j + 1), text: b[j].to_string() })
    };
    for h in &c.hunks {
        while i < h.left.start && j < h.right.start {
            same(i, j, &mut out);
            i += 1;
            j += 1;
        }
        for n in h.left.clone() {
            out.push(Line { side: Side::Removed, old: Some(n + 1), new: None, text: a[n].to_string() });
        }
        for n in h.right.clone() {
            out.push(Line { side: Side::Added, old: None, new: Some(n + 1), text: b[n].to_string() });
        }
        (i, j) = (h.left.end, h.right.end);
    }
    while i < a.len() && j < b.len() {
        same(i, j, &mut out);
        i += 1;
        j += 1;
    }
    out
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

    fn apply(right: &str, (range, text): (Range<usize>, String)) -> String {
        let mut s = right.to_string();
        s.replace_range(range, &text);
        s
    }

    #[test]
    fn a_known_pair_is_aligned_by_its_hunks_and_fillers() {
        let left = "one\ntwo\nthree\nfour\nfive\n";
        let right = "one\n2\nthree\nfour\nfour and a half\nfive\nsix\n";
        let c = compare(left, right, false, None);
        assert_eq!(
            c.hunks,
            [Hunk { left: 1..2, right: 1..2 }, Hunk { left: 4..4, right: 4..5 }, Hunk { left: 5..5, right: 6..7 }]
        );
        assert_eq!((c.left_lines, c.right_lines), (6, 8));
        let (l, r) = fillers(&c.hunks);
        assert_eq!(l, [(4, 1), (5, 1)], "the original gets a row under four and under five");
        assert!(r.is_empty());
        // Every line both have stands on the same row once the fillers are counted.
        let row = |fill: &[(usize, usize)], line: usize| {
            line + fill.iter().filter(|f| f.0 <= line).map(|f| f.1).sum::<usize>()
        };
        assert_eq!(row(&l, 3), row(&r, 3), "four beside four");
        assert_eq!(row(&l, 4), row(&r, 5), "five beside five");
        assert_eq!(row(&l, 5), row(&r, 7), "the last lines beside each other");
        assert_eq!(counts(&c.hunks), (3, 1));
    }

    #[test]
    fn the_words_that_changed_are_marked_on_each_side() {
        let c = compare("let a = old_name(1);\n", "let a = new_name(1, 2);\n", false, None);
        assert_eq!(c.hunks, [Hunk { left: 0..1, right: 0..1 }]);
        assert_eq!(c.left_words, [Word { line: 0, cols: 8..16 }]);
        assert_eq!(c.right_words, [Word { line: 0, cols: 8..16 }, Word { line: 0, cols: 18..21 }]);
        // Lines only put in are not compared by word: the whole line is the change.
        let c = compare("a\n", "a\nb\n", false, None);
        assert!(c.right_words.is_empty());
    }

    #[test]
    fn ignoring_trim_whitespace_makes_reindented_lines_the_same() {
        let (left, right) = ("fn a() {\n  x();\n}\n", "fn a() {\n    x();  \n}\n");
        assert_eq!(compare(left, right, false, None).hunks.len(), 1);
        assert!(compare(left, right, true, None).hunks.is_empty());
        // A real change inside a reindented line marks the word only, not the spaces.
        let c = compare("  a b\n", "    a c\n", true, None);
        assert_eq!(c.right_words, [Word { line: 0, cols: 6..7 }]);
    }

    #[test]
    fn a_reverted_hunk_gives_back_the_original() {
        let cases = [
            ("one\ntwo\nthree\n", "one\n2\nthree\n"),
            ("one\ntwo\n", "one\n"),
            ("one\n", "one\ntwo\n"),
            ("a\nb", "a\nb\nc"),
            ("a\nb\nc", "a\nb"),
            ("x", "y\nz"),
            ("", "new\n"),
            ("a\r\nb\r\n", "a\r\nc\r\n"),
        ];
        for (left, right) in cases {
            let c = compare(left, right, false, None);
            assert_eq!(c.hunks.len(), 1, "{left:?} {right:?}");
            assert_eq!(apply(right, revert(left, right, &c.hunks[0])), left, "{left:?} {right:?}");
        }
        // One of several, the others left.
        let (left, right) = ("a\nb\nc\nd\n", "a\nB\nc\nD\n");
        let c = compare(left, right, false, None);
        assert_eq!(apply(right, revert(left, right, &c.hunks[1])), "a\nB\nc\nd\n");
    }

    #[test]
    fn next_and_previous_change_go_round() {
        let hunks = [Hunk { left: 2..3, right: 2..3 }, Hunk { left: 9..9, right: 9..12 }];
        assert_eq!(next(&hunks, 0, true), Some(0));
        assert_eq!(next(&hunks, 2, true), Some(1));
        assert_eq!(next(&hunks, 9, true), Some(0));
        assert_eq!(next(&hunks, 9, false), Some(0));
        assert_eq!(next(&hunks, 0, false), Some(1));
        assert_eq!(next(&[], 0, true), None);
    }

    #[test]
    fn the_inline_view_puts_the_lines_that_went_before_those_that_came() {
        let (left, right) = ("a\nb\nc", "a\nB\nc");
        let rows = inline(left, right, &compare(left, right, false, None));
        let sides: Vec<Side> = rows.iter().map(|r| r.side).collect();
        assert_eq!(sides, [Side::Same, Side::Removed, Side::Added, Side::Same]);
        assert_eq!((rows[3].old, rows[3].new), (Some(3), Some(3)));
    }
}
