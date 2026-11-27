//! VS Code's find widget over the editor (Ctrl+F): what it looks for and how, where each match is, and the widget
//! itself.

use std::rc::Rc;

use floem::IntoView;
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::text::FamilyOwned;
use floem::views::editor::Editor;
use floem::views::editor::command::CommandExecuted;
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::keypress::KeypressMap;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::editor::text::{Document, SimpleStyling, WrapMethod};
use floem::views::{Decorators, TextEditor, h_stack, label, text_editor_keys, tooltip, v_stack};

use super::keys::default_key;
use super::{focus, live};
use crate::state::Ui;
use crate::widgets::{UI_FONT, action, action_lit, line, toggle};

/// How what is looked for is looked for: in its case only (Match Case), as a whole word only (Match Whole Word), as
/// a regular expression (Use Regular Expression).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct How {
    pub case: bool,
    pub word: bool,
    pub regex: bool,
}

/// The most matches the widget counts, as VS Code counts no further.
pub const MOST: usize = 20_000;

/// VS Code's `editor.wordSeparators`, and white space: what a whole word stands between.
fn separator(c: char) -> bool {
    c.is_whitespace() || "`~!@#$%^&*()-=+[{]}\\|;:'\",.<>/?".contains(c)
}

/// Whether a match (from, to) stands as a whole word: at each of its ends the text's edge, a separator beside it, or
/// a separator its own end (as VS Code has it, `.b` is a whole word in `a.b`).
fn whole_word(text: &str, from: usize, to: usize) -> bool {
    let hit = &text[from..to];
    let left = text[..from].chars().next_back().is_none_or(separator) || hit.chars().next().is_some_and(separator);
    let right = text[to..].chars().next().is_none_or(separator) || hit.chars().next_back().is_some_and(separator);
    left && right
}

/// What is looked for as a regular expression over many lines; none when it is not one.
fn regex_of(query: &str, how: How) -> Option<syntect::parsing::Regex> {
    let mut pattern = String::from(if how.case { "(?m)" } else { "(?mi)" });
    if how.regex {
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
        return None;
    }
    Some(syntect::parsing::Regex::new(pattern))
}

/// Each match of what is looked for in a text, in order, the first `most` of them, within `scope` (from, to) when
/// one is given; each given with the regular expression's groups. Err when it is not a regular expression.
fn search(
    text: &str,
    query: &str,
    how: How,
    scope: Option<(usize, usize)>,
    most: usize,
    mut each: impl FnMut(&syntect::parsing::Region),
) -> Result<(), ()> {
    if query.is_empty() {
        return Ok(());
    }
    let re = regex_of(query, how).ok_or(())?;
    let (mut at, mut end) = scope.unwrap_or((0, text.len()));
    end = end.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut region = syntect::parsing::Region::new();
    let mut found = 0;
    while at <= end && found < most && re.search(text, at, end, Some(&mut region)) {
        let Some((from, to)) = region.pos(0) else { break };
        if to > from {
            if !how.word || whole_word(text, from, to) {
                each(&region);
                found += 1;
            }
            at = to;
        } else {
            // What matches nothing (`x*` where there is no x) is not a match; on to the next character.
            at = to + text[to..].chars().next().map_or(1, char::len_utf8);
        }
    }
    Ok(())
}

/// Where a text has what is looked for: each match's place (from, to), the first `MOST` of them, within `scope`
/// when one is given; or that what is looked for is not a regular expression. Words are looked for as they are
/// written unless `how.regex`, in any case unless `how.case`, anywhere unless `how.word`.
pub fn matches(text: &str, query: &str, how: How, scope: Option<(usize, usize)>) -> Result<Vec<(usize, usize)>, ()> {
    let mut out = Vec::new();
    search(text, query, how, scope, MOST, |r| out.extend(r.pos(0)))?;
    Ok(out)
}

/// Each match (from, to) with what replaces it, every one of them (Replace All leaves none out), within `scope`
/// when one is given.
pub fn replacements(
    text: &str,
    query: &str,
    how: How,
    scope: Option<(usize, usize)>,
    with: &str,
    preserve: bool,
) -> Result<Vec<(usize, usize, String)>, ()> {
    let mut out = Vec::new();
    search(text, query, how, scope, usize::MAX, |r| {
        let Some((from, to)) = r.pos(0) else { return };
        let mut by = if how.regex {
            let mut groups: Vec<Option<&str>> = (0..100).map(|i| r.pos(i).map(|(a, b)| &text[a..b])).collect();
            // Past the last group that took part there are none.
            while groups.last().is_some_and(Option::is_none) {
                groups.pop();
            }
            expand(with, &groups)
        } else {
            with.to_string()
        };
        if preserve {
            by = preserve_case(&text[from..to], &by);
        }
        out.push((from, to, by));
    })?;
    Ok(out)
}

/// What replaces a match of a regular expression, as VS Code reads it: `$1` to `$99` its groups, `$0` and `$&` all
/// of it, `$$` a dollar, `\n` and `\t` a new line and a tab, `\\` a backslash; the rest as it is written.
pub fn expand(with: &str, groups: &[Option<&str>]) -> String {
    let c: Vec<char> = with.chars().collect();
    let group = |n: usize| groups.get(n).copied().flatten().unwrap_or("");
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        match (c[i], c.get(i + 1).copied()) {
            ('$', Some('$')) => out.push('$'),
            ('$', Some('&')) => out.push_str(group(0)),
            ('$', Some(d)) if d.is_ascii_digit() => {
                let one = d.to_digit(10).unwrap_or(0) as usize;
                let two = c.get(i + 2).and_then(|e| e.to_digit(10)).map(|e| one * 10 + e as usize);
                match two.filter(|n| *n < groups.len()) {
                    Some(n) => {
                        out.push_str(group(n));
                        i += 1;
                    }
                    None if one < groups.len() => out.push_str(group(one)),
                    // No such group: written as it is.
                    None => {
                        out.push('$');
                        out.push(d);
                    }
                }
            }
            ('\\', Some('n')) => out.push('\n'),
            ('\\', Some('t')) => out.push('\t'),
            ('\\', Some('\\')) => out.push('\\'),
            (ch, _) => {
                out.push(ch);
                i += 1;
                continue;
            }
        }
        i += 2;
    }
    out
}

/// What replaces a match in the match's case (Preserve Case): all capitals, all small letters, or its first letter's.
pub fn preserve_case(matched: &str, with: &str) -> String {
    let Some(first) = matched.chars().next() else { return with.to_string() };
    if !matched.chars().any(char::is_alphabetic) || with.is_empty() {
        return with.to_string();
    }
    if matched.to_uppercase() == matched {
        return with.to_uppercase();
    }
    if matched.to_lowercase() == matched {
        return with.to_lowercase();
    }
    let mut rest = with.chars();
    let head: String = match rest.next() {
        Some(h) if first.is_uppercase() => h.to_uppercase().collect(),
        Some(h) => h.to_lowercase().collect(),
        None => String::new(),
    };
    head + rest.as_str()
}

/// Where a place in a text is after edits (from, to, what replaces it) made in it at once: moved by those before
/// it, at the start of the one it was in.
pub fn shift(at: usize, edits: &[(usize, usize, String)]) -> usize {
    let mut out = at as isize;
    for (from, to, by) in edits {
        if *to <= at {
            out += by.len() as isize - (*to - *from) as isize;
        } else if *from < at {
            out -= (at - *from) as isize;
        }
    }
    out.max(0) as usize
}

/// The word (letters, digits, `_`) a place in a line is in or just after: what Ctrl+F looks for when nothing is
/// selected.
pub fn word_around(line: &str, at: usize) -> Option<(usize, usize)> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let at = at.min(line.len());
    if !line.is_char_boundary(at) {
        return None;
    }
    let from = line[..at].char_indices().rev().take_while(|(_, c)| word(*c)).last().map_or(at, |(i, _)| i);
    let to = at + line[at..].chars().take_while(|c| word(*c)).map(char::len_utf8).sum::<usize>();
    (to > from).then_some((from, to))
}

/// What was looked for (or replaced) kept as the newest of the last 50, once.
pub fn remember(history: &mut Vec<String>, what: &str) {
    if what.is_empty() || history.last().is_some_and(|l| l == what) {
        return;
    }
    history.retain(|h| h != what);
    history.push(what.to_string());
    if history.len() > 50 {
        history.remove(0);
    }
}

/// Up or Down in a box: the older or the newer of what it had (which one it shows now, none for what is typed, and
/// the text); past the newest, an empty box. None where there is nothing further.
pub fn recall(history: &[String], at: Option<usize>, current: &str, up: bool) -> Option<(Option<usize>, String)> {
    let n = history.len();
    // What is in the box is the newest already: Up goes to the one before it.
    let at = at.or_else(|| history.last().is_some_and(|l| l == current).then(|| n - 1));
    match (at, up) {
        (None, true) if n > 0 => Some((Some(n - 1), history[n - 1].clone())),
        (Some(i), true) if i > 0 => Some((Some(i - 1), history[i - 1].clone())),
        (Some(i), false) if i + 1 < n => Some((Some(i + 1), history[i + 1].clone())),
        (Some(_), false) if !current.is_empty() => Some((None, String::new())),
        _ => None,
    }
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

/// The editor drawn for the tab shown.
fn active(ui: Ui) -> Option<Editor> {
    ui.active.get_untracked().and_then(|p| live(ui, &p))
}

/// How the find widget looks for what it looks for now.
fn how(ui: Ui) -> How {
    How { case: ui.find_case.get_untracked(), word: ui.find_word.get_untracked(), regex: ui.find_regex.get_untracked() }
}

thread_local! {
    /// The find widget was asked for with its replace row (Ctrl+H): that box takes the keys when there is something
    /// to look for.
    static TO_REPLACE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The find widget shown, its box taking the keys.
pub fn find_show(ui: Ui) {
    ui.find.set(true);
    ui.find_asked.update(|n| *n += 1);
}

/// The find widget shown with its replace row (Ctrl+H).
pub fn replace_show(ui: Ui) {
    if !ui.find_replacing.get_untracked() {
        ui.find_replacing.set(true);
    }
    TO_REPLACE.with(|t| t.set(true));
    find_show(ui);
}

/// The find widget closed, the keys back in the editor; what it looked for kept, the range it looked in let go.
pub(super) fn find_hide(ui: Ui) {
    ui.find.set(false);
    keep(ui);
    if ui.find_in_selection.get_untracked() {
        ui.find_in_selection.set(false);
    }
    if let Some(editor) = active(ui) {
        focus(&editor);
    }
}

/// What is looked for and what replaces it kept in their boxes' histories.
fn keep(ui: Ui) {
    let (query, with) = (ui.find_query.get_untracked(), ui.find_replace.get_untracked());
    if ui.find_history.with_untracked(|h| !query.is_empty() && h.last() != Some(&query)) {
        ui.find_history.update(|h| remember(h, &query));
    }
    if ui.find_replacing.get_untracked() && ui.replace_history.with_untracked(|h| !with.is_empty() && h.last() != Some(&with)) {
        ui.replace_history.update(|h| remember(h, &with));
    }
}

/// The next match selected, or the previous.
pub(super) fn find_step(ui: Ui, forward: bool) {
    keep(ui);
    let Some(editor) = active(ui) else { return };
    let hits = ui.find_hits.get_untracked();
    if let Some(n) = step(&hits, selection(&editor), forward) {
        select(&editor, hits[n]);
    }
}

/// Replace (one: the match selected, then the next one selected; the next one selected only, when the selection is
/// no match) or Replace All (in one edit, one undo step), worked out off the window's thread over the text as it is
/// now; dropped when the text changed meanwhile.
pub(super) fn replace(ui: Ui, all: bool) {
    let Some(path) = ui.active.get_untracked() else { return };
    let (Some(editor), query) = (live(ui, &path), ui.find_query.get_untracked()) else { return };
    if query.is_empty() {
        return;
    }
    keep(ui);
    let (how, scope, with, preserve) =
        (how(ui), ui.find_scope.get_untracked(), ui.find_replace.get_untracked(), ui.find_preserve.get_untracked());
    let (rope, edited, picked) = (editor.doc().text(), ui.edited.get_untracked(), selection(&editor));
    let done = floem::ext_event::create_ext_action(ui.scope, move |found: Result<Vec<(usize, usize, String)>, ()>| {
        let Ok(mut found) = found else { return };
        let Some(editor) = live(ui, &path).filter(|_| ui.edited.try_get_untracked() == Some(edited)) else { return };
        if found.is_empty() {
            return;
        }
        let doc = editor.doc();
        let edits = if all {
            std::mem::take(&mut found)
        } else if let Some(at) = found.iter().position(|(f, t, _)| (*f, *t) == picked) {
            vec![found[at].clone()]
        } else {
            // The selection is no match: the next one selected, nothing replaced.
            return find_step(ui, true);
        };
        floem::reactive::batch(|| {
            let caret = editor.cursor.with_untracked(|c| c.offset());
            doc.edit(&mut edits.iter().map(|(f, t, by)| (Selection::region(*f, *t), by.as_str())), EditType::Other);
            let len = doc.text().len();
            if let Some((from, to)) = ui.find_scope.get_untracked() {
                ui.find_scope.set(Some((shift(from, &edits), shift(to, &edits).min(len))));
            }
            if all {
                editor.cursor.update(|c| c.set_insert(Selection::caret(shift(caret, &edits).min(len))));
                return;
            }
            // The match after the one replaced (moved by it), or around to the first.
            let (from, to, by) = &edits[0];
            let moved = |h: &(usize, usize, String)| {
                if h.0 >= *to { (shift(h.0, &edits), shift(h.1, &edits)) } else { (h.0, h.1) }
            };
            let next = found.iter().find(|h| h.0 >= *to).or(found.first().filter(|h| h.0 < *from)).map(moved);
            match next {
                Some(hit) => select(&editor, hit),
                None => editor.cursor.update(|c| c.set_insert(Selection::caret((from + by.len()).min(len)))),
            }
        });
    });
    std::thread::spawn(move || done(replacements(&rope.to_string(), &query, how, scope, &with, preserve)));
}

/// The find widget's keys wherever they are pressed (the editor, either box): Ctrl+H, F3 and Shift+F3, Escape, and
/// while it is shown Alt+C, Alt+W, Alt+R, Alt+L and Alt+P (its toggles), Ctrl+Shift+1 (Replace) and Ctrl+Alt+Enter
/// (Replace All). True when it took the key.
pub(super) fn find_key(ui: Ui, key: &Key, mods: Modifiers) -> bool {
    let shown = ui.find.get_untracked();
    let (ctrl, alt, shift) =
        (mods.contains(Modifiers::CONTROL), mods.contains(Modifiers::ALT), mods.contains(Modifiers::SHIFT));
    let flip = |s: RwSignal<bool>| s.update(|b| *b = !*b);
    match key {
        Key::Character(c) if ctrl && !alt && !shift && c.eq_ignore_ascii_case("h") => replace_show(ui),
        Key::Character(c) if shown && ctrl && shift && !alt && (c == "1" || c == "!") => replace(ui, false),
        Key::Named(NamedKey::Enter) if shown && ctrl && alt => replace(ui, true),
        Key::Character(c) if shown && alt && !ctrl && !shift => match c.to_lowercase().as_str() {
            "c" => flip(ui.find_case),
            "w" => flip(ui.find_word),
            "r" => flip(ui.find_regex),
            "l" => flip(ui.find_in_selection),
            "p" => flip(ui.find_preserve),
            _ => return false,
        },
        Key::Named(NamedKey::F3) if !ctrl && !alt => find_step(ui, !shift),
        Key::Named(NamedKey::Escape) if shown => find_hide(ui),
        _ => return false,
    }
    true
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
        let (how, scope) = (how(ui), ui.find_scope.get_untracked());
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
        std::thread::spawn(move || done(matches(&rope.to_string(), &query, how, scope)));
    };
    // Find in Selection turned on: the range it looks in is the selection then (the caret's line when nothing is
    // selected); turned off, the whole text.
    create_effect(move |_| {
        let on = ui.find_in_selection.get();
        let scope = if on {
            active(ui).map(|editor| {
                let (from, to) = selection(&editor);
                if from < to {
                    return (from, to);
                }
                let text = editor.rope_text();
                let line = text.line_of_offset(from);
                (text.offset_of_line(line), text.line_end_offset(line, true))
            })
        } else {
            None
        };
        if ui.find_scope.get_untracked() != scope {
            ui.find_scope.set(scope);
        }
    });
    // What is looked for and how, where, and in which file.
    type Asked = ((bool, String, How, Option<(usize, usize)>), Option<String>);
    create_effect(move |last: Option<Asked>| {
        let how = How { case: ui.find_case.get(), word: ui.find_word.get(), regex: ui.find_regex.get() };
        let asked: Asked = ((ui.find.get(), ui.find_query.get(), how, ui.find_scope.get()), ui.active.get());
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
            // Another file shown: its matches, its caret left where it is; a range of the file before let go.
            Some(last) if last.0 == asked.0 => {
                if ui.find_in_selection.get_untracked() {
                    ui.find_in_selection.set(false);
                }
                look(false)
            }
            _ => look(true),
        }
        asked
    });
}

/// A box of the find widget: one line of the window's font, its text taken to `into`, and Up and Down to what it
/// had before (`history`); the keys of `more` first.
fn find_box(
    ui: Ui,
    placeholder: &'static str,
    into: RwSignal<String>,
    history: RwSignal<Vec<String>>,
    more: impl Fn(RwSignal<Editor>, &Key, Modifiers) -> bool + 'static,
) -> (TextEditor, Editor) {
    let t = ui.colors;
    let map = Rc::new(KeypressMap::default());
    // Which of what it had before it shows, none for what is typed.
    let at = create_rw_signal(None::<usize>);
    let styling = SimpleStyling::builder()
        .font_size(13)
        .line_height(18.0)
        .font_family(FamilyOwned::parse_list(&UI_FONT).collect())
        .wrap(WrapMethod::None)
        .build();
    let input = text_editor_keys("", move |editor, press, mods| {
        let KeyInput::Keyboard(key, _) = &press.key else { return default_key(&map, editor, press, mods) };
        if more(editor, key, mods) || find_key(ui, key, mods) {
            return CommandExecuted::Yes;
        }
        let up = match key {
            Key::Named(NamedKey::ArrowUp) if mods.is_empty() => true,
            Key::Named(NamedKey::ArrowDown) if mods.is_empty() => false,
            Key::Character(c) if mods == Modifiers::CONTROL && c.eq_ignore_ascii_case("f") => {
                editor.with_untracked(|e| {
                    let len = e.rope_text().len();
                    e.cursor.update(|c| c.set_insert(Selection::region(0, len)));
                });
                return CommandExecuted::Yes;
            }
            _ => return default_key(&map, editor, press, mods),
        };
        let current = into.get_untracked();
        if let Some((n, text)) = history.with_untracked(|h| recall(h, at.get_untracked(), &current, up)) {
            editor.with_untracked(|e| put(e, &text));
            at.set(n);
        }
        CommandExecuted::Yes
    })
    .styling(styling)
    .placeholder(placeholder)
    .editor_style(move |s| {
        let c = t.get();
        s.hide_gutter(true)
            .cursor_surrounding_lines(0)
            .selection_color(c.selection)
            .cursor_color(c.cursor)
            .placeholder_color(c.placeholder)
    });
    let editor = input.editor().clone();
    // What is in the box (its first line: nothing is looked for across lines); typed, what it shows is no longer
    // one of what it had.
    let input = {
        let doc = Rc::downgrade(&editor.doc());
        input.update(move |_| {
            if let Some(doc) = doc.upgrade() {
                let text = doc.text().to_string();
                let line = text.lines().next().unwrap_or("").to_string();
                if into.with_untracked(|q| *q != line) {
                    let shown = at.get_untracked().and_then(|n| history.with_untracked(|h| h.get(n).cloned()));
                    if shown.as_ref() != Some(&line) && at.get_untracked().is_some() {
                        at.set(None);
                    }
                    into.set(line);
                }
            }
        })
    };
    (input, editor)
}

/// A box's text replaced, the caret at its end.
fn put(editor: &Editor, text: &str) {
    let doc = editor.doc();
    floem::reactive::batch(|| {
        doc.edit_single(Selection::region(0, doc.text().len()), text, EditType::Other);
        editor.cursor.update(|c| c.set_insert(Selection::caret(text.len())));
    });
}

/// A box's border: red when what it looks for is no regular expression, lit while it has the keys.
fn box_style(ui: Ui, s: floem::style::Style, focused: RwSignal<bool>, error: bool) -> floem::style::Style {
    let c = ui.colors.get();
    s.width(240.0)
        .height(24.0)
        .padding_right(2.0)
        .gap(2.0)
        .items_center()
        .border(1.0)
        .border_radius(2.0)
        .border_color(if error {
            c.error
        } else if focused.get() {
            c.focus
        } else {
            c.input_border
        })
        .background(c.input_bg)
}

/// Lit while the keys are in a box.
fn focus_lit(editor: &Editor) -> RwSignal<bool> {
    let focused = create_rw_signal(false);
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
    focused
}

/// A button of the widget with its tooltip.
fn button(ui: Ui, name: &'static str, tip: &'static str, on: impl Fn() + 'static) -> impl IntoView {
    tooltip(action(ui.colors, name, on), move || label(move || tip))
}

/// VS Code's find widget, at the editor's top right: Toggle Replace at its left; the box with its Match Case, Match
/// Whole Word and Use Regular Expression toggles, which match of how many, previous and next, Find in Selection,
/// close; under them, when shown, the replace box with Preserve Case, Replace and Replace All. Enter goes to the
/// next match and Shift+Enter to the previous (in the replace box Enter replaces), Tab goes from box to box, Up and
/// Down to what was looked for before; Escape closes it.
pub(super) fn find_widget(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    follow_find(ui);
    let other = create_rw_signal(None::<Editor>);
    let (find_input, editor) = find_box(ui, "Find", ui.find_query, ui.find_history, move |_, key, mods| {
        let shift = mods.contains(Modifiers::SHIFT);
        match key {
            Key::Named(NamedKey::Enter) if !mods.intersects(Modifiers::CONTROL | Modifiers::ALT) => {
                find_step(ui, !shift)
            }
            Key::Named(NamedKey::Tab) if mods.is_empty() && ui.find_replacing.get_untracked() => {
                if let Some(e) = other.get_untracked() {
                    focus(&e);
                }
            }
            _ => return false,
        }
        true
    });
    let back = create_rw_signal(None::<Editor>);
    let (replace_input, replacer) = find_box(ui, "Replace", ui.find_replace, ui.replace_history, move |_, key, mods| {
        match key {
            Key::Named(NamedKey::Enter) if mods.is_empty() => replace(ui, false),
            Key::Named(NamedKey::Tab) if mods == Modifiers::SHIFT => {
                if let Some(e) = back.get_untracked() {
                    focus(&e);
                }
            }
            _ => return false,
        }
        true
    });
    other.set(Some(replacer.clone()));
    back.set(Some(editor.clone()));
    // Asked for (Ctrl+F, Ctrl+H): the box takes the keys, with the editor's selection in it when it is within a line
    // (the word at the caret when nothing is selected), and all of it selected, so what is typed takes its place.
    // Asked for with the replace row and something to look for, the replace box takes them.
    {
        let (editor, replacer) = (editor.clone(), replacer.clone());
        create_effect(move |first: Option<()>| {
            ui.find_asked.track();
            if first.is_none() {
                return;
            }
            let doc = editor.doc();
            // The text and the selection changed as one (no caret past the end of the text between the two).
            floem::reactive::batch(|| {
                if let Some(main) = active(ui) {
                    let (mut from, mut to) = selection(&main);
                    if from == to {
                        let text = main.rope_text();
                        let line = text.line_of_offset(from);
                        let start = text.offset_of_line(line);
                        let words = text.slice_to_cow(start..text.line_end_offset(line, true)).to_string();
                        if let Some((a, b)) = word_around(&words, from - start) {
                            (from, to) = (start + a, start + b);
                        }
                    }
                    let picked = main.rope_text().slice_to_cow(from..to).to_string();
                    if !picked.is_empty() && !picked.contains('\n') && doc.text().to_string() != picked {
                        doc.edit_single(Selection::region(0, doc.text().len()), &picked, EditType::Other);
                    }
                }
                let len = doc.text().len();
                editor.cursor.update(|c| c.set_insert(Selection::region(0, len)));
            });
            if TO_REPLACE.with(|t| t.replace(false)) && !doc.text().is_empty() {
                let len = replacer.doc().text().len();
                replacer.cursor.update(|c| c.set_insert(Selection::region(0, len)));
                focus(&replacer);
            } else {
                focus(&editor);
            }
        });
    }
    let focused = focus_lit(&editor);
    let replace_focused = focus_lit(&replacer);
    // A toggle clicked: the keys stay in (or go back to) the box it is of.
    {
        let (editor, replacer) = (editor.clone(), replacer.clone());
        create_effect(move |first: Option<()>| {
            ui.find_case.track();
            ui.find_word.track();
            ui.find_regex.track();
            ui.find_in_selection.track();
            if first.is_some() && ui.find.get_untracked() && !replace_focused.get_untracked() {
                focus(&editor);
            }
        });
        create_effect(move |first: Option<()>| {
            ui.find_preserve.track();
            if first.is_some() && ui.find.get_untracked() {
                focus(&replacer);
            }
        });
    }
    let field = h_stack((
        find_input.style(move |s| input_style(ui, s)),
        toggle(t, "case-sensitive", ui.find_case, "Match Case (Alt+C)"),
        toggle(t, "whole-word", ui.find_word, "Match Whole Word (Alt+W)"),
        toggle(t, "regex", ui.find_regex, "Use Regular Expression (Alt+R)"),
    ))
    .style(move |s| box_style(ui, s, focused, ui.find_error.get()));
    let replace_field = h_stack((
        replace_input.style(move |s| input_style(ui, s)),
        toggle(t, "preserve-case", ui.find_preserve, "Preserve Case (Alt+P)"),
    ))
    .style(move |s| box_style(ui, s, replace_focused, false));
    // Which match the editor's selection is, of how many: "3 of 12", "? of 12" between matches, "No results".
    let count = label(move || {
        let hits = ui.find_hits.get();
        if hits.is_empty() {
            return "No results".to_string();
        }
        let more = if hits.len() >= MOST { "+" } else { "" };
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
    let (up, down) = (editor.clone(), editor.clone());
    let find_row = h_stack((
        field,
        count,
        button(ui, "arrow-up", "Previous Match (Shift+Enter)", move || {
            find_step(ui, false);
            focus(&up);
        }),
        button(ui, "arrow-down", "Next Match (Enter)", move || {
            find_step(ui, true);
            focus(&down);
        }),
        toggle(t, "selection", ui.find_in_selection, "Find in Selection (Alt+L)"),
        button(ui, "close", "Close (Escape)", move || find_hide(ui)),
    ))
    .style(|s| s.height(24.0).gap(2.0).items_center());
    let replace_row = h_stack((
        replace_field,
        button(ui, "replace", "Replace (Enter)", move || replace(ui, false)),
        button(ui, "replace-all", "Replace All (Ctrl+Alt+Enter)", move || replace(ui, true)),
    ))
    .style(move |s| s.height(24.0).gap(2.0).items_center().apply_if(!ui.find_replacing.get(), |s| s.hide()));
    let opened = replacer.clone();
    let chevron = action_lit(
        t,
        move || if ui.find_replacing.get() { "chevron-down" } else { "chevron-right" },
        move || {
            let on = !ui.find_replacing.get_untracked();
            ui.find_replacing.set(on);
            if on {
                focus(&opened);
            }
        },
    )
    .style(|s| s.width(16.0).height_full().border_radius(0.0));
    let chevron = tooltip(chevron, || label(|| "Toggle Replace")).style(|s| s.height_full());
    h_stack((chevron, v_stack((find_row, replace_row)).style(|s| s.gap(5.0))))
        .style(move |s| {
            let c = t.get();
            // Over an editor only (not a file's changes, nor the watermark).
            let over = ui.find.get() && ui.live.with(|l| l.is_some());
            line(s, c.focus, 'l')
                .absolute()
                .inset_top(0.0)
                .inset_right(28.0)
                .padding_vert(4.0)
                .padding_left(2.0)
                .padding_right(6.0)
                .gap(4.0)
                .z_index(20)
                .background(c.widget)
                .color(c.fg)
                .box_shadow_blur(8.0)
                .box_shadow_v_offset(2.0)
                .box_shadow_color(c.shadow)
                .apply_if(!over, |s| s.hide())
        })
}

/// A box's own text, filling what its toggles leave.
fn input_style(ui: Ui, s: floem::style::Style) -> floem::style::Style {
    let c = ui.colors.get();
    s.flex_grow(1.0_f32)
        .flex_basis(0.0)
        .min_width(0.0)
        .height(18.0)
        .margin_left(4.0)
        .set(floem::views::editor::Foreground, c.input_fg)
        .color(c.input_fg)
}

/// Its commands, with VS Code's ids and keys: Replace (Ctrl+H) opens the widget with its replace row; while it is
/// shown over an editor, Replace, Replace All, Find Next and Find Previous, and its toggles.
pub fn register(_ui: Ui, _send: &crate::state::Send) {
    use crate::commands::{add, command};
    let over = |ui: Ui| ui.live.with_untracked(|l| l.is_some());
    let shown = move |ui: Ui| ui.find.get_untracked() && over(ui);
    add(command("editor.action.startFindReplaceAction", "", "Replace", |ui, _| replace_show(ui)).keys("Ctrl+H").when(over));
    add(command("editor.action.nextMatchFindAction", "", "Find Next", |ui, _| find_step(ui, true)).keys("F3").when(over));
    add(command("editor.action.previousMatchFindAction", "", "Find Previous", |ui, _| find_step(ui, false))
        .keys("Shift+F3")
        .when(over));
    add(command("editor.action.replaceOne", "", "Replace", |ui, _| replace(ui, false)).keys("Ctrl+Shift+1").when(shown).hidden());
    add(command("editor.action.replaceAll", "", "Replace All", |ui, _| replace(ui, true)).keys("Ctrl+Alt+Enter").when(shown).hidden());
    let toggles: [(&str, &str, &str, fn(Ui) -> RwSignal<bool>); 5] = [
        ("toggleFindCaseSensitive", "Toggle Match Case", "Alt+C", |ui| ui.find_case),
        ("toggleFindWholeWord", "Toggle Match Whole Word", "Alt+W", |ui| ui.find_word),
        ("toggleFindRegex", "Toggle Use Regular Expression", "Alt+R", |ui| ui.find_regex),
        ("toggleFindInSelection", "Toggle Find in Selection", "Alt+L", |ui| ui.find_in_selection),
        ("togglePreserveCase", "Toggle Preserve Case", "Alt+P", |ui| ui.find_preserve),
    ];
    for (id, title, keys, of) in toggles {
        add(command(id, "", title, move |ui, _| of(ui).update(|b| *b = !*b)).keys(keys).when(shown).hidden());
    }
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &serde_json::Value, _send: &crate::state::Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: How = How { case: false, word: false, regex: false };

    #[test]
    fn what_is_looked_for_is_found_as_written_or_as_a_regular_expression() {
        let text = "let Cart = cart(1);\nlet é = a.b + a*b;\n";
        let case = How { case: true, ..ALL };
        let regex = How { regex: true, ..ALL };
        let both = How { case: true, regex: true, ..ALL };
        assert_eq!(matches(text, "cart", ALL, None), Ok(vec![(4, 8), (11, 15)]));
        assert_eq!(matches(text, "cart", case, None), Ok(vec![(11, 15)]));
        // As written: a dot is a dot, a star a star; and places are in bytes, after a letter of two.
        assert_eq!(matches(text, "a.b", ALL, None), Ok(vec![(29, 32)]));
        assert_eq!(matches(text, "a*b", ALL, None), Ok(vec![(35, 38)]));
        assert_eq!(matches(text, "a.b", regex, None), Ok(vec![(29, 32), (35, 38)]));
        assert_eq!(matches(text, "^let", both, None), Ok(vec![(0, 3), (20, 23)]));
        // What matches nothing is no match, and what is not a regular expression is said not to be one.
        assert_eq!(matches(text, "x*", both, None), Ok(vec![]));
        assert_eq!(matches(text, "(", both, None), Err(()));
        assert_eq!(matches(text, "(", case, None), Ok(vec![(15, 16)]));
        assert_eq!(matches(text, "", ALL, None), Ok(vec![]));
    }

    #[test]
    fn a_whole_word_stands_between_separators_and_a_range_bounds_the_matches() {
        let text = "cart carts cart_id (cart) a.cart";
        let word = How { word: true, ..ALL };
        assert_eq!(matches(text, "cart", word, None), Ok(vec![(0, 4), (20, 24), (28, 32)]));
        // A separator at a match's own end makes that end a word's.
        assert_eq!(matches(text, ".cart", word, None), Ok(vec![(27, 32)]));
        assert_eq!(matches(text, "cart", ALL, Some((5, 18))), Ok(vec![(5, 9), (11, 15)]));
        // A match running past the range is out of it.
        assert_eq!(matches(text, "cart", ALL, Some((0, 7))), Ok(vec![(0, 4)]));
        // `^` is the start of a line still, within a range.
        assert_eq!(matches("ab\nab", "^a", How { regex: true, ..ALL }, Some((1, 5))), Ok(vec![(3, 4)]));
    }

    #[test]
    fn replacements_take_the_groups_and_the_case_of_the_match() {
        let regex = How { regex: true, ..ALL };
        assert_eq!(
            replacements("x = 1, y = 22", r"(\w) = (\d+)", regex, None, "$2 := $1 ($&) $$\\t", false),
            Ok(vec![(0, 5, "1 := x (x = 1) $\t".into()), (7, 13, "22 := y (y = 22) $\t".into())])
        );
        // No such group: written as it is.
        assert_eq!(expand("$3$1", &[Some("ab"), Some("a")]), "$3a");
        assert_eq!(expand("$10", &[Some("ab"), Some("a")]), "a0");
        // As written, without a regular expression.
        assert_eq!(replacements("a.b a.b", "a.b", ALL, None, "$1", false), Ok(vec![(0, 3, "$1".into()), (4, 7, "$1".into())]));
        assert_eq!(
            replacements("Cart CART cart", "cart", ALL, None, "basket", true).map(|r| r.into_iter().map(|e| e.2).collect::<Vec<_>>()),
            Ok(vec!["Basket".to_string(), "BASKET".into(), "basket".into()])
        );
        assert_eq!(preserve_case("cArt", "Basket"), "basket");
        assert_eq!(preserve_case("123", "Basket"), "Basket");
        assert_eq!(preserve_case("Cart", ""), "");
    }

    #[test]
    fn places_move_with_the_edits_made_before_them() {
        let edits = vec![(2, 4, "xyz".to_string()), (10, 12, String::new())];
        assert_eq!(shift(0, &edits), 0);
        assert_eq!(shift(2, &edits), 2);
        assert_eq!(shift(3, &edits), 2);
        assert_eq!(shift(4, &edits), 5);
        assert_eq!(shift(12, &edits), 11);
        assert_eq!(shift(20, &edits), 19);
    }

    #[test]
    fn the_word_at_the_caret_and_the_boxes_history() {
        assert_eq!(word_around("let cart_id = 1;", 6), Some((4, 11)));
        assert_eq!(word_around("let cart_id = 1;", 11), Some((4, 11)));
        assert_eq!(word_around("let cart_id = 1;", 12), None);
        assert_eq!(word_around("é", 1), None);
        let mut h = Vec::new();
        for w in ["a", "b", "a", "", "c", "c"] {
            remember(&mut h, w);
        }
        assert_eq!(h, ["b", "a", "c"]);
        // Up from what is typed: the newest; from the newest shown, the one before.
        assert_eq!(recall(&h, None, "x", true), Some((Some(2), "c".into())));
        assert_eq!(recall(&h, None, "c", true), Some((Some(1), "a".into())));
        assert_eq!(recall(&h, Some(0), "b", true), None);
        assert_eq!(recall(&h, Some(1), "a", false), Some((Some(2), "c".into())));
        // Past the newest: an empty box, and then nothing further.
        assert_eq!(recall(&h, Some(2), "c", false), Some((None, String::new())));
        assert_eq!(recall(&h, None, "", false), None);
        assert_eq!(recall(&[], None, "", true), None);
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
