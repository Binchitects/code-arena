//! The editor's keys: its own keymap's, and the workbench's that are about the editor.

use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::Editor;
use floem::views::editor::command::CommandExecuted;
use floem::views::editor::core::mode::Mode;
use floem::views::editor::keypress::KeypressMap;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::editor::keypress::press::KeyPress;

use super::find::{find_show, find_step};
use super::tell;
use crate::state::Ui;

/// A key as the editor's own keymap has it (what `default_key_handler` does, with the keymap made once).
pub(super) fn default_key(
    map: &KeypressMap,
    editor: RwSignal<Editor>,
    press: &KeyPress,
    mods: Modifiers,
) -> CommandExecuted {
    let command = map.keymaps.get(press).or_else(|| {
        let insert = editor.with_untracked(|e| e.cursor.with_untracked(|c| c.get_mode())) == Mode::Insert;
        let mut press = press.clone();
        press.mods.set(Modifiers::SHIFT, false);
        if insert { map.keymaps.get(&press) } else { None }
    });
    match command {
        Some(command) => editor.with_untracked(|e| e.doc().run_command(e, command, Some(1), mods)),
        None => CommandExecuted::No,
    }
}

/// The workbench's keys that are about the editor, taken by the editor itself (a key it is given goes no further):
/// Ctrl+F finds, F3 and Shift+F3 go to the next and the previous match, Ctrl+S saves, Ctrl+W closes the tab, Escape
/// closes the find widget.
pub(super) fn editor_key(ui: Ui, press: &KeyPress, mods: Modifiers) -> bool {
    let KeyInput::Keyboard(key, _) = &press.key else { return false };
    let shift = mods.contains(Modifiers::SHIFT);
    let ctrl = mods.contains(Modifiers::CONTROL) && !mods.contains(Modifiers::ALT);
    match key {
        Key::Character(c) if ctrl && !shift => match c.to_lowercase().as_str() {
            "f" => find_show(ui),
            "s" => tell(ui, |send| ui.save(send)),
            "w" => {
                if let Some(p) = ui.active.get_untracked() {
                    ui.close(&p);
                }
            }
            _ => return false,
        },
        Key::Named(NamedKey::F3) => find_step(ui, !shift),
        Key::Named(NamedKey::Escape) if ui.find.get_untracked() => ui.find.set(false),
        _ => return false,
    }
    true
}
