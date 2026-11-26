//! The IDE's colour themes of the person's own, in VS Code's format: a theme's JSON file (`name`, `type`, `colors`,
//! `tokenColors`, an `include` of another), or a VS Code theme extension's folder (its `package.json` naming its
//! themes under `contributes.themes`). Read from `themes/` in the config folder, `~/.code-arena/themes` and the
//! project's `.code-arena/themes`; the page gets them with the built-in ones.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

/// A theme file is read up to this size, and so many themes at most.
const MOST_BYTES: u64 = 2 * 1024 * 1024;
const MOST_THEMES: usize = 200;

pub fn folders(config: &Path, home: Option<&Path>, root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = vec![("yours".to_string(), config.join("themes"))];
    if let Some(h) = home {
        out.push(("yours (~/.code-arena)".into(), h.join(".code-arena").join("themes")));
    }
    out.push(("this project".into(), root.join(".code-arena").join("themes")));
    out
}

fn read(file: &Path) -> Option<Value> {
    if std::fs::metadata(file).ok()?.len() > MOST_BYTES {
        return None;
    }
    ca_settings::jsonc::parse(&std::fs::read_to_string(file).ok()?).ok()
}

/// A theme's JSON with what it includes put under it (VS Code's `include`: the other theme first, this one over it).
fn load_theme(file: &Path, depth: usize) -> Option<Value> {
    let mut v = read(file)?;
    if depth < 4
        && let Some(inc) = v.get("include").and_then(Value::as_str)
        && let Some(base) = load_theme(&file.parent()?.join(inc), depth + 1)
    {
        let mut colors: Map<String, Value> = base.get("colors").and_then(Value::as_object).cloned().unwrap_or_default();
        colors.extend(v.get("colors").and_then(Value::as_object).cloned().unwrap_or_default());
        let mut tokens: Vec<Value> = base.get("tokenColors").and_then(Value::as_array).cloned().unwrap_or_default();
        tokens.extend(v.get("tokenColors").and_then(Value::as_array).cloned().unwrap_or_default());
        if let Some(o) = v.as_object_mut() {
            o.insert("colors".into(), Value::Object(colors));
            o.insert("tokenColors".into(), Value::Array(tokens));
            if o.get("type").is_none()
                && let Some(t) = base.get("type")
            {
                o.insert("type".into(), t.clone());
            }
        }
    }
    Some(v)
}

/// VS Code's `uiTheme` (vs, vs-dark, hc-black, hc-light) or a theme's `type`, as the page's types.
fn kind(ui: Option<&str>, own: Option<&str>) -> &'static str {
    match ui.or(own).unwrap_or("dark") {
        "vs" | "light" => "light",
        "hc-black" | "hc" => "hc",
        "hc-light" | "hcLight" => "hcLight",
        _ => "dark",
    }
}

fn shaped(v: &Value, id: String, name: String, ui: Option<&str>, from: &Path) -> Value {
    json!({
        "id": id,
        "name": name,
        "type": kind(ui, v.get("type").and_then(Value::as_str)),
        "colors": v.get("colors").cloned().filter(Value::is_object).unwrap_or_else(|| json!({})),
        // A theme whose tokenColors name a .tmTheme file instead: its colours are left out (the base's are used).
        "tokenColors": v.get("tokenColors").cloned().filter(Value::is_array).unwrap_or_else(|| json!([])),
        "from": from.display().to_string(),
    })
}

fn slug(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Every theme of the person's own, each once by id (the first folder's wins).
pub fn all(folders: &[(String, PathBuf)]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut push = |t: Value| {
        if out.len() < MOST_THEMES && !out.iter().any(|o| o["id"] == t["id"]) {
            out.push(t);
        }
    };
    for (_, dir) in folders {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                // A VS Code theme extension: its package.json names its themes.
                let Some(pkg) = read(&p.join("package.json")) else { continue };
                for th in pkg.pointer("/contributes/themes").and_then(Value::as_array).into_iter().flatten() {
                    let (Some(label), Some(rel)) = (th["label"].as_str(), th["path"].as_str()) else { continue };
                    let file = p.join(rel);
                    // Only a file inside the extension's folder.
                    let inside =
                        file.canonicalize().ok().zip(p.canonicalize().ok()).is_some_and(|(f, d)| f.starts_with(d));
                    if !inside {
                        continue;
                    }
                    if let Some(v) = load_theme(&file, 0) {
                        let id = th["id"].as_str().map(str::to_string).unwrap_or_else(|| slug(label));
                        push(shaped(&v, id, label.to_string(), th["uiTheme"].as_str(), &file));
                    }
                }
            } else if p.extension().is_some_and(|x| x == "json")
                && let Some(v) = load_theme(&p, 0)
            {
                let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let name = v.get("name").and_then(Value::as_str).map(str::to_string).unwrap_or(stem);
                push(shaped(&v, slug(&name), name, None, &p));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_themes_file_and_an_extensions_folder_with_its_include() {
        let d = tempfile::tempdir().expect("temp");
        let dir = d.path().join("themes");
        std::fs::create_dir_all(dir.join("night-owl/themes")).expect("dir");
        std::fs::write(
            dir.join("mine.json"),
            "{\n  // my own\n  \"name\": \"My Blue\",\n  \"type\": \"dark\",\n  \"colors\": {\"editor.background\": \"#001133\",},\n}\n",
        )
        .expect("w");
        std::fs::write(
            dir.join("night-owl/package.json"),
            r#"{"contributes": {"themes": [{"label": "Night Owl", "uiTheme": "vs-dark", "path": "./themes/owl.json"}, {"label": "Escape", "path": "../../outside.json"}]}}"#,
        )
        .expect("w");
        std::fs::write(dir.join("night-owl/themes/base.json"), r##"{"colors": {"editor.foreground": "#d6deeb", "editor.background": "#000000"}, "tokenColors": [{"scope": "comment", "settings": {"foreground": "#637777"}}]}"##).expect("w");
        std::fs::write(
            dir.join("night-owl/themes/owl.json"),
            r##"{"include": "./base.json", "colors": {"editor.background": "#011627"}}"##,
        )
        .expect("w");
        std::fs::write(d.path().join("outside.json"), r#"{"colors": {}}"#).expect("w");
        let got = all(&[("yours".into(), dir)]);
        let names: Vec<&str> = got.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["My Blue", "Night Owl"], "a file outside its extension is not read");
        assert_eq!(got[0]["id"], "my-blue");
        assert_eq!(got[0]["colors"]["editor.background"], "#001133");
        let owl = &got[1];
        assert_eq!(owl["type"], "dark");
        assert_eq!(owl["colors"]["editor.background"], "#011627", "its own over what it includes");
        assert_eq!(owl["colors"]["editor.foreground"], "#d6deeb", "what it includes");
        assert_eq!(owl["tokenColors"][0]["scope"], "comment");
    }
}
