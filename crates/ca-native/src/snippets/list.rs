//! The snippets files read: VS Code's format (JSON with comments, each snippet by its name with its `prefix`, its
//! `body`, its `description`, and in a `.code-snippets` file its `scope`; `include` and `exclude` globs), and which
//! snippets are for a file.

use globset::{Glob, GlobMatcher};
use serde_json::Value;

/// Where a snippet comes from, as its suggestion says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    User,
    Workspace,
}

impl Source {
    pub fn of(source: &str) -> Source {
        if source == "workspace" { Source::Workspace } else { Source::User }
    }

    /// VS Code's words for it: the suggestion's and the pickers' group.
    pub fn label(self) -> &'static str {
        match self {
            Source::User => "User Snippet",
            Source::Workspace => "Workspace Snippet",
        }
    }
}

/// One snippet.
#[derive(Clone, Debug)]
pub struct Snippet {
    pub name: String,
    pub prefixes: Vec<String>,
    pub body: String,
    pub description: String,
    /// The languages it is for; empty: every language.
    pub scope: Vec<String>,
    pub include: Vec<GlobMatcher>,
    pub exclude: Vec<GlobMatcher>,
    pub source: Source,
}

/// A string or strings, as a list.
fn strings(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

/// A glob of `include` or `exclude`: one that names no folder is matched against the file's name.
fn glob(g: &str) -> Option<GlobMatcher> {
    let g = g.strip_prefix("./").unwrap_or(g);
    let g = if g.contains('/') || g.starts_with("**") { g.to_string() } else { format!("**/{g}") };
    Glob::new(&g).ok().map(|g| g.compile_matcher())
}

/// A snippets file's text read: `language` is a person's `NAME.json`'s (its snippets are for it alone). What is not
/// a snippet (no prefix, no body) is left out; text that is not JSON is the error.
pub fn read(text: &str, language: Option<&str>, source: Source) -> Result<Vec<Snippet>, String> {
    let value: Value = serde_json::from_str(&ca_settings::jsonc::strip(text)).map_err(|e| e.to_string())?;
    let Value::Object(all) = value else { return Err("a snippets file is an object of snippets".into()) };
    let mut out = Vec::new();
    for (name, s) in all {
        let Value::Object(s) = s else { continue };
        let prefixes: Vec<String> =
            strings(s.get("prefix").unwrap_or(&Value::Null)).into_iter().filter(|p| !p.is_empty()).collect();
        let body = match &s.get("body") {
            Some(Value::String(b)) => b.clone(),
            Some(Value::Array(lines)) => lines.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"),
            _ => continue,
        };
        if prefixes.is_empty() {
            continue;
        }
        let description = strings(s.get("description").unwrap_or(&Value::Null)).join("\n");
        let mut scope: Vec<String> = s
            .get("scope")
            .and_then(Value::as_str)
            .map(|v| v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
            .unwrap_or_default();
        if let Some(l) = language
            && scope.is_empty()
            && l != "global"
        {
            scope.push(l.to_string());
        }
        let globs = |key: &str| strings(s.get(key).unwrap_or(&Value::Null)).iter().filter_map(|g| glob(g)).collect();
        out.push(Snippet {
            name,
            prefixes,
            body,
            description,
            scope,
            include: globs("include"),
            exclude: globs("exclude"),
            source,
        });
    }
    Ok(out)
}

/// Whether a snippet is for a file of a language, at its path in the project.
pub fn applies(s: &Snippet, language: &str, path: &str) -> bool {
    if !s.scope.is_empty() && !s.scope.iter().any(|l| l == language) {
        return false;
    }
    if !s.include.is_empty() && !s.include.iter().any(|g| g.is_match(path)) {
        return false;
    }
    !s.exclude.iter().any(|g| g.is_match(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"{
        // A comment, as VS Code's example.
        "For loop": {
            "scope": "javascript, typescript",
            "prefix": ["for", "fori"],
            "body": ["for (let ${1:i} = 0; $1 < $2; $1++) {", "\t$0", "}"],
            "description": "A for loop",
        },
        "Tests only": {"prefix": "test", "body": "it('$1')", "include": "*.test.ts", "exclude": ["skip/**"]},
        "No body": {"prefix": "x"},
        "No prefix": {"body": "y"},
    }"#;

    #[test]
    fn a_file_is_read_with_comments_lists_and_scopes() {
        let all = read(FILE, None, Source::Workspace).expect("read");
        assert_eq!(all.len(), 2);
        let f = all.iter().find(|s| s.name == "For loop").expect("for");
        assert_eq!(f.prefixes, ["for", "fori"]);
        assert_eq!(f.body, "for (let ${1:i} = 0; $1 < $2; $1++) {\n\t$0\n}");
        assert_eq!(f.scope, ["javascript", "typescript"]);
        assert_eq!(f.description, "A for loop");
        assert!(applies(f, "typescript", "src/a.ts"));
        assert!(!applies(f, "rust", "src/a.rs"));
        let t = all.iter().find(|s| s.name == "Tests only").expect("test");
        assert!(applies(t, "typescript", "src/a.test.ts"));
        assert!(!applies(t, "typescript", "src/a.ts"));
        assert!(!applies(t, "typescript", "skip/a.test.ts"));
        assert!(read("{ nope", None, Source::User).is_err());
    }

    #[test]
    fn a_language_file_is_for_its_language_alone() {
        let all =
            read(r#"{"main": {"prefix": "main", "body": "fn main() {}"}}"#, Some("rust"), Source::User).expect("read");
        assert_eq!(all[0].scope, ["rust"]);
        assert!(!applies(&all[0], "python", "a.py"));
        assert_eq!(Source::of("user").label(), "User Snippet");
        assert_eq!(Source::of("workspace").label(), "Workspace Snippet");
    }
}
