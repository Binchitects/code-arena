//! Markdown as an answer is written in it, read into the blocks the Agent view draws: paragraphs, headings, list
//! items, quotes, fenced code and rules, their text with its bold, italic, code and link runs. An answer comes a
//! piece at a time, so every beginning of a text reads well: a fence not closed yet is code to the end, and a mark
//! not closed yet is the mark itself.

use std::ops::Range;

/// How a run of text is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: bool,
}

/// A block's text without its marks, and the runs of it that are not plain (a line break in it is a hard one).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Text {
    pub text: String,
    pub spans: Vec<(Range<usize>, Style)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Block {
    Paragraph(Text),
    /// Its level, from 1.
    Heading(u8, Text),
    /// A list's item: how deep it is nested (from 0), its bullet or number (none: a further paragraph of the item
    /// before it), its text.
    Item {
        depth: u8,
        marker: String,
        text: Text,
    },
    Quote(Text),
    /// A fenced block: the language its fence names (lower case, may be none), its lines.
    Code {
        lang: String,
        text: String,
    },
    Rule,
}

/// What is being gathered from the lines read so far.
enum Open {
    Paragraph,
    Item { depth: u8, marker: String },
    Quote,
}

fn close(open: &mut Option<(Open, String)>, out: &mut Vec<Block>) {
    if let Some((kind, raw)) = open.take() {
        let text = inline(raw.trim_end());
        out.push(match kind {
            Open::Paragraph => Block::Paragraph(text),
            Open::Item { depth, marker } => Block::Item { depth, marker, text },
            Open::Quote => Block::Quote(text),
        });
    }
}

/// How many times `c` is repeated from `at`.
fn run(b: &[u8], at: usize, c: u8) -> usize {
    b[at.min(b.len())..].iter().take_while(|x| **x == c).count()
}

/// A fence's opening: its character, its length, the language it names.
fn fence(rest: &str) -> Option<(u8, usize, String)> {
    let c = *rest.as_bytes().first()?;
    if c != b'`' && c != b'~' {
        return None;
    }
    let n = run(rest.as_bytes(), 0, c);
    let info = rest[n..].trim();
    if n < 3 || (c == b'`' && info.contains('`')) {
        return None;
    }
    let lang = info.split(|x: char| x.is_whitespace() || x == ',' || x == '{').next().unwrap_or("");
    Some((c, n, lang.to_lowercase()))
}

fn heading(rest: &str) -> Option<(u8, &str)> {
    let n = run(rest.as_bytes(), 0, b'#');
    let after = &rest[n..];
    if !(1..=6).contains(&n) || !(after.is_empty() || after.starts_with([' ', '\t'])) {
        return None;
    }
    let text = after.trim();
    // Closing #s, when they stand apart.
    let cut = text.trim_end_matches('#');
    let text = if cut.is_empty() || cut.ends_with(' ') { cut.trim_end() } else { text };
    Some((n as u8, text))
}

/// A line of three or more of one of `-`, `*`, `_`, with nothing but spaces between.
fn rule(rest: &str) -> bool {
    let mut marks = rest.bytes().filter(|c| !c.is_ascii_whitespace());
    let Some(first) = marks.next() else { return false };
    matches!(first, b'-' | b'*' | b'_') && marks.clone().all(|c| c == first) && marks.count() >= 2
}

/// A list item's opening: its bullet or number as drawn, its number when it has one, and the text after.
fn item(rest: &str) -> Option<(String, Option<u64>, &str)> {
    let b = rest.as_bytes();
    let (marker, number, len) = match *b.first()? {
        b'-' | b'*' | b'+' => ("•".to_string(), None, 1),
        c if c.is_ascii_digit() => {
            let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
            if digits > 9 || !matches!(b.get(digits), Some(b'.' | b')')) {
                return None;
            }
            (format!("{}.", &rest[..digits]), rest[..digits].parse().ok(), digits + 1)
        }
        _ => return None,
    };
    match b.get(len) {
        None => Some((marker, number, "")),
        Some(b' ' | b'\t') => Some((marker, number, rest[len + 1..].trim_start())),
        _ => None,
    }
}

/// The text read into its blocks.
pub fn parse(src: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut open: Option<(Open, String)> = None;
    // The indents of the lists open, the outermost first.
    let mut levels: Vec<usize> = Vec::new();
    let mut lines = src.lines();
    while let Some(line) = lines.next() {
        let rest = line.trim_start();
        let indent = line[..line.len() - rest.len()].bytes().map(|c| if c == b'\t' { 4 } else { 1 }).sum::<usize>();
        if rest.is_empty() {
            close(&mut open, &mut out);
            continue;
        }
        if let Some((c, n, lang)) = fence(rest) {
            close(&mut open, &mut out);
            if indent == 0 {
                levels.clear();
            }
            let mut text = String::new();
            for l in lines.by_ref() {
                let t = l.trim_start();
                if run(t.as_bytes(), 0, c) >= n && t.trim_start_matches(c as char).trim().is_empty() {
                    break;
                }
                // The fence's own indent is not the code's.
                let own = l.len() - t.len();
                text.push_str(&l[own.min(indent)..]);
                text.push('\n');
            }
            text.pop();
            out.push(Block::Code { lang, text });
            continue;
        }
        if let Some((level, text)) = heading(rest) {
            close(&mut open, &mut out);
            levels.clear();
            out.push(Block::Heading(level, inline(text)));
            continue;
        }
        // A paragraph underlined is a heading.
        if matches!(open, Some((Open::Paragraph, _))) && rest.trim_end().bytes().all(|c| c == b'=' || c == b'-') {
            let all = |c: u8| rest.trim_end().bytes().all(|x| x == c);
            if all(b'=') || (all(b'-') && rest.trim_end().len() >= 2) {
                let raw = open.take().map(|(_, raw)| raw).unwrap_or_default();
                out.push(Block::Heading(if all(b'=') { 1 } else { 2 }, inline(raw.trim_end())));
                continue;
            }
        }
        if rule(rest) {
            close(&mut open, &mut out);
            levels.clear();
            out.push(Block::Rule);
            continue;
        }
        if let Some(quoted) = rest.strip_prefix('>') {
            let quoted = quoted.strip_prefix(' ').unwrap_or(quoted).trim_end();
            match &mut open {
                Some((Open::Quote, raw)) => {
                    raw.push(if quoted.is_empty() || raw.ends_with('\n') { '\n' } else { ' ' });
                    raw.push_str(quoted);
                }
                _ => {
                    close(&mut open, &mut out);
                    levels.clear();
                    open = Some((Open::Quote, quoted.to_string()));
                }
            }
            continue;
        }
        if let Some((marker, number, text)) = item(rest)
            // Only a list that begins at 1 breaks into a paragraph ("... in\n1984. Then").
            && !(matches!(open, Some((Open::Paragraph, _))) && number.is_some_and(|n| n != 1))
        {
            close(&mut open, &mut out);
            while levels.last().is_some_and(|l| *l > indent) {
                levels.pop();
            }
            if levels.last() != Some(&indent) {
                levels.push(indent);
            }
            let depth = (levels.len() - 1).min(8) as u8;
            open = Some((Open::Item { depth, marker }, text.to_string()));
            continue;
        }
        match &mut open {
            Some((kind, raw)) => {
                // Two spaces or a backslash at a line's end break the line; a table's rows stay rows.
                let hard = raw.ends_with("  ") || raw.ends_with('\\');
                let row = matches!(kind, Open::Paragraph) && rest.starts_with('|');
                let kept = raw.trim_end_matches('\\').trim_end().len();
                raw.truncate(kept);
                raw.push(if hard || row { '\n' } else { ' ' });
                raw.push_str(rest);
            }
            None if !levels.is_empty() && indent >= 2 => {
                let depth = (levels.len() - 1).min(8) as u8;
                open = Some((Open::Item { depth, marker: String::new() }, rest.to_string()));
            }
            None => {
                levels.clear();
                open = Some((Open::Paragraph, rest.to_string()));
            }
        }
    }
    close(&mut open, &mut out);
    out
}

/// A block's text read: its marks taken out, the runs they made kept.
pub fn inline(src: &str) -> Text {
    let mut out = Text::default();
    walk(src, Style::default(), &mut out);
    out
}

fn push(out: &mut Text, s: &str, style: Style) {
    if s.is_empty() {
        return;
    }
    let start = out.text.len();
    out.text.push_str(s);
    if style != Style::default() {
        match out.spans.last_mut() {
            Some((r, st)) if *st == style && r.end == start => r.end = out.text.len(),
            _ => out.spans.push((start..out.text.len(), style)),
        }
    }
}

/// A letter or a digit (a letter beyond ASCII too): what `_` inside a word stands next to.
fn word(c: Option<&u8>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric() || *c >= 0x80)
}

/// Where the run of `n` backticks that closes a code span is, looked for from `from`.
fn closing_ticks(b: &[u8], from: usize, n: usize) -> Option<usize> {
    let mut j = from;
    while j < b.len() {
        if b[j] == b'`' {
            let m = run(b, j, b'`');
            if m == n {
                return Some(j);
            }
            j += m;
        } else {
            j += 1;
        }
    }
    None
}

/// Where the `n` of `c` that close an emphasis are, looked for from `from` (code spans and escapes passed over).
fn closer(b: &[u8], from: usize, c: u8, n: usize) -> Option<usize> {
    let mut j = from;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'`' => {
                let m = run(b, j, b'`');
                j = closing_ticks(b, j + m, m).map(|e| e + m).unwrap_or(j + m);
            }
            x if x == c => {
                let m = run(b, j, c);
                let closes =
                    j > from && !b[j - 1].is_ascii_whitespace() && m >= n && (c == b'*' || !word(b.get(j + m)));
                // `**` met while a single `*` is open is a bold inside it.
                if closes && !(n == 1 && m == 2) {
                    return Some(j + (m - n));
                }
                j += m;
            }
            _ => j += 1,
        }
    }
    None
}

/// A link's parts, `[` being at `at`: where its text ends, and where the whole of it ends.
fn link(b: &[u8], at: usize) -> Option<(usize, usize)> {
    let (mut j, mut depth) = (at + 1, 0usize);
    let text_end = loop {
        match *b.get(j)? {
            b'\\' => j += 1,
            b'`' => {
                let m = run(b, j, b'`');
                j = closing_ticks(b, j + m, m).map(|e| e + m).unwrap_or(j + m) - 1;
            }
            b'[' => depth += 1,
            b']' if depth == 0 => break j,
            b']' => depth -= 1,
            _ => {}
        }
        j += 1;
    };
    if b.get(text_end + 1) != Some(&b'(') {
        return None;
    }
    let (mut j, mut depth) = (text_end + 2, 0usize);
    loop {
        match *b.get(j)? {
            b'\n' => return None,
            b'(' => depth += 1,
            b')' if depth == 0 => return Some((text_end, j + 1)),
            b')' => depth -= 1,
            _ => {}
        }
        j += 1;
    }
}

/// Where an address written bare ends: at a space, less the punctuation that closes the sentence around it.
fn address_end(s: &str, at: usize) -> usize {
    let end = s[at..].find(|c: char| c.is_whitespace() || c == '<' || c == '`').map(|n| at + n).unwrap_or(s.len());
    at + s[at..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '\'', '"', '*', '_']).len()
}

fn walk(s: &str, style: Style, out: &mut Text) {
    let b = s.as_bytes();
    // Every place the text is cut at is an ASCII mark's, so no letter is cut in two.
    let (mut i, mut plain) = (0, 0);
    while i < b.len() {
        // The marked run taken: what it ends at, once what it holds has been pushed.
        let mut taken = None;
        match b[i] {
            b'\\' if b.get(i + 1).is_some_and(u8::is_ascii_punctuation) => {
                push(out, &s[plain..i], style);
                push(out, &s[i + 1..i + 2], style);
                taken = Some(i + 2);
            }
            b'`' => {
                let n = run(b, i, b'`');
                match closing_ticks(b, i + n, n) {
                    Some(end) => {
                        push(out, &s[plain..i], style);
                        let mut code = &s[i + n..end];
                        if code.len() >= 2 && code.starts_with(' ') && code.ends_with(' ') && !code.trim().is_empty() {
                            code = &code[1..code.len() - 1];
                        }
                        push(out, code, Style { code: true, ..style });
                        taken = Some(end + n);
                    }
                    None => i += n,
                }
            }
            c @ (b'*' | b'_') => {
                let all = run(b, i, c);
                let n = all.min(3);
                let opens = all <= 3
                    && b.get(i + n).is_some_and(|x| !x.is_ascii_whitespace())
                    && (c == b'*' || !word(i.checked_sub(1).and_then(|p| b.get(p))));
                match closer(b, i + n, c, n).filter(|_| opens) {
                    Some(end) => {
                        push(out, &s[plain..i], style);
                        let inner = Style { bold: style.bold || n >= 2, italic: style.italic || n != 2, ..style };
                        walk(&s[i + n..end], inner, out);
                        taken = Some(end + n);
                    }
                    None => i += all,
                }
            }
            b'[' | b'!' => {
                let at = if b[i] == b'!' { i + 1 } else { i };
                match if b.get(at) == Some(&b'[') { link(b, at) } else { None } {
                    Some((text_end, end)) => {
                        push(out, &s[plain..i], style);
                        let linked = Style { link: true, ..style };
                        if text_end == at + 1 {
                            push(out, &s[text_end + 2..end - 1], linked);
                        } else {
                            walk(&s[at + 1..text_end], linked, out);
                        }
                        taken = Some(end);
                    }
                    None => i += 1,
                }
            }
            b'<' => {
                let end = s[i..].find('>').map(|n| i + n);
                let address = end.map(|e| &s[i + 1..e]).filter(|a| {
                    (a.starts_with("http://") || a.starts_with("https://") || a.starts_with("mailto:"))
                        && !a.contains(char::is_whitespace)
                });
                match (address, end) {
                    (Some(a), Some(end)) => {
                        push(out, &s[plain..i], style);
                        push(out, a, Style { link: true, ..style });
                        taken = Some(end + 1);
                    }
                    _ => i += 1,
                }
            }
            b'h' if !style.link
                && (s[i..].starts_with("http://") || s[i..].starts_with("https://"))
                && !word(i.checked_sub(1).and_then(|p| b.get(p))) =>
            {
                let end = address_end(s, i);
                push(out, &s[plain..i], style);
                push(out, &s[i..end], Style { link: true, ..style });
                taken = Some(end);
            }
            _ => i += 1,
        }
        if let Some(end) = taken {
            i = end;
            plain = end;
        }
    }
    push(out, &s[plain.min(s.len())..], style);
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

    fn runs(t: &Text) -> Vec<(&str, Style)> {
        t.spans.iter().map(|(r, s)| (&t.text[r.clone()], *s)).collect()
    }

    const BOLD: Style = Style { bold: true, italic: false, code: false, link: false };
    const ITALIC: Style = Style { bold: false, italic: true, code: false, link: false };
    const CODE: Style = Style { bold: false, italic: false, code: true, link: false };
    const LINK: Style = Style { bold: false, italic: false, code: false, link: true };

    #[test]
    fn a_texts_marks_become_its_runs() {
        let t = inline("A **bold** and *slanted* and `let x = 1;` word, and [the docs](https://x.dev/a).");
        assert_eq!(t.text, "A bold and slanted and let x = 1; word, and the docs.");
        assert_eq!(runs(&t), [("bold", BOLD), ("slanted", ITALIC), ("let x = 1;", CODE), ("the docs", LINK)]);
        let t = inline("***both*** and __bold__ and _slanted_, **with `code` in it**");
        assert_eq!(t.text, "both and bold and slanted, with code in it");
        let both = Style { bold: true, italic: true, ..Style::default() };
        let bold_code = Style { bold: true, code: true, ..Style::default() };
        assert_eq!(
            runs(&t),
            [
                ("both", both),
                ("bold", BOLD),
                ("slanted", ITALIC),
                ("with ", BOLD),
                ("code", bold_code),
                (" in it", BOLD)
            ]
        );
        let t = inline("*a **b** c*");
        assert_eq!(t.text, "a b c");
        assert_eq!(runs(&t)[1], ("b", both));
    }

    #[test]
    fn what_is_not_a_mark_stays_as_written() {
        assert_eq!(inline("snake_case_name and 2 * 3 * 4").spans, []);
        assert_eq!(inline("snake_case_name and 2 * 3 * 4").text, "snake_case_name and 2 * 3 * 4");
        // Not closed yet (the answer is still being written): the mark itself.
        assert_eq!(inline("a **bold beginning").text, "a **bold beginning");
        assert_eq!(inline("a `code beginning").text, "a `code beginning");
        assert_eq!(inline("a [link](https://x").text, "a [link](https://x");
        assert_eq!(inline(r"\*not slanted\*").text, "*not slanted*");
        // Marks inside code are code.
        let t = inline("`**a**` and ``a ` b``");
        assert_eq!(runs(&t), [("**a**", CODE), ("a ` b", CODE)]);
    }

    #[test]
    fn addresses_are_links() {
        let t = inline("See https://example.com/a_b?c=1. Or <https://x.dev>, or ![a picture](p.png)");
        assert_eq!(t.text, "See https://example.com/a_b?c=1. Or https://x.dev, or a picture");
        assert_eq!(runs(&t), [("https://example.com/a_b?c=1", LINK), ("https://x.dev", LINK), ("a picture", LINK)]);
    }

    #[test]
    fn lines_become_blocks() {
        let blocks = parse("# Title\n\nOne line\nand its next.\n\n## Second ##\nText\n===\n---\n> quoted\n> more\n");
        assert_eq!(
            blocks,
            [
                Block::Heading(1, inline("Title")),
                Block::Paragraph(inline("One line and its next.")),
                Block::Heading(2, inline("Second")),
                Block::Heading(1, inline("Text")),
                Block::Rule,
                Block::Quote(inline("quoted more")),
            ]
        );
        assert_eq!(parse("a  \nb\n| x |\n| y |")[0], Block::Paragraph(inline("a\nb\n| x |\n| y |")));
        assert_eq!(parse("#not a heading"), [Block::Paragraph(inline("#not a heading"))]);
    }

    #[test]
    fn lists_keep_their_depth_and_their_numbers() {
        let it = |depth, marker: &str, text| Block::Item { depth, marker: marker.into(), text: inline(text) };
        let blocks = parse(
            "- one\n  continued\n- two\n  - inner\n    1. first\n    2. second\n- three\n\n  More of three.\n\nAfter.",
        );
        assert_eq!(
            blocks,
            [
                it(0, "•", "one continued"),
                it(0, "•", "two"),
                it(1, "•", "inner"),
                it(2, "1.", "first"),
                it(2, "2.", "second"),
                it(0, "•", "three"),
                it(0, "", "More of three."),
                Block::Paragraph(inline("After.")),
            ]
        );
        assert_eq!(parse("It was\n1984. Then")[0], Block::Paragraph(inline("It was 1984. Then")));
        assert_eq!(parse("* * *"), [Block::Rule]);
        assert_eq!(parse("3) third")[0], it(0, "3.", "third"));
    }

    #[test]
    fn a_fence_is_code_closed_or_not() {
        let blocks = parse("Run:\n```Rust title\nfn main() {\n    let a = \"**x**\";\n}\n```\nDone.");
        assert_eq!(
            blocks,
            [
                Block::Paragraph(inline("Run:")),
                Block::Code { lang: "rust".into(), text: "fn main() {\n    let a = \"**x**\";\n}".into() },
                Block::Paragraph(inline("Done.")),
            ]
        );
        // Still being written.
        assert_eq!(
            parse("```sh\nls -la\n# not a heading"),
            [Block::Code { lang: "sh".into(), text: "ls -la\n# not a heading".into() }]
        );
        assert_eq!(parse("~~~\n```\n~~~"), [Block::Code { lang: String::new(), text: "```".into() }]);
        // Under a list's item, its indent is the fence's, not the code's.
        assert_eq!(
            parse("- so:\n  ```py\n  if x:\n      y()\n  ```")[1],
            Block::Code { lang: "py".into(), text: "if x:\n    y()".into() }
        );
    }

    #[test]
    fn every_beginning_of_an_answer_reads() {
        let all = "# Zusammenfassung — “naïve” ✓\n\nDas **Ergebnis** ist `größer` als *erwartet* (siehe [Doku](https://x.dev/ä)).\n\n1. Erstens: `café`\n   - 日本語 **太字** _斜体_\n2. Zweitens \\* kein Stern\n\n```rust\nlet s = \"héllo\"; // ✓\n```\n\n> Zitat mit <https://x.dev> und https://y.dev/ü.\n\n| a | b |\n|---|---|\n| ä | ö |\n";
        let whole: String = parse(all)
            .iter()
            .map(|b| match b {
                Block::Paragraph(t) | Block::Heading(_, t) | Block::Quote(t) | Block::Item { text: t, .. } => {
                    t.text.clone()
                }
                Block::Code { text, .. } => text.clone(),
                Block::Rule => String::new(),
            })
            .collect();
        assert!(whole.contains("größer") && whole.contains("太字") && whole.contains("héllo"));
        for end in (0..=all.len()).filter(|n| all.is_char_boundary(*n)) {
            for block in parse(&all[..end]) {
                if let Block::Paragraph(t) | Block::Heading(_, t) | Block::Quote(t) | Block::Item { text: t, .. } =
                    block
                {
                    for (r, _) in &t.spans {
                        assert!(t.text.get(r.clone()).is_some(), "a run within its text, at letters' bounds");
                    }
                }
            }
        }
    }
}
