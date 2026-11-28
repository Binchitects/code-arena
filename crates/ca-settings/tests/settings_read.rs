//! Every setting the code reads is in the schema, and every setting in the schema is read: so the Settings editor
//! shows all of them and none that does nothing. This test fails when code reads a key that is not declared, and
//! when a declared one is read nowhere.
//!
//! How a read is seen:
//!
//! - One of the IDE's settings ([`ca_settings::ide_schema`]) is read by its key, written as a literal in the call:
//!   in the window `settings::setting(ui, "editor.tabSize", …)` (and `now`, `flag`, `number`, `text`, `memo`), in
//!   the driver `settings.ide("git.autofetch", …)`. A key handed on in a variable is not seen: write the literal.
//! - One of the others ([`ca_settings::schema`]) is read through its own item: `schema::MODE`.
//!
//! The IDE's settings were declared before the parts that read them were written: those are listed in
//! [`NOT_READ_YET`]. A part that begins to read one may leave it there (the list is no one's to keep in step while
//! the parts are written side by side); a setting declared after this must be read, or be listed here with why.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ca_settings::{ide_schema, schema};

/// The calls that read one of the IDE's settings by its key.
const READERS: &[&str] = &[
    "settings::setting(",
    "settings::now(",
    "settings::flag(",
    "settings::number(",
    "settings::text(",
    "settings::memo(",
    ".ide(",
];

/// The IDE's settings whose reader is a part of the window still to be written (the editor's options, the
/// terminal's, source control, the debugger, the language servers…).
const NOT_READ_YET: &[&str] = &[
    "editor.fontSize",
    "editor.fontFamily",
    "editor.fontWeight",
    "editor.fontLigatures",
    "editor.lineHeight",
    "editor.letterSpacing",
    "editor.tabSize",
    "editor.insertSpaces",
    "editor.detectIndentation",
    "editor.trimAutoWhitespace",
    "editor.wordWrap",
    "editor.wordWrapColumn",
    "editor.rulers",
    "editor.lineNumbers",
    "editor.renderWhitespace",
    "editor.renderControlCharacters",
    "editor.renderLineHighlight",
    "editor.cursorStyle",
    "editor.cursorBlinking",
    "editor.cursorSmoothCaretAnimation",
    "workbench.editor.enablePreview",
    "explorer.autoReveal",
    "explorer.confirmDragAndDrop",
    "git.autofetch",
    "git.confirmSync",
    "git.enableStatusBarSync",
    "merge-conflict.codeLens.enabled",
    "merge-conflict.decorators.enabled",
    "git.rebaseWhenSync",
    "git.autoRepositoryDetection",
    "git.repositoryScanMaxDepth",
    "git.repositoryScanIgnoredFolders",
    "breadcrumbs.enabled",
    "breadcrumbs.filePath",
    "breadcrumbs.symbolPath",
    "editor.minimap.enabled",
    "editor.minimap.renderCharacters",
    "editor.minimap.side",
    "editor.stickyScroll.enabled",
    "editor.bracketPairColorization.enabled",
    "editor.guides.bracketPairs",
    "editor.guides.indentation",
    "editor.folding",
    "editor.glyphMargin",
    "editor.smoothScrolling",
    "editor.scrollBeyondLastLine",
    "editor.mouseWheelZoom",
    "editor.multiCursorModifier",
    "editor.autoClosingBrackets",
    "editor.autoClosingQuotes",
    "editor.autoIndent",
    "editor.linkedEditing",
    "editor.matchBrackets",
    "editor.occurrencesHighlight",
    "editor.selectionHighlight",
    "editor.emptySelectionClipboard",
    "editor.dragAndDrop",
    "editor.hover.enabled",
    "editor.hover.delay",
    "editor.parameterHints.enabled",
    "editor.quickSuggestions",
    "editor.suggestOnTriggerCharacters",
    "editor.acceptSuggestionOnEnter",
    "editor.tabCompletion",
    "editor.wordBasedSuggestions",
    "editor.snippetSuggestions",
    "editor.inlineSuggest.enabled",
    "editor.unicodeHighlight.ambiguousCharacters",
    "editor.copyWithSyntaxHighlighting",
    "editor.accessibilitySupport",
    "editor.defaultFormatter",
    "editor.formatOnSave",
    "editor.formatOnSaveMode",
    "editor.formatOnPaste",
    "codeArena.formatOnSaveTimeout",
    "files.hotExit",
    "files.autoSave",
    "files.autoSaveDelay",
    "files.trimTrailingWhitespace",
    "files.insertFinalNewline",
    "files.trimFinalNewlines",
    "files.eol",
    "files.associations",
    "window.autoDetectHighContrast",
    "window.autoDetectColorScheme",
    "workbench.preferredDarkColorTheme",
    "workbench.preferredLightColorTheme",
    "workbench.iconTheme",
    "diffEditor.renderSideBySide",
    "diffEditor.ignoreTrimWhitespace",
    "diffEditor.hideUnchangedRegions.enabled",
    "diffEditor.renderIndicators",
    "diffEditor.wordWrap",
    "diffEditor.maxComputationTime",
    "terminal.integrated.fontSize",
    "terminal.integrated.fontFamily",
    "terminal.integrated.lineHeight",
    "terminal.integrated.cursorStyle",
    "terminal.integrated.cursorBlinking",
    "terminal.integrated.scrollback",
    "terminal.integrated.minimumContrastRatio",
    "terminal.integrated.allowChords",
    "terminal.integrated.sendKeybindingsToShell",
    "terminal.integrated.commandsToSkipShell",
    "scm.defaultViewMode",
    "scm.compactFolders",
    "scm.diffDecorations",
    "debug.saveBeforeStart",
    "debug.internalConsoleOptions",
    "debug.openDebug",
    "debug.inlineValues",
    "debug.toolBarLocation",
    "codeArena.languageServers.enabled",
    "codeArena.languageServers.typescript.enabled",
    "codeArena.languageServers.pyright.enabled",
    "codeArena.languageServers.basedpyright.enabled",
    "codeArena.languageServers.pylsp.enabled",
    "codeArena.languageServers.gopls.enabled",
    "codeArena.languageServers.rust-analyzer.enabled",
    "codeArena.languageServers.clangd.enabled",
    "codeArena.languageServers.csharp-ls.enabled",
    "codeArena.languageServers.json.enabled",
    "codeArena.languageServers.css.enabled",
    "codeArena.languageServers.html.enabled",
    "codeArena.languageServers.yaml.enabled",
    "codeArena.languageServers.bash.enabled",
];

/// The other settings that no code names by its item, each with why it is declared all the same.
const NOT_NAMED: &[(&str, &str)] = &[
    // Read from each settings file by itself (the person's always, a project's only once it is trusted), so not
    // through the layers: `session::load_hooks`.
    ("hooks", "read from the files one by one, by trust"),
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).expect("read dir").flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "target" && n != "templates") {
                sources(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The keys a text reads through the readers, and the calls whose key is not a literal (by their line).
fn reads(text: &str) -> (Vec<String>, Vec<usize>) {
    let (mut keys, mut unseen) = (Vec::new(), Vec::new());
    for reader in READERS {
        let mut from = 0;
        while let Some(at) = text[from..].find(reader) {
            let call = from + at + reader.len();
            from = call;
            // The key is the call's first literal: nothing but the names before it (`ui, `), and no call ends first.
            let rest = &text[call..];
            let quote = rest.find('"');
            let ends = rest.find([')', ';', '{']).unwrap_or(rest.len());
            match quote.filter(|q| *q < ends) {
                Some(q) => {
                    let literal = &rest[q + 1..];
                    keys.push(literal[..literal.find('"').unwrap_or(literal.len())].to_string());
                }
                None => unseen.push(text[..call].lines().count()),
            }
        }
    }
    (keys, unseen)
}

/// The settings' own items in `schema.rs`, each with its key: `pub static MODE: Setting = Setting { key: "agent.mode"`.
fn items(schema_source: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for part in schema_source.split("pub static ").skip(1) {
        let Some((name, rest)) = part.split_once(": Setting") else { continue };
        let Some(key) = rest.split_once("key: \"").and_then(|(_, k)| k.split_once('"')).map(|(k, _)| k) else {
            continue;
        };
        out.push((name.trim().to_string(), key.to_string()));
    }
    out
}

/// Whether a text names one of the settings' items: `schema::MODE`, or `MODE` in what it uses of `schema`.
fn names(text: &str, item: &str) -> bool {
    let word = |at: usize| !text[at + item.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_');
    let direct = format!("schema::{item}");
    if text.match_indices(&direct).any(|(at, _)| word(at + "schema::".len())) {
        return true;
    }
    text.split("schema::{").skip(1).any(|list| {
        let list = &list[..list.find('}').unwrap_or(list.len())];
        list.split(',').any(|name| name.trim() == item)
    })
}

#[test]
fn every_setting_read_is_declared_and_every_setting_declared_is_read() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("crates");
    let mut files = Vec::new();
    sources(crates, &mut files);
    assert!(files.len() > 20, "found the sources");
    let schema_source = std::fs::read_to_string(crates.join("ca-settings/src/schema.rs")).expect("the schema");
    let items = items(&schema_source);
    assert_eq!(items.len(), schema::ALL.len(), "every setting of the schema is an item of its own, and is in ALL");
    let mut read = BTreeSet::new();
    let mut named = BTreeSet::new();
    let mut faults = Vec::new();
    for f in files {
        let rel = f.strip_prefix(crates).expect("in").to_string_lossy().replace('\\', "/");
        let text = std::fs::read_to_string(&f).expect("read");
        // The settings' own crate declares them and tests them: it reads none.
        if rel.starts_with("ca-settings/") {
            continue;
        }
        for (item, key) in &items {
            if names(&text, item) {
                named.insert(key.clone());
            }
        }
        // A file's tests come last: what they read is no part of the program's. Nor is what a comment shows.
        let program: String = text
            .split("\n#[cfg(test)]\nmod ")
            .next()
            .unwrap_or(&text)
            .lines()
            .map(|l| if l.trim_start().starts_with("//") { "" } else { l })
            .collect::<Vec<_>>()
            .join("\n");
        let (keys, unseen) = reads(&program);
        for line in unseen {
            faults
                .push(format!("{rel}:{line}: the setting's key is not written as a literal, so this read is not seen"));
        }
        for key in keys {
            if schema::find(&key).is_none() {
                faults.push(format!("{rel}: reads \"{key}\", which is not in the schema"));
            }
            read.insert(key);
        }
    }
    for s in ide_schema::ALL {
        if !read.contains(s.key) && !NOT_READ_YET.contains(&s.key) {
            faults.push(format!("\"{}\" is declared (ide_schema.rs) and read nowhere", s.key));
        }
    }
    for key in NOT_READ_YET {
        if !ide_schema::ALL.iter().any(|s| s.key == *key) {
            faults.push(format!("\"{key}\" is listed as not read yet, and is not one of the IDE's settings"));
        }
    }
    for (item, key) in &items {
        let excused = NOT_NAMED.iter().any(|(k, _)| k == key);
        if !named.contains(key) && !read.contains(key) && !excused {
            faults.push(format!("\"{key}\" is declared (schema::{item}) and read nowhere"));
        }
    }
    for (key, why) in NOT_NAMED {
        assert!(schema::ALL.iter().any(|s| s.key == *key) && !why.is_empty(), "{key} is excused and is no setting");
    }
    assert!(faults.is_empty(), "the schema and what the code reads differ:\n{}", faults.join("\n"));
}

#[test]
fn a_read_is_seen_by_its_literal_key() {
    let text = "let a = settings::setting(ui, \"editor.tabSize\", lang);\nlet b = app.settings.ide(\n    \"git.autofetch\",\n    None,\n);\nlet c = settings::flag(ui, key, None);\n";
    assert_eq!(reads(text), (vec!["editor.tabSize".to_string(), "git.autofetch".to_string()], vec![6]));
    assert!(names("use ca_settings::schema::{MODE, SHELL};", "SHELL") && names("&schema::MODE)", "MODE"));
    assert!(!names("schema::MODEL", "MODE") && !names("use ca_settings::schema::{MODEL};", "MODE"));
    assert_eq!(
        items("pub static MODE: Setting = Setting {\n    key: \"agent.mode\",\n"),
        [("MODE".to_string(), "agent.mode".to_string())]
    );
}
