//! glob and grep, over the files git would see (`.gitignore` holds), never through a link that leads outside.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ca_llm::ToolDef;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, SearcherBuilder, Sink, SinkContext, SinkContextKind, SinkMatch};
use serde_json::{Value, json};

use crate::{Context, Kind, Output, Tool, arg_bool, arg_str, arg_u64};

const MOST_FILES: usize = 2000;
const MOST_LINES: usize = 500;
/// A file this big in a folder is skipped (and counted); named as `path`, it is searched.
const BIG_FILE: u64 = 5 * 1024 * 1024;
const TIME: Duration = Duration::from_secs(30);

/// The files under `dir` that git would see, in path order. Links that lead outside are left out.
fn walk(dir: &Path, ctx: &Context) -> impl Iterator<Item = PathBuf> {
    let ws = ctx.workspace.clone();
    ignore::WalkBuilder::new(dir)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_name(std::cmp::Ord::cmp)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file() || t.is_symlink()))
        .filter_map(move |e| {
            let p = e.into_path();
            // A link is followed only when where it leads is inside.
            ws.resolve(&p.to_string_lossy()).ok().filter(|real| real.is_file()).map(|_| p)
        })
}

pub struct Glob;

#[async_trait::async_trait]
impl Tool for Glob {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "glob".into(),
            description: "Finds files by name: **/*.rs, src/**/*.{ts,tsx}. Follows .gitignore. Gives paths relative to the working directory.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "A glob: * within a name, ** across folders, {a,b} for either."},
                    "path": {"type": "string", "description": "The folder to look in (the working directory when not given)."}
                },
                "required": ["pattern"]
            }),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, args: &Value) -> String {
        args.get("pattern").and_then(Value::as_str).unwrap_or_default().to_string()
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let pattern = match arg_str(&args, "pattern") {
            Ok(p) => p.to_string(),
            Err(o) => return o,
        };
        let base = args.get("path").and_then(Value::as_str).unwrap_or(".").to_string();
        let dir = match ctx.workspace.resolve(&base) {
            Ok(d) => d,
            Err(e) => return Output::error(e.to_string()),
        };
        let glob = match globset::GlobBuilder::new(&pattern).literal_separator(true).build() {
            Ok(g) => g.compile_matcher(),
            Err(e) => return Output::error(format!("{pattern} is not a glob: {e}")),
        };
        // A pattern with no folder in it matches a name anywhere, as people mean `*.rs`.
        let anywhere = !pattern.contains('/');
        let ctx2 = ctx.clone();
        let found = tokio::task::spawn_blocking(move || {
            let mut found = Vec::new();
            let mut more = 0usize;
            for p in walk(&dir, &ctx2) {
                let rel = p.strip_prefix(&dir).unwrap_or(&p);
                let hit = glob.is_match(rel) || (anywhere && rel.file_name().is_some_and(|n| glob.is_match(n)));
                if hit {
                    if found.len() < MOST_FILES {
                        found.push(ctx2.workspace.display(&p));
                    } else {
                        more += 1;
                    }
                }
            }
            (found, more)
        })
        .await;
        let Ok((found, more)) = found else { return Output::error("the search stopped") };
        if found.is_empty() {
            return Output::ok(format!("No file matches {pattern}."));
        }
        let mut out = found.join("\n");
        if more > 0 {
            out.push_str(&format!("\n… and {more} more: narrow the pattern."));
        }
        Output::ok(out)
    }
}

pub struct Grep;

#[async_trait::async_trait]
impl Tool for Grep {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "grep".into(),
            description: "Searches file contents by regular expression (Rust's syntax, no backtracking), with context lines. Follows .gitignore; binary files are skipped. Gives path:line: text.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "A regular expression."},
                    "path": {"type": "string", "description": "A folder or a file (the working directory when not given)."},
                    "glob": {"type": "string", "description": "Only files whose name or path matches it: *.rs, src/**/*.ts."},
                    "ignore_case": {"type": "boolean"},
                    "context": {"type": "integer", "description": "Lines to show before and after each match (0 to 10)."}
                },
                "required": ["pattern"]
            }),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, args: &Value) -> String {
        args.get("pattern").and_then(Value::as_str).unwrap_or_default().to_string()
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let pattern = match arg_str(&args, "pattern") {
            Ok(p) => p.to_string(),
            Err(o) => return o,
        };
        let base = args.get("path").and_then(Value::as_str).unwrap_or(".").to_string();
        let target = match ctx.workspace.resolve(&base) {
            Ok(d) => d,
            Err(e) => return Output::error(e.to_string()),
        };
        let matcher = match RegexMatcherBuilder::new().case_insensitive(arg_bool(&args, "ignore_case")).build(&pattern)
        {
            Ok(m) => m,
            Err(e) => return Output::error(format!("{pattern} is not a regular expression this reads: {e}")),
        };
        let filter = match args.get("glob").and_then(Value::as_str) {
            Some(g) => match globset::Glob::new(g) {
                Ok(g) => Some(g.compile_matcher()),
                Err(e) => return Output::error(format!("{g} is not a glob: {e}")),
            },
            None => None,
        };
        let context = arg_u64(&args, "context").unwrap_or(0).min(10) as usize;
        let ctx2 = ctx.clone();
        let result = tokio::task::spawn_blocking(move || {
            let started = Instant::now();
            let files: Vec<PathBuf> =
                if target.is_file() { vec![target.clone()] } else { walk(&target, &ctx2).collect() };
            let mut searcher = SearcherBuilder::new()
                .binary_detection(BinaryDetection::quit(0))
                .line_number(true)
                .before_context(context)
                .after_context(context)
                .build();
            let mut lines: Vec<String> = Vec::new();
            let mut matches = 0usize;
            let mut skipped_big = 0usize;
            let mut left = 0usize;
            for (i, f) in files.iter().enumerate() {
                if started.elapsed() > TIME {
                    left = files.len() - i;
                    break;
                }
                if !target.is_file() && std::fs::metadata(f).map(|m| m.len() > BIG_FILE).unwrap_or(true) {
                    skipped_big += 1;
                    continue;
                }
                if let Some(g) = &filter {
                    let rel = f.strip_prefix(&target).unwrap_or(f);
                    if !g.is_match(rel) && !rel.file_name().is_some_and(|n| g.is_match(n)) {
                        continue;
                    }
                }
                let shown = ctx2.workspace.display(f);
                let mut sink = Collect { shown: &shown, lines: &mut lines, matches: &mut matches, last: None };
                let _ = searcher.search_path(&matcher, f, &mut sink);
                if lines.len() >= MOST_LINES {
                    break;
                }
            }
            (lines, matches, skipped_big, left)
        })
        .await;
        let Ok((mut lines, _matches, skipped_big, left)) = result else { return Output::error("the search stopped") };
        if lines.is_empty() {
            return Output::ok(format!("No match for {pattern}."));
        }
        let cut = lines.len() > MOST_LINES;
        lines.truncate(MOST_LINES);
        let mut out = lines.join("\n");
        if cut {
            out.push_str(&format!("\n… more matches past {MOST_LINES} lines: narrow the pattern or the path."));
        }
        if skipped_big > 0 {
            out.push_str(&format!("\n({skipped_big} files over 5 MB were skipped: give one as path to search it.)"));
        }
        if left > 0 {
            out.push_str(&format!("\n(The search stopped after 30 s, with {left} files left.)"));
        }
        Output::ok(out)
    }
}

struct Collect<'a> {
    shown: &'a str,
    lines: &'a mut Vec<String>,
    matches: &'a mut usize,
    last: Option<u64>,
}

impl Collect<'_> {
    fn push(&mut self, number: Option<u64>, sep: char, bytes: &[u8]) {
        if let (Some(n), Some(last)) = (number, self.last)
            && n > last + 1
        {
            self.lines.push("--".into());
        }
        self.last = number.or(self.last);
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim_end_matches(['\n', '\r']);
        let text: String = if text.chars().count() > 300 {
            text.chars().take(300).chain("…".chars()).collect()
        } else {
            text.to_string()
        };
        self.lines.push(format!("{}{sep}{}{sep} {text}", self.shown, number.unwrap_or(0)));
    }
}

impl Sink for Collect<'_> {
    type Error = std::io::Error;
    fn matched(&mut self, _s: &grep_searcher::Searcher, m: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        *self.matches += 1;
        self.push(m.line_number(), ':', m.bytes());
        Ok(self.lines.len() <= MOST_LINES)
    }
    fn context(&mut self, _s: &grep_searcher::Searcher, c: &SinkContext<'_>) -> Result<bool, Self::Error> {
        if matches!(c.kind(), SinkContextKind::Before | SinkContextKind::After) {
            self.push(c.line_number(), '-', c.bytes());
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::context;

    fn tree(d: &Path) {
        std::fs::create_dir_all(d.join("src/deep")).expect("mkdir");
        std::fs::create_dir_all(d.join("target")).expect("mkdir");
        std::fs::write(d.join(".gitignore"), "target/\n").expect("write");
        std::fs::write(d.join("src/main.rs"), "fn main() {\n    let total = 1;\n    println!(\"{total}\");\n}\n")
            .expect("write");
        std::fs::write(d.join("src/deep/lib.rs"), "pub fn total() -> u32 { 2 }\n").expect("write");
        std::fs::write(d.join("target/out.rs"), "fn total() {}\n").expect("write");
        std::fs::write(d.join("notes.md"), "Total: none\n").expect("write");
    }

    #[tokio::test]
    async fn glob_follows_gitignore() {
        let (d, ctx) = context();
        tree(d.path());
        let o = Glob.run(json!({"pattern": "**/*.rs"}), &ctx).await;
        assert_eq!(o.text, "src/deep/lib.rs\nsrc/main.rs");
        let o = Glob.run(json!({"pattern": "*.md"}), &ctx).await;
        assert_eq!(o.text, "notes.md");
    }

    #[tokio::test]
    async fn grep_with_context_and_case() {
        let (d, ctx) = context();
        tree(d.path());
        let o = Grep.run(json!({"pattern": "total", "glob": "*.rs"}), &ctx).await;
        assert_eq!(
            o.text,
            "src/deep/lib.rs:1: pub fn total() -> u32 { 2 }\nsrc/main.rs:2:     let total = 1;\nsrc/main.rs:3:     println!(\"{total}\");"
        );
        let o = Grep.run(json!({"pattern": "^total", "ignore_case": true}), &ctx).await;
        assert_eq!(o.text, "notes.md:1: Total: none");
        let o = Grep.run(json!({"pattern": "let total", "context": 1, "path": "src/main.rs"}), &ctx).await;
        assert_eq!(
            o.text,
            "src/main.rs-1- fn main() {\nsrc/main.rs:2:     let total = 1;\nsrc/main.rs-3-     println!(\"{total}\");"
        );
    }

    #[tokio::test]
    async fn a_bad_pattern_is_said() {
        let (_d, ctx) = context();
        assert!(Grep.run(json!({"pattern": "(unclosed"}), &ctx).await.is_error);
    }
}
