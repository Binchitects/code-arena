//! VS Code's find widget over the editor (Ctrl+F): what it looks for and how, where each match is, and the widget
//! itself.

use std::rc::Rc;

use floem::IntoView;
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::reactive::{SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::text::FamilyOwned;
use floem::views::editor::Editor;
use floem::views::editor::command::CommandExecuted;
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::keypress::KeypressMap;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::editor::text::{Document, SimpleStyling, WrapMethod};
use floem::views::{Decorators, h_stack, label, text_editor_keys};

use super::keys::default_key;
use super::{focus, live};
use crate::state::Ui;
use crate::widgets::{UI_FONT, action, line, toggle};

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
pub(super) fn find_hide(ui: Ui) {
    ui.find.set(false);
    if let Some(editor) = ui.active.get_untracked().and_then(|p| live(ui, &p)) {
        focus(&editor);
    }
}

/// The next match selected, or the previous.
pub(super) fn find_step(ui: Ui, forward: bool) {
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
        // A file's text, or a tab of a kind that draws an editor on its own text (it is then the one on screen).
        let tab =
            ui.active.get_untracked().and_then(|p| ui.tab(&p)).filter(|t| t.is_file() || live(ui, &t.path).is_some());
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
pub(super) fn find_widget(ui: Ui) -> impl IntoView {
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

/// What it adds to the window's registries, once at start.
pub fn register(_ui: crate::state::Ui, _send: &crate::state::Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: crate::state::Ui, _m: &serde_json::Value, _send: &crate::state::Send) -> bool {
    false
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
