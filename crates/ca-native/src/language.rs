//! A file's language: found, shown in the status bar, changed. As VS Code's: by `files.associations` (a pattern
//! with a `/` against the whole path, else against the file's name), then the file's name, then its extension,
//! then its first line (`#!/usr/bin/env python`, `<?php`); Plain Text when nothing says. Its id is VS Code's
//! (`rust`, `typescriptreact`, `shellscript`), the one a `[lang]` block of the settings names. Change Language
//! Mode (the status bar's item, Ctrl+K M) picks another for a file's tab, which lasts until the tab is closed.
//! What every part has (`register`, `take`) is said in `ext.rs`.

use std::collections::HashMap;
use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::Memo;
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;
use floem::{AnyView, IntoView};
use globset::{GlobBuilder, GlobMatcher};
use serde_json::{Value, json};
use syntect::parsing::SyntaxReference;

use crate::state::{Send, Ui};
use crate::widgets::quick;

/// VS Code's command that picks a file's language.
pub const CHANGE: &str = "workbench.action.editor.changeLanguageMode";

/// The language of what nothing else names.
pub const PLAIN: &str = "plaintext";

/// A language the window knows: VS Code's id and name, the files it is by name and by extension, the programs a
/// `#!` line runs it with, and a file name its grammar is found by (empty: drawn plain).
pub struct Language {
    pub id: &'static str,
    pub name: &'static str,
    names: &'static [&'static str],
    extensions: &'static [&'static str],
    programs: &'static [&'static str],
    grammar: &'static str,
}

const fn lang(
    id: &'static str,
    name: &'static str,
    names: &'static [&'static str],
    extensions: &'static [&'static str],
    programs: &'static [&'static str],
    grammar: &'static str,
) -> Language {
    Language { id, name, names, extensions, programs, grammar }
}

/// The languages, as VS Code has them built in (and a few its common extensions add), by name.
pub static LANGUAGES: &[Language] = &[
    lang("bat", "Batch", &[], &["bat", "cmd"], &[], "a.bat"),
    lang("c", "C", &[], &["c", "i"], &[], "a.c"),
    lang("csharp", "C#", &[], &["cs", "csx", "cake"], &[], "a.cs"),
    lang(
        "cpp",
        "C++",
        &[],
        &["cpp", "cppm", "cc", "ccm", "cxx", "c++", "hpp", "hh", "hxx", "h++", "h", "ii", "ino", "inl", "ipp", "tpp"],
        &[],
        "a.cpp",
    ),
    lang("clojure", "Clojure", &[], &["clj", "cljs", "cljc", "cljx", "clojure", "edn"], &[], "a.clj"),
    lang("cmake", "CMake", &["cmakelists.txt"], &["cmake"], &[], ""),
    lang("coffeescript", "CoffeeScript", &[], &["coffee", "cson", "iced"], &["coffee"], ""),
    lang("css", "CSS", &[], &["css"], &[], "a.css"),
    lang("dart", "Dart", &[], &["dart"], &[], ""),
    lang("diff", "Diff", &[], &["diff", "patch", "rej"], &[], "a.diff"),
    lang("dockerfile", "Docker", &["dockerfile", "containerfile"], &["dockerfile", "containerfile"], &[], ""),
    lang("fsharp", "F#", &[], &["fs", "fsi", "fsx", "fsscript"], &[], ""),
    lang("git-commit", "Git Commit Message", &["commit_editmsg", "merge_msg"], &[], &[], ""),
    lang("git-rebase", "Git Rebase Message", &["git-rebase-todo"], &[], &[], ""),
    lang("go", "Go", &[], &["go"], &[], "a.go"),
    lang("groovy", "Groovy", &["jenkinsfile"], &["groovy", "gvy", "gradle", "nf"], &["groovy"], "a.groovy"),
    lang("handlebars", "Handlebars", &[], &["handlebars", "hbs", "hjs"], &[], ""),
    lang("html", "HTML", &[], &["html", "htm", "shtml", "xhtml", "mdoc", "jsp", "asp", "aspx", "jshtm"], &[], "a.html"),
    lang("ignore", "Ignore", &[".gitignore", ".gitignore_global", ".npmignore", ".dockerignore"], &[], &[], ""),
    lang("ini", "Ini", &[], &["ini"], &[], ""),
    lang("java", "Java", &[], &["java", "jav"], &[], "a.java"),
    lang("javascript", "JavaScript", &["jakefile"], &["js", "es6", "mjs", "cjs", "pac"], &["node"], "a.js"),
    lang("javascriptreact", "JavaScript JSX", &[], &["jsx"], &[], "a.js"),
    lang(
        "json",
        "JSON",
        &[".arcconfig", ".watchmanconfig"],
        &["json", "bowerrc", "jscsrc", "webmanifest", "har"],
        &[],
        "a.json",
    ),
    lang(
        "jsonc",
        "JSON with Comments",
        &[
            "settings.json",
            "keybindings.json",
            "launch.json",
            "tasks.json",
            "extensions.json",
            "tsconfig.json",
            "jsconfig.json",
            "devcontainer.json",
            ".babelrc",
            ".eslintrc",
            ".eslintrc.json",
            ".jshintrc",
            ".jscsrc",
            ".swcrc",
        ],
        &["jsonc", "code-workspace", "code-snippets", "code-profile"],
        &[],
        "a.json",
    ),
    lang("julia", "Julia", &[], &["jl"], &["julia"], ""),
    lang("kotlin", "Kotlin", &[], &["kt", "kts"], &[], ""),
    lang("latex", "LaTeX", &[], &["tex", "ltx", "ctx"], &[], "a.tex"),
    lang("less", "Less", &[], &["less"], &[], ""),
    lang("log", "Log", &[], &["log"], &[], ""),
    lang("lua", "Lua", &[], &["lua"], &["lua"], "a.lua"),
    lang("makefile", "Makefile", &["makefile", "gnumakefile", "ocamlmakefile"], &["mak", "mk"], &["make"], "Makefile"),
    lang(
        "markdown",
        "Markdown",
        &[],
        &["md", "mkd", "mdwn", "mdown", "markdown", "markdn", "mdtxt", "mdtext", "workbook"],
        &[],
        "a.md",
    ),
    lang("objective-c", "Objective-C", &[], &["m"], &[], "a.m"),
    lang("objective-cpp", "Objective-C++", &[], &["mm"], &[], "a.mm"),
    lang("perl", "Perl", &[], &["pl", "pm", "pod", "t", "psgi"], &["perl"], "a.pl"),
    lang("php", "PHP", &[], &["php", "php4", "php5", "phtml", "ctp"], &["php"], "a.php"),
    lang("plaintext", "Plain Text", &[], &["txt"], &[], ""),
    lang("powershell", "PowerShell", &[], &["ps1", "psm1", "psd1", "pssc", "psrc"], &["pwsh", "powershell"], ""),
    lang(
        "properties",
        "Properties",
        &[],
        &[
            "properties",
            "cfg",
            "conf",
            "directory",
            "editorconfig",
            "gitattributes",
            "gitconfig",
            "gitmodules",
            "npmrc",
        ],
        &[],
        "",
    ),
    lang("pug", "Pug", &[], &["pug", "jade"], &[], ""),
    lang(
        "python",
        "Python",
        &["sconstruct", "sconscript"],
        &["py", "rpy", "pyw", "cpy", "gyp", "gypi", "pyi", "ipy", "pyt"],
        &["python"],
        "a.py",
    ),
    lang("r", "R", &[], &["r", "rhistory", "rprofile", "rt"], &["rscript"], "a.r"),
    lang("razor", "Razor", &[], &["cshtml", "razor"], &[], ""),
    lang(
        "ruby",
        "Ruby",
        &["rakefile", "gemfile", "guardfile", "podfile", "capfile", "fastfile", "appfile", "brewfile", "vagrantfile"],
        &["rb", "rbx", "rjs", "gemspec", "rake", "ru", "erb", "podspec", "rbi"],
        &["ruby"],
        "a.rb",
    ),
    lang("rust", "Rust", &[], &["rs"], &[], "a.rs"),
    lang("scss", "SCSS", &[], &["scss"], &[], ""),
    lang(
        "shellscript",
        "Shell Script",
        &[
            "apkbuild",
            "pkgbuild",
            ".envrc",
            ".hushlogin",
            "zshrc",
            "zshenv",
            "zlogin",
            "zprofile",
            "zlogout",
            "bashrc_apple_terminal",
            "zshrc_apple_terminal",
        ],
        &[
            "sh",
            "bash",
            "bashrc",
            "bash_aliases",
            "bash_profile",
            "bash_login",
            "ebuild",
            "eclass",
            "profile",
            "bash_logout",
            "xprofile",
            "zsh",
            "zshrc",
            "zprofile",
            "zlogin",
            "zlogout",
            "zshenv",
            "zsh-theme",
            "ksh",
            "csh",
            "tcsh",
            "fish",
        ],
        &["sh", "bash", "zsh", "ksh", "dash", "ash", "fish"],
        "a.sh",
    ),
    lang("sql", "SQL", &[], &["sql", "dsql"], &[], "a.sql"),
    lang("swift", "Swift", &[], &["swift"], &[], ""),
    lang("toml", "TOML", &["cargo.lock", "pipfile"], &["toml"], &[], ""),
    lang("typescript", "TypeScript", &[], &["ts", "cts", "mts"], &["deno", "ts-node", "tsx", "bun"], "a.ts"),
    lang("typescriptreact", "TypeScript JSX", &[], &["tsx"], &[], "a.ts"),
    lang("vb", "Visual Basic", &[], &["vb", "brs", "vbs", "bas", "vba"], &[], ""),
    lang("vue", "Vue", &[], &["vue"], &[], ""),
    lang(
        "xml",
        "XML",
        &[],
        &[
            "xml", "xsd", "ascx", "atom", "axml", "bpmn", "csl", "csproj", "dita", "dtd", "fsproj", "fxml", "iml",
            "isml", "mxml", "nuspec", "opml", "plist", "props", "rdf", "resx", "rss", "svg", "targets", "tld",
            "vbproj", "vcxproj", "wsdl", "xaml", "xlf", "xliff", "xsl", "xslt", "xul",
        ],
        &[],
        "a.xml",
    ),
    lang(
        "yaml",
        "YAML",
        &[".clang-format", ".clang-tidy", ".clangd"],
        &["yml", "yaml", "eyaml", "eyml", "cff"],
        &[],
        "a.yaml",
    ),
];

/// A language by its id.
pub fn find(id: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|l| l.id == id)
}

/// A language's name as the status bar says it: VS Code's, or its id when the window has none for it.
pub fn name(id: &str) -> &str {
    find(id).map_or(id, |l| l.name)
}

/// The grammar a language is drawn with; none: plain.
pub fn grammar(id: &str) -> Option<&'static SyntaxReference> {
    find(id).filter(|l| !l.grammar.is_empty()).and_then(|l| crate::highlight::syntax_for(l.grammar, ""))
}

/// `files.associations` made ready: the patterns with their languages, those against the whole path first. A
/// language the window does not know is left out.
#[derive(Default, Debug)]
pub struct Associations {
    said: Vec<(String, &'static str)>,
    by_path: Vec<(GlobMatcher, &'static str)>,
    by_name: Vec<(GlobMatcher, &'static str)>,
}

impl PartialEq for Associations {
    fn eq(&self, other: &Self) -> bool {
        self.said == other.said
    }
}

impl Associations {
    pub fn of(v: &Value) -> Associations {
        let mut a = Associations::default();
        let Some(map) = v.as_object() else { return a };
        for (pattern, id) in map {
            let Some(l) = id.as_str().and_then(find) else { continue };
            let Ok(glob) = GlobBuilder::new(pattern).literal_separator(true).case_insensitive(true).build() else {
                continue;
            };
            a.said.push((pattern.clone(), l.id));
            if pattern.contains('/') {
                a.by_path.push((glob.compile_matcher(), l.id));
            } else {
                a.by_name.push((glob.compile_matcher(), l.id));
            }
        }
        a
    }

    fn of_path(&self, path: &str, name: &str) -> Option<&'static str> {
        let path = path.strip_prefix("./").unwrap_or(path);
        let by_path = self.by_path.iter().find(|(g, _)| {
            // A pattern of the project's own folders (`src/*.inc`) is matched from the folder too.
            g.is_match(path) || (!path.starts_with('/') && g.is_match(format!("/{path}")))
        });
        by_path.or_else(|| self.by_name.iter().find(|(g, _)| g.is_match(name))).map(|(_, id)| *id)
    }
}

/// A file's language by its path and first line, as VS Code finds it: the associations, the file's name, its
/// extension (the longest that is one), its first line; Plain Text when none says.
/// The first line is asked for only when nothing before it says.
pub fn detect(associations: &Associations, path: &str, first_line: impl FnOnce() -> String) -> &'static str {
    if path.starts_with("user:") {
        // The person's settings, keybindings and snippets: JSON with comments.
        return "jsonc";
    }
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if let Some(id) = associations.of_path(path, name) {
        return id;
    }
    let lower = name.to_ascii_lowercase();
    if let Some(l) = LANGUAGES.iter().find(|l| l.names.contains(&lower.as_str())) {
        return l.id;
    }
    let by_extension = LANGUAGES
        .iter()
        .flat_map(|l| l.extensions.iter().map(move |e| (l.id, *e)))
        .filter(|(_, e)| lower.len() > e.len() && lower.ends_with(e) && lower[..lower.len() - e.len()].ends_with('.'))
        .max_by_key(|(_, e)| e.len());
    if let Some((id, _)) = by_extension {
        return id;
    }
    by_first_line(&first_line()).unwrap_or(PLAIN)
}

/// The language a file's first line says: the program of a `#!` line (`/usr/bin/env -S python3 -u`), or `<?php`,
/// `<?xml`.
pub fn by_first_line(line: &str) -> Option<&'static str> {
    let line = line.trim_start_matches('\u{feff}');
    if line.starts_with("<?php") {
        return Some("php");
    }
    if line.starts_with("<?xml") {
        return Some("xml");
    }
    let rest = line.strip_prefix("#!")?;
    let mut words = rest.split_whitespace();
    let mut program = words.next()?.rsplit('/').next()?;
    if program == "env" {
        program = words.find(|w| !w.starts_with('-') && !w.contains('='))?;
    }
    // `python3.12`, `ruby2.7`: the program without its version.
    let program = program.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    LANGUAGES.iter().find(|l| l.programs.contains(&program)).map(|l| l.id)
}

/// The window's languages: each tab's chosen one, and the associations as the settings have them.
#[derive(Clone, Copy)]
pub struct Languages {
    /// The language picked for a tab (by its path), until it is closed.
    pub chosen: RwSignal<HashMap<String, &'static str>>,
    pub associations: Memo<Rc<Associations>>,
    /// The language whose settings were asked to be configured: its block is made when the settings file comes.
    configure: RwSignal<Option<&'static str>>,
}

impl crate::ext::State for Languages {
    fn new(ui: Ui) -> Self {
        let said = crate::settings::memo(ui, "files.associations", || None);
        Languages {
            chosen: ui.scope.create_rw_signal(HashMap::new()),
            associations: ui.scope.create_memo(move |_| Rc::new(Associations::of(&said.get()))),
            configure: ui.scope.create_rw_signal(None),
        }
    }
}

/// A tab's first line, when it is short enough to be a `#!` line (a minified file's one line is not copied).
fn first_line(ui: Ui, path: &str) -> String {
    let Some(tab) = ui.tab(path) else { return String::new() };
    let text = tab.doc.rope_text();
    if text.line_end_offset(0, true) > 1000 {
        return String::new();
    }
    text.line_content(0).to_string()
}

/// A tab's language (by its path), followed: told again when it is picked, or the associations change.
pub fn id(ui: Ui, path: &str) -> &'static str {
    let s = ui.state::<Languages>();
    if let Some(id) = s.chosen.with(|c| c.get(path).copied()) {
        return id;
    }
    s.associations.with(|a| detect(a, path, || first_line(ui, path)))
}

/// A tab's language now, not followed: for a command or a key.
pub fn now(ui: Ui, path: &str) -> &'static str {
    let s = ui.state::<Languages>();
    if let Some(id) = s.chosen.with_untracked(|c| c.get(path).copied()) {
        return id;
    }
    s.associations.with_untracked(|a| detect(a, path, || first_line(ui, path)))
}

/// A tab's language now, as the settings' calls take it.
pub fn of(ui: Ui, path: &str) -> Option<&'static str> {
    Some(now(ui, path))
}

/// The grammar a tab is drawn with, followed. A file whose language is neither picked nor associated keeps the
/// grammar its name or first line finds (`.scala`, `.erb`), else its language's; otherwise its language's alone.
pub fn syntax(ui: Ui, path: &str) -> Option<&'static SyntaxReference> {
    let id = id(ui, path);
    let s = ui.state::<Languages>();
    let picked = s.chosen.with(|c| c.contains_key(path));
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if picked || s.associations.with(|a| a.of_path(path, name).is_some()) {
        return grammar(id);
    }
    crate::highlight::syntax_for(path, &first_line(ui, path)).or_else(|| grammar(id))
}

/// The tab shown now, when its language may be changed: a file's text.
fn file_shown(ui: Ui) -> Option<String> {
    let path = ui.active.get_untracked()?;
    ui.tab(&path).filter(|t| t.is_file()).map(|t| t.path)
}

/// Picks a language for `path`: none goes back to the one found.
pub fn choose(ui: Ui, path: &str, id: Option<&'static str>) {
    let chosen = ui.state::<Languages>().chosen;
    if chosen.with_untracked(|c| c.get(path).copied()) == id {
        return;
    }
    chosen.update(|c| match id {
        Some(id) => {
            c.insert(path.to_string(), id);
        }
        None => {
            c.remove(path);
        }
    });
}

/// The rows of Change Language Mode for a file of `current`: Auto Detect, Configure its settings, then every
/// language by name with its id, the current one said so.
pub fn rows(current: &str) -> Vec<quick::Item> {
    let mut rows = vec![
        quick::item("auto", "Auto Detect"),
        quick::item("configure", &format!("Configure '{}' language based settings...", name(current))),
    ];
    let mut all: Vec<&Language> = LANGUAGES.iter().collect();
    all.sort_by_key(|l| l.name.to_lowercase());
    rows.extend(all.into_iter().map(|l| {
        let description = if l.id == current { format!("({}) - current", l.id) } else { format!("({})", l.id) };
        quick::item(l.id, l.name).description(&description).group("languages (identifier)")
    }));
    rows
}

/// Change Language Mode: the picker for the file shown.
pub fn change(ui: Ui, send: &Send) {
    let Some(path) = file_shown(ui) else { return };
    let current = now(ui, &path);
    let send = send.clone();
    quick::pick("").back_to_editor(ui).placeholder("Select Language Mode").items(rows(current)).active(current).open(ui, move |picked| {
        let Some(picked) = picked else { return };
        match picked.id.as_str() {
            "auto" => choose(ui, &path, None),
            "configure" => configure(ui, &send, current),
            id => choose(ui, &path, find(id).map(|l| l.id)),
        }
    });
}

/// The person's `settings.json` with an empty block for `lang` (`"[rust]": {}`) after its other settings; none
/// when it has one already, or is not an object of settings.
pub fn with_block(text: &str, lang: &str) -> Option<String> {
    let block = format!("[{lang}]");
    let parsed = if text.trim().is_empty() { json!({}) } else { ca_settings::jsonc::parse(text).ok()? };
    if parsed.as_object()?.contains_key(&block) {
        return None;
    }
    // A setting kept in the block and taken out again: the block is made where `edited` makes one, empty.
    let made = ca_settings::settings::edited(text, Some(lang), "editor.tabSize", Some(&json!(4))).ok()?;
    ca_settings::settings::edited(&made, Some(lang), "editor.tabSize", None).ok()
}

/// Configure '<lang>' language based settings: the person's `settings.json` asked for, to be given an empty block
/// for the language (when it has none) and opened.
fn configure(ui: Ui, send: &Send, lang: &'static str) {
    let name = "settings.json";
    let tab = ui.tab(&crate::settings::user_tab(name));
    match tab {
        // Its own changes are kept: the block is made in them.
        Some(tab) if tab.dirty.get_untracked() => {
            if let Some(text) = with_block(&tab.text(), lang) {
                let all = tab.doc.rope_text().len();
                tab.doc.edit_single(Selection::region(0, all), &text, EditType::Other);
            }
            crate::settings::open_user(ui, send, name);
        }
        _ => {
            ui.state::<Languages>().configure.set(Some(lang));
            send(json!({"type": "user_file", "name": name}));
        }
    }
}

/// The status bar's item: the language of the tab shown, which picks another for a file's text.
fn status(ui: Ui, send: Send) -> AnyView {
    let t = ui.colors;
    let shown = ui.scope.create_memo(move |_| {
        let path = ui.active.get()?;
        let (text, file) = ui.tabs.with(|tabs| {
            let tab = tabs.iter().find(|t| t.path == path)?;
            Some((tab.kind.is_none() || tab.kind == Some(crate::settings::USER), tab.is_file()))
        })?;
        text.then(|| (name(id(ui, &path)).to_string(), file))
    });
    let item = label(move || shown.with(|s| s.as_ref().map(|(n, _)| n.clone()).unwrap_or_default()))
        .on_click_stop(move |_| {
            if shown.with_untracked(|s| s.as_ref().is_some_and(|(_, f)| *f)) {
                change(ui, &send);
            }
        })
        .style(move |s| {
            let file = shown.with(|s| s.as_ref().map(|(_, f)| *f));
            crate::statusbar::item(t, s)
                .apply_if(file.is_none(), |s| s.hide())
                // A tab that is not a file's text says its language, and is not lit nor clicked.
                .apply_if(file == Some(false), |s| s.hover(|s| s.background(floem::peniko::Color::TRANSPARENT)))
                .apply_if(file == Some(true), |s| s.cursor(floem::style::CursorStyle::Pointer))
        });
    floem::views::tooltip(item, || label(|| "Select Language Mode")).into_any()
}

/// What it adds to the window's registries, once at start: the status bar's language, Change Language Mode, and
/// the chosen languages let go of as their tabs close.
pub fn register(ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    let s = ui.state::<Languages>();
    crate::statusbar::add("status.language", crate::statusbar::Side::Right, 240, status);
    add(command(CHANGE, "", "Change Language Mode", change).keys("Ctrl+K M").enabled(|ui| file_shown(ui).is_some()));
    ui.scope.create_effect(move |_| {
        let open: Vec<String> = ui.tabs.with(|t| t.iter().map(|t| t.path.clone()).collect());
        if s.chosen.with_untracked(|c| c.keys().any(|p| !open.contains(p))) {
            s.chosen.update(|c| c.retain(|p, _| open.contains(p)));
        }
    });
}

/// The session's messages that are this part's, taken in: the person's `settings.json` asked for to configure a
/// language, given its block and saved, then opened.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    if m["type"] != "user_file" || m["name"] != "settings.json" {
        return false;
    }
    let configure = ui.state::<Languages>().configure;
    let Some(lang) = configure.get_untracked() else { return false };
    configure.set(None);
    if m["error"].is_null()
        && let Some(text) = with_block(m["text"].as_str().unwrap_or(""), lang)
    {
        send(json!({"type": "save_user", "name": "settings.json", "text": text}));
    }
    crate::settings::open_user(ui, send, "settings.json");
    false
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn found(path: &str, first: &str) -> &'static str {
        detect(&Associations::default(), path, || first.to_string())
    }

    #[test]
    fn a_file_is_known_by_its_name_then_its_extension_then_its_first_line() {
        assert_eq!(found("src/main.rs", ""), "rust");
        assert_eq!(found("web/App.TSX", ""), "typescriptreact");
        assert_eq!(found("a/types.d.ts", ""), "typescript");
        assert_eq!(found("Makefile", ""), "makefile");
        assert_eq!(found("docker/Dockerfile", ""), "dockerfile");
        assert_eq!(found(".vscode/settings.json", ""), "jsonc");
        assert_eq!(found("package.json", ""), "json");
        assert_eq!(found("user:keybindings.json", ""), "jsonc");
        assert_eq!(found("lib/a.h", ""), "cpp");
        assert_eq!(found(".gitignore", ""), "ignore");
        assert_eq!(found("bin/tool", "#!/usr/bin/env python3"), "python");
        assert_eq!(found("bin/tool", "#!/usr/bin/env -S node --no-warnings"), "javascript");
        assert_eq!(found("bin/tool", "#!/bin/bash -e"), "shellscript");
        assert_eq!(found("index", "<?php echo 1;"), "php");
        assert_eq!(found("notes", "just words"), PLAIN);
        // The extension wins over the first line.
        assert_eq!(found("a.txt", "#!/bin/sh"), PLAIN);
    }

    #[test]
    fn the_associations_win_those_of_a_path_over_those_of_a_name() {
        let a = Associations::of(&json!({"*.inc": "php", "src/**/*.inc": "c", "*.conf": "no-such", "[": "rust"}));
        assert_eq!(detect(&a, "lib/a.inc", String::new), "php");
        assert_eq!(detect(&a, "src/x/a.inc", String::new), "c");
        assert_eq!(detect(&a, "/w/src/a.INC", String::new), "php", "an absolute path is not under the project's src");
        assert_eq!(detect(&a, "a.conf", String::new), "properties", "a language not known is left out");
        assert_eq!(Associations::of(&json!({"*.inc": "php"})), Associations::of(&json!({"*.inc": "php"})));
    }

    #[test]
    fn every_language_has_a_name_and_one_with_a_grammar_finds_it() {
        for l in LANGUAGES {
            assert!(!l.name.is_empty());
            if !l.grammar.is_empty() {
                assert!(grammar(l.id).is_some(), "{} has its grammar", l.id);
            }
        }
        assert_eq!(name("rust"), "Rust");
        assert_eq!(name("shellscript"), "Shell Script");
        assert_eq!(name("mine"), "mine");
        assert!(grammar(PLAIN).is_none());
    }

    #[test]
    fn the_picker_lists_every_language_with_its_id_and_says_the_current_one() {
        let rows = rows("rust");
        assert_eq!(rows[0].label, "Auto Detect");
        assert_eq!(rows[1].label, "Configure 'Rust' language based settings...");
        assert_eq!(rows.len(), LANGUAGES.len() + 2);
        let rust = rows.iter().find(|r| r.id == "rust").expect("listed");
        assert_eq!(rust.description, "(rust) - current");
        assert_eq!(rows.iter().find(|r| r.id == "go").expect("listed").description, "(go)");
        let names: Vec<&str> = rows[2..].iter().map(|r| r.label.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_by_key(|n| n.to_lowercase());
        assert_eq!(names, sorted);
    }

    #[test]
    fn configuring_a_language_adds_its_empty_block_once() {
        let made = with_block("{\n    // mine\n    \"editor.tabSize\": 2\n}\n", "rust").expect("made");
        let v = ca_settings::jsonc::parse(&made).expect("still JSON");
        assert_eq!(v["[rust]"], json!({}));
        assert_eq!(v["editor.tabSize"], json!(2));
        assert!(made.contains("// mine"));
        assert!(with_block(&made, "rust").is_none(), "there already");
        let empty = with_block("", "python").expect("made");
        assert_eq!(ca_settings::jsonc::parse(&empty).expect("JSON"), json!({"[python]": {}}));
        assert!(with_block("[1]", "go").is_none());
    }

    fn window() -> (Ui, Send, Rc<RefCell<Vec<Value>>>) {
        let ui = Ui::new();
        let told = Rc::new(RefCell::new(Vec::new()));
        let send: Send = {
            let to = told.clone();
            Rc::new(move |v: Value| to.borrow_mut().push(v))
        };
        crate::editor::groups::register(ui, &send);
        crate::settings::register(ui, &send);
        register(ui, &send);
        (ui, send, told)
    }

    #[test]
    fn a_language_picked_lasts_until_its_tab_closes() {
        let (ui, send, _) = window();
        ui.take(&json!({"type": "file", "path": "a.txt", "text": "x\n", "modified": 1}), &send);
        ui.open("a.txt", &send);
        assert_eq!(id(ui, "a.txt"), PLAIN);
        assert!(crate::commands::find(CHANGE).expect("there").may_run(ui));
        choose(ui, "a.txt", Some("rust"));
        assert_eq!(id(ui, "a.txt"), "rust");
        assert!(grammar("rust").is_some() && syntax(ui, "a.txt").is_some());
        choose(ui, "a.txt", None);
        assert_eq!(id(ui, "a.txt"), PLAIN);
        choose(ui, "a.txt", Some("go"));
        ui.close("a.txt");
        // A file neither picked nor associated keeps the grammar its name finds.
        for path in ["a.erb", "a.properties", "a.jsp", "a.scala", "a.ts", "a.h", "Makefile"] {
            ui.take(&json!({"type": "file", "path": path, "text": "x\n", "modified": 1}), &send);
            let by_name = crate::highlight::syntax_for(path, "x").map(|s| s.name.clone());
            assert_eq!(syntax(ui, path).map(|s| s.name.clone()), by_name, "{path}");
        }
        assert!(ui.state::<Languages>().chosen.with_untracked(|c| c.is_empty()));
    }

    #[test]
    fn configure_saves_the_block_and_opens_the_settings() {
        let (ui, send, told) = window();
        configure(ui, &send, "rust");
        assert_eq!(told.borrow().last().expect("asked")["type"], "user_file");
        let file = json!({"type": "user_file", "name": "settings.json", "text": "{\n}\n", "modified": 1});
        take(ui, &file, &send);
        let saved = told.borrow().iter().find(|m| m["type"] == "save_user").cloned().expect("saved");
        assert_eq!(
            ca_settings::jsonc::parse(saved["text"].as_str().unwrap_or("")).expect("JSON"),
            json!({"[rust]": {}})
        );
        assert!(ui.tab(&crate::settings::user_tab("settings.json")).is_some());
        // Said again unasked: nothing more is made.
        let n = told.borrow().len();
        take(ui, &file, &send);
        assert_eq!(told.borrow().len(), n);
    }
}
