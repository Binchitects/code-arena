//! An edit's diff under its call, and the answer's "N files changed", as VS Code's chat shows what an edit did.
//!
//! Under each edit that wrote its file: "Changes to PATH" with a chevron (open while the turn runs), "new file"
//! when the edit made it, and its "+N −M". Opened, it lists the changed lines with three around them, each with its
//! number in the old and the new text (the new one opens the file at that line), a "⋮" where lines are skipped,
//! removed lines red and added ones green, and "… N more changed lines" when the session listed only the first.
//! The path opens the file at its first change. An answer whose one call is an edit that went well shows only the
//! diff, not the call's card.
//!
//! Under a finished answer that edited files: "N files changed" with the totals, each file once with its own, and
//! a file opens the agent's compare of it (the Changes view's), else the file at its first change. What the session
//! says of a diff is in `model::EditDiff`; the driver works it out as the edit ends.

use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::CursorStyle;
use floem::views::{Decorators, dyn_container, empty, h_stack, label, v_stack, v_stack_from_iter};
use floem::{AnyView, IntoView};
use serde_json::Value;

use super::slots::{self, ItemSlot};
use crate::icons::icon;
use crate::model::{DiffLine, EditDiff, Item, Kind, Tool};
use crate::state::{Send, Ui};
use crate::widgets::{MONO_FONT, bold};

/// The most rows of a diff drawn (the session lists at most 200 changed lines and those around them).
const MOST_ROWS: usize = 600;

/// A row of a diff as it is drawn: a line, or the mark where lines are skipped.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Line(DiffLine),
    Gap,
}

/// A diff's rows: its lines, with a gap between two whose numbers do not follow each other (lines skipped).
pub fn rows(d: &EditDiff) -> Vec<Row> {
    let mut out = Vec::with_capacity(d.lines.len() + 4);
    let mut last: Option<(usize, usize)> = None;
    for l in &d.lines {
        if let Some((old, new)) = last {
            let follows = (l.old == 0 || old == 0 || l.old == old + 1) && (l.new == 0 || new == 0 || l.new == new + 1);
            if !follows {
                out.push(Row::Gap);
            }
        }
        // A side the line is not in keeps the number before it, so the next line of that side is read against it.
        let (prev_old, prev_new) = last.unwrap_or((0, 0));
        last = Some((if l.old > 0 { l.old } else { prev_old }, if l.new > 0 { l.new } else { prev_new }));
        out.push(Row::Line(l.clone()));
    }
    out
}

/// The line of the new text a diff's first change is at (1 when it says none): a removed line's is that of the
/// line after it.
pub fn first_line(d: &EditDiff) -> usize {
    let Some(at) = d.lines.iter().position(|l| l.op != ' ') else { return 1 };
    let line = d.lines[at..].iter().find(|l| l.new > 0).map(|l| l.new);
    let before = d.lines[..at].iter().rev().find(|l| l.new > 0).map(|l| l.new + 1);
    line.or(before).unwrap_or(1).max(1)
}

/// "+N −M", as VS Code's chat counts an edit's lines.
pub fn counts(added: usize, removed: usize) -> (String, String) {
    (format!("+{added}"), format!("\u{2212}{removed}"))
}

/// "… N more changed lines".
pub fn more_line(n: usize) -> String {
    if n == 1 { "\u{2026} 1 more changed line".into() } else { format!("\u{2026} {n} more changed lines") }
}

/// "N files changed".
pub fn files_line(n: usize) -> String {
    if n == 1 { "1 file changed".into() } else { format!("{n} files changed") }
}

/// A file an answer changed: its path, its lines added and removed by all its edits, whether an edit made it, and
/// the line of its first change.
#[derive(Clone, Debug, PartialEq)]
pub struct Total {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub created: bool,
    pub first: usize,
}

/// The files an answer's edits changed, each once in the order first edited.
pub fn totals(tools: &[Tool]) -> Vec<Total> {
    let mut out: Vec<Total> = Vec::new();
    for d in tools.iter().filter(|t| t.ok).filter_map(|t| t.diff.as_deref()) {
        match out.iter_mut().find(|f| f.path == d.path) {
            Some(f) => {
                f.added += d.added;
                f.removed += d.removed;
            }
            None => out.push(Total {
                path: d.path.clone(),
                added: d.added,
                removed: d.removed,
                created: d.created,
                first: first_line(d),
            }),
        }
    }
    out
}

/// Whether an answer's one call is an edit that went well: it shows only its diff.
pub fn lone(tools: &[Tool]) -> bool {
    matches!(tools, [t] if t.done && t.ok && t.diff.is_some())
}

/// A path's file name, and the folder it is in (empty: at the root).
fn split(path: &str) -> (String, String) {
    match path.rsplit_once('/') {
        Some((dir, name)) => (name.to_string(), dir.to_string()),
        None => (path.to_string(), String::new()),
    }
}

/// The "+N −M" of a file or an edit, green and red.
fn count_labels(ui: Ui, added: usize, removed: usize) -> impl IntoView {
    let t = ui.colors;
    let (plus, minus) = counts(added, removed);
    h_stack((
        label(move || plus.clone()).style(move |s| s.color(t.get().added_fg)),
        label(move || minus.clone()).style(move |s| s.color(t.get().removed_fg)),
    ))
    .style(|s| s.gap(4.0).font_size(12.0).flex_shrink(0.0_f32).selectable(false))
}

/// The diff under a call: none when it has none. Its chevron is the person's, else open while the turn runs.
pub(super) fn under(ui: Ui, send: Send, me: Memo<Tool>) -> impl IntoView {
    let diff = create_memo(move |_| me.with(|m| if m.ok { m.diff.clone() } else { None }));
    dyn_container(
        move || diff.get(),
        move |d| match d {
            Some(d) => change(ui, send.clone(), d).into_any(),
            None => empty().into_any(),
        },
    )
    .style(|s| s.width_full().min_width(0.0))
}

/// One edit's "Changes to PATH" and, opened, its lines.
fn change(ui: Ui, send: Send, d: std::rc::Rc<EditDiff>) -> impl IntoView {
    let t = ui.colors;
    let running = ui.running;
    let chosen: RwSignal<Option<bool>> = floem::reactive::create_rw_signal(None);
    let open = create_memo(move |_| chosen.get().unwrap_or_else(|| running.get()));
    let path = d.path.clone();
    let first = first_line(&d);
    let created = d.created;
    let link_send = send.clone();
    let head = h_stack((
        dyn_container(move || open.get(), |o| icon(if o { "chevron-down" } else { "chevron-right" }, 16.0)),
        label(|| "Changes to").style(move |s| s.color(t.get().desc).flex_shrink(0.0_f32)),
        label(move || path.clone())
            .on_click_stop({
                let path = d.path.clone();
                move |_| ui.open_at(&path, first, &link_send)
            })
            .style(move |s| {
                let c = t.get();
                s.font_family(MONO_FONT.clone())
                    .font_size(12.0)
                    .color(c.link)
                    .text_ellipsis()
                    .min_width(0.0)
                    .flex_shrink(1.0_f32)
                    .hover(|s| s.color(c.link_active))
            }),
        label(|| "new file").style(move |s| {
            let c = t.get();
            s.font_size(11.0)
                .padding_horiz(4.0)
                .border(1.0)
                .border_radius(3.0)
                .border_color(c.widget_border)
                .color(c.desc)
                .flex_shrink(0.0_f32)
                .apply_if(!created, |s| s.hide())
        }),
        empty().style(|s| s.flex_grow(1.0_f32)),
        count_labels(ui, d.added, d.removed),
    ))
    .on_click_stop(move |_| chosen.set(Some(!open.get_untracked())))
    .style(move |s| {
        let c = t.get();
        s.gap(6.0)
            .items_center()
            .padding_horiz(8.0)
            .padding_vert(4.0)
            .width_full()
            .min_width(0.0)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(c.list_hover))
    });
    let body = dyn_container(
        move || open.get(),
        move |o| if o { lines(ui, send.clone(), &d).into_any() } else { empty().into_any() },
    )
    .style(|s| s.width_full().min_width(0.0));
    v_stack((head, body)).style(move |s| {
        let c = t.get();
        s.width_full().min_width(0.0).border(1.0).border_color(c.widget_border).border_radius(4.0)
    })
}

/// A diff's lines: numbers in the old and the new text, its mark, and its text; red removed, green added.
fn lines(ui: Ui, send: Send, d: &EditDiff) -> impl IntoView {
    let t = ui.colors;
    let path = d.path.clone();
    let mut views: Vec<AnyView> = Vec::new();
    for r in rows(d).into_iter().take(MOST_ROWS) {
        views.push(match r {
            Row::Gap => label(|| "\u{22ee}")
                .style(move |s| s.padding_left(30.0).height(18.0).color(t.get().line_number).selectable(false))
                .into_any(),
            Row::Line(l) => line(ui, send.clone(), &path, l).into_any(),
        });
    }
    if d.more > 0 {
        let more = more_line(d.more);
        views.push(label(move || more.clone()).style(move |s| s.padding(6.0).color(t.get().desc)).into_any());
    }
    v_stack_from_iter(views).style(move |s| {
        let c = t.get();
        s.width_full()
            .min_width(0.0)
            .padding_vert(2.0)
            .border_top(1.0)
            .border_color(c.widget_border)
            .background(c.editor_bg)
            .font_family(MONO_FONT.clone())
            .font_size(12.0)
    })
}

/// One line of a diff.
fn line(ui: Ui, send: Send, path: &str, l: DiffLine) -> impl IntoView {
    let t = ui.colors;
    let number = move |n: usize| {
        label(move || if n > 0 { n.to_string() } else { String::new() }).style(move |s| {
            s.width(36.0).justify_end().padding_right(6.0).flex_shrink(0.0_f32).color(t.get().line_number)
        })
    };
    let new = l.new;
    let path = path.to_string();
    let op = l.op;
    let mark = match op {
        '-' => "-",
        '+' => "+",
        _ => " ",
    };
    let text = l.text.replace('\t', "    ");
    h_stack((
        number(l.old).style(|s| s.selectable(false)),
        number(new)
            .on_click_stop(move |_| {
                if new > 0 {
                    ui.open_at(&path, new, &send);
                }
            })
            .style(move |s| s.selectable(false).apply_if(new > 0, |s| s.cursor(CursorStyle::Pointer))),
        label(move || mark).style(|s| s.width(14.0).flex_shrink(0.0_f32).selectable(false)),
        label(move || text.clone()).style(|s| s.text_ellipsis().min_width(0.0).flex_shrink(1.0_f32)),
    ))
    .style(move |s| {
        let c = t.get();
        let bg = match op {
            '-' => c.diff_removed,
            '+' => c.diff_added,
            _ => floem::peniko::Color::TRANSPARENT,
        };
        s.height(18.0).width_full().min_width(0.0).items_center().background(bg).color(c.editor_fg)
    })
}

/// Under a finished answer: the files its edits changed.
fn changed(ui: Ui, send: Send, item: &Item) -> AnyView {
    let Kind::Assistant(answer) = &item.kind else { return empty().into_any() };
    let t = ui.colors;
    let id = item.id;
    let tools = answer.tools;
    let running = ui.running;
    let items = ui.items;
    let files = create_memo(move |_| {
        let writing = running.get() && items.with(|l| l.last().is_some_and(|i| i.id == id));
        if writing { Vec::new() } else { tools.with(|ts| totals(ts)) }
    });
    let open: RwSignal<bool> = floem::reactive::create_rw_signal(true);
    let head = h_stack((
        dyn_container(move || open.get(), |o| icon(if o { "chevron-down" } else { "chevron-right" }, 16.0)),
        label(move || files.with(|f| files_line(f.len()))).style(move |s| bold(s).color(t.get().fg)),
        dyn_container(
            move || files.with(|f| (f.iter().map(|f| f.added).sum::<usize>(), f.iter().map(|f| f.removed).sum())),
            move |(a, r)| count_labels(ui, a, r).into_any(),
        ),
    ))
    .on_click_stop(move |_| open.update(|o| *o = !*o))
    .style(move |s| {
        let c = t.get();
        s.gap(6.0)
            .items_center()
            .padding_horiz(8.0)
            .padding_vert(4.0)
            .width_full()
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(c.list_hover))
    });
    let list = dyn_container(
        move || (open.get(), files.get()),
        move |(o, fs)| {
            if !o {
                return empty().into_any();
            }
            let rows: Vec<AnyView> = fs.into_iter().map(|f| file_row(ui, send.clone(), f).into_any()).collect();
            v_stack_from_iter(rows).style(|s| s.width_full().padding_bottom(4.0)).into_any()
        },
    )
    .style(|s| s.width_full());
    v_stack((head, list))
        .style(move |s| {
            let c = t.get();
            s.width_full()
                .min_width(0.0)
                .margin_top(8.0)
                .border(1.0)
                .border_color(c.widget_border)
                .border_radius(4.0)
                .apply_if(files.with(|f| f.is_empty()), |s| s.hide())
        })
        .into_any()
}

/// A file of "N files changed": its name, its folder, "new file", its counts; it opens the agent's compare of it.
fn file_row(ui: Ui, send: Send, f: Total) -> impl IntoView {
    let t = ui.colors;
    let (name, dir) = split(&f.path);
    let path = f.path.clone();
    let first = f.first;
    let created = f.created;
    h_stack((
        icon(if created { "new-file" } else { "file" }, 14.0),
        label(move || name.clone()).style(move |s| s.color(t.get().fg).flex_shrink(0.0_f32)),
        label(move || dir.clone()).style(move |s| s.color(t.get().desc).font_size(12.0).text_ellipsis().min_width(0.0)),
        empty().style(|s| s.flex_grow(1.0_f32)),
        count_labels(ui, f.added, f.removed),
    ))
    .on_click_stop(move |_| open_compare(ui, &send, &path, first))
    .style(move |s| {
        let c = t.get();
        s.gap(6.0)
            .items_center()
            .padding_left(28.0)
            .padding_right(8.0)
            .padding_vert(2.0)
            .width_full()
            .min_width(0.0)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(c.list_hover))
    })
}

/// The agent's compare of a file (what it was before the agent's first change in this run, and is now), else the
/// file at its first change.
fn open_compare(ui: Ui, send: &Send, path: &str, first: usize) {
    match ui.changes.with_untracked(|cs| cs.iter().find(|c| c.path == path).cloned()) {
        Some(c) => ui.open_changes(&c),
        None => ui.open_at(path, first, send),
    }
}

/// What it adds to the window's registries, once at start: "N files changed" under an answer.
pub fn register(_ui: Ui, _send: &Send) {
    slots::add_item(ItemSlot::AnswerFoot, "files-changed", 10, |ui, send, item| changed(ui, send, item));
}

/// The session's messages are read by the calls' cards (`tools.rs`), whose calls carry their diffs: none is
/// taken here.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

    fn l(op: char, old: usize, new: usize) -> DiffLine {
        DiffLine { op, old, new, text: String::new() }
    }

    fn diff(path: &str, lines: Vec<DiffLine>) -> EditDiff {
        let added = lines.iter().filter(|l| l.op == '+').count();
        let removed = lines.iter().filter(|l| l.op == '-').count();
        EditDiff { path: path.into(), added, removed, lines, ..EditDiff::default() }
    }

    #[test]
    fn lines_that_do_not_follow_each_other_have_a_gap_between_them() {
        let d = diff("a", vec![l(' ', 1, 1), l('-', 2, 0), l('+', 0, 2), l(' ', 3, 3), l(' ', 20, 20), l('+', 0, 21)]);
        let gaps: Vec<usize> = rows(&d).iter().enumerate().filter(|(_, r)| **r == Row::Gap).map(|(i, _)| i).collect();
        assert_eq!(gaps, vec![4]);
        assert_eq!(rows(&diff("a", vec![l('+', 0, 1), l('+', 0, 2)])).len(), 2);
    }

    #[test]
    fn the_first_change_is_its_line_in_the_new_text_and_a_removal_the_line_after_it() {
        assert_eq!(first_line(&diff("a", vec![l(' ', 3, 3), l('-', 4, 0), l('+', 0, 4)])), 4);
        assert_eq!(first_line(&diff("a", vec![l(' ', 3, 3), l('-', 4, 0), l(' ', 5, 4)])), 4);
        assert_eq!(first_line(&diff("a", vec![l('-', 1, 0)])), 1);
        assert_eq!(first_line(&EditDiff::default()), 1);
    }

    #[test]
    fn the_counts_and_the_lines_are_said_as_vs_code_says_them() {
        assert_eq!(counts(3, 1), ("+3".to_string(), "\u{2212}1".to_string()));
        assert_eq!(more_line(1), "\u{2026} 1 more changed line");
        assert_eq!(more_line(7), "\u{2026} 7 more changed lines");
        assert_eq!((files_line(1).as_str(), files_line(2).as_str()), ("1 file changed", "2 files changed"));
        assert_eq!(split("src/a/b.rs"), ("b.rs".to_string(), "src/a".to_string()));
        assert_eq!(split("b.rs"), ("b.rs".to_string(), String::new()));
    }

    fn edit(d: EditDiff, ok: bool) -> Tool {
        Tool { name: "edit_file".into(), done: true, ok, diff: Some(Rc::new(d)), ..Tool::default() }
    }

    #[test]
    fn each_file_is_counted_once_with_all_its_edits_and_a_failed_edit_not_at_all() {
        let a1 = diff("a.rs", vec![l(' ', 9, 9), l('+', 0, 10)]);
        let b = EditDiff { created: true, ..diff("b.rs", vec![l('+', 0, 1), l('+', 0, 2)]) };
        let a2 = diff("a.rs", vec![l('-', 3, 0)]);
        let read = Tool { name: "read_file".into(), done: true, ok: true, ..Tool::default() };
        let tools = vec![edit(a1, true), read, edit(b, true), edit(a2, true), edit(diff("c.rs", vec![]), false)];
        let t = totals(&tools);
        assert_eq!(t.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["a.rs", "b.rs"]);
        assert_eq!((t[0].added, t[0].removed, t[0].first), (1, 1, 10));
        assert!(t[1].created && !t[0].created);
    }

    #[test]
    fn only_an_answers_one_edit_that_went_well_shows_its_diff_alone() {
        let d = diff("a", vec![l('+', 0, 1)]);
        assert!(lone(&[edit(d.clone(), true)]));
        assert!(!lone(&[edit(d.clone(), false)]));
        assert!(!lone(&[edit(d.clone(), true), edit(d, true)]));
        assert!(!lone(&[Tool { done: true, ok: true, ..Tool::default() }]));
    }
}
