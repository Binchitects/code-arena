//! The default keys that are not a command's own: VS Code's for the workbench, each with its `when`, and the old
//! Code Arena's other keys for the commands a browser keeps the keys of (Ctrl+W, Ctrl+Shift+T, Ctrl+1 … Ctrl+9:
//! Ctrl+Alt+W, Ctrl+Alt+Shift+T, Alt+1 … Alt+9), which the native window keeps so the hands know them.
//!
//! These come first, before the commands' own keys (`commands::Command::keys`): a command's own key wins over a
//! rule here of the same key, and a rule here binds only what no command's key does where its `when` holds. A rule
//! of a command the window does not have is never in force (a later part that adds the command finds its keys
//! here). Keys are written for Linux and Windows; on macOS Ctrl is Cmd, unless a rule gives its own Mac key.

use serde_json::Value;

use super::{Os, Rule, Source, for_mac, when};
use crate::commands::parse_keys;
use crate::state::{Send, Ui};

/// The table: keys, command, `when`, and the Mac's own keys where they are not Ctrl as Cmd.
const TABLE: &[(&str, &str, &str, Option<&str>)] = &[
    // The workbench (VS Code's default keybindings).
    ("f1", "workbench.action.showCommands", "", None),
    ("ctrl+shift+p", "workbench.action.showCommands", "", None),
    ("ctrl+p", "workbench.action.quickOpen", "", None),
    ("ctrl+e", "workbench.action.quickOpen", "", None),
    ("ctrl+b", "workbench.action.toggleSidebarVisibility", "", None),
    ("ctrl+alt+b", "workbench.action.toggleAuxiliaryBar", "", None),
    ("ctrl+j", "workbench.action.togglePanel", "", None),
    ("ctrl+shift+e", "workbench.view.explorer", "", None),
    ("ctrl+shift+f", "workbench.action.findInFiles", "", None),
    ("ctrl+shift+h", "workbench.action.replaceInFiles", "", None),
    ("ctrl+shift+g", "workbench.view.scm", "", None),
    ("ctrl+shift+d", "workbench.view.debug", "", None),
    ("ctrl+shift+x", "workbench.view.extensions", "", None),
    ("ctrl+shift+m", "workbench.actions.view.problems", "", None),
    ("ctrl+shift+u", "workbench.action.output.toggleOutput", "", None),
    ("ctrl+shift+y", "workbench.debug.action.toggleRepl", "", None),
    ("ctrl+`", "workbench.action.terminal.toggleTerminal", "", Some("ctrl+`")),
    ("ctrl+shift+`", "workbench.action.terminal.new", "", Some("ctrl+shift+`")),
    ("ctrl+shift+5", "workbench.action.terminal.split", "terminalFocus", None),
    ("ctrl+pagedown", "workbench.action.terminal.focusNext", "terminalFocus", None),
    ("ctrl+pageup", "workbench.action.terminal.focusPrevious", "terminalFocus", None),
    ("ctrl+shift+c", "workbench.action.terminal.copySelection", "terminalFocus", None),
    ("ctrl+shift+v", "workbench.action.terminal.paste", "terminalFocus", None),
    ("ctrl+k ctrl+t", "workbench.action.selectTheme", "", None),
    ("ctrl+k ctrl+s", "workbench.action.openGlobalKeybindings", "", None),
    ("ctrl+,", "workbench.action.openSettings", "", None),
    ("ctrl+k ctrl+o", "workbench.action.files.openFolder", "", None),
    ("ctrl+r", "workbench.action.openRecent", "", None),
    ("ctrl+n", "workbench.action.files.newUntitledFile", "", None),
    ("ctrl+s", "workbench.action.files.save", "", None),
    ("ctrl+shift+s", "workbench.action.files.saveAs", "", None),
    ("ctrl+k s", "workbench.action.files.saveAll", "", None),
    ("ctrl+w", "workbench.action.closeActiveEditor", "", None),
    ("ctrl+f4", "workbench.action.closeActiveEditor", "", None),
    ("ctrl+k ctrl+w", "workbench.action.closeAllEditors", "", None),
    ("ctrl+k w", "workbench.action.closeEditorsInGroup", "", None),
    ("ctrl+k u", "workbench.action.closeUnmodifiedEditors", "", None),
    ("ctrl+shift+t", "workbench.action.reopenClosedEditor", "", None),
    ("ctrl+pagedown", "workbench.action.nextEditor", "", Some("cmd+alt+right")),
    ("ctrl+pageup", "workbench.action.previousEditor", "", Some("cmd+alt+left")),
    ("ctrl+tab", "workbench.action.quickOpenPreviousRecentlyUsedEditorInGroup", "", Some("ctrl+tab")),
    ("ctrl+\\", "workbench.action.splitEditor", "", None),
    ("ctrl+1", "workbench.action.focusFirstEditorGroup", "", None),
    ("ctrl+2", "workbench.action.focusSecondEditorGroup", "", None),
    ("ctrl+3", "workbench.action.focusThirdEditorGroup", "", None),
    ("ctrl+k ctrl+left", "workbench.action.focusLeftGroup", "", None),
    ("ctrl+k ctrl+right", "workbench.action.focusRightGroup", "", None),
    ("ctrl+alt+-", "workbench.action.navigateBack", "", Some("ctrl+-")),
    ("ctrl+shift+-", "workbench.action.navigateForward", "", Some("ctrl+shift+-")),
    ("ctrl+g", "workbench.action.gotoLine", "", Some("ctrl+g")),
    ("ctrl+shift+o", "workbench.action.gotoSymbol", "", None),
    ("ctrl+t", "workbench.action.showAllSymbols", "", None),
    ("ctrl+=", "workbench.action.zoomIn", "", None),
    ("ctrl+-", "workbench.action.zoomOut", "", None),
    ("ctrl+numpad0", "workbench.action.zoomReset", "", None),
    ("f11", "workbench.action.toggleFullScreen", "", Some("ctrl+cmd+f")),
    ("ctrl+k z", "workbench.action.toggleZenMode", "", None),
    ("ctrl+k m", "workbench.action.editor.changeLanguageMode", "", None),
    ("ctrl+k ctrl+h", "workbench.action.output.show.extension-output", "", None),
    ("ctrl+shift+n", "workbench.action.newWindow", "", None),
    ("ctrl+shift+w", "workbench.action.closeWindow", "", None),
    ("ctrl+q", "workbench.action.quit", "", None),
    ("ctrl+alt+i", "workbench.action.chat.open", "", Some("ctrl+cmd+i")),
    ("f5", "workbench.action.debug.start", "!inDebugMode", None),
    ("ctrl+f5", "workbench.action.debug.run", "!inDebugMode", None),
    ("f5", "workbench.action.debug.continue", "inDebugMode && debugState == 'stopped'", None),
    ("shift+f5", "workbench.action.debug.stop", "inDebugMode", None),
    ("ctrl+shift+f5", "workbench.action.debug.restart", "inDebugMode", None),
    ("f10", "workbench.action.debug.stepOver", "inDebugMode && debugState == 'stopped'", None),
    ("f11", "workbench.action.debug.stepInto", "inDebugMode && debugState == 'stopped'", None),
    ("shift+f11", "workbench.action.debug.stepOut", "inDebugMode && debugState == 'stopped'", None),
    ("f9", "editor.debug.action.toggleBreakpoint", "editorTextFocus", None),
    ("f8", "editor.action.marker.nextInFiles", "editorFocus", None),
    ("shift+f8", "editor.action.marker.prevInFiles", "editorFocus", None),
    ("ctrl+shift+b", "workbench.action.tasks.build", "", None),
    // The editor's (in force where a file's editor has the keys).
    ("ctrl+f", "actions.find", "editorFocus", None),
    ("ctrl+h", "editor.action.startFindReplaceAction", "editorFocus", None),
    ("ctrl+shift+i", "editor.action.formatDocument", "editorTextFocus", None),
    ("ctrl+k ctrl+f", "editor.action.formatSelection", "editorTextFocus", None),
    ("ctrl+.", "editor.action.quickFix", "editorTextFocus", None),
    ("f12", "editor.action.revealDefinition", "editorTextFocus", None),
    ("shift+f12", "editor.action.goToReferences", "editorTextFocus", None),
    ("f2", "editor.action.rename", "editorTextFocus", None),
    ("ctrl+k ctrl+i", "editor.action.showHover", "editorTextFocus", None),
    ("ctrl+space", "editor.action.triggerSuggest", "editorTextFocus", None),
    ("ctrl+shift+space", "editor.action.triggerParameterHints", "editorTextFocus", None),
    ("ctrl+shift+[", "editor.fold", "editorTextFocus", Some("alt+cmd+[")),
    ("ctrl+shift+]", "editor.unfold", "editorTextFocus", Some("alt+cmd+]")),
    ("alt+z", "editor.action.toggleWordWrap", "", None),
    // The Explorer's (where its tree has the keys).
    ("f2", "renameFile", "filesExplorerFocus && !inputFocus", Some("enter")),
    ("delete", "deleteFile", "filesExplorerFocus && !inputFocus", Some("cmd+backspace")),
    ("ctrl+c", "filesExplorer.copy", "filesExplorerFocus && !inputFocus", None),
    ("ctrl+x", "filesExplorer.cut", "filesExplorerFocus && !inputFocus", None),
    ("ctrl+v", "filesExplorer.paste", "filesExplorerFocus && !inputFocus", None),
    // The old Code Arena's keys beside the ones a browser takes.
    ("ctrl+alt+w", "workbench.action.closeActiveEditor", "", None),
    ("ctrl+k ctrl+alt+w", "workbench.action.closeAllEditors", "", None),
    ("ctrl+alt+shift+t", "workbench.action.reopenClosedEditor", "", None),
    ("ctrl+alt+pagedown", "workbench.action.nextEditor", "", None),
    ("ctrl+alt+pageup", "workbench.action.previousEditor", "", None),
    ("alt+1", "workbench.action.openEditorAtIndex1", "", None),
    ("alt+2", "workbench.action.openEditorAtIndex2", "", None),
    ("alt+3", "workbench.action.openEditorAtIndex3", "", None),
    ("alt+4", "workbench.action.openEditorAtIndex4", "", None),
    ("alt+5", "workbench.action.openEditorAtIndex5", "", None),
    ("alt+6", "workbench.action.openEditorAtIndex6", "", None),
    ("alt+7", "workbench.action.openEditorAtIndex7", "", None),
    ("alt+8", "workbench.action.openEditorAtIndex8", "", None),
    ("alt+9", "workbench.action.lastEditorInGroup", "", None),
];

/// The table's rules, for the system the window runs on.
pub fn rules(os: Os) -> Vec<Rule> {
    TABLE
        .iter()
        .filter_map(|(keys, command, w, mac)| {
            let keys = match (os, mac) {
                (Os::Mac, Some(own)) => parse_keys(own)?,
                (Os::Mac, None) => for_mac(&parse_keys(keys)?),
                _ => parse_keys(keys)?,
            };
            let when = if w.is_empty() { None } else { Some(std::rc::Rc::new(when::parse(w).ok()?)) };
            Some(Rule {
                keys,
                command: (*command).into(),
                when,
                when_text: (*w).into(),
                source: Source::Default,
                written: None,
            })
        })
        .collect()
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_rule_is_read_on_every_system() {
        for os in [Os::Linux, Os::Mac, Os::Windows] {
            assert_eq!(rules(os).len(), TABLE.len(), "{os:?}");
        }
        let mac = rules(Os::Mac);
        let palette = mac.iter().find(|r| r.command == "workbench.action.showCommands" && r.keys.len() == 1);
        assert!(palette.is_some_and(|r| r.keys[0].key == "f1"));
        let quick = mac.iter().find(|r| r.command == "workbench.action.quickOpen").expect("quick open");
        assert_eq!(super::super::label(&quick.keys, Os::Mac), "⌘P");
        let back = mac.iter().find(|r| r.command == "workbench.action.navigateBack").expect("back");
        assert_eq!(super::super::label(&back.keys, Os::Mac), "⌃-");
    }
}
