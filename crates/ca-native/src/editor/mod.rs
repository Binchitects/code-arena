//! The editor group, as VS Code's: its 35 px tabs (the open one with its top border, a dot for changes not saved, a
//! close button), the breadcrumbs, then the editor (line numbers, the current line, the caret, VS Code's font); with
//! nothing open, VS Code's watermark of keys. Over the editor, VS Code's find widget (Ctrl+F); above it, a notice
//! when the file changed on disk under changes not saved; and the question asked before such a tab closes.

use std::rc::Rc;

use floem::IntoView;
use floem::keyboard::Modifiers;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::views::editor::Editor;
use floem::views::editor::command::CommandExecuted;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::keypress::KeypressMap;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::{Decorators, dyn_container, stack, text_editor_keys, v_stack};

use crate::state::{Send, Tab, Ui};

mod breadcrumbs;
mod disk;
mod find;
mod keys;
mod tabs;
mod watermark;

pub use disk::take;
pub use find::find_show;

pub fn view(ui: Ui) -> impl IntoView {
    let editor = stack((body(ui), find::find_widget(ui)))
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full());
    stack((
        v_stack((tabs::tabs(ui), breadcrumbs::breadcrumbs(ui), disk::disk_bar(ui), editor))
            .style(|s| s.size_full().min_width(0.0).min_height(0.0)),
        tabs::close_question(ui),
    ))
    .style(|s| s.size_full().min_width(0.0).min_height(0.0))
}

/// The editor drawn for a tab, when it is the one on screen.
pub(super) fn live(ui: Ui, path: &str) -> Option<Editor> {
    ui.live.with_untracked(|l| l.as_ref().filter(|(p, _)| p == path).map(|(_, e)| e.clone()))
}

/// The keys go to an editor.
pub(super) fn focus(editor: &Editor) {
    if let Some(id) = editor.editor_view_id.get_untracked() {
        id.request_focus();
    }
}

/// The caret put on a line (from 1), shown in the middle once it is laid out, as VS Code shows a search's hit.
fn go_to_line(editor: &Editor, line: usize) {
    let at = editor.offset_of_line(line.saturating_sub(1).min(editor.last_line()));
    editor.cursor.update(|c| c.set_insert(Selection::caret(at)));
    let e = editor.clone();
    floem::action::exec_after(std::time::Duration::from_millis(30), move |_| {
        // Unless another tab was shown meanwhile (this editor is gone then).
        if e.cursor.try_get_untracked().is_some() {
            e.center_window();
        }
    });
}

/// What the session's driver is told from the editor group's own keys and buttons.
pub(super) fn tell(ui: Ui, what: impl FnOnce(&Send)) {
    if let Some(send) = ui.out.get_untracked() {
        what(&send);
    }
}

fn body(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let map = Rc::new(KeypressMap::default());
    // The line gone to last (its number among those asked for): a tab shown again stays where it was left.
    let gone = create_rw_signal(None::<u64>);
    // A line asked for in the file on screen: the caret goes there, the editor stands.
    create_effect(move |_| {
        if let Some((path, line, n)) = ui.goto.get()
            && gone.get_untracked() != Some(n)
            && let Some(editor) = live(ui, &path)
        {
            gone.set(Some(n));
            go_to_line(&editor, line);
        }
    });
    dyn_container(
        move || (ui.active.get(), t.get()),
        move |(path, c)| match path.and_then(|p| ui.tab(&p)) {
            None => {
                ui.live.set(None);
                watermark::watermark(ui).into_any()
            }
            Some(Tab { diff: Some(lines), .. }) => {
                ui.live.set(None);
                crate::changes::diff_view(ui, lines).into_any()
            }
            Some(tab) => {
                tab.colours.set_theme(crate::highlight::theme_of(&ui.theme_json()));
                let caret = ui.caret;
                let map = map.clone();
                let ed = text_editor_keys("", move |editor, press, mods| {
                    if keys::editor_key(ui, press, mods) {
                        return CommandExecuted::Yes;
                    }
                    match keys::default_key(&map, editor, press, mods) {
                        // Not the editor's: the workbench's keys (Ctrl+B, Ctrl+P, Ctrl+J…) work while it has focus.
                        CommandExecuted::No => {
                            if let KeyInput::Keyboard(key, _) = &press.key
                                && mods.intersects(Modifiers::CONTROL | Modifiers::ALT)
                            {
                                tell(ui, |send| crate::workbench::key(ui, send, key, mods));
                            }
                            CommandExecuted::No
                        }
                        done => done,
                    }
                })
                .use_doc(tab.doc.clone())
                .styling_rc(tab.colours.clone())
                .editor_style(move |s| {
                    s.gutter_dim_color(c.line_number)
                        .gutter_accent_color(c.line_number_active)
                        .gutter_current_color(c.editor_bg)
                        .gutter_left_padding(18.0)
                        .gutter_right_padding(22.0)
                        .current_line_color(c.line_highlight)
                        .selection_color(c.selection)
                        .cursor_color(c.cursor)
                        .indent_guide_color(c.indent_guide)
                        .visible_whitespace(c.whitespace)
                        .phantom_color(c.desc)
                        .placeholder_color(c.placeholder)
                        // As VS Code: long lines go on, scrolled to, not wrapped.
                        .wrap_method(floem::views::editor::text::WrapMethod::None)
                })
                .style(move |s| {
                    s.size_full()
                        .background(c.editor_bg)
                        .set(floem::views::editor::Foreground, c.editor_fg)
                        .color(c.editor_fg)
                });
                let editor = ed.editor().clone();
                ui.live.set(Some((tab.path.clone(), editor.clone())));
                if let Some((_, line, n)) =
                    ui.goto.get_untracked().filter(|g| g.0 == tab.path && gone.get_untracked() != Some(g.2))
                {
                    gone.set(Some(n));
                    go_to_line(&editor, line);
                }
                let cursor = editor.cursor;
                let id = editor.id();
                let shown = editor.clone();
                create_effect(move |_| {
                    let offset = cursor.with(|c| c.offset());
                    let (line, col) = shown.offset_to_line_col(offset);
                    caret.set((line + 1, col + 1));
                });
                // Gone from the screen (and not for the next one, which is drawn before this one goes).
                ed.on_cleanup(move || {
                    if ui.live.with_untracked(|l| l.as_ref().is_some_and(|(_, e)| e.id() == id)) {
                        ui.live.set(None);
                    }
                })
                .into_any()
            }
        },
    )
    .style(move |s| s.size_full().background(t.get().editor_bg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_group_and_the_outline_are_built_and_follow_the_tabs() {
        use floem::views::editor::core::editor::EditType;
        use floem::views::editor::text::Document;
        use serde_json::json;
        // A font to lay text out with, where the machine has none.
        crate::icons::load();
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let _group = view(ui).into_view();
        let _outline = crate::outline::view(ui, send.clone()).into_view();
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "fn a() {}\nfn b() {}\n", "modified": 1}), &send);
        ui.take(&json!({"type": "file", "path": "b.md", "text": "# B\n", "modified": 1}), &send);
        ui.open_at("a.rs", 2, &send);
        ui.find_case.set(true);
        ui.find_query.set("fn".into());
        // No editor is drawn without a window: there is nothing to step in, and nothing breaks.
        find::find_step(ui, true);
        find::find_hide(ui);
        ui.tab("a.rs").expect("open").doc.edit_single(Selection::caret(0), "x", EditType::InsertChars);
        ui.close("a.rs");
        assert_eq!(ui.closing.get_untracked().as_deref(), Some("a.rs"));
        ui.close_now("a.rs");
        ui.close("b.md");
        assert!(ui.tabs.with_untracked(|t| t.is_empty()) && ui.closing.get_untracked().is_none());
    }
}
