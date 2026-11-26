//! The editor group, as VS Code's: its 35 px tabs (the open one with its top border, a dot for changes not saved, a
//! close button), the breadcrumbs, then the editor (line numbers, the current line, the caret, VS Code's font); with
//! nothing open, VS Code's watermark of keys. Over the editor, VS Code's find widget (Ctrl+F); above it, a notice
//! when the file changed on disk under changes not saved; and the question asked before such a tab closes.

use std::rc::Rc;

use floem::event::EventListener;
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::style::{CursorStyle, TextOverflow};
use floem::text::FamilyOwned;
use floem::views::editor::Editor;
use floem::views::editor::command::CommandExecuted;
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::mode::Mode;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::keypress::KeypressMap;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::editor::keypress::press::KeyPress;
use floem::views::editor::text::{Document, SimpleStyling, WrapMethod};
use floem::views::{
    Decorators, container, dyn_container, dyn_stack, empty, h_stack, label, scroll, stack, text_editor_keys, v_stack,
};
use floem::{IntoView, View};

use crate::icons::icon;
use crate::state::{OnDisk, Send, Tab, Ui};
use crate::widgets::{Theme, UI_FONT, action, button, line, toggle};

pub fn view(ui: Ui) -> impl IntoView {
    let editor =
        stack((body(ui), find_widget(ui))).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full());
    stack((
        v_stack((tabs(ui), breadcrumbs(ui), disk_bar(ui), editor))
            .style(|s| s.size_full().min_width(0.0).min_height(0.0)),
        close_question(ui),
    ))
    .style(|s| s.size_full().min_width(0.0).min_height(0.0))
}

fn tabs(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    h_stack((
        scroll(
            dyn_stack(move || ui.tabs.get(), |tab: &Tab| tab.path.clone(), move |tab: Tab| one_tab(ui, tab))
                .style(|s| s.flex_row().height_full()),
        )
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).height_full()),
        h_stack((action(t, "split-horizontal", || {}), action(t, "ellipsis", || {}))).style(move |s| {
            s.gap(2.0).padding_horiz(8.0).items_center().apply_if(ui.tabs.with(|t| t.is_empty()), |s| s.hide())
        }),
    ))
    .style(move |s| {
        let c = t.get();
        line(s, c.tabs_border, 'b')
            .height(35.0)
            .width_full()
            .background(c.tabs_bg)
            .items_center()
            .apply_if(ui.tabs.with(|t| t.is_empty()), |s| s.hide())
    })
}

fn one_tab(ui: Ui, tab: Tab) -> impl IntoView {
    let t = ui.colors;
    let path = tab.path.clone();
    let here = tab.path.clone();
    let is_active = move || ui.active.with(|a| a.as_deref() == Some(here.as_str()));
    let active = is_active.clone();
    let hover = create_rw_signal(false);
    let dirty = tab.dirty;
    let shut = tab.path.clone();
    let top = is_active.clone();
    let content = h_stack((
        label({
            let n = tab.name();
            move || n.clone()
        })
        .style(|s| s.font_size(13.0).selectable(false)),
        dyn_container(
            move || (dirty.get(), hover.get() || is_active()),
            move |(dirty, shown)| {
                let shut = shut.clone();
                if dirty && !hover.get_untracked() {
                    icon("circle-filled", 16.0).style(|s| s.font_size(10.0)).into_any()
                } else if shown {
                    action(t, "close", move || ui.close(&shut)).into_any()
                } else {
                    empty().style(|s| s.width(22.0)).into_any()
                }
            },
        )
        .style(|s| s.width(22.0).height(22.0).items_center().justify_center()),
    ))
    .style(|s| s.flex_grow(1.0_f32).items_center().gap(4.0).padding_left(10.0).padding_right(4.0));
    v_stack((
        // The open tab's top border (tab.activeBorderTop).
        empty().style(move |s| {
            let c = t.get();
            s.height(1.0).width_full().background(if top() { c.tab_top } else { floem::peniko::Color::TRANSPARENT })
        }),
        content,
    ))
    .on_click_stop(move |_| ui.active.set(Some(path.clone())))
    .on_event_cont(floem::event::EventListener::PointerEnter, move |_| hover.set(true))
    .on_event_cont(floem::event::EventListener::PointerLeave, move |_| hover.set(false))
    .style(move |s| {
        let c = t.get();
        let s =
            s.height_full().min_width(80.0).border_right(1.0).border_color(c.tab_border).cursor(CursorStyle::Pointer);
        if active() {
            s.background(c.tab_active_bg).color(c.tab_active_fg)
        } else {
            s.background(c.tab_inactive_bg).color(c.tab_inactive_fg).hover(|s| s.background(c.tab_hover))
        }
    })
}

fn breadcrumbs(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    dyn_container(
        move || ui.active.get(),
        move |path| {
            let file = path.and_then(|p| ui.tab(&p)).map(|t| t.file).unwrap_or_default();
            let parts: Vec<String> = file.split('/').map(str::to_string).collect();
            let last = parts.len().saturating_sub(1);
            h_stack((dyn_stack(
                move || parts.clone().into_iter().enumerate().collect::<Vec<_>>(),
                |(n, p): &(usize, String)| (*n, p.clone()),
                move |(n, p): (usize, String)| {
                    h_stack((
                        label(move || p.clone()),
                        icon("chevron-right", 16.0).style(move |s| s.apply_if(n == last, |s| s.hide())),
                    ))
                    .style(|s| s.items_center().gap(2.0))
                },
            )
            .style(|s| s.flex_row().gap(2.0)),))
            .into_any()
        },
    )
    .style(move |s| {
        let c = t.get();
        s.height(22.0)
            .width_full()
            .items_center()
            .padding_left(12.0)
            .font_size(13.0)
            .color(c.breadcrumb)
            .background(c.editor_bg)
            .selectable(false)
            .apply_if(ui.active.with(|a| a.is_none()), |s| s.hide())
    })
}

/// The editor drawn for a tab, when it is the one on screen.
fn live(ui: Ui, path: &str) -> Option<Editor> {
    ui.live.with_untracked(|l| l.as_ref().filter(|(p, _)| p == path).map(|(_, e)| e.clone()))
}

/// The keys go to an editor.
fn focus(editor: &Editor) {
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
fn tell(ui: Ui, what: impl FnOnce(&Send)) {
    if let Some(send) = ui.out.get_untracked() {
        what(&send);
    }
}

/// A key as the editor's own keymap has it (what `default_key_handler` does, with the keymap made once).
fn default_key(map: &KeypressMap, editor: RwSignal<Editor>, press: &KeyPress, mods: Modifiers) -> CommandExecuted {
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
fn editor_key(ui: Ui, press: &KeyPress, mods: Modifiers) -> bool {
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
                watermark(ui).into_any()
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
                    if editor_key(ui, press, mods) {
                        CommandExecuted::Yes
                    } else {
                        default_key(&map, editor, press, mods)
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

/// Where a text has what is looked for: each match's place (from, to), the first 20 000 of them (as VS Code counts
/// no further); or that what is looked for is not a regular expression. Words are looked for as they are written
/// unless `regex`, in any case unless `case`.
pub fn matches(text: &str, query: &str, case: bool, regex: bool) -> Result<Vec<(usize, usize)>, ()> {
    const MOST: usize = 20_000;
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let mut pattern = String::from(if case { "(?m)" } else { "(?mi)" });
    if regex {
        pattern.push_str(query);
    } else {
        for c in query.chars() {
            if !(c.is_alphanumeric() || c == '_' || c == ' ') {
                pattern.push('\\');
            }
            pattern.push(c);
        }
    }
    if syntect::parsing::Regex::try_compile(&pattern).is_some() {
        return Err(());
    }
    let re = syntect::parsing::Regex::new(pattern);
    let mut region = syntect::parsing::Region::new();
    let mut out = Vec::new();
    let mut at = 0;
    while at <= text.len() && out.len() < MOST && re.search(text, at, text.len(), Some(&mut region)) {
        let Some((from, to)) = region.pos(0) else { break };
        if to > from {
            out.push((from, to));
            at = to;
        } else {
            // What matches nothing (`x*` where there is no x) is not a match; on to the next character.
            at = to + text[to..].chars().next().map_or(1, char::len_utf8);
        }
    }
    Ok(out)
}

/// The match after the selection (from, to), or before it; around the end to the first, and the start to the last.
pub fn step(hits: &[(usize, usize)], selection: (usize, usize), forward: bool) -> Option<usize> {
    if hits.is_empty() {
        return None;
    }
    let (from, to) = selection;
    Some(if forward {
        hits.iter().position(|h| h.0 > from || (h.0 == from && h.1 > to)).unwrap_or(0)
    } else {
        hits.iter().rposition(|h| h.0 < from).unwrap_or(hits.len() - 1)
    })
}

/// The editor's selection, its lesser end first.
fn selection(editor: &Editor) -> (usize, usize) {
    let (a, b) = editor.cursor.with_untracked(|c| c.get_selection().unwrap_or((c.offset(), c.offset())));
    (a.min(b), a.max(b))
}

/// A match selected in the editor (scrolled to, as the caret is).
fn select(editor: &Editor, hit: (usize, usize)) {
    if hit.1 <= editor.rope_text().len() {
        editor.cursor.update(|c| c.set_insert(Selection::region(hit.0, hit.1)));
    }
}

/// The find widget shown, its box taking the keys.
pub fn find_show(ui: Ui) {
    ui.find.set(true);
    ui.find_asked.update(|n| *n += 1);
}

/// The find widget closed, the keys back in the editor.
fn find_hide(ui: Ui) {
    ui.find.set(false);
    if let Some(editor) = ui.active.get_untracked().and_then(|p| live(ui, &p)) {
        focus(&editor);
    }
}

/// The next match selected, or the previous.
fn find_step(ui: Ui, forward: bool) {
    let Some(editor) = ui.active.get_untracked().and_then(|p| live(ui, &p)) else { return };
    let hits = ui.find_hits.get_untracked();
    if let Some(n) = step(&hits, selection(&editor), forward) {
        select(&editor, hits[n]);
    }
}

/// Looks again for what the find widget looks for: at once when what is looked for changes (the first match from
/// the caret on is then selected) or another file is shown, a moment after the last edit else (the caret stays);
/// off the window's thread, what comes back late (for what was looked for before) dropped.
fn follow_find(ui: Ui) {
    let generation = create_rw_signal(0u64);
    let look = move |reveal: bool| {
        generation.update(|g| *g += 1);
        let g = generation.get_untracked();
        let tab = ui.active.get_untracked().and_then(|p| ui.tab(&p)).filter(|t| t.diff.is_none());
        let query = ui.find_query.get_untracked();
        let (Some(tab), true, false) = (tab, ui.find.get_untracked(), query.is_empty()) else {
            ui.find_hits.set(Rc::new(Vec::new()));
            ui.find_error.set(false);
            return;
        };
        let (case, regex) = (ui.find_case.get_untracked(), ui.find_regex.get_untracked());
        let (path, rope) = (tab.path.clone(), tab.doc.text());
        let done = floem::ext_event::create_ext_action(ui.scope, move |found: Result<Vec<(usize, usize)>, ()>| {
            // For what was looked for before, or after the widget is gone: dropped.
            if generation.try_get_untracked() != Some(g) {
                return;
            }
            ui.find_error.set(found.is_err());
            let hits = Rc::new(found.unwrap_or_default());
            ui.find_hits.set(hits.clone());
            if reveal && let Some(editor) = live(ui, &path) {
                // From where the selection starts: what is typed further keeps the match it is in.
                let from = selection(&editor).0;
                if let Some(hit) = hits.iter().find(|h| h.0 >= from).or(hits.first()) {
                    select(&editor, *hit);
                }
            }
        });
        std::thread::spawn(move || done(matches(&rope.to_string(), &query, case, regex)));
    };
    // What is looked for and how, and in which file.
    type Asked = ((bool, String, bool, bool), Option<String>);
    create_effect(move |last: Option<Asked>| {
        let asked: Asked =
            ((ui.find.get(), ui.find_query.get(), ui.find_case.get(), ui.find_regex.get()), ui.active.get());
        ui.edited.track();
        match last {
            Some(last) if last == asked => {
                if asked.0.0 {
                    let g = generation.get_untracked();
                    floem::action::exec_after(std::time::Duration::from_millis(250), move |_| {
                        if generation.try_get_untracked() == Some(g) {
                            look(false);
                        }
                    });
                }
            }
            // Another file shown: its matches, its caret left where it is.
            Some(last) if last.0 == asked.0 => look(false),
            _ => look(true),
        }
        asked
    });
}

/// VS Code's find widget, at the editor's top right: the box with its Match Case and Regular Expression toggles,
/// which match of how many, previous and next, close. Enter goes to the next match and Shift+Enter to the previous;
/// Escape closes it.
fn find_widget(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    follow_find(ui);
    let map = Rc::new(KeypressMap::default());
    let focused = create_rw_signal(false);
    let styling = SimpleStyling::builder()
        .font_size(13)
        .line_height(18.0)
        .font_family(FamilyOwned::parse_list(&UI_FONT).collect())
        .wrap(WrapMethod::None)
        .build();
    let input = text_editor_keys("", move |editor, press, mods| {
        let shift = mods.contains(Modifiers::SHIFT);
        match &press.key {
            KeyInput::Keyboard(Key::Named(NamedKey::Enter), _) => find_step(ui, !shift),
            KeyInput::Keyboard(Key::Named(NamedKey::F3), _) => find_step(ui, !shift),
            KeyInput::Keyboard(Key::Named(NamedKey::Escape), _) => find_hide(ui),
            KeyInput::Keyboard(Key::Character(c), _) if mods == Modifiers::CONTROL && c.eq_ignore_ascii_case("f") => {
                editor.with_untracked(|e| {
                    let len = e.rope_text().len();
                    e.cursor.update(|c| c.set_insert(Selection::region(0, len)));
                })
            }
            _ => return default_key(&map, editor, press, mods),
        }
        CommandExecuted::Yes
    })
    .styling(styling)
    .placeholder("Find")
    .editor_style(move |s| {
        let c = t.get();
        s.hide_gutter(true)
            .cursor_surrounding_lines(0)
            .selection_color(c.selection)
            .cursor_color(c.cursor)
            .placeholder_color(c.placeholder)
    });
    let editor = input.editor().clone();
    // What is in the box is what is looked for (its first line: nothing is looked for across lines).
    let input = {
        let doc = Rc::downgrade(&editor.doc());
        input.update(move |_| {
            if let Some(doc) = doc.upgrade() {
                let text = doc.text().to_string();
                let line = text.lines().next().unwrap_or("").to_string();
                if ui.find_query.with_untracked(|q| *q != line) {
                    ui.find_query.set(line);
                }
            }
        })
    };
    // Asked for (Ctrl+F): the box takes the keys, with the editor's selection in it when it is within a line, and
    // all of it selected, so what is typed takes its place.
    {
        let editor = editor.clone();
        create_effect(move |first: Option<()>| {
            ui.find_asked.track();
            if first.is_none() {
                return;
            }
            let doc = editor.doc();
            // The text and the selection changed as one (no caret past the end of the text between the two).
            floem::reactive::batch(|| {
                if let Some(main) = ui.active.get_untracked().and_then(|p| live(ui, &p)) {
                    let (from, to) = selection(&main);
                    let picked = main.rope_text().slice_to_cow(from..to).to_string();
                    if !picked.is_empty() && !picked.contains('\n') && doc.text().to_string() != picked {
                        doc.edit_single(Selection::region(0, doc.text().len()), &picked, EditType::Other);
                    }
                }
                let len = doc.text().len();
                editor.cursor.update(|c| c.set_insert(Selection::region(0, len)));
            });
            focus(&editor);
        });
    }
    // Lit while the keys are in it.
    {
        let (gained, lost) = (editor.editor_view_focused, editor.editor_view_focus_lost);
        create_effect(move |first: Option<()>| {
            gained.track();
            if first.is_some() {
                focused.set(true);
            }
        });
        create_effect(move |first: Option<()>| {
            lost.track();
            if first.is_some() {
                focused.set(false);
            }
        });
    }
    // A toggle clicked: the keys stay in the box.
    {
        let editor = editor.clone();
        create_effect(move |first: Option<()>| {
            ui.find_case.track();
            ui.find_regex.track();
            if first.is_some() && ui.find.get_untracked() {
                focus(&editor);
            }
        });
    }
    let field = h_stack((
        input.style(move |s| {
            let c = t.get();
            s.flex_grow(1.0_f32)
                .flex_basis(0.0)
                .min_width(0.0)
                .height(18.0)
                .margin_left(4.0)
                .set(floem::views::editor::Foreground, c.input_fg)
                .color(c.input_fg)
        }),
        toggle(t, "case-sensitive", ui.find_case, "Match Case"),
        toggle(t, "regex", ui.find_regex, "Use Regular Expression"),
    ))
    .style(move |s| {
        let c = t.get();
        s.width(220.0)
            .height(24.0)
            .padding_right(2.0)
            .gap(2.0)
            .items_center()
            .border(1.0)
            .border_radius(2.0)
            .border_color(if ui.find_error.get() {
                c.error
            } else if focused.get() {
                c.focus
            } else {
                c.input_border
            })
            .background(c.input_bg)
    });
    // Which match the editor's selection is, of how many: "3 of 12", "? of 12" between matches, "No results".
    let count = label(move || {
        let hits = ui.find_hits.get();
        if hits.is_empty() {
            return "No results".to_string();
        }
        let more = if hits.len() >= 20_000 { "+" } else { "" };
        let at = ui.live.with(|l| {
            l.as_ref().and_then(|(_, e)| {
                let (a, b) = e.cursor.with(|c| c.get_selection().unwrap_or((c.offset(), c.offset())));
                hits.binary_search(&(a.min(b), a.max(b))).ok()
            })
        });
        match at {
            Some(n) => format!("{} of {}{more}", n + 1, hits.len()),
            None => format!("? of {}{more}", hits.len()),
        }
    })
    .style(move |s| {
        let c = t.get();
        let none = ui.find_hits.with(|h| h.is_empty()) && ui.find_query.with(|q| !q.is_empty());
        s.min_width(72.0).padding_horiz(4.0).font_size(12.0).selectable(false).color(if none { c.error } else { c.fg })
    });
    let back = editor.clone();
    let on = editor.clone();
    h_stack((
        field,
        count,
        action(t, "arrow-up", move || {
            find_step(ui, false);
            focus(&back);
        }),
        action(t, "arrow-down", move || {
            find_step(ui, true);
            focus(&on);
        }),
        action(t, "close", move || find_hide(ui)),
    ))
    .style(move |s| {
        let c = t.get();
        // Over an editor only (not a file's changes, nor the watermark).
        let over = ui.find.get() && ui.live.with(|l| l.is_some());
        line(s, c.focus, 'l')
            .absolute()
            .inset_top(0.0)
            .inset_right(28.0)
            .height(33.0)
            .padding_left(6.0)
            .padding_right(6.0)
            .gap(2.0)
            .items_center()
            .z_index(20)
            .background(c.widget)
            .color(c.fg)
            .box_shadow_blur(8.0)
            .box_shadow_v_offset(2.0)
            .box_shadow_color(c.shadow)
            .apply_if(!over, |s| s.hide())
    })
}

/// A small button of a bar (the notice above the editor).
fn small(t: Theme, text: &'static str, on: impl Fn() + 'static) -> impl IntoView {
    label(move || text).on_click_stop(move |_| on()).style(move |s| {
        let c = t.get();
        s.padding_horiz(8.0)
            .height(20.0)
            .items_center()
            .border_radius(2.0)
            .font_size(12.0)
            .background(c.button2_bg)
            .color(c.button2_fg)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(c.button2_hover))
    })
}

/// The notice above the editor when the shown file changed on disk (or is gone from it) while it has changes not
/// saved: Revert takes what is on disk, Overwrite saves the tab's text over it, the cross leaves things as they are.
fn disk_bar(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let state = move || ui.active.with(|a| a.as_ref().and_then(|p| ui.on_disk.with(|d| d.get(p).copied())));
    let on_active = move |what: fn(Ui, &str, &Send)| {
        if let Some(path) = ui.active.get_untracked() {
            tell(ui, |send| what(ui, &path, send));
        }
    };
    h_stack((
        icon("warning", 16.0).style(move |s| s.color(t.get().warning)),
        label(move || {
            let name = ui.active.with(|a| a.as_deref().map(|p| p.rsplit('/').next().unwrap_or(p).to_string()));
            let name = name.unwrap_or_default();
            match state() {
                Some(OnDisk::Gone) => format!("{name} was deleted on disk. Saving it makes it again."),
                _ => format!("{name} has changed on disk, and has changes of yours that are not saved."),
            }
        })
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).text_ellipsis()),
        small(t, "Revert", move || on_active(|ui, path, send| ui.take_disk(path, send)))
            .style(move |s| s.apply_if(state() != Some(OnDisk::Changed), |s| s.hide())),
        small(t, "Overwrite", move || on_active(|ui, path, send| ui.save_tab(path, send))),
        action(t, "close", move || {
            if let Some(path) = ui.active.get_untracked() {
                ui.forget_on_disk(&path);
            }
        }),
    ))
    .style(move |s| {
        let c = t.get();
        line(s, c.notice_border, 'b')
            .width_full()
            .height(28.0)
            .padding_horiz(12.0)
            .gap(8.0)
            .items_center()
            .font_size(13.0)
            .background(c.notice_bg)
            .color(c.fg)
            .selectable(false)
            .apply_if(state().is_none(), |s| s.hide())
    })
}

/// Asked before a tab with changes not saved closes, as VS Code asks: Save (it closes once saved), Don't Save,
/// Cancel; over the editor group, which takes no clicks meanwhile.
fn close_question(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let name = move || {
        ui.closing.with(|c| c.as_deref().map(|p| p.rsplit('/').next().unwrap_or(p).to_string())).unwrap_or_default()
    };
    let dialog = v_stack((
        label(move || format!("Do you want to save the changes you made to {}?", name()))
            .style(|s| s.font_size(15.0).text_overflow(TextOverflow::Wrap)),
        label(|| "Your changes will be lost if you don't save them.")
            .style(move |s| s.margin_top(8.0).color(t.get().desc).text_overflow(TextOverflow::Wrap)),
        h_stack((
            button(t, "Save", true, move || {
                if let Some(path) = ui.closing.get_untracked() {
                    ui.close_saved.set(Some(path.clone()));
                    ui.closing.set(None);
                    tell(ui, |send| ui.save_tab(&path, send));
                }
            }),
            button(t, "Don't Save", false, move || {
                if let Some(path) = ui.closing.get_untracked() {
                    ui.close_now(&path);
                }
            }),
            button(t, "Cancel", false, move || ui.closing.set(None)),
        ))
        .style(|s| s.margin_top(20.0).gap(8.0).justify_end()),
    ))
    .on_click_stop(|_| {})
    .style(move |s| {
        let c = t.get();
        s.width(440.0)
            .max_width_pct(90.0)
            .padding(20.0)
            .border(1.0)
            .border_radius(6.0)
            .border_color(c.widget_border)
            .background(c.widget)
            .color(c.fg)
            .box_shadow_blur(16.0)
            .box_shadow_v_offset(4.0)
            .box_shadow_color(c.shadow)
    });
    let cancel = move || ui.closing.set(None);
    let over = container(dialog);
    let id = over.id();
    // The keys are the question's while it is asked (Escape cancels), and the editor's again after.
    create_effect(move |was: Option<bool>| {
        let asked = ui.closing.with(|c| c.is_some());
        if asked {
            id.request_focus();
        } else if was == Some(true)
            && let Some(editor) = ui.active.get_untracked().and_then(|p| live(ui, &p))
        {
            focus(&editor);
        }
        asked
    });
    over.keyboard_navigable()
        .on_key_down(Key::Named(NamedKey::Escape), |_| true, move |_| cancel())
        .on_click_stop(|_| {})
        .on_event_stop(EventListener::PointerDown, |_| {})
        .on_event_stop(EventListener::PointerWheel, |_| {})
        .style(move |s| {
            s.absolute()
                .inset(0.0)
                .items_center()
                .justify_center()
                .z_index(30)
                .background(floem::peniko::Color::rgba8(0, 0, 0, 0x4D))
                .selectable(false)
                .apply_if(ui.closing.with(|c| c.is_none()), |s| s.hide())
        })
}

/// VS Code's empty editor: its keys, faint, in the middle.
fn watermark(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let row = move |what: &'static str, keys: &'static str| {
        h_stack((
            label(move || what).style(|s| s.width(170.0).justify_end().font_size(13.0)),
            h_stack((dyn_stack(
                move || keys.split('+').map(str::to_string).collect::<Vec<_>>(),
                |k: &String| k.clone(),
                move |k: String| {
                    label(move || k.clone()).style(move |s| {
                        let c = t.get();
                        s.font_size(11.0)
                            .padding_horiz(5.0)
                            .padding_vert(2.0)
                            .border(1.0)
                            .border_radius(3.0)
                            .border_color(c.widget_border)
                            .background(c.code)
                    })
                },
            )
            .style(|s| s.flex_row().gap(3.0)),))
            .style(|s| s.width(170.0)),
        ))
        .style(|s| s.gap(12.0).items_center())
    };
    v_stack((
        row("Show All Commands", "Ctrl+Shift+P"),
        row("Go to File", "Ctrl+P"),
        row("Find in Files", "Ctrl+Shift+F"),
        row("Toggle Terminal", "Ctrl+`"),
        row("Open Chat", "Ctrl+Alt+I"),
    ))
    .style(move |s| s.gap(10.0).size_full().items_center().justify_center().color(t.get().watermark).selectable(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_looked_for_is_found_as_written_or_as_a_regular_expression() {
        let text = "let Cart = cart(1);\nlet é = a.b + a*b;\n";
        assert_eq!(matches(text, "cart", false, false), Ok(vec![(4, 8), (11, 15)]));
        assert_eq!(matches(text, "cart", true, false), Ok(vec![(11, 15)]));
        // As written: a dot is a dot, a star a star; and places are in bytes, after a letter of two.
        assert_eq!(matches(text, "a.b", false, false), Ok(vec![(29, 32)]));
        assert_eq!(matches(text, "a*b", false, false), Ok(vec![(35, 38)]));
        assert_eq!(matches(text, "a.b", false, true), Ok(vec![(29, 32), (35, 38)]));
        assert_eq!(matches(text, "^let", true, true), Ok(vec![(0, 3), (20, 23)]));
        // What matches nothing is no match, and what is not a regular expression is said not to be one.
        assert_eq!(matches(text, "x*", true, true), Ok(vec![]));
        assert_eq!(matches(text, "(", true, true), Err(()));
        assert_eq!(matches(text, "(", true, false), Ok(vec![(15, 16)]));
        assert_eq!(matches(text, "", false, false), Ok(vec![]));
    }

    #[test]
    fn the_group_and_the_outline_are_built_and_follow_the_tabs() {
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
        find_step(ui, true);
        find_hide(ui);
        ui.tab("a.rs").expect("open").doc.edit_single(Selection::caret(0), "x", EditType::InsertChars);
        ui.close("a.rs");
        assert_eq!(ui.closing.get_untracked().as_deref(), Some("a.rs"));
        ui.close_now("a.rs");
        ui.close("b.md");
        assert!(ui.tabs.with_untracked(|t| t.is_empty()) && ui.closing.get_untracked().is_none());
    }

    #[test]
    fn the_next_match_and_the_previous_go_around() {
        let hits = [(4, 8), (11, 15), (30, 33)];
        // From the caret: the match it is before; from a match: the one after.
        assert_eq!(step(&hits, (0, 0), true), Some(0));
        assert_eq!(step(&hits, (4, 4), true), Some(0));
        assert_eq!(step(&hits, (4, 8), true), Some(1));
        assert_eq!(step(&hits, (30, 33), true), Some(0));
        assert_eq!(step(&hits, (11, 15), false), Some(0));
        assert_eq!(step(&hits, (4, 8), false), Some(2));
        assert_eq!(step(&[], (0, 0), true), None);
    }
}
