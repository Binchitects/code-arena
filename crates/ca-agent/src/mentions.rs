//! `@files` in a message: `@src/parser.rs` sends the file with it, its lines numbered as read_file numbers them;
//! `@src/parser.rs:40-60` (or `#L40-L60`) those lines with three around them; a folder its list; `@"a b.txt"` a
//! name with spaces. Anything that is not a file the tools may read (`@Override`, an email address) stays as typed.

use ca_workspace::Workspace;

/// A file sends this much at most; a message all its files this much. Past it, the middle is cut.
const FILE_CHARS: usize = 60_000;
const MESSAGE_CHARS: usize = 150_000;
/// A file this big sends its first lines and how to read the rest.
const BIG_LINES: usize = 500;

/// A mention found in a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    pub path: String,
    pub lines: Option<(usize, usize)>,
}

/// The mentions in `text`, in order, each once.
pub fn find(text: &str) -> Vec<Mention> {
    let mut out: Vec<Mention> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // `@` begins a mention only at the start or after a space: `me@example.com` is an address.
        if chars[i] != '@' || (i > 0 && !chars[i - 1].is_whitespace() && !matches!(chars[i - 1], '(' | '[')) {
            i += 1;
            continue;
        }
        let (raw, next) = if chars.get(i + 1) == Some(&'"') {
            let end = chars[i + 2..].iter().position(|c| *c == '"').map(|p| i + 2 + p);
            match end {
                Some(e) => (chars[i + 2..e].iter().collect::<String>(), e + 1),
                None => {
                    i += 1;
                    continue;
                }
            }
        } else {
            let end = chars[i + 1..]
                .iter()
                .position(|c| c.is_whitespace() || matches!(c, ',' | ')' | ']' | '`' | '"' | '\''))
                .map(|p| i + 1 + p)
                .unwrap_or(chars.len());
            (chars[i + 1..end].iter().collect::<String>(), end)
        };
        i = next;
        let raw = raw.trim_end_matches(['.', ';', '?', '!']).to_string();
        if raw.is_empty() {
            continue;
        }
        let m = parse(&raw);
        if !out.contains(&m) {
            out.push(m);
        }
    }
    out
}

fn parse(raw: &str) -> Mention {
    // path#L40-L60
    if let Some((p, r)) = raw.split_once("#L") {
        let mut it = r.split("-L").chain(r.split('-')).filter_map(|x| x.trim_start_matches('L').parse::<usize>().ok());
        if let Some(a) = it.next() {
            let b = it.next().unwrap_or(a);
            return Mention { path: p.to_string(), lines: Some((a.min(b), a.max(b))) };
        }
    }
    // path:40-60 or path:40
    if let Some((p, r)) = raw.rsplit_once(':')
        && !p.is_empty()
        && !r.is_empty()
        && r.chars().all(|c| c.is_ascii_digit() || c == '-')
    {
        let mut it = r.split('-').filter_map(|x| x.parse::<usize>().ok());
        if let Some(a) = it.next() {
            let b = it.next().unwrap_or(a);
            return Mention { path: p.to_string(), lines: Some((a.min(b), a.max(b))) };
        }
    }
    Mention { path: raw.to_string(), lines: None }
}

/// What goes with the message: each mentioned file or folder the tools may read, after the words typed.
pub fn attach(text: &str, ws: &Workspace) -> String {
    let mut out = String::new();
    let mut budget = MESSAGE_CHARS;
    for m in find(text) {
        let Ok(real) = ws.resolve(&m.path) else { continue };
        let shown = ws.display(&real);
        let piece = if real.is_dir() {
            let Ok(rd) = std::fs::read_dir(&real) else { continue };
            let mut names: Vec<String> = rd
                .flatten()
                .map(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if e.path().is_dir() { format!("{n}/") } else { n }
                })
                .filter(|n| n != ".git/")
                .collect();
            names.sort();
            format!("<folder path=\"{shown}\">\n{}\n</folder>", names.join("\n"))
        } else if real.is_file() {
            let Ok(bytes) = std::fs::read(&real) else { continue };
            let Ok((body, _)) = ca_tools::text::decode(&bytes) else { continue };
            let lines: Vec<&str> = body.lines().collect();
            let (from, to, note) = match m.lines {
                Some((a, b)) => (a.saturating_sub(3).max(1), (b + 3).min(lines.len()), String::new()),
                None if lines.len() > BIG_LINES => (
                    1,
                    60,
                    format!("\n(The first 60 of its {} lines: read_file reads the part you need.)", lines.len()),
                ),
                None => (1, lines.len(), String::new()),
            };
            let numbered: String = lines
                .iter()
                .enumerate()
                .skip(from.saturating_sub(1))
                .take(to.saturating_sub(from.saturating_sub(1)))
                .map(|(i, l)| format!("{:>6}\t{l}\n", i + 1))
                .collect();
            let numbered = ca_tools::text::cut_middle(&numbered, FILE_CHARS);
            format!("<file path=\"{shown}\">\n{numbered}</file>{note}")
        } else {
            continue;
        };
        if piece.len() > budget {
            out.push_str(&format!(
                "\n\n(@{shown} was not sent: the message's files are past {MESSAGE_CHARS} characters.)"
            ));
            continue;
        }
        budget -= piece.len();
        out.push_str("\n\n");
        out.push_str(&piece);
    }
    if out.is_empty() { text.to_string() } else { format!("{text}{out}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_and_what_is_not_one() {
        let ms = find(
            "look at @src/parser.rs:40-60 and @\"my notes.md\", not me@example.com or @Override. Also @src/parser.rs#L3-L5.",
        );
        assert_eq!(ms[0], Mention { path: "src/parser.rs".into(), lines: Some((40, 60)) });
        assert_eq!(ms[1], Mention { path: "my notes.md".into(), lines: None });
        assert_eq!(ms[2], Mention { path: "Override".into(), lines: None });
        assert_eq!(ms[3], Mention { path: "src/parser.rs".into(), lines: Some((3, 5)) });
        assert_eq!(ms.len(), 4);
    }

    #[test]
    fn files_and_folders_go_with_the_message() {
        let d = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(d.path().join("src")).expect("mkdir");
        std::fs::write(d.path().join("src/a.rs"), (1..=20).map(|i| format!("l{i}\n")).collect::<String>())
            .expect("write");
        let ws = Workspace::open(d.path(), &[]).expect("ws");
        let t = attach("why @src/a.rs:10-11 and @src and @Override?", &ws);
        assert!(t.starts_with("why @src/a.rs:10-11 and @src and @Override?\n\n<file path=\"src/a.rs\">\n     7\tl7\n"));
        assert!(t.contains("    14\tl14\n</file>"));
        assert!(!t.contains("l15"));
        assert!(t.contains("<folder path=\"src\">\na.rs\n</folder>"));
        assert_eq!(attach("nothing here", &ws), "nothing here");
        assert_eq!(attach("not @../../etc/passwd", &ws), "not @../../etc/passwd");
    }
}
