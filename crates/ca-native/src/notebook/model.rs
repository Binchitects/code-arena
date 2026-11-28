//! A notebook as it is kept: nbformat 4's cells (code, Markdown, raw), each with its source, its outputs and its
//! execution count, read from its file and written back as Jupyter and VS Code write one: the keys sorted all the
//! way down, the file's own indentation (one space when it has none), multi-line text as a list of lines, non-ASCII
//! left as it is, a final line break. What a cell or the notebook has besides (metadata, ids, attachments, outputs
//! as they were) is kept as it was, so a notebook opened and saved unchanged is the same file.
//!
//! The changes made to the cells (one added, deleted, moved, made Markdown, its outputs cleared) are each one step
//! of `History`, taken back with Undo. Nothing here is of the window: it is read and written off its thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value, json};

/// What a cell is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Code,
    Markdown,
    Raw,
}

impl Kind {
    /// Its name as nbformat writes it (`cell_type`).
    pub fn name(self) -> &'static str {
        match self {
            Kind::Code => "code",
            Kind::Markdown => "markdown",
            Kind::Raw => "raw",
        }
    }

    fn of(name: &str) -> Kind {
        match name {
            "markdown" => Kind::Markdown,
            "raw" => Kind::Raw,
            _ => Kind::Code,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    /// The cell among the window's while the notebook is open: another cell's never, kept as it is changed and
    /// moved (the cell's own `id` is in `rest`, when the notebook has ids).
    pub key: u64,
    pub kind: Kind,
    pub source: String,
    /// A code cell's outputs, as the file has them: shared, not copied, by the steps of Undo and the window.
    pub outputs: Arc<Vec<Value>>,
    pub count: Option<u64>,
    /// Everything else the cell has (its id, its metadata, its attachments), written back as it was.
    pub rest: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notebook {
    pub cells: Vec<Cell>,
    pub minor: u64,
    /// The spaces the file is indented by, kept when it is saved.
    pub indent: usize,
    /// Everything else the notebook has (`nbformat`, `metadata`), written back as it was.
    pub rest: Map<String, Value>,
}

static KEYS: AtomicU64 = AtomicU64::new(1);

/// A key no other cell has.
pub fn next_key() -> u64 {
    KEYS.fetch_add(1, Ordering::Relaxed)
}

/// A new cell id: 8 hexadecimal digits, as Jupyter makes them.
pub fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(next_key());
    format!("{:08x}", h.finish() as u32)
}

/// A multi-line text as nbformat writes it (a list of lines, or one string), as one string.
pub fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(lines) => lines.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

/// A text as nbformat writes multi-line text: its lines, each with its line break but the last.
pub fn lines_of(text: &str) -> Vec<Value> {
    text.split_inclusive('\n').map(|l| Value::String(l.to_string())).collect()
}

impl Notebook {
    /// A new notebook, as Create: New Jupyter Notebook makes one: one empty Python cell.
    pub fn empty() -> Notebook {
        let mut rest = Map::new();
        rest.insert(
            "metadata".into(),
            json!({
                "kernelspec": {"display_name": "Python 3", "language": "python", "name": "python3"},
                "language_info": {"name": "python"}
            }),
        );
        rest.insert("nbformat".into(), json!(4));
        let mut nb = Notebook { cells: Vec::new(), minor: 5, indent: 1, rest };
        let cell = nb.new_cell(Kind::Code, "");
        nb.cells.push(cell);
        nb
    }

    /// Whether its cells have ids (nbformat 4.5 and after).
    pub fn has_ids(&self) -> bool {
        self.minor >= 5
    }

    /// A cell to add: with an id when the notebook's cells have them.
    pub fn new_cell(&self, kind: Kind, source: &str) -> Cell {
        let mut rest = Map::new();
        if self.has_ids() {
            rest.insert("id".into(), Value::String(new_id()));
        }
        rest.insert("metadata".into(), json!({}));
        Cell { key: next_key(), kind, source: source.to_string(), outputs: Arc::default(), count: None, rest }
    }

    /// The language its code is in, as its metadata says (Python when it says none).
    pub fn language(&self) -> String {
        let m = &self.rest.get("metadata").cloned().unwrap_or(Value::Null);
        m["language_info"]["name"]
            .as_str()
            .or_else(|| m["kernelspec"]["language"].as_str())
            .filter(|l| !l.is_empty())
            .unwrap_or("python")
            .to_lowercase()
    }

    /// What its kernel is called in its metadata (none: it names none).
    #[allow(dead_code)] // Read by the notebook kernel's part.
    pub fn kernel_name(&self) -> Option<String> {
        let m = self.rest.get("metadata")?;
        m["kernelspec"]["display_name"].as_str().filter(|n| !n.is_empty()).map(str::to_string)
    }

    pub fn at(&self, key: u64) -> Option<usize> {
        self.cells.iter().position(|c| c.key == key)
    }
}

/// A notebook's text read (an empty file is a new notebook); one that is not nbformat 4 says why.
pub fn parse(text: &str) -> Result<Notebook, String> {
    if text.trim().is_empty() {
        return Ok(Notebook::empty());
    }
    let raw: Value = serde_json::from_str(text).map_err(|e| format!("The notebook is not JSON: {e}"))?;
    let Value::Object(mut rest) = raw else { return Err("The notebook is not a JSON object.".into()) };
    match rest.get("nbformat").and_then(Value::as_u64) {
        Some(4) => {}
        Some(n) => {
            return Err(format!("Only nbformat 4 notebooks open as notebooks (this one is {n}): open it as text."));
        }
        None => return Err("The notebook says no nbformat: open it as text.".into()),
    }
    let Some(Value::Array(cells)) = rest.remove("cells") else {
        return Err("The notebook has no list of cells.".into());
    };
    let minor = rest.remove("nbformat_minor").and_then(|m| m.as_u64()).unwrap_or(0);
    let cells = cells
        .into_iter()
        .filter_map(|c| match c {
            Value::Object(o) => Some(cell_of(o)),
            _ => None,
        })
        .collect();
    Ok(Notebook { cells, minor, indent: indent_of(text), rest })
}

fn cell_of(mut o: Map<String, Value>) -> Cell {
    let kind = Kind::of(o.remove("cell_type").as_ref().and_then(Value::as_str).unwrap_or("code"));
    let source = o.remove("source").map(|s| text_of(&s)).unwrap_or_default();
    let (mut outputs, mut count) = (Arc::default(), None);
    if kind == Kind::Code {
        if let Some(Value::Array(o)) = o.remove("outputs") {
            outputs = Arc::new(o);
        }
        count = o.remove("execution_count").and_then(|c| c.as_u64());
    }
    Cell { key: next_key(), kind, source, outputs, count, rest: o }
}

/// The spaces a JSON file's first key is indented by (one, when it is on one line).
fn indent_of(text: &str) -> usize {
    let after = text.trim_start().strip_prefix('{').unwrap_or("");
    let after = after.strip_prefix("\r\n").or_else(|| after.strip_prefix('\n'));
    match after {
        Some(line) => {
            let n = line.len() - line.trim_start_matches(' ').len();
            if n > 0 && line[n..].starts_with('"') { n } else { 1 }
        }
        None => 1,
    }
}

/// A value with its objects' keys sorted, all the way down (Jupyter writes with `sort_keys`).
fn sorted(v: Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.into_iter().map(sorted).collect()),
        Value::Object(o) => {
            let mut pairs: Vec<(String, Value)> = o.into_iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(pairs.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        v => v,
    }
}

/// A notebook as its file: what Jupyter would write.
pub fn write(nb: &Notebook) -> String {
    let cells: Vec<Value> = nb
        .cells
        .iter()
        .map(|c| {
            let mut o = c.rest.clone();
            o.insert("cell_type".into(), Value::String(c.kind.name().into()));
            o.insert("source".into(), Value::Array(lines_of(&c.source)));
            if !o.contains_key("metadata") {
                o.insert("metadata".into(), json!({}));
            }
            if c.kind == Kind::Code {
                o.insert("execution_count".into(), c.count.map_or(Value::Null, Value::from));
                o.insert("outputs".into(), Value::Array(c.outputs.to_vec()));
            }
            Value::Object(o)
        })
        .collect();
    let mut top = nb.rest.clone();
    top.insert("cells".into(), Value::Array(cells));
    top.insert("nbformat_minor".into(), Value::from(nb.minor));
    let mut text = String::new();
    put(&mut text, &sorted(Value::Object(top)), &" ".repeat(nb.indent.max(1)), 0);
    text.push('\n');
    text
}

/// A value written as Python's `json.dumps` writes it with an indent: each item of a list or an object on a line
/// of its own, an empty one as `[]` or `{}`, a key and its value parted by ": ".
fn put(out: &mut String, v: &Value, indent: &str, depth: usize) {
    let pad = |out: &mut String, depth: usize| (0..depth).for_each(|_| out.push_str(indent));
    match v {
        Value::Array(a) if !a.is_empty() => {
            out.push_str("[\n");
            for (n, item) in a.iter().enumerate() {
                pad(out, depth + 1);
                put(out, item, indent, depth + 1);
                out.push_str(if n + 1 < a.len() { ",\n" } else { "\n" });
            }
            pad(out, depth);
            out.push(']');
        }
        Value::Object(o) if !o.is_empty() => {
            out.push_str("{\n");
            for (n, (k, item)) in o.iter().enumerate() {
                pad(out, depth + 1);
                out.push_str(&Value::String(k.clone()).to_string());
                out.push_str(": ");
                put(out, item, indent, depth + 1);
                out.push_str(if n + 1 < o.len() { ",\n" } else { "\n" });
            }
            pad(out, depth);
            out.push('}');
        }
        v => out.push_str(&v.to_string()),
    }
}

/// A change made to a notebook's cells: each is one step of Undo.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// A new cell of a kind at a place (its key: the cell made).
    Insert(usize, Kind, u64),
    Delete(u64),
    /// A cell moved up (true) or down by one.
    Move(u64, bool),
    Kind(u64, Kind),
    /// The outputs and counts cleared: of one cell, or of them all.
    Clear(Option<u64>),
    /// Cells put back (pasted) after a place, or at the top.
    Paste(Option<u64>, Vec<Cell>),
}

/// A change made; false when it changed nothing (a cell moved past the end, one made the kind it is).
pub fn apply(nb: &mut Notebook, change: Change) -> bool {
    match change {
        Change::Insert(at, kind, key) => {
            let mut cell = nb.new_cell(kind, "");
            cell.key = key;
            nb.cells.insert(at.min(nb.cells.len()), cell);
            true
        }
        Change::Delete(key) => match nb.at(key) {
            Some(at) => {
                nb.cells.remove(at);
                true
            }
            None => false,
        },
        Change::Move(key, up) => match nb.at(key) {
            Some(at) if up && at > 0 => {
                nb.cells.swap(at, at - 1);
                true
            }
            Some(at) if !up && at + 1 < nb.cells.len() => {
                nb.cells.swap(at, at + 1);
                true
            }
            _ => false,
        },
        Change::Kind(key, kind) => match nb.at(key) {
            Some(at) if nb.cells[at].kind != kind => {
                let c = &mut nb.cells[at];
                c.kind = kind;
                c.outputs = Arc::default();
                c.count = None;
                true
            }
            _ => false,
        },
        Change::Clear(key) => {
            let mut changed = false;
            for c in nb.cells.iter_mut().filter(|c| key.is_none_or(|k| c.key == k)) {
                changed |= !c.outputs.is_empty() || c.count.is_some();
                c.outputs = Arc::default();
                c.count = None;
            }
            changed
        }
        Change::Paste(after, cells) => {
            let at = after.and_then(|k| nb.at(k)).map_or(0, |a| a + 1);
            let made: Vec<Cell> = cells
                .into_iter()
                .map(|mut c| {
                    c.key = next_key();
                    if nb.has_ids() {
                        c.rest.insert("id".into(), Value::String(new_id()));
                    }
                    c
                })
                .collect();
            let n = made.len();
            nb.cells.splice(at..at, made);
            n > 0
        }
    }
}

/// What it adds to the window's registries, once at start: nothing (the notebook editor, `notebook/mod.rs`, is
/// what the window has of it).
pub fn register(_ui: crate::state::Ui, _send: &crate::state::Send) {}

/// The session's messages: none is this file's.
pub fn take(_ui: crate::state::Ui, _m: &Value, _send: &crate::state::Send) -> bool {
    false
}

/// The cells as they were before each change, to go back to (Undo) and forward again (Redo).
#[derive(Clone, Debug, Default)]
pub struct History {
    back: Vec<Vec<Cell>>,
    forward: Vec<Vec<Cell>>,
}

/// As many changes as are kept to take back.
const MOST_STEPS: usize = 100;

impl History {
    /// The cells as they were before a change.
    pub fn before(&mut self, cells: Vec<Cell>) {
        self.back.push(cells);
        if self.back.len() > MOST_STEPS {
            self.back.remove(0);
        }
        self.forward.clear();
    }

    /// The cells as they were before the last change (none: nothing to take back); `now` is kept for Redo.
    pub fn undo(&mut self, now: Vec<Cell>) -> Option<Vec<Cell>> {
        let was = self.back.pop()?;
        self.forward.push(now);
        Some(was)
    }

    pub fn redo(&mut self, now: Vec<Cell>) -> Option<Vec<Cell>> {
        let next = self.forward.pop()?;
        self.back.push(now);
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A notebook as Jupyter writes one (`json.dumps(nb, sort_keys=True, indent=1, ensure_ascii=False) + "\n"`).
    const JUPYTER: &str = r##"{
 "cells": [
  {
   "cell_type": "markdown",
   "id": "a1b2c3d4",
   "metadata": {},
   "source": [
    "# Title — été\n",
    "\n",
    "Some *text*."
   ]
  },
  {
   "cell_type": "code",
   "execution_count": 3,
   "id": "e5f6a7b8",
   "metadata": {
    "tags": [
     "x"
    ]
   },
   "outputs": [
    {
     "name": "stdout",
     "output_type": "stream",
     "text": [
      "hi\n"
     ]
    }
   ],
   "source": [
    "print('hi')"
   ]
  },
  {
   "cell_type": "code",
   "execution_count": null,
   "id": "00000001",
   "metadata": {},
   "outputs": [],
   "source": []
  }
 ],
 "metadata": {
  "kernelspec": {
   "display_name": "Python 3",
   "language": "python",
   "name": "python3"
  },
  "language_info": {
   "name": "python",
   "version": "3.12.1"
  }
 },
 "nbformat": 4,
 "nbformat_minor": 5
}
"##;

    #[test]
    fn a_notebook_saved_unchanged_is_the_same_file() {
        let nb = parse(JUPYTER).expect("a notebook");
        assert_eq!(nb.cells.len(), 3);
        assert_eq!(nb.cells[0].kind, Kind::Markdown);
        assert_eq!(nb.cells[0].source, "# Title — été\n\nSome *text*.");
        assert_eq!(nb.cells[1].count, Some(3));
        assert_eq!(nb.cells[1].outputs.len(), 1);
        assert_eq!(nb.cells[2].source, "");
        assert_eq!(nb.language(), "python");
        assert_eq!(nb.kernel_name().as_deref(), Some("Python 3"));
        assert_eq!(write(&nb), JUPYTER);
    }

    #[test]
    fn the_files_own_indentation_and_order_of_keys_are_jupyters() {
        let two = "{\n  \"nbformat\": 4,\n  \"metadata\": {\"b\": 1, \"a\": 2},\n  \"nbformat_minor\": 2,\n  \"cells\": []\n}";
        let nb = parse(two).expect("a notebook");
        assert_eq!(nb.indent, 2);
        assert_eq!(
            write(&nb),
            "{\n  \"cells\": [],\n  \"metadata\": {\n    \"a\": 2,\n    \"b\": 1\n  },\n  \"nbformat\": 4,\n  \"nbformat_minor\": 2\n}\n"
        );
        assert_eq!(parse("{\"nbformat\": 4, \"cells\": []}").expect("one line").indent, 1);
    }

    #[test]
    fn what_is_no_notebook_says_why() {
        assert!(parse("[1]").is_err());
        assert!(parse("{").expect_err("not JSON").starts_with("The notebook is not JSON"));
        assert!(parse("{\"nbformat\": 3, \"cells\": []}").expect_err("old").contains("nbformat 4"));
        assert!(parse("{\"nbformat\": 4}").is_err());
        // An empty file is a new notebook.
        assert_eq!(parse("  \n").expect("new").cells.len(), 1);
    }

    #[test]
    fn a_new_notebook_has_one_empty_python_cell_with_an_id() {
        let nb = Notebook::empty();
        let text = write(&nb);
        let back = parse(&text).expect("read back");
        assert_eq!(back.cells.len(), 1);
        assert_eq!(back.cells[0].kind, Kind::Code);
        assert_eq!(back.cells[0].rest["id"].as_str().map(str::len), Some(8));
        assert_eq!(back.minor, 5);
        assert!(text.contains("\"execution_count\": null"));
        assert_eq!(write(&back), text);
    }

    #[test]
    fn source_is_written_as_lines_with_their_breaks() {
        assert_eq!(lines_of("a\nb\n"), vec![json!("a\n"), json!("b\n")]);
        assert_eq!(lines_of("a\nb"), vec![json!("a\n"), json!("b")]);
        assert!(lines_of("").is_empty());
        assert_eq!(text_of(&json!(["a\n", "b"])), "a\nb");
        assert_eq!(text_of(&json!("one")), "one");
    }

    #[test]
    fn changes_to_the_cells() {
        let mut nb = parse(JUPYTER).expect("a notebook");
        let (md, code, last) = (nb.cells[0].key, nb.cells[1].key, nb.cells[2].key);
        let new = next_key();
        assert!(apply(&mut nb, Change::Insert(1, Kind::Markdown, new)));
        assert_eq!(nb.at(new), Some(1));
        assert!(nb.cells[1].rest.contains_key("id"), "a 4.5 notebook's new cell has an id");
        assert!(apply(&mut nb, Change::Move(new, true)));
        assert_eq!(nb.at(new), Some(0));
        assert!(!apply(&mut nb, Change::Move(new, true)), "the first goes no higher");
        assert!(apply(&mut nb, Change::Kind(code, Kind::Markdown)));
        assert!(nb.cells[nb.at(code).expect("there")].outputs.is_empty(), "Markdown has no outputs");
        assert!(!apply(&mut nb, Change::Kind(md, Kind::Markdown)));
        assert!(apply(&mut nb, Change::Delete(last)));
        assert!(!apply(&mut nb, Change::Delete(last)));
        let written = write(&nb);
        let cells = &serde_json::from_str::<Value>(&written).expect("there")["cells"];
        assert!(cells[2].get("outputs").is_none() && cells[2].get("execution_count").is_none());
        assert_eq!(cells[2]["metadata"]["tags"], json!(["x"]), "what else a cell has is kept");
    }

    #[test]
    fn outputs_are_cleared_of_one_cell_or_all() {
        let mut nb = parse(JUPYTER).expect("a notebook");
        let code = nb.cells[1].key;
        let first = nb.cells[0].key;
        assert!(!apply(&mut nb, Change::Clear(Some(first))));
        assert!(apply(&mut nb, Change::Clear(Some(code))));
        assert!(nb.cells[1].outputs.is_empty() && nb.cells[1].count.is_none());
        assert!(!apply(&mut nb, Change::Clear(None)), "nothing left to clear");
    }

    #[test]
    fn pasted_cells_are_new_cells() {
        let mut nb = parse(JUPYTER).expect("a notebook");
        let copied = vec![nb.cells[1].clone()];
        let first = nb.cells[0].key;
        assert!(apply(&mut nb, Change::Paste(Some(first), copied.clone())));
        assert_eq!(nb.cells.len(), 4);
        assert_eq!(nb.cells[1].source, "print('hi')");
        assert_ne!(nb.cells[1].key, copied[0].key);
        assert_ne!(nb.cells[1].rest["id"], copied[0].rest["id"]);
        assert!(apply(&mut nb, Change::Paste(None, copied)));
        assert_eq!(nb.cells[0].source, "print('hi')");
    }

    #[test]
    fn undo_and_redo_go_back_and_forth_one_change_at_a_time() {
        let mut nb = Notebook::empty();
        let mut h = History::default();
        let one = nb.cells.clone();
        h.before(nb.cells.clone());
        apply(&mut nb, Change::Insert(1, Kind::Markdown, next_key()));
        let two = nb.cells.clone();
        let back = h.undo(nb.cells.clone()).expect("one step");
        assert_eq!(back, one);
        assert_eq!(h.redo(back).expect("again"), two);
        assert!(h.redo(two.clone()).is_none());
        h.undo(two).expect("back");
        assert!(h.undo(one).is_none());
    }
}
