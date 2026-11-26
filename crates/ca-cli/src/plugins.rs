//! Plugins: a folder (or a git repository) bundling commands, sub-agents, skills, hooks and MCP servers, in Claude
//! Code's layout (`.claude-plugin/plugin.json`, `commands/`, `agents/`, `skills/`, `hooks/hooks.json`, `.mcp.json`).
//! Installed into the data folder (`code-arena plugin install PATH|URL`), and used once enabled (`plugins.enabled`, in
//! your settings or the project's). `${CLAUDE_PLUGIN_ROOT}` in its hooks and servers is its folder.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use serde_json::Value;

pub fn dir(data: &Path) -> PathBuf {
    data.join("plugins")
}

/// A plugin installed: its name, version, description, folder.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub name: String,
    pub version: String,
    pub description: String,
    pub root: PathBuf,
}

fn manifest(root: &Path) -> Option<Value> {
    [root.join(".claude-plugin/plugin.json"), root.join("plugin.json")]
        .iter()
        .find_map(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
}

fn read(root: &Path) -> Option<Plugin> {
    let m = manifest(root).unwrap_or(Value::Null);
    let name = m["name"]
        .as_str()
        .map(str::to_string)
        .or_else(|| root.file_name().map(|n| n.to_string_lossy().into_owned()))?;
    Some(Plugin {
        name,
        version: m["version"].as_str().unwrap_or("").to_string(),
        description: m["description"].as_str().unwrap_or("").to_string(),
        root: root.to_path_buf(),
    })
}

pub fn installed(data: &Path) -> Vec<Plugin> {
    let mut out: Vec<Plugin> = std::fs::read_dir(dir(data))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| read(&e.path()))
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The plugins in use: installed and enabled (by name).
pub fn enabled(data: &Path, names: &[String]) -> Vec<Plugin> {
    installed(data).into_iter().filter(|p| names.contains(&p.name)).collect()
}

/// `${CLAUDE_PLUGIN_ROOT}` (and Code Arena's own name for it) read as the plugin's folder.
pub fn rooted(text: &str, root: &Path) -> String {
    let r = root.display().to_string();
    text.replace("${CLAUDE_PLUGIN_ROOT}", &r).replace("${CODE_ARENA_PLUGIN_ROOT}", &r)
}

fn rooted_value(v: &Value, root: &Path) -> Value {
    match v {
        Value::String(s) => Value::String(rooted(s, root)),
        Value::Array(a) => Value::Array(a.iter().map(|x| rooted_value(x, root)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), rooted_value(x, root))).collect()),
        other => other.clone(),
    }
}

/// The plugin's hooks (`hooks/hooks.json`), its folder put in.
pub fn hooks(p: &Plugin) -> Option<Value> {
    let t = std::fs::read_to_string(p.root.join("hooks/hooks.json")).ok()?;
    let v: Value = serde_json::from_str(&t).ok()?;
    Some(rooted_value(v.get("hooks").unwrap_or(&v), &p.root))
}

/// The plugin's MCP servers (`.mcp.json`), each named `PLUGIN-SERVER`, its folder put in.
pub fn servers(p: &Plugin) -> Vec<(String, Value)> {
    let Some(v) =
        std::fs::read_to_string(p.root.join(".mcp.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return vec![];
    };
    let map = v.get("mcpServers").unwrap_or(&v);
    map.as_object().into_iter().flatten().map(|(n, s)| (format!("{}-{n}", p.name), rooted_value(s, &p.root))).collect()
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let name = e.file_name();
        if name == ".git" {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            copy_tree(&p, &to.join(&name))?;
        } else {
            std::fs::copy(&p, to.join(&name))?;
        }
    }
    Ok(())
}

fn is_url(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://") || s.starts_with("git@") || s.starts_with("ssh://")
}

/// `code-arena plugin …`.
pub fn command(data: &Path, settings_file: &Path, enabled_now: &[String], args: &[String]) -> anyhow::Result<i32> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let keep_enabled =
        |names: Vec<String>| ca_settings::settings::keep(settings_file, "plugins.enabled", &serde_json::json!(names));
    match words.as_slice() {
        [] | ["list"] => {
            let all = installed(data);
            if all.is_empty() {
                println!("No plugins yet: code-arena plugin install PATH|GIT_URL.");
            }
            for p in all {
                println!(
                    "{} {} {}{}",
                    if enabled_now.contains(&p.name) { "●" } else { "○" },
                    p.name,
                    if p.version.is_empty() { String::new() } else { format!("{} ", p.version) },
                    p.description
                );
            }
            Ok(0)
        }
        ["install", src] => {
            let clone = dir(data).join(format!(".clone-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&clone);
            struct Gone(PathBuf);
            impl Drop for Gone {
                fn drop(&mut self) {
                    let _ = std::fs::remove_dir_all(&self.0);
                }
            }
            let _gone = Gone(clone.clone());
            let from: PathBuf = if is_url(src) {
                std::fs::create_dir_all(dir(data))?;
                let to = clone.clone();
                let ok = std::process::Command::new("git")
                    .args(["clone", "--depth", "1", "--quiet", src])
                    .arg(&to)
                    .env("GIT_TERMINAL_PROMPT", "0")
                    .status()
                    .context("git could not start")?
                    .success();
                if !ok {
                    bail!("git could not clone {src}");
                }
                to
            } else {
                std::fs::canonicalize(src).with_context(|| format!("the folder {src}"))?
            };
            let p = read(&from)
                .ok_or_else(|| anyhow::anyhow!("{src} has no name: its .claude-plugin/plugin.json says none"))?;
            if !p.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                bail!("the plugin's name {:?} cannot be a folder's", p.name);
            }
            let to = dir(data).join(&p.name);
            let _ = std::fs::remove_dir_all(&to);
            copy_tree(&from, &to)?;
            let mut parts = Vec::new();
            for (d, what) in [("commands", "commands"), ("agents", "sub-agents"), ("skills", "skills")] {
                if to.join(d).is_dir() {
                    parts.push(what);
                }
            }
            if to.join("hooks/hooks.json").is_file() {
                parts.push("hooks");
            }
            if to.join(".mcp.json").is_file() {
                parts.push("MCP servers");
            }
            println!(
                "Installed {} ({}). It is used once enabled: code-arena plugin enable {}",
                p.name,
                if parts.is_empty() { "nothing in it Code Arena reads".to_string() } else { parts.join(", ") },
                p.name
            );
            Ok(0)
        }
        ["remove", name] => {
            let to = dir(data).join(name);
            if !to.is_dir() {
                bail!("no plugin {name}");
            }
            std::fs::remove_dir_all(&to)?;
            keep_enabled(enabled_now.iter().filter(|n| n != name).cloned().collect())?;
            println!("Removed {name}.");
            Ok(0)
        }
        ["enable", name] => {
            if !dir(data).join(name).is_dir() {
                bail!("no plugin {name} is installed: code-arena plugin list");
            }
            let mut names = enabled_now.to_vec();
            if !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
            keep_enabled(names)?;
            println!(
                "{name} is enabled for you (a project enables its own in .code-arena/settings.json, \"plugins.enabled\")."
            );
            Ok(0)
        }
        ["disable", name] => {
            keep_enabled(enabled_now.iter().filter(|n| n != name).cloned().collect())?;
            println!("{name} is disabled.");
            Ok(0)
        }
        _ => {
            eprintln!("code-arena plugin [list | install PATH|GIT_URL | enable NAME | disable NAME | remove NAME]");
            Ok(2)
        }
    }
}
