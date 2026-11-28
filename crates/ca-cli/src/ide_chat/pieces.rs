//! Lines of files sent with a message (the chips the IDE's "Add to chat" puts over its box): `send{text,
//! context: [{path, start, end, text?}]}`.
//!
//! - The model reads each piece after the words: its lines with three around them, numbered as read_file numbers
//!   them, read from the file as the tools may read it (`ca_agent::mentions` does the reading, with its budget).
//!   A piece that carries its `text` (the file has changes not saved) is sent as that text, without lines around
//!   it, since the file on disk is not what the person sees.
//! - What the question shows stays the words typed. The pieces go in the `user` (or `queued`) message's `files`,
//!   so the faces draw them under the question ("Sent with it") and the session keeps them in the question's
//!   details.
//!
//! Only a message the face sent with `context` is shaped: a question asked again already has its pieces in what
//! the model read, and its `files` in its details.

use serde_json::{Value, json};

use crate::ide_ext::{Sent, Shared};

/// The most pieces a message is sent with (the box keeps as many).
const MOST: usize = 8;
/// The most characters of an unsaved piece's text.
const MOST_TEXT: usize = 60_000;

/// A message's pieces read for the model, and named in its `files`.
pub fn sent(sh: &Shared, sent: &mut Sent) {
    let (read, files) = expand(&sent.msg["context"], &sh.workspace);
    if files.is_empty() {
        return;
    }
    sent.model = format!("{}{read}", sent.model);
    sent.fields.insert("files".into(), Value::Array(files));
}

/// A piece as the face sent it: its file, its first and last line (from 1), and its text when the face sent it.
fn piece(v: &Value) -> Option<(String, usize, usize, Option<String>)> {
    let path = v["path"].as_str().map(str::trim).filter(|p| !p.is_empty())?.to_string();
    let start = v["start"].as_u64().filter(|s| *s > 0)? as usize;
    let end = (v["end"].as_u64().unwrap_or(0) as usize).max(start);
    let text = v["text"].as_str().filter(|t| !t.is_empty()).map(str::to_string);
    Some((path, start, end, text))
}

/// What the model reads after the message's words (each piece after a blank line), and the pieces sent, as the
/// `files` of the question. A piece of a file the tools may not read is left out.
pub fn expand(context: &Value, ws: &ca_workspace::Workspace) -> (String, Vec<Value>) {
    let mut read = String::new();
    let mut files = Vec::new();
    for (path, start, end, text) in context.as_array().into_iter().flatten().filter_map(piece).take(MOST) {
        let Ok(real) = ws.resolve(&path) else { continue };
        if !real.is_file() {
            continue;
        }
        let shown = ws.display(&real);
        let block = match &text {
            Some(text) => {
                let numbered: String = text.lines().enumerate().map(|(i, l)| format!("{:>6}\t{l}\n", start + i)).collect();
                let numbered = ca_tools::text::cut_middle(&numbered, MOST_TEXT);
                format!("\n\n<file path=\"{shown}\" unsaved=\"true\">\n{numbered}</file>")
            }
            None => {
                // The words before the expansion are the mention itself: only what is added after them is kept.
                let mention = if shown.contains(' ') { format!("@\"{shown}\"") } else { format!("@{shown}") };
                let mention = format!("{mention}:{start}-{end}");
                let whole = ca_agent::mentions::attach(&mention, ws);
                let added = whole.strip_prefix(&mention).unwrap_or("").to_string();
                if added.trim().is_empty() {
                    continue;
                }
                added
            }
        };
        read.push_str(&block);
        let mut file = json!({"path": shown, "start": start, "end": end});
        if let Some(text) = text {
            file["text"] = json!(text);
        }
        files.push(file);
    }
    (read, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_are_read_with_three_lines_around_and_named_for_the_question() {
        let d = tempfile::tempdir().expect("dir");
        let body: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        std::fs::write(d.path().join("a.rs"), body).expect("write");
        let ws = ca_workspace::Workspace::open(d.path(), &[]).expect("ws");
        let context = json!([
            {"path": "a.rs", "start": 10, "end": 12},
            {"path": "a.rs", "start": 2, "end": 2, "text": "fn changed() {}"},
            {"path": "gone.rs", "start": 1, "end": 1},
            {"path": "", "start": 1, "end": 1},
        ]);
        let (read, files) = expand(&context, &ws);
        assert!(read.starts_with("\n\n<file path=\"a.rs\">"), "{read}");
        assert!(read.contains("     7\tline 7\n") && read.contains("    15\tline 15\n"));
        assert!(!read.contains("\tline 6\n") && !read.contains("\tline 16\n"));
        assert!(read.contains("<file path=\"a.rs\" unsaved=\"true\">\n     2\tfn changed() {}\n</file>"));
        assert_eq!(
            files,
            [
                json!({"path": "a.rs", "start": 10, "end": 12}),
                json!({"path": "a.rs", "start": 2, "end": 2, "text": "fn changed() {}"}),
            ]
        );
        // None sent, nothing read; a file outside the folder is not one the tools may read.
        assert_eq!(expand(&Value::Null, &ws), (String::new(), vec![]));
        let (read, files) = expand(&json!([{"path": "/etc/passwd", "start": 1, "end": 1}]), &ws);
        assert!(read.is_empty() && files.is_empty());
    }
}
