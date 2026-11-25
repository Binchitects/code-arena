//! read_file, write_file, edit_file and list_dir.

use std::path::Path;

use ca_llm::ToolDef;
use serde_json::{Value, json};

use crate::text::{self, Encoding};
use crate::{Context, Kind, Output, Tool, arg_bool, arg_str, arg_u64};

/// Lines read at once when no range is given.
const PAGE: usize = 2000;
/// Past this a file read without a range gives its start and how to read on.
const BIG_LINES: usize = 500;
const BIG_CHARS: usize = 24_000;
/// Past this a file is not read whole into memory.
const HUGE_BYTES: u64 = 16 * 1024 * 1024;
/// A line longer than this is cut, so one minified line does not fill the window.
const LONG_LINE: usize = 2000;

fn path_schema() -> Value {
    json!({"type": "string", "description": "A path relative to the working directory, or a whole path inside the folders the tools may use."})
}

/// Reads a file as text, refusing what is not a regular file (a pipe, a device) or not text.
fn read_text(path: &Path, shown: &str) -> Result<(String, Encoding), Output> {
    let meta = std::fs::metadata(path).map_err(|e| Output::error(format!("{shown}: {}", io_words(&e))))?;
    if meta.is_dir() {
        return Err(Output::error(format!("{shown} is a folder: list_dir lists it")));
    }
    if !meta.is_file() {
        return Err(Output::error(format!(
            "{shown} is not a regular file (a pipe, a socket or a device): it is not read"
        )));
    }
    let bytes = std::fs::read(path).map_err(|e| Output::error(format!("{shown}: {}", io_words(&e))))?;
    text::decode(&bytes)
        .map_err(|_| Output::error(format!("{shown} is not text (binary, or not UTF-8, UTF-16 or UTF-32)")))
}

pub(crate) fn io_words(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "not there".into(),
        std::io::ErrorKind::PermissionDenied => "not readable here (permission denied)".into(),
        _ => e.to_string(),
    }
}

fn numbered(lines: &[&str], first: usize) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.chars().count() > LONG_LINE {
            let cut: String = line.chars().take(LONG_LINE).collect();
            out.push_str(&format!("{:>6}\t{cut}… (line cut at {LONG_LINE} characters)\n", first + i));
        } else {
            out.push_str(&format!("{:>6}\t{line}\n", first + i));
        }
    }
    out
}

pub struct ReadFile;

#[async_trait::async_trait]
impl Tool for ReadFile {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "read_file".into(),
            description: "Reads a text file, its lines numbered, 2,000 lines at a time (offset, limit). A large file read without a range gives its first lines and its size; then read the part you need.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": path_schema(),
                    "offset": {"type": "integer", "description": "The first line to read, from 1."},
                    "limit": {"type": "integer", "description": "How many lines to read (2,000 at most)."}
                },
                "required": ["path"]
            }),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, args: &Value) -> String {
        args.get("path").and_then(Value::as_str).unwrap_or_default().to_string()
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let shown = match arg_str(&args, "path") {
            Ok(p) => p.to_string(),
            Err(o) => return o,
        };
        let real = match ctx.workspace.resolve(&shown) {
            Ok(p) => p,
            Err(e) => return Output::error(e.to_string()),
        };
        if let Ok(m) = std::fs::metadata(&real)
            && m.is_file()
            && m.len() > HUGE_BYTES
        {
            return Output::error(format!(
                "{shown} is {} MB: read it with grep, or a command (head, sed -n)",
                m.len() / (1024 * 1024)
            ));
        }
        let (text, _) = match read_text(&real, &shown) {
            Ok(t) => t,
            Err(o) => return o,
        };
        let lines: Vec<&str> = text.split_inclusive('\n').map(|l| l.strip_suffix('\n').unwrap_or(l)).collect();
        let total = lines.len();
        let ranged = args.get("offset").is_some() || args.get("limit").is_some();
        if !ranged && (total > BIG_LINES || text.chars().count() > BIG_CHARS) {
            let head = numbered(&lines[..total.min(30)], 1);
            return Output::ok(format!(
                "{shown} has {total} lines ({} characters): its first 30 lines follow. Read the part you need with offset and limit, or find it with grep.\n{head}",
                text.chars().count()
            ));
        }
        let offset = arg_u64(&args, "offset").unwrap_or(1).max(1) as usize;
        let limit = (arg_u64(&args, "limit").unwrap_or(PAGE as u64) as usize).clamp(1, PAGE);
        if offset > total.max(1) {
            return Output::error(format!("{shown} has {total} lines: offset {offset} is past its end"));
        }
        let end = (offset - 1 + limit).min(total);
        let mut out = numbered(&lines[offset - 1..end], offset);
        if total == 0 {
            out = format!("{shown} is empty.");
        } else if end < total {
            out.push_str(&format!("… {} more lines: read on with offset {}.", total - end, end + 1));
        }
        Output::ok(out)
    }
}

pub struct WriteFile;

#[async_trait::async_trait]
impl Tool for WriteFile {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "write_file".into(),
            description: "Creates a file, or replaces all of one, with the content given. A file that uses Windows line endings keeps them. Prefer edit_file to change part of a file.".into(),
            parameters: json!({
                "type": "object",
                "properties": {"path": path_schema(), "content": {"type": "string", "description": "The whole new content."}},
                "required": ["path", "content"]
            }),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Edit
    }
    fn subject(&self, args: &Value) -> String {
        args.get("path").and_then(Value::as_str).unwrap_or_default().to_string()
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let (shown, content) = match (arg_str(&args, "path"), arg_str(&args, "content")) {
            (Ok(p), Ok(c)) => (p.to_string(), c.to_string()),
            (Err(o), _) | (_, Err(o)) => return o,
        };
        let real = match ctx.workspace.resolve(&shown) {
            Ok(p) => p,
            Err(e) => return Output::error(e.to_string()),
        };
        let (content, enc, existed) = match read_text(&real, &shown) {
            Ok((old, enc)) => (if text::uses_crlf(&old) { text::to_crlf(&content) } else { content }, enc, true),
            Err(_) if real.exists() && !real.is_file() => {
                return Output::error(format!("{shown} is not a file it can write"));
            }
            Err(_) => (content, Encoding::Utf8, real.exists()),
        };
        if let Some(dir) = real.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            return Output::error(format!("{shown}: its folder could not be made: {e}"));
        }
        if let Err(e) = std::fs::write(&real, text::encode(&content, enc)) {
            return Output::error(format!("{shown}: {}", io_words(&e)));
        }
        let lines = content.lines().count();
        let mut o = Output::ok(format!("{} {shown} ({lines} lines).", if existed { "Replaced" } else { "Created" }));
        o.wrote.push(real);
        o
    }
}

pub struct EditFile;

#[async_trait::async_trait]
impl Tool for EditFile {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "edit_file".into(),
            description: "Replaces an exact piece of a file with new text. old_string must match exactly once, whitespace included (or set replace_all). An empty old_string makes a new file. Read the file first.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": path_schema(),
                    "old_string": {"type": "string", "description": "The exact text to replace."},
                    "new_string": {"type": "string", "description": "The text to put in its place."},
                    "replace_all": {"type": "boolean", "description": "Replace every match, not only one."}
                },
                "required": ["path", "old_string", "new_string"]
            }),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Edit
    }
    fn subject(&self, args: &Value) -> String {
        args.get("path").and_then(Value::as_str).unwrap_or_default().to_string()
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let (shown, old, new) =
            match (arg_str(&args, "path"), arg_str(&args, "old_string"), arg_str(&args, "new_string")) {
                (Ok(p), Ok(o), Ok(n)) => (p.to_string(), o.to_string(), n.to_string()),
                (Err(o), _, _) | (_, Err(o), _) | (_, _, Err(o)) => return o,
            };
        let all = arg_bool(&args, "replace_all");
        let real = match ctx.workspace.resolve(&shown) {
            Ok(p) => p,
            Err(e) => return Output::error(e.to_string()),
        };
        if old.is_empty() {
            if real.exists() && std::fs::metadata(&real).map(|m| m.len() > 0).unwrap_or(true) {
                return Output::error(format!(
                    "{shown} is there already: give old_string to change part of it, or use write_file"
                ));
            }
            return WriteFile.run(json!({"path": shown, "content": new}), ctx).await;
        }
        let (text, enc) = match read_text(&real, &shown) {
            Ok(t) => t,
            Err(o) => return o,
        };
        let crlf = text::uses_crlf(&text);
        let (old, new) = if crlf { (text::to_crlf(&old), text::to_crlf(&new)) } else { (old, new) };
        let count = text.matches(&old).count();
        if count == 0 {
            return Output::error(format!(
                "old_string was not found in {shown}: the file may have changed since it was read, or the whitespace differs. Read it again."
            ));
        }
        if count > 1 && !all {
            return Output::error(format!(
                "old_string is in {shown} {count} times: give more of the lines around it, or set replace_all"
            ));
        }
        let at = text.find(&old).unwrap_or(0);
        let first_line = text[..at].matches('\n').count() + 1;
        let changed = if all { text.replace(&old, &new) } else { text.replacen(&old, &new, 1) };
        if let Err(e) = std::fs::write(&real, text::encode(&changed, enc)) {
            return Output::error(format!("{shown}: {}", io_words(&e)));
        }
        let last_line = first_line + new.matches('\n').count();
        let diff = similar::TextDiff::from_lines(old.as_str(), new.as_str());
        let mut shown_diff = String::new();
        for change in diff.iter_all_changes() {
            let sign = match change.tag() {
                similar::ChangeTag::Delete => '-',
                similar::ChangeTag::Insert => '+',
                similar::ChangeTag::Equal => ' ',
            };
            shown_diff.push(sign);
            shown_diff.push_str(change.value().trim_end_matches(['\n', '\r']));
            shown_diff.push('\n');
        }
        let what = if all && count > 1 { format!("{count} places") } else { format!("lines {first_line}-{last_line}") };
        let mut o = Output::ok(format!("Edited {shown} ({what}).\n{}", text::cut_middle(&shown_diff, 4000)));
        o.wrote.push(real);
        o
    }
}

pub struct ListDir;

#[async_trait::async_trait]
impl Tool for ListDir {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "list_dir".into(),
            description:
                "Lists a folder: its folders (with /) first, then its files with their sizes. .git is left out.".into(),
            parameters: json!({"type": "object", "properties": {"path": path_schema()}}),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, args: &Value) -> String {
        args.get("path").and_then(Value::as_str).unwrap_or(".").to_string()
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let shown = args.get("path").and_then(Value::as_str).unwrap_or(".").to_string();
        let real = match ctx.workspace.resolve(&shown) {
            Ok(p) => p,
            Err(e) => return Output::error(e.to_string()),
        };
        let entries = match std::fs::read_dir(&real) {
            Ok(e) => e,
            Err(e) => return Output::error(format!("{shown}: {}", io_words(&e))),
        };
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                continue;
            }
            match e.file_type() {
                Ok(t) if t.is_dir() => dirs.push(format!("{name}/")),
                Ok(t) if t.is_symlink() => files.push(format!(
                    "{name} -> {}",
                    std::fs::read_link(e.path()).map(|p| p.display().to_string()).unwrap_or_default()
                )),
                _ => files.push(format!("{name} ({} bytes)", e.metadata().map(|m| m.len()).unwrap_or(0))),
            }
        }
        dirs.sort();
        files.sort();
        let total = dirs.len() + files.len();
        let mut all: Vec<String> = dirs.into_iter().chain(files).collect();
        let more = all.len().saturating_sub(1000);
        all.truncate(1000);
        let mut out = if all.is_empty() { format!("{shown} is empty.") } else { all.join("\n") };
        if more > 0 {
            out.push_str(&format!("\n… and {more} more ({total} in all): use glob to narrow it."));
        }
        Output::ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::context;

    #[tokio::test]
    async fn read_numbers_lines_and_pages() {
        let (d, ctx) = context();
        let text: String = (1..=10).map(|i| format!("line {i}\n")).collect();
        std::fs::write(d.path().join("a.txt"), text).expect("write");
        let o = ReadFile.run(json!({"path": "a.txt", "offset": 3, "limit": 2}), &ctx).await;
        assert!(!o.is_error);
        assert_eq!(o.text, "     3\tline 3\n     4\tline 4\n… 6 more lines: read on with offset 5.");
    }

    #[tokio::test]
    async fn a_big_file_gives_its_start() {
        let (d, ctx) = context();
        let text: String = (1..=900).map(|i| format!("l{i}\n")).collect();
        std::fs::write(d.path().join("big.txt"), text).expect("write");
        let o = ReadFile.run(json!({"path": "big.txt"}), &ctx).await;
        assert!(o.text.starts_with("big.txt has 900 lines"));
        assert!(o.text.contains("    30\tl30\n"));
        assert!(!o.text.contains("l31\n"));
    }

    #[tokio::test]
    async fn outside_and_binary_are_refused() {
        let (d, ctx) = context();
        std::fs::write(d.path().join("bin"), b"\x00\x01\x02").expect("write");
        assert!(ReadFile.run(json!({"path": "bin"}), &ctx).await.text.contains("not text"));
        assert!(ReadFile.run(json!({"path": "../../etc/passwd"}), &ctx).await.is_error);
    }

    #[tokio::test]
    async fn edit_keeps_crlf_and_utf16() {
        let (d, ctx) = context();
        let p = d.path().join("w.txt");
        std::fs::write(&p, text::encode("one\r\ntwo\r\nthree\r\n", Encoding::Utf16Le)).expect("write");
        let o = EditFile.run(json!({"path": "w.txt", "old_string": "two\n", "new_string": "2\n2b\n"}), &ctx).await;
        assert!(!o.is_error, "{}", o.text);
        assert!(o.text.starts_with("Edited w.txt (lines 2-4)."), "{}", o.text);
        let (back, enc) = text::decode(&std::fs::read(&p).expect("read")).expect("text");
        assert_eq!(enc, Encoding::Utf16Le);
        assert_eq!(back, "one\r\n2\r\n2b\r\nthree\r\n");
        assert_eq!(o.wrote.len(), 1);
    }

    #[tokio::test]
    async fn edit_must_match_once() {
        let (d, ctx) = context();
        std::fs::write(d.path().join("x.rs"), "a\na\n").expect("write");
        assert!(
            EditFile
                .run(json!({"path": "x.rs", "old_string": "a", "new_string": "b"}), &ctx)
                .await
                .text
                .contains("2 times")
        );
        assert!(
            EditFile
                .run(json!({"path": "x.rs", "old_string": "zzz", "new_string": "b"}), &ctx)
                .await
                .text
                .contains("not found")
        );
        let o = EditFile
            .run(json!({"path": "x.rs", "old_string": "a", "new_string": "b", "replace_all": true}), &ctx)
            .await;
        assert!(o.text.contains("2 places"));
        assert_eq!(std::fs::read_to_string(d.path().join("x.rs")).expect("read"), "b\nb\n");
    }

    #[tokio::test]
    async fn write_creates_folders_and_keeps_crlf() {
        let (d, ctx) = context();
        let o = WriteFile.run(json!({"path": "new/dir/f.txt", "content": "a\nb\n"}), &ctx).await;
        assert_eq!(o.text, "Created new/dir/f.txt (2 lines).");
        std::fs::write(d.path().join("w.txt"), "x\r\ny\r\n").expect("write");
        WriteFile.run(json!({"path": "w.txt", "content": "p\nq\n"}), &ctx).await;
        assert_eq!(std::fs::read_to_string(d.path().join("w.txt")).expect("read"), "p\r\nq\r\n");
    }

    #[tokio::test]
    async fn list_dir_folders_first() {
        let (d, ctx) = context();
        std::fs::create_dir(d.path().join("src")).expect("mkdir");
        std::fs::create_dir(d.path().join(".git")).expect("mkdir");
        std::fs::write(d.path().join("Cargo.toml"), "x").expect("write");
        assert_eq!(ListDir.run(json!({}), &ctx).await.text, "src/\nCargo.toml (1 bytes)");
    }
}
