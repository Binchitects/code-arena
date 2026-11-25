//! Calls mended before they run: a tool named as another harness names it, arguments that are not quite JSON, and
//! arguments named or typed not as the tool says. The result says what was mended, so the model learns.

use serde_json::{Map, Value};

/// Another harness's name for one of ours.
const ALIASES: &[(&str, &str)] = &[
    ("bash", "run_shell"),
    ("shell", "run_shell"),
    ("run_command", "run_shell"),
    ("execute_command", "run_shell"),
    ("exec", "run_shell"),
    ("read", "read_file"),
    ("view", "read_file"),
    ("cat", "read_file"),
    ("write", "write_file"),
    ("create_file", "write_file"),
    ("edit", "edit_file"),
    ("str_replace", "edit_file"),
    ("str_replace_editor", "edit_file"),
    ("replace", "edit_file"),
    ("ls", "list_dir"),
    ("list_directory", "list_dir"),
    ("list_files", "list_dir"),
    ("find_files", "glob"),
    ("search", "grep"),
    ("search_files", "grep"),
    ("ripgrep", "grep"),
];

/// Another harness's name for an argument.
const ARG_ALIASES: &[(&str, &str)] = &[
    ("file_path", "path"),
    ("filepath", "path"),
    ("filename", "path"),
    ("file", "path"),
    ("dir", "path"),
    ("directory", "path"),
    ("cmd", "command"),
    ("old_str", "old_string"),
    ("old", "old_string"),
    ("new_str", "new_string"),
    ("new", "new_string"),
    ("text", "content"),
    ("contents", "content"),
    ("query", "pattern"),
    ("regex", "pattern"),
    ("start_line", "offset"),
];

/// A call as it will run, and what was mended to get there.
#[derive(Debug, Clone, PartialEq)]
pub struct Mended {
    pub name: String,
    pub args: Value,
    pub notes: Vec<String>,
}

/// Why a call cannot run, with what the tool takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmendable(pub String);

/// Mends a call against the tools there are (`name` → its schema).
pub fn mend(name: &str, arguments: &str, tools: &[(String, Value)]) -> Result<Mended, Unmendable> {
    let mut notes = Vec::new();
    let (tool, schema) = find_tool(name, tools, &mut notes).ok_or_else(|| {
        let names: Vec<&str> = tools.iter().map(|(n, _)| n.as_str()).collect();
        Unmendable(format!("There is no tool named {name}. The tools are: {}.", names.join(", ")))
    })?;
    let mut args = parse_args(arguments, &mut notes)
        .map_err(|why| Unmendable(format!("{tool}'s arguments are not JSON ({why}). {}", describe(&tool, schema))))?;
    if let Value::Object(map) = &mut args {
        rename_and_type(map, schema, &mut notes);
        let missing: Vec<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|r| !map.contains_key(*r))
            .collect();
        if !missing.is_empty() {
            return Err(Unmendable(format!("{tool} needs {}. {}", missing.join(" and "), describe(&tool, schema))));
        }
    } else {
        return Err(Unmendable(format!("{tool}'s arguments must be a JSON object. {}", describe(&tool, schema))));
    }
    Ok(Mended { name: tool, args, notes })
}

/// The note that ends a mended call's result.
pub fn note(m: &Mended) -> Option<String> {
    (!m.notes.is_empty())
        .then(|| format!("This call was mended before it ran: {}. Call {} so next time.", m.notes.join("; "), m.name))
}

fn find_tool<'a>(name: &str, tools: &'a [(String, Value)], notes: &mut Vec<String>) -> Option<(String, &'a Value)> {
    let exact = |n: &str| tools.iter().find(|(t, _)| t == n).map(|(t, s)| (t.clone(), s));
    if let Some(t) = exact(name) {
        return Some(t);
    }
    // functions.read_file, default_api:read_file, tools/read_file
    let bare = name.rsplit(['.', ':', '/']).next().unwrap_or(name);
    let lower = bare.to_ascii_lowercase().replace('-', "_");
    let found = exact(&lower)
        .or_else(|| ALIASES.iter().find(|(a, _)| *a == lower).and_then(|(_, t)| exact(t)))
        .or_else(|| {
            // A letter off: the one tool within one edit of it.
            let near: Vec<_> = tools.iter().filter(|(t, _)| edit_distance(t, &lower) <= 1).collect();
            (near.len() == 1).then(|| (near[0].0.clone(), &near[0].1))
        })?;
    notes.push(format!("{name} → {}", found.0));
    Some(found)
}

/// Letters added, dropped, changed or swapped with the next (optimal string alignment).
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// The arguments as JSON, mending what models get wrong: a code fence, trailing commas, single quotes, Python's
/// `True`/`False`/`None`, a raw line break in a string, closing braces missing, the object sent as a string.
pub fn parse_args(text: &str, notes: &mut Vec<String>) -> Result<Value, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Ok(unwrap_string(v, notes));
    }
    let mut s = t.to_string();
    if let Some(inner) =
        s.strip_prefix("```json").or_else(|| s.strip_prefix("```")).and_then(|r| r.trim_end().strip_suffix("```"))
    {
        s = inner.trim().to_string();
        notes.push("a code fence around the arguments".into());
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            return Ok(unwrap_string(v, notes));
        }
    }
    let repaired = repair(&s);
    match serde_json::from_str::<Value>(&repaired) {
        Ok(v) => {
            notes.push("arguments that were not quite JSON".into());
            Ok(unwrap_string(v, notes))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// `"{\"path\": \"a\"}"`: the object sent as a string.
fn unwrap_string(v: Value, notes: &mut Vec<String>) -> Value {
    if let Value::String(s) = &v
        && let Ok(inner @ Value::Object(_)) = serde_json::from_str::<Value>(s)
    {
        notes.push("the arguments sent as a string".into());
        return inner;
    }
    v
}

fn repair(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut in_str: Option<char> = None;
    let mut stack: Vec<char> = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_str {
            if c == '\\' && i + 1 < chars.len() {
                out.push(c);
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == q {
                in_str = None;
                out.push('"');
            } else if c == '"' {
                out.push_str("\\\"");
            } else if c == '\n' {
                out.push_str("\\n");
            } else if c == '\r' {
                out.push_str("\\r");
            } else if c == '\t' {
                out.push_str("\\t");
            } else {
                out.push(c);
            }
            i += 1;
            continue;
        }
        match c {
            '"' | '\'' => {
                in_str = Some(c);
                out.push('"');
            }
            '{' => {
                stack.push('}');
                out.push(c);
            }
            '[' => {
                stack.push(']');
                out.push(c);
            }
            '}' | ']' => {
                // A trailing comma before it goes.
                let trimmed = out.trim_end().len();
                if out[..trimmed].ends_with(',') {
                    out.truncate(trimmed - 1);
                }
                stack.pop();
                out.push(c);
            }
            _ if c.is_alphabetic() => {
                let word: String = chars[i..].iter().take_while(|c| c.is_alphanumeric() || **c == '_').collect();
                let mapped = match word.as_str() {
                    "True" => "true",
                    "False" => "false",
                    "None" => "null",
                    w => w,
                };
                out.push_str(mapped);
                i += word.chars().count();
                continue;
            }
            _ => out.push(c),
        }
        i += 1;
    }
    if in_str.is_some() {
        out.push('"');
    }
    let trimmed = out.trim_end().len();
    if out[..trimmed].ends_with(',') {
        out.truncate(trimmed - 1);
    }
    while let Some(close) = stack.pop() {
        out.push(close);
    }
    out
}

fn rename_and_type(map: &mut Map<String, Value>, schema: &Value, notes: &mut Vec<String>) {
    let props = schema.get("properties").and_then(Value::as_object).cloned().unwrap_or_default();
    let keys: Vec<String> = map.keys().cloned().collect();
    for key in keys {
        if props.contains_key(&key) {
            continue;
        }
        let lower = key.to_ascii_lowercase();
        let target = props.keys().find(|p| p.to_ascii_lowercase() == lower).cloned().or_else(|| {
            ARG_ALIASES.iter().find(|(a, t)| *a == lower && props.contains_key(*t)).map(|(_, t)| t.to_string())
        });
        if let Some(t) = target
            && !map.contains_key(&t)
            && let Some(v) = map.remove(&key)
        {
            notes.push(format!("{key} → {t}"));
            map.insert(t, v);
        }
    }
    for (name, prop) in &props {
        let Some(v) = map.get_mut(name) else { continue };
        let want = prop.get("type").and_then(Value::as_str).unwrap_or("");
        let fixed = match (want, &*v) {
            ("integer" | "number", Value::String(s)) => s.trim().parse::<i64>().ok().map(Value::from),
            ("boolean", Value::String(s)) => match s.to_ascii_lowercase().as_str() {
                "true" | "yes" | "1" => Some(Value::Bool(true)),
                "false" | "no" | "0" => Some(Value::Bool(false)),
                _ => None,
            },
            ("string", Value::Number(n)) => Some(Value::String(n.to_string())),
            ("array", Value::String(s)) => match serde_json::from_str::<Value>(s) {
                Ok(a @ Value::Array(_)) => Some(a),
                _ => Some(Value::Array(vec![Value::String(s.clone())])),
            },
            _ => None,
        };
        if let Some(f) = fixed {
            notes.push(format!("{name} as {want}"));
            *v = f;
        }
        // An option in another case: "Plan" for "plan".
        if let (Some(options), Value::String(s)) = (prop.get("enum").and_then(Value::as_array), &*v)
            && !options.iter().any(|o| o.as_str() == Some(s))
            && let Some(o) = options.iter().filter_map(Value::as_str).find(|o| o.eq_ignore_ascii_case(s))
        {
            notes.push(format!("{name}: {s} → {o}"));
            *v = Value::String(o.to_string());
        }
    }
}

/// A tool's parameters, each with its type and what it is, and an example.
fn describe(tool: &str, schema: &Value) -> String {
    let props = schema.get("properties").and_then(Value::as_object).cloned().unwrap_or_default();
    let required: Vec<&str> =
        schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let mut lines = vec![format!("{tool} takes:")];
    let mut example = Map::new();
    for (name, p) in &props {
        let ty = p.get("type").and_then(Value::as_str).unwrap_or("any");
        let what = p.get("description").and_then(Value::as_str).unwrap_or("");
        let need = if required.contains(&name.as_str()) { "required" } else { "optional" };
        lines.push(format!("- {name} ({ty}, {need}): {what}"));
        if required.contains(&name.as_str()) {
            example.insert(
                name.clone(),
                match ty {
                    "integer" | "number" => Value::from(1),
                    "boolean" => Value::Bool(true),
                    "array" => Value::Array(vec![]),
                    _ => Value::String("…".into()),
                },
            );
        }
    }
    lines.push(format!("For example: {}", Value::Object(example)));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<(String, Value)> {
        vec![
            (
                "read_file".into(),
                json!({"type": "object", "properties": {"path": {"type": "string"}, "offset": {"type": "integer"}, "limit": {"type": "integer"}}, "required": ["path"]}),
            ),
            (
                "run_shell".into(),
                json!({"type": "object", "properties": {"command": {"type": "string"}, "timeout": {"type": "integer"}}, "required": ["command"]}),
            ),
            (
                "edit_file".into(),
                json!({"type": "object", "properties": {"path": {"type": "string"}, "old_string": {"type": "string"}, "new_string": {"type": "string"}, "replace_all": {"type": "boolean"}}, "required": ["path", "old_string", "new_string"]}),
            ),
        ]
    }

    #[test]
    fn other_harnesses_names() {
        let m = mend("Read", r#"{"file_path": "a.rs", "offset": "12"}"#, &tools()).expect("mended");
        assert_eq!(m.name, "read_file");
        assert_eq!(m.args, json!({"path": "a.rs", "offset": 12}));
        assert_eq!(
            note(&m).expect("note"),
            "This call was mended before it ran: Read → read_file; file_path → path; offset as integer. Call read_file so next time."
        );
        assert_eq!(
            mend("functions.run_shell", r#"{"cmd": "ls"}"#, &tools()).expect("m").args,
            json!({"command": "ls"})
        );
        assert_eq!(
            mend("str_replace", r#"{"path":"a","old_str":"x","new_str":"y","replace_all":"yes"}"#, &tools())
                .expect("m")
                .args["replace_all"],
            json!(true)
        );
        assert_eq!(mend("read_fiel", r#"{"path":"a"}"#, &tools()).expect("m").name, "read_file");
    }

    #[test]
    fn arguments_that_are_nearly_json() {
        let mut n = vec![];
        assert_eq!(parse_args("```json\n{\"path\": \"a\"}\n```", &mut n).expect("ok"), json!({"path": "a"}));
        assert_eq!(
            parse_args("{'path': 'a', 'all': True, 'x': None,}", &mut n).expect("ok"),
            json!({"path": "a", "all": true, "x": null})
        );
        assert_eq!(parse_args("{\"command\": \"echo a\nb\"", &mut n).expect("ok"), json!({"command": "echo a\nb"}));
        assert_eq!(parse_args(r#""{\"path\": \"a\"}""#, &mut n).expect("ok"), json!({"path": "a"}));
        assert_eq!(parse_args("", &mut n).expect("ok"), json!({}));
    }

    #[test]
    fn a_call_missing_what_it_needs_is_explained() {
        let err = mend("edit_file", r#"{"path": "a"}"#, &tools()).expect_err("refused");
        assert!(err.0.starts_with("edit_file needs old_string and new_string. edit_file takes:"), "{}", err.0);
        assert!(err.0.contains("For example: {"));
        assert!(mend("teleport", "{}", &tools()).expect_err("none").0.contains("There is no tool named teleport"));
    }

    #[test]
    fn a_clean_call_has_no_note() {
        let m = mend("read_file", r#"{"path": "a"}"#, &tools()).expect("ok");
        assert_eq!(note(&m), None);
    }
}
