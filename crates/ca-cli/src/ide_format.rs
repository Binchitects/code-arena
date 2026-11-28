//! The formatters a file has, and a text formatted by one of them. What is found and how each runs is
//! `ca_tools::formatters`; this area answers the faces with it, off the driver's loop.
//!
//! - `formatters{path, language?, id?}` [T] is answered with `formatters{path, language, id?, items, default}`:
//!   the file's formatters in the order they are offered, each `{id, name, source, range, version, available,
//!   reason, hint, path}` (`source` is `project`, `global`, `settings` or null when not found; `range` that it
//!   formats a selection; `reason` why it cannot run; `hint` how to install it), and `default`, the id
//!   `editor.defaultFormatter` names for the language (VS Code's extension ids are taken as Code Arena's), else the
//!   first that can run, else null. A listing is kept ten seconds. In a folder that is not trusted every item says
//!   so in `reason`; the listing itself is no refusal.
//! - `format{path, text, language?, formatter?, range?, options?, id?}` [T] is answered with
//!   `formatted{path, id?, text, formatter, name}`, the whole text formatted (only the range, when one was given,
//!   if the formatter keeps to it), or `formatted{path, id?, formatter?, error, line, col, kind}`: `kind` is
//!   `not_trusted` (the window offers to trust the folder), `no_range` (the formatter formats whole files only),
//!   `not_found` (no such formatter, or it cannot run), `failed` or `timeout`; `line` and `col` are the fault's
//!   place, 1-based as the formatter said it, or null. `formatter` is an id or VS Code's extension id; none: the
//!   default. `range` is `{start: {line, character}, end: {…}}`, 0-based, columns in characters, the end not
//!   taken. `options` is `{tabSize, insertSpaces}`, for the person's own formatters' `${tabSize}` and
//!   `${insertSpaces}`.
//!
//! Prettier is given the `prettier.*` settings as flags when the folder has no Prettier configuration of its own.
//! The formatters run in the sandbox as `tools.sandbox` allows (read-only, a `/tmp` of their own, no network),
//! with none of the driver's keys in their environment.
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver.

use std::path::{Path, PathBuf};

use ca_settings::Settings;
use ca_settings::schema;
use ca_tools::formatters::{self, Cache, Failure, Formatter, Gate, Job, Look, PrettierSettings, Runner, Span};
use serde_json::{Value, json};

use crate::ide_ext::{Area, Rule, Shared, kind, send};

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[("formatters", Rule::Turn), ("format", Rule::Turn)];

    fn shared(sh: &Shared, m: &Value) -> bool {
        if !matches!(kind(m), "formatters" | "format") {
            return false;
        }
        let (sh, m) = (sh.clone(), m.clone());
        tokio::task::spawn_blocking(move || {
            let reply = if kind(&m) == "format" { format(&sh, &m) } else { listing(&sh, &m) };
            send(&sh.out, reply);
        });
        true
    }
}

/// What a face asked about: the file as it named it and as it is, its language.
struct Asked {
    given: Value,
    id: Option<Value>,
    root: PathBuf,
    file: PathBuf,
    language: String,
}

impl Asked {
    fn of(sh: &Shared, m: &Value) -> Asked {
        let root = sh.workspace.root().to_path_buf();
        let given = m["path"].as_str().unwrap_or("");
        let p = Path::new(given);
        let file = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
        let language = m["language"]
            .as_str()
            .filter(|l| crate::ide_settings::language_id(l))
            .map(String::from)
            .unwrap_or_else(|| formatters::language_of(&file).to_string());
        Asked { given: m["path"].clone(), id: m.get("id").cloned(), root, file, language }
    }

    /// A reply of this area's, with the keys it answers.
    fn reply(&self, kind: &str) -> Value {
        let mut v = json!({"type": kind, "path": self.given});
        if let Some(id) = &self.id {
            v["id"] = id.clone();
        }
        v
    }
}

/// The `formatters` reply.
fn listing(sh: &Shared, m: &Value) -> Value {
    let a = Asked::of(sh, m);
    let settings = crate::ide_settings::current(sh);
    let trusted = crate::ide_trust::trusted(&sh.paths.data, &a.root);
    let items = found(sh, &settings, &a, trusted);
    let configured = settings.ide("editor.defaultFormatter", Some(&a.language));
    let mut v = a.reply("formatters");
    v["language"] = json!(a.language);
    v["default"] = json!(formatters::default_of(&items, configured.as_str().unwrap_or("")));
    v["items"] = json!(items);
    v
}

/// The `formatted` reply.
fn format(sh: &Shared, m: &Value) -> Value {
    let a = Asked::of(sh, m);
    let mut v = a.reply("formatted");
    let fail = |mut v: Value, e: Failure| {
        v["error"] = json!(e.message);
        v["line"] = json!(e.line);
        v["col"] = json!(e.col);
        v["kind"] = json!(e.kind);
        v
    };
    if !crate::ide_trust::trusted(&sh.paths.data, &a.root) {
        return fail(
            v,
            Failure::new(
                "not_trusted",
                "Formatting runs the folder's own programs: trust the folder to format its files.",
            ),
        );
    }
    let settings = crate::ide_settings::current(sh);
    let items = found(sh, &settings, &a, true);
    let wanted = match m["formatter"].as_str().map(formatters::canonical).filter(|f| !f.is_empty()) {
        Some(w) => Some(w),
        None => {
            let configured = settings.ide("editor.defaultFormatter", Some(&a.language));
            formatters::default_of(&items, configured.as_str().unwrap_or(""))
        }
    };
    let Some(f) = wanted.as_deref().and_then(|w| items.iter().find(|f| f.id == w)) else {
        let message = match (&wanted, items.first()) {
            (Some(w), _) if !items.iter().any(|f| f.id == *w) => {
                format!("No formatter {w} formats {} files.", a.language)
            }
            (_, Some(first)) => first.reason.clone().unwrap_or_else(|| format!("{} cannot run.", first.name)),
            _ => format!("No formatter is installed for {} files.", a.language),
        };
        return fail(v, Failure::new("not_found", message));
    };
    v["formatter"] = json!(f.id);
    v["name"] = json!(f.name);
    let options = &m["options"];
    let job = Job {
        text: m["text"].as_str().unwrap_or("").to_string(),
        range: m.get("range").and_then(Span::of),
        tab_size: options["tabSize"].as_u64().map_or(4, |n| n.clamp(1, 64) as u32),
        insert_spaces: options["insertSpaces"].as_bool().unwrap_or(true),
        prettier: if f.id == "prettier" && !formatters::prettier_config(&a.file, &a.root) {
            prettier(&settings, Some(&a.language)).flags()
        } else {
            Vec::new()
        },
    };
    let plan = match formatters::plan(f, &job, &a.root, &a.file) {
        Ok(p) => p,
        Err(e) => return fail(v, e),
    };
    let gate = sh.kept.of::<Gate>();
    let runner = match runner(sh, &settings, &gate) {
        Ok(r) => r,
        Err(e) => return fail(v, Failure::new("failed", e)),
    };
    match runner.run(&plan, &job.text) {
        Ok(text) => {
            v["text"] = json!(text);
            v
        }
        Err(e) => fail(v, e),
    }
}

/// A file's formatters, as found in the last ten seconds or now (their versions asked when the folder is
/// trusted).
fn found(sh: &Shared, settings: &Settings, a: &Asked, trusted: bool) -> Vec<Formatter> {
    let custom = settings.ide("formatters", None);
    let dir = a.file.parent().unwrap_or(&a.root);
    let ext = a.file.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let key = format!("{}\n{}\n{ext}\n{}\n{trusted}\n{custom}", a.root.display(), dir.display(), a.language);
    sh.kept.of::<Cache>().get_or(&key, || {
        let look = Look {
            root: &a.root,
            file: &a.file,
            language: &a.language,
            custom: &custom,
            path_var: std::env::var_os("PATH"),
            trusted,
        };
        let mut items = formatters::discover(&look);
        let gate = sh.kept.of::<Gate>();
        if trusted && let Ok(r) = runner(sh, settings, &gate) {
            formatters::versions(&mut items, &r, &a.root);
        }
        items
    })
}

/// What runs the formatters: the sandbox `tools.sandbox` allows (with no network, whatever `tools.sandboxNetwork`
/// says), the environment's harmless names.
fn runner<'a>(sh: &Shared, settings: &Settings, gate: &'a Gate) -> Result<Runner<'a>, String> {
    let mode = settings.string(&schema::SANDBOX).unwrap_or_else(|| "auto".into());
    let sandbox = if mode == "off" {
        None
    } else {
        match ca_tools::sandbox::find() {
            Ok(kind) => {
                let own = [sh.paths.config.clone(), sh.paths.data.clone()];
                let runtime = settings.env().get("XDG_RUNTIME_DIR").map(PathBuf::from);
                Some(ca_tools::sandbox::Sandbox::new(
                    kind,
                    false,
                    &[],
                    &[],
                    dirs::home_dir().as_deref(),
                    &own,
                    runtime.as_deref(),
                ))
            }
            Err(why) if mode == "on" => return Err(format!("tools.sandbox is on, but {why}.")),
            Err(_) => None,
        }
    };
    // The child's environment is the system's business, as the PATH is: only its harmless names are passed on, so
    // no key of the driver's reaches a formatter.
    Ok(Runner { sandbox, env: formatters::passed_env(std::env::vars_os()), gate })
}

/// The `prettier.*` settings for a language.
fn prettier(s: &Settings, lang: Option<&str>) -> PrettierSettings {
    let int = |v: Value, d: i64| v.as_i64().unwrap_or(d);
    let flag = |v: Value, d: bool| v.as_bool().unwrap_or(d);
    let word = |v: Value, d: &str| v.as_str().unwrap_or(d).to_string();
    PrettierSettings {
        print_width: int(s.ide("prettier.printWidth", lang), 80),
        tab_width: int(s.ide("prettier.tabWidth", lang), 2),
        use_tabs: flag(s.ide("prettier.useTabs", lang), false),
        semi: flag(s.ide("prettier.semi", lang), true),
        single_quote: flag(s.ide("prettier.singleQuote", lang), false),
        trailing_comma: word(s.ide("prettier.trailingComma", lang), "all"),
        bracket_spacing: flag(s.ide("prettier.bracketSpacing", lang), true),
        arrow_parens: word(s.ide("prettier.arrowParens", lang), "always"),
        end_of_line: word(s.ide("prettier.endOfLine", lang), "lf"),
    }
}
