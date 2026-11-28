//! The editor groups (`groups.rs`: up to eight, side by side or one under the other), each as VS Code's: its 35 px tabs (the open one with its top border, a dot for changes not saved, a
//! close button), the breadcrumbs, then the editor (line numbers, the current line, the caret, VS Code's font); with
//! nothing open, VS Code's watermark of keys. Over the editor, VS Code's find widget (Ctrl+F); above it, a notice
//! when the file changed on disk under changes not saved; and the question asked before such a tab closes.
//!
//! A tab that is not a file's text (a picture, a preview, the settings) is of a kind its part added (`kinds.rs`),
//! and is drawn by it in the editor's place.

use std::rc::Rc;

use floem::IntoView;
use floem::reactive::{SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::views::editor::Editor;
use floem::views::editor::command::CommandExecuted;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::keypress::KeypressMap;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::{Decorators, dyn_container, stack, v_stack};

use crate::state::{Send, Tab, Ui};

pub mod breadcrumbs;
pub mod disk;
pub mod editing;
pub mod find;
pub mod folding;
pub mod groups;
pub mod highlights;
pub mod keys;
pub mod kinds;
pub mod minimap;
pub mod nav;
pub mod options;
pub mod tabs;
pub mod watermark;
pub mod window;

pub use find::find_show;

pub fn view(ui: Ui) -> impl IntoView {
    groups::area(ui)
}

/// One group: its tabs, its breadcrumbs, its editor. Only the focused group has the find widget and the notice of a
/// file changed on disk (they are of the tab shown, `ui.active`).
pub(super) fn group_view(ui: Ui, shown: groups::Shown) -> floem::views::Stack {
    let focused = shown.focused;
    let only_focused = move |what: fn(Ui) -> floem::AnyView| {
        dyn_container(move || focused.get(), move |f| if f { what(ui) } else { floem::views::empty().into_any() })
    };
    // The pointer goes to the topmost part under it only: the find widget's place is its own size (at the editor's
    // top right), so the editor beside it still has the pointer.
    let find = only_focused(|ui| {
        find::find_widget(ui).style(|s| s.position(floem::style::Position::Relative).inset_right(0.0)).into_any()
    })
    .style(|s| s.absolute().inset_top(0.0).inset_right(28.0).z_index(20));
    let editor =
        stack((body(ui, shown), find)).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full());
    let disk = only_focused(|ui| disk::disk_bar(ui).into_any()).style(|s| s.width_full());
    v_stack((tabs::tabs(ui, shown), breadcrumbs::breadcrumbs(ui, shown.active, shown.focused), disk, editor))
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

/// The caret put on a line (from 1), shown in the middle once it is laid out, as VS Code shows a search's hit (a
/// fold it is in opens).
fn go_to_line(editor: &Editor, path: &str, line: usize) {
    let at = editor.offset_of_line(line.saturating_sub(1).min(editor.last_line()));
    editor.cursor.update(|c| c.set_insert(Selection::caret(at)));
    let (e, path) = (editor.clone(), path.to_string());
    floem::action::exec_after(std::time::Duration::from_millis(30), move |_| {
        // Unless another tab was shown meanwhile (this editor is gone then).
        if e.cursor.try_get_untracked().is_some() {
            folding::center(&e, &path);
        }
    });
}

/// The file editor's colours and gutter, as Floem's `TextEditor::editor_style` sets them: the line numbers with
/// their padding (the chevrons right of them), the current line, the selection, the caret, the guides.
pub(crate) fn editor_look(s: floem::style::Style, c: &crate::colors::Colors, focused: bool) -> floem::style::Style {
    use floem::views::editor::gutter::{DimColor, GutterClass, LeftOfCenterPadding, RightOfCenterPadding};
    use floem::views::editor::view::EditorViewClass;
    use floem::views::editor::*;
    let c = *c;
    s.size_full()
        .set(Foreground, c.editor_fg)
        .color(c.editor_fg)
        .class(GutterClass, |s| {
            s.color(c.line_number_active)
                .set(DimColor, Some(c.line_number))
                .set(CurrentLineColor, Some(c.editor_bg))
                .set(LeftOfCenterPadding, 18.0)
                .set(RightOfCenterPadding, folding::CHEVRONS)
        })
        .class(EditorViewClass, |s| {
            s.set(CurrentLineColor, Some(c.line_highlight))
                .set(SelectionColor, c.selection)
                // As VS Code: the caret only in the group that has the keys.
                .set(floem::style::CursorColor, if focused { c.cursor } else { floem::peniko::Color::TRANSPARENT })
                .set(IndentGuideColor, c.indent_guide)
                .set(VisibleWhitespaceColor, c.whitespace)
                .set(PhantomColor, c.desc)
                .set(PlaceholderColor, c.placeholder)
                // As VS Code: long lines go on, scrolled to, not wrapped.
                .set(WrapProp, text::WrapMethod::None)
        })
}

/// What the session's driver is told from the editor group's own keys and buttons.
pub(crate) fn tell(ui: Ui, what: impl FnOnce(&Send)) {
    if let Some(send) = ui.out.get_untracked() {
        what(&send);
    }
}

/// The editor a group has on screen made the window's (`ui.live`) when the group is focused, and none when the
/// focused group shows no file's text.
fn make_live(ui: Ui, id: u64) {
    let g = groups::groups(ui);
    let mine = g.editors.with_untracked(|e| e.get(&id).cloned());
    let same = ui.live.with_untracked(|l| match (l, &mine) {
        (None, None) => true,
        (Some((p, a)), Some((q, b))) => p == q && a.id() == b.id(),
        _ => false,
    });
    if !same {
        ui.live.set(mine);
    }
}

/// What a group draws for its tab shown (none, a kind's, a file's changes, a file's text): the editor on screen
/// is the group's, and the window's too while the group is focused.
fn body(ui: Ui, shown: groups::Shown) -> impl IntoView {
    let t = ui.colors;
    let id = shown.id;
    let g = groups::groups(ui);
    let map = Rc::new(KeypressMap::default());
    // The line gone to last (its number among those asked for): a tab shown again stays where it was left.
    let gone = create_rw_signal(None::<u64>);
    // A line asked for in the file on screen: the caret goes there, the editor stands.
    create_effect(move |_| {
        if let Some((path, line, n)) = ui.goto.get()
            && shown.focused.get_untracked()
            && gone.get_untracked() != Some(n)
            && let Some(editor) = live(ui, &path)
        {
            gone.set(Some(n));
            go_to_line(&editor, &path, line);
        }
    });
    // Focused: its editor is the window's, and the caret said is its.
    create_effect(move |_| {
        if shown.focused.get() {
            make_live(ui, id);
            if let Some((_, e)) = g.editors.with_untracked(|m| m.get(&id).cloned()) {
                let (line, col) = e.offset_to_line_col(e.cursor.with_untracked(|c| c.offset()));
                ui.caret.set((line + 1, col + 1));
            }
        }
    });
    let unset = move || {
        if g.editors.with_untracked(|e| e.contains_key(&id)) {
            g.editors.update(|e| {
                e.remove(&id);
            });
        }
        // (A group being taken away has its signals gone already.)
        if shown.focused.try_get_untracked() == Some(true) {
            make_live(ui, id);
        }
    };
    let active = shown.active;
    dyn_container(
        move || (active.get(), t.get()),
        move |(path, c)| match path.and_then(|p| ui.tab(&p)) {
            None => {
                unset();
                watermark::watermark(ui).into_any()
            }
            // A tab that is not a file's text: drawn by its kind. An editor the kind draws (the settings' JSON)
            // is the group's; the window's only while the group is focused.
            Some(tab) if tab.kind.is_some() => {
                unset();
                let before = ui.live.get_untracked();
                let view = kinds::draw(ui, &tab);
                if let Some(made) =
                    ui.live.get_untracked().filter(|(_, e)| before.as_ref().is_none_or(|(_, b)| b.id() != e.id()))
                {
                    g.editors.update(|e| {
                        e.insert(id, made);
                    });
                    if !shown.focused.get_untracked() {
                        ui.live.set(before);
                    }
                }
                view
            }
            Some(Tab { diff: Some(lines), .. }) => {
                unset();
                crate::changes::diff_view(ui, lines).into_any()
            }
            Some(tab) => {
                tab.colours.set_theme(crate::highlight::theme_of(&ui.theme_json()));
                let caret = ui.caret;
                let map = map.clone();
                let focused = shown.focused;
                let doc = crate::decor::doc(tab.doc.clone(), &tab.path);
                let editor = Editor::new(floem::reactive::Scope::current(), doc, tab.colours.clone(), false);
                // The options of the file, read for its language: the editor follows them as they change.
                let file = tab.path.clone();
                let o = options::memo(ui, move || Some(file.clone()));
                // The editor's width, by which a bounded wrap is found.
                let width = create_rw_signal(0.0);
                {
                    let viewport = editor.viewport;
                    create_effect(move |_| {
                        let w = viewport.with(|v| v.width());
                        if width.get_untracked() != w {
                            width.set(w);
                        }
                    });
                }
                // Floem's editor, drawn with the folds (`folding.rs`).
                let ed = folding::view(ui, editor.clone(), &tab.doc, &tab.path, move |editor, press, mods| {
                    if let KeyInput::Keyboard(key, _) = &press.key
                        && options::key(ui, editor, o, key, mods)
                    {
                        return CommandExecuted::Yes;
                    }
                    // The key after a chord's first (Ctrl+K Enter, Ctrl+K Shift+Enter) ends the chord: the
                    // editor does not take it as its own (a new line) when the chord runs a command.
                    if crate::commands::waiting().is_some()
                        && let KeyInput::Keyboard(key, _) = &press.key
                    {
                        let mut ran = false;
                        tell(ui, |send| {
                            ran = crate::commands::press(ui, send, key, mods) != crate::commands::Pressed::Free;
                        });
                        if ran {
                            return CommandExecuted::Yes;
                        }
                    }
                    if let KeyInput::Keyboard(key, _) = &press.key
                        && crate::layout_commands::zen_escape(ui, key, mods)
                    {
                        return CommandExecuted::Yes;
                    }
                    if keys::editor_key(ui, press, mods) {
                        return CommandExecuted::Yes;
                    }
                    match keys::default_key(&map, editor, press, mods) {
                        // Not the editor's: the workbench's keys (Ctrl+B, Ctrl+P, Ctrl+J…) work while it has focus.
                        CommandExecuted::No => {
                            if let KeyInput::Keyboard(key, _) = &press.key
                                && crate::keys_hook::editor(key, mods)
                            {
                                tell(ui, |send| crate::workbench::key(ui, send, key, mods));
                            }
                            CommandExecuted::No
                        }
                        done => done,
                    }
                })
                // No background of its own: the group's is under it, and the overview ruler between (`decor.rs`).
                .style(move |s| {
                    let s = editor_look(s, &c, focused.get());
                    o.with(|o| options::look(s, o, &c, width.get()))
                });
                // Its right-click menu (`editing.rs`), once the editor has put the caret where it was clicked.
                let view = floem::View::id(&ed);
                let ed = ed.on_event_cont(floem::event::EventListener::PointerDown, move |e| {
                    editing::context_menu(ui, view, e)
                });
                g.editors.update(|e| {
                    e.insert(id, (tab.path.clone(), editor.clone()));
                });
                if focused.get_untracked() {
                    make_live(ui, id);
                }
                // Where it was left in this group: its caret and scroll back.
                let key = (id, tab.path.clone());
                let kept = g.places.with_untracked(|p| p.get(&key).copied());
                let placed = create_rw_signal(kept.is_none());
                if let Some(at) = kept {
                    at.put(&editor, placed);
                }
                if let Some((_, line, n)) = ui
                    .goto
                    .get_untracked()
                    .filter(|g| g.0 == tab.path && gone.get_untracked() != Some(g.2) && focused.get_untracked())
                {
                    gone.set(Some(n));
                    go_to_line(&editor, &tab.path, line);
                }
                // The editor given the keys (clicked): its group is the focused one.
                let focus_in = editor.editor_view_focused;
                create_effect(move |first: Option<()>| {
                    focus_in.track();
                    if first.is_some() {
                        groups::focus(ui, id);
                    }
                });
                let cursor = editor.cursor;
                let eid = editor.id();
                let here = editor.clone();
                create_effect(move |_| {
                    let offset = cursor.with(|c| c.offset());
                    if focused.get_untracked() {
                        let (line, col) = here.offset_to_line_col(offset);
                        caret.set((line + 1, col + 1));
                    }
                });
                // Where it is, noted as the caret moves and it scrolls (its signals are gone by the time it is
                // cleaned up); not before it was put back where it was left, else that place is lost.
                let last = Rc::new(std::cell::Cell::new(None::<groups::Place>));
                {
                    let (last, here, viewport) = (last.clone(), editor.clone(), editor.viewport);
                    create_effect(move |_| {
                        cursor.track();
                        viewport.track();
                        if placed.get() {
                            last.set(Some(groups::Place::of(&here)));
                        }
                    });
                }
                // Gone from the screen (and not for the next one, which is drawn before this one goes): where it
                // was left is kept for when it is shown again in this group.
                let ed = ed.on_cleanup(move || {
                    if let Some(at) = last.get() {
                        g.places.update(|p| {
                            p.insert(key.clone(), at);
                        });
                    }
                    if g.editors.with_untracked(|e| e.get(&id).is_some_and(|(_, e)| e.id() == eid)) {
                        g.editors.update(|e| {
                            e.remove(&id);
                        });
                    }
                    if ui.live.with_untracked(|l| l.as_ref().is_some_and(|(_, e)| e.id() == eid)) {
                        make_live(ui, id);
                    }
                });
                // With its highlights (brackets, the word at the caret, links), the decorations' glyph margin and the
                // overview ruler.
                let ed = options::follow(ui, ed, &editor, tab.colours.clone(), o);
                let ed = highlights::follow(ui, ed, &editor, tab.doc.clone(), &tab.path);
                crate::decor::around(ui, ed, &editor, &tab.path).into_any()
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
