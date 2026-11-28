//! Which keys pressed where another part has the keys are the workbench's all the same: the terminal sends what it
//! is given to its shell, and an editor has a keymap of its own, so each asks here before it keeps a key.
//!
//! - `terminal(key, mods)`: true when the workbench gets a key pressed in the terminal, not the shell (VS Code's
//!   `terminal.integrated.commandsToSkipShell`).
//! - `editor(key, mods)`: true when the workbench is given a key the editor's own keymap did not take.
//!
//! What is here is the window's first list: the keys of the commands it began with.

use floem::keyboard::{Key, Modifiers, NamedKey};

/// Whether a key pressed in the terminal is the workbench's: Ctrl with B, J, `, P or W (the side bar, the panel, the
/// terminal itself, Quick Open and the palette, closing an editor), and F1 (the palette).
pub fn terminal(key: &Key, m: Modifiers) -> bool {
    matches!(key, Key::Named(NamedKey::F1))
        || matches!(key, Key::Character(c)
        if m.contains(Modifiers::CONTROL) && matches!(c.to_lowercase().as_str(), "b" | "j" | "`" | "~" | "p" | "w"))
}

/// Whether a key the editor's keymap did not take goes to the workbench: any with Ctrl or Alt held, and the
/// function keys (F1 the palette).
pub fn editor(key: &Key, m: Modifiers) -> bool {
    m.intersects(Modifiers::CONTROL | Modifiers::ALT)
        || matches!(
            key,
            Key::Named(
                NamedKey::F1
                    | NamedKey::F2
                    | NamedKey::F3
                    | NamedKey::F4
                    | NamedKey::F5
                    | NamedKey::F6
                    | NamedKey::F7
                    | NamedKey::F8
                    | NamedKey::F9
                    | NamedKey::F10
                    | NamedKey::F11
                    | NamedKey::F12
            )
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(c: &str) -> Key {
        Key::Character(c.into())
    }

    #[test]
    fn the_workbenchs_keys_are_not_the_shells_nor_the_editors() {
        for c in ["b", "J", "`", "~", "p", "W"] {
            assert!(terminal(&k(c), Modifiers::CONTROL), "{c}");
        }
        assert!(terminal(&k("p"), Modifiers::CONTROL | Modifiers::SHIFT));
        // The shell's own: Ctrl+C, Ctrl+R, a letter typed.
        assert!(!terminal(&k("c"), Modifiers::CONTROL));
        assert!(!terminal(&k("r"), Modifiers::CONTROL));
        assert!(!terminal(&k("b"), Modifiers::empty()));
        assert!(editor(&k("b"), Modifiers::CONTROL));
        assert!(editor(&k("i"), Modifiers::CONTROL | Modifiers::ALT));
        assert!(!editor(&k("b"), Modifiers::SHIFT));
        // F1, the palette, wherever the keys are.
        assert!(editor(&Key::Named(NamedKey::F1), Modifiers::empty()));
        assert!(terminal(&Key::Named(NamedKey::F1), Modifiers::empty()));
        assert!(!terminal(&Key::Named(NamedKey::F2), Modifiers::empty()));
    }
}
