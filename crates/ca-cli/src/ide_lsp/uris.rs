//! The window's file URIs and the server's. The window names a file of the folder `file:///` and its path in the
//! folder; the server names it `file://` and its full path. Every string in a message that is a whole file URI is
//! rewritten, and so is every key that is one (a WorkspaceEdit's `changes`), but not text: a document's (`text`),
//! an edit's (`newText`, `insertText`) or a hover's (`value`). A file of the server's outside the folder (a
//! library's, the standard library's) is `outside://` and its full path for the window, and back. The folder is
//! matched as named and where it leads (its links resolved).

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

/// The scheme the window sees a file outside the folder under.
pub const OUTSIDE: &str = "outside://";

const PAGE_FILE: &str = "file:///";
const FILE: &str = "file://";

/// The keys whose strings are text, never rewritten.
const TEXT: &[&str] = &["text", "newText", "insertText", "value"];

/// The folder, as named and as it really is.
pub struct Uris {
    root: PathBuf,
    roots: Vec<PathBuf>,
}

impl Uris {
    pub fn new(root: &Path) -> Uris {
        let mut roots = vec![root.to_path_buf()];
        if let Ok(real) = std::fs::canonicalize(root)
            && real != root
        {
            roots.push(real);
        }
        Uris { root: root.to_path_buf(), roots }
    }

    /// The window's URIs in a message, as the server's (in place).
    pub fn to_server(&self, v: &mut Value) {
        walk(v, &|s| self.server_uri(s), 0);
    }

    /// The server's URIs in a message, as the window's (in place).
    pub fn to_window(&self, v: &mut Value) {
        walk(v, &|s| self.window_uri(s), 0);
    }

    /// A window's URI as the server's: a file of the folder by its full path, one outside as it was; anything else
    /// unchanged.
    pub fn server_uri(&self, uri: &str) -> String {
        if let Some(rest) = uri.strip_prefix(OUTSIDE) {
            return format!("{FILE}{rest}");
        }
        let Some(after) = uri.strip_prefix(PAGE_FILE) else { return uri.to_string() };
        let (path, rest) = split(after);
        let relative = unescape(path);
        // A path of the folder's only: none that leads out of it (.., a full path, a drive, a backslash).
        if relative.contains('\0')
            || relative.contains('\\')
            || relative.starts_with('/')
            || Path::new(&relative).is_absolute()
            || (cfg!(windows) && relative.contains(':'))
            || relative.split('/').any(|p| p == "..")
        {
            return uri.to_string();
        }
        let full = if relative.is_empty() { self.root.clone() } else { self.root.join(&relative) };
        format!("{}{rest}", file_uri(&full))
    }

    /// A server's URI as the window's: a file of the folder by its path in it, one outside under outside://;
    /// anything else unchanged.
    pub fn window_uri(&self, uri: &str) -> String {
        let Some(after) = uri.strip_prefix(FILE) else { return uri.to_string() };
        if !after.starts_with('/') {
            // An authority (a share, file://server/…): not the folder's.
            return format!("{OUTSIDE}{after}");
        }
        let (encoded, rest) = split(after);
        let mut path = unescape(encoded);
        // file:///C:/x on Windows; file:///c%3A/x as VS Code writes it.
        let b = path.as_bytes();
        if cfg!(windows) && b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
            path.remove(0);
        }
        if path.contains('\0') {
            return format!("{OUTSIDE}{after}");
        }
        let full = normal(Path::new(&path));
        for root in &self.roots {
            if let Ok(rel) = full.strip_prefix(root) {
                let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                return format!("{PAGE_FILE}{}{rest}", encode(&rel.join("/")));
            }
        }
        format!("{OUTSIDE}{after}")
    }
}

/// The server's URI of a full path: file:// and the path, each byte but letters, digits, - . _ ~ / (and a drive's
/// :) percent-encoded.
pub fn file_uri(full: &Path) -> String {
    let mut path = full.display().to_string().replace('\\', "/");
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    format!("{FILE}{}", encode(&path))
}

/// A path with `.` and `..` taken as they read.
fn normal(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

fn encode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut s = String::with_capacity(path.len() + 16);
    for (i, &b) in bytes.iter().enumerate() {
        let drive = b == b':' && i == 2 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic();
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') || drive {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// Percent-escapes as the bytes they stand for (a broken one is kept as written).
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A URI's path and what follows it (a query, a fragment).
fn split(s: &str) -> (&str, &str) {
    match s.find(['?', '#']) {
        Some(at) => (&s[..at], &s[at..]),
        None => (s, ""),
    }
}

fn is_uri(s: &str) -> bool {
    (s.starts_with(FILE) || s.starts_with(OUTSIDE)) && !s.contains([' ', '\t', '\r', '\n'])
}

fn walk(v: &mut Value, map: &dyn Fn(&str) -> String, depth: usize) {
    if depth > 64 {
        return;
    }
    match v {
        Value::Object(o) => {
            let keys: Vec<String> = o.keys().filter(|k| is_uri(k)).cloned().collect();
            for (k, inner) in o.iter_mut() {
                match inner {
                    Value::String(s) if !TEXT.contains(&k.as_str()) && is_uri(s) => *s = map(s),
                    Value::String(_) => {}
                    inner => walk(inner, map, depth + 1),
                }
            }
            for k in keys {
                let to = map(&k);
                if to != k
                    && let Some(moved) = o.remove(&k)
                {
                    o.insert(to, moved);
                }
            }
        }
        Value::Array(a) => {
            for inner in a {
                match inner {
                    Value::String(s) if is_uri(s) => *s = map(s),
                    Value::String(_) => {}
                    inner => walk(inner, map, depth + 1),
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_windows_paths_of_the_folder_are_the_servers_full_ones_and_back() {
        let u = Uris::new(Path::new("/w/my project"));
        assert_eq!(u.server_uri("file:///src/a%20b.rs"), "file:///w/my%20project/src/a%20b.rs");
        assert_eq!(u.server_uri("file:///"), "file:///w/my%20project");
        assert_eq!(u.server_uri("file:///src/x.rs#L3"), "file:///w/my%20project/src/x.rs#L3");
        // Nothing that leads out of the folder.
        for out in ["file:///../etc/passwd", "file:///src/%2E%2E/%2E%2E/x", "file:////etc/passwd", "file:///a\\b"] {
            assert_eq!(u.server_uri(out), out);
        }
        assert_eq!(u.server_uri("outside:///usr/lib/x.h"), "file:///usr/lib/x.h");
        assert_eq!(u.server_uri("untitled:1"), "untitled:1");
        assert_eq!(u.window_uri("file:///w/my%20project/src/a%20b.rs"), "file:///src/a%20b.rs");
        assert_eq!(u.window_uri("file:///w/my%20project/src/../lib.rs"), "file:///lib.rs");
        assert_eq!(u.window_uri("file:///w/my%20projectx/a.rs"), "outside:///w/my%20projectx/a.rs");
        assert_eq!(u.window_uri("file:///usr/include/stdio.h"), "outside:///usr/include/stdio.h");
        assert_eq!(u.window_uri("file://server/share/a"), "outside://server/share/a");
        assert_eq!(u.window_uri("https://x.y"), "https://x.y");
    }

    #[cfg(unix)]
    #[test]
    fn a_message_has_its_uris_and_uri_keys_rewritten_and_its_text_kept() {
        let u = Uris::new(Path::new("/w"));
        let mut m = json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": "file:///a.rs", "text": "file:///a.rs"},
            "edit": {"changes": {"file:///b.rs": [{"newText": "file:///c.rs"}]}},
            "list": ["file:///d.rs", "file:///has space"],
        }});
        u.to_server(&mut m);
        let p = &m["params"];
        assert_eq!(p["textDocument"], json!({"uri": "file:///w/a.rs", "text": "file:///a.rs"}));
        assert_eq!(p["edit"]["changes"], json!({"file:///w/b.rs": [{"newText": "file:///c.rs"}]}));
        assert_eq!(p["list"], json!(["file:///w/d.rs", "file:///has space"]));
        u.to_window(&mut m);
        assert_eq!(m["params"]["edit"]["changes"], json!({"file:///b.rs": [{"newText": "file:///c.rs"}]}));
        assert_eq!(m["params"]["textDocument"]["uri"], "file:///a.rs");
    }

    #[test]
    fn a_broken_escape_is_kept_and_a_deep_message_ends() {
        assert_eq!(unescape("a%2"), "a%2");
        assert_eq!(unescape("%C3%A9%zz"), "é%zz");
        let mut deep = json!("file:///x");
        for _ in 0..100 {
            deep = json!([deep]);
        }
        Uris::new(Path::new("/w")).to_server(&mut deep);
    }
}
