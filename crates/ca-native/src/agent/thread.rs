//! The thread: the requests, the answers, the notices and the agent's questions, one after the other.
//!
//! It follows new content only while it is read at its end (within [`NEAR_END`] px of the bottom); scrolled up, it
//! stays where it is and 'Jump to the latest' takes it back down. Its top offers 'Show earlier (N more turns)' while
//! the session has turns before those drawn: each press asks the session for the 30 turns before
//! (`older{before_turn}`) and puts them above, the place read kept. Under the last item, while a turn runs:
//! 'Working…' until the answer begins, a blinking block while it is written, 'Waiting for the model…' between its
//! steps. What Code Arena says during a turn (a retry, a wait) is a warning line atop the answer; a failed turn's
//! reason is the last line of the thread (`answer.rs`).
//!
//! A face that comes while a turn runs is sent the thread before the turn (`state{running: true}`) and then every
//! message of the turn so far again (`replay: true`): they are taken as they came, so it sees the turn from its start.

use floem::IntoView;
use floem::kurbo::{Point, Rect};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, Position, TextOverflow};
use floem::views::{Decorators, dyn_container, dyn_stack, h_stack, label, scroll, stack, v_stack};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::model::{Item, Kind};
use crate::state::{Send, Ui};

/// How near the bottom (px) the thread is still read at its end, and so follows what comes.
pub const NEAR_END: f64 = 80.0;

/// The space between the thread's rows.
const GAP: f64 = 16.0;

/// Where the running turn stands, for the line under the thread.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// No turn runs.
    Idle,
    /// The turn began and nothing of the answer came yet.
    Working,
    /// Its words are being written.
    Writing,
    /// A call runs or waits for the person: its card says so, the line nothing.
    Calling,
    /// A step ended (its calls ran): the model is asked again.
    Waiting,
}

/// The thread's own state: where it is scrolled, what it holds before what is drawn, how the turn stands.
#[derive(Clone, Copy)]
struct Follow {
    /// Read at its end as last scrolled: what comes is followed.
    stick: RwSignal<bool>,
    /// The viewport's top and height, and the content's height.
    top: RwSignal<f64>,
    seen: RwSignal<f64>,
    content: RwSignal<f64>,
    /// Where to scroll, told by a new count each time.
    to: RwSignal<(u64, f64)>,
    /// The content's height before earlier turns were put above: the place is kept by what they add.
    above: RwSignal<Option<f64>>,
    /// The turns before those drawn, the first one drawn, and the page asked for (by its `before_turn`).
    earlier: RwSignal<i64>,
    first_turn: RwSignal<i64>,
    asked: RwSignal<Option<i64>>,
    /// The height 'Show earlier' takes over the first turn, as last laid out.
    row: RwSignal<f64>,
    phase: RwSignal<Phase>,
    /// The block's blink and the dots' count, and whether their timer runs.
    tick: RwSignal<u32>,
    ticking: RwSignal<bool>,
}

impl crate::ext::State for Follow {
    fn new(ui: Ui) -> Follow {
        let s = ui.scope;
        Follow {
            stick: s.create_rw_signal(true),
            top: s.create_rw_signal(0.0),
            seen: s.create_rw_signal(0.0),
            content: s.create_rw_signal(0.0),
            to: s.create_rw_signal((0, 0.0)),
            above: s.create_rw_signal(None),
            earlier: s.create_rw_signal(0),
            first_turn: s.create_rw_signal(1),
            asked: s.create_rw_signal(None),
            row: s.create_rw_signal(0.0),
            phase: s.create_rw_signal(Phase::Idle),
            tick: s.create_rw_signal(0),
            ticking: s.create_rw_signal(false),
        }
    }
}

/// Sets a signal only when it changes (what reads it is not told of an equal value).
fn change<T: PartialEq + Clone + 'static>(s: RwSignal<T>, v: T) {
    if s.with_untracked(|old| *old != v) {
        s.set(v);
    }
}

/// Whether a viewport at `top`, `seen` high, over content `content` high, is read at its end.
pub fn near_end(top: f64, seen: f64, content: f64) -> bool {
    content - (top + seen) <= NEAR_END
}

/// Whether the thread still follows after its viewport changed: decided again only when its top moved (the person
/// scrolled); a viewport that only changed size (a bar shown over the box, the window made smaller) keeps it.
pub fn stays(stick: bool, old_top: f64, top: f64, seen: f64, content: f64) -> bool {
    if stick && (top - old_top).abs() < 0.5 { true } else { near_end(top, seen, content) }
}

/// Where the viewport goes when the content's height changes from `old` to `new`: kept by what was put above
/// (`above`: the height before), to the end when it is read there, else nowhere.
pub fn scrolled(top: f64, stick: bool, above: Option<f64>, new: f64) -> Option<f64> {
    match above {
        Some(before) => Some(top + (new - before)),
        None if stick => Some(new),
        None => None,
    }
}

/// The content's height the place is kept from when earlier turns come: all of it, less 'Show earlier' (`row`
/// high, and the gap under it) when those were the last turns before, since it goes then.
pub fn kept_from(content: f64, row: f64, earlier_was: i64, earlier: i64) -> f64 {
    if earlier_was > 0 && earlier <= 0 { content - row - GAP } else { content }
}

/// The phase a message of the session leads to (none: it does not change it).
pub fn phase_of(m: &Value, now: Phase) -> Option<Phase> {
    Some(match m["type"].as_str()? {
        "turn_start" => Phase::Working,
        "turn_end" => Phase::Idle,
        "text" if now != Phase::Idle => Phase::Writing,
        // A call or a question card shows itself how it stands.
        "tool_start" | "tool_waiting" | "ask" if now != Phase::Idle => Phase::Calling,
        "step_end" if now != Phase::Idle => Phase::Waiting,
        _ => return None,
    })
}

/// 'Show earlier (N more turns)'.
pub fn earlier_words(n: i64) -> String {
    if n == 1 { "Show earlier (1 more turn)".into() } else { format!("Show earlier ({n} more turns)") }
}

impl Follow {
    fn scroll_to(self, y: f64) {
        let (n, _) = self.to.get_untracked();
        self.to.set((n + 1, y));
    }

    /// To the end, and following again.
    fn follow_on(self) {
        change(self.stick, true);
        self.scroll_to(self.content.get_untracked().max(1.0e7));
    }
}

pub(super) fn thread(ui: Ui, send: Send) -> impl IntoView {
    let f = ui.state::<Follow>();
    // Whether there is a thread at all, told only when that changes: the thread is built once, not again at each
    // message (Floem's dyn_container builds again whenever what it reads is set, and building it again under an
    // answer being drawn drops what the answer's blocks read).
    let empty = floem::reactive::create_memo(move |_| ui.items.with(|l| l.is_empty()));
    dyn_container(
        move || empty.get(),
        move |empty| {
            if empty {
                return super::composer::welcome(ui).into_any();
            }
            let send = send.clone();
            let ask = send.clone();
            let list = v_stack((
                earlier_row(ui, ask),
                dyn_stack(move || ui.items.get(), |i: &Item| i.id, move |i: Item| item(ui, i, send.clone()))
                    .style(|s| s.flex_col().width_full().gap(16.0)),
                tail(ui),
            ))
            .on_resize(move |r: Rect| {
                let new = r.height();
                if (new - f.content.get_untracked()).abs() < 0.5 {
                    return;
                }
                let to = scrolled(f.top.get_untracked(), f.stick.get_untracked(), f.above.get_untracked(), new);
                f.content.set(new);
                change(f.above, None);
                if let Some(y) = to {
                    f.scroll_to(y);
                }
            })
            .style(|s| s.flex_col().width_full().padding_horiz(16.0).padding_vert(12.0).gap(GAP));
            let view = scroll(list)
                .on_scroll(move |r: Rect| {
                    let (old, content) = (f.top.get_untracked(), f.content.get_untracked());
                    change(f.top, r.y0);
                    change(f.seen, r.height());
                    // Content put above moves the place itself, and is not the person scrolling.
                    if f.above.get_untracked().is_some() {
                        return;
                    }
                    let stick = stays(f.stick.get_untracked(), old, r.y0, r.height(), content);
                    change(f.stick, stick);
                    // Followed, but the viewport changed size (a bar over the box, the window): to the end again.
                    if stick && !near_end(r.y0, r.height(), content) {
                        f.scroll_to(content);
                    }
                })
                .scroll_to(move || {
                    let (n, y) = f.to.get();
                    (n > 0).then_some(Point::new(0.0, y))
                })
                .style(|s| s.size_full());
            stack((view, jump(ui))).style(|s| s.size_full()).into_any()
        },
    )
    .style(move |s| s.flex_grow(1.0_f32).flex_basis(0.0).min_height(0.0).width_full())
}

/// 'Show earlier (N more turns)' over the first turn drawn, while there are turns before it.
fn earlier_row(ui: Ui, send: Send) -> impl IntoView {
    let (f, t) = (ui.state::<Follow>(), ui.colors);
    let words = move || {
        if f.asked.get().is_some() { "Loading…".to_string() } else { earlier_words(f.earlier.get()) }
    };
    label(words)
        .on_resize(move |r: Rect| {
            if r.height() > 0.0 {
                change(f.row, r.height());
            }
        })
        .on_click_stop(move |_| {
            if f.asked.get_untracked().is_some() {
                return;
            }
            let before = f.first_turn.get_untracked();
            f.asked.set(Some(before));
            send(json!({"type": "older", "before_turn": before}));
        })
        .style(move |s| {
            let t = t.get();
            s.apply_if(f.earlier.get() <= 0, |s| s.hide())
                .margin_horiz(floem::unit::PxPctAuto::Auto)
                .font_size(12.0)
                .color(t.link)
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.color(t.link_active))
        })
}

/// 'Jump to the latest', over the thread's bottom while it is scrolled up from its end.
fn jump(ui: Ui) -> impl IntoView {
    let (f, t) = (ui.state::<Follow>(), ui.colors);
    let away = floem::reactive::create_memo(move |_| {
        !f.stick.get() && f.content.get() > f.seen.get() + NEAR_END && f.above.get().is_none()
    });
    h_stack((icon("arrow-down", 14.0), label(|| "Jump to the latest"))).on_click_stop(move |_| f.follow_on()).style(
        move |s| {
            let t = t.get();
            s.apply_if(!away.get(), |s| s.hide())
                .position(Position::Absolute)
                .inset_bottom(10.0)
                .inset_left_pct(50.0)
                .margin_left(-70.0)
                .width(140.0)
                .height(24.0)
                .justify_center()
                .items_center()
                .gap(4.0)
                .font_size(12.0)
                .border_radius(12.0)
                .border(1.0)
                .border_color(t.widget_border)
                .background(t.button2_bg)
                .color(t.button2_fg)
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.background(t.button2_hover))
        },
    )
}

/// Under the last item while a turn runs: 'Working…', the block of words being written, 'Waiting for the model…'.
fn tail(ui: Ui) -> impl IntoView {
    let (f, t) = (ui.state::<Follow>(), ui.colors);
    let words = move || match f.phase.get() {
        Phase::Idle | Phase::Calling => String::new(),
        Phase::Working => "Working…".into(),
        Phase::Writing => if f.tick.get() % 2 == 0 { "▋" } else { " " }.into(),
        Phase::Waiting => format!("Waiting for the model{}", ".".repeat(1 + (f.tick.get() % 3) as usize)),
    };
    label(words).style(move |s| {
        let t = t.get();
        s.apply_if(matches!(f.phase.get(), Phase::Idle | Phase::Calling), |s| s.hide())
            .font_size(12.0)
            .color(t.desc)
            .min_height(16.0)
    })
}

/// The block's blink and the dots' count, every half second while a turn runs.
fn tick(ui: Ui) {
    let f = ui.state::<Follow>();
    if f.phase.get_untracked() == Phase::Idle {
        f.ticking.set(false);
        return;
    }
    f.tick.update(|n| *n = n.wrapping_add(1));
    floem::action::exec_after(std::time::Duration::from_millis(530), move |_| tick(ui));
}

fn item(ui: Ui, i: Item, send: Send) -> impl IntoView {
    use super::slots::{ItemSlot, around_item};
    let t = ui.colors;
    match i.kind.clone() {
        Kind::User(question) | Kind::Queued { question, .. } => {
            let own = super::question::bubble(ui, question);
            around_item(ui, &send, ItemSlot::QuestionTop, own, ItemSlot::QuestionFoot, &i)
        }
        Kind::Assistant(answer) => {
            let own = super::question::flashable(ui, i.id, super::answer::view(ui, answer, send.clone()).into_any());
            around_item(ui, &send, ItemSlot::AnswerTop, own, ItemSlot::AnswerFoot, &i)
        }
        Kind::Compaction { .. } => super::compaction::view(ui, send, &i),
        Kind::Notice(text) => label(move || text.clone())
            .style(move |s| s.color(t.get().desc).font_size(12.0).text_overflow(TextOverflow::Wrap))
            .into_any(),
        Kind::Error(text) => h_stack((
            icon("error", 16.0).style(move |s| s.color(t.get().error)),
            label(move || text.clone()).style(move |s| s.color(t.get().error).text_overflow(TextOverflow::Wrap)),
        ))
        .style(|s| s.gap(6.0))
        .into_any(),
        Kind::Ask { id, tool, subject, answer } => super::ask::card(ui, send, id, tool, subject, answer).into_any(),
    }
}

/// What Code Arena said while an answer was written, each a warning line over it.
fn notices(ui: Ui, item: &Item) -> floem::AnyView {
    let Kind::Assistant(answer) = &item.kind else { return floem::views::empty().into_any() };
    let (list, t) = (answer.notices, ui.colors);
    dyn_stack(
        move || list.get().into_iter().enumerate().collect::<Vec<_>>(),
        |(n, _)| *n,
        move |(_, text)| {
            h_stack((
                icon("warning", 14.0).style(move |s| s.color(t.get().warning).flex_shrink(0.0_f32)),
                label(move || text.clone())
                    .style(move |s| s.color(t.get().desc).font_size(12.0).text_overflow(TextOverflow::Wrap)),
            ))
            .style(|s| s.gap(6.0).items_start())
        },
    )
    .style(move |s| s.flex_col().gap(4.0).margin_bottom(6.0).apply_if(list.with(Vec::is_empty), |s| s.hide()))
    .into_any()
}

/// What it adds to the window's registries, once at start: the notices atop an answer.
pub fn register(_ui: Ui, _send: &Send) {
    super::slots::add_item(super::slots::ItemSlot::AnswerTop, "notices", 5, |ui, _send, item| notices(ui, item));
}

/// The session's messages about the thread as a whole, taken in: a notice, how the turn stands, a page of earlier
/// turns, and where the thread is scrolled when it is made again or a question is sent. Never taken away from the
/// rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let f = ui.state::<Follow>();
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    if let Some(p) = phase_of(m, f.phase.get_untracked()) {
        change(f.phase, p);
        if p != Phase::Idle && !f.ticking.get_untracked() {
            f.ticking.set(true);
            tick(ui);
        }
    }
    match m["type"].as_str().unwrap_or("") {
        "notice" => {
            let t = s("text");
            if t.starts_with("› ") && t.ends_with("(sent while it worked)") {
                return false;
            }
            // During a turn, over the answer being written; else a line of its own.
            if f.phase.get_untracked() != Phase::Idle
                && let Kind::Assistant(answer) = ui.current().kind
            {
                answer.notices.update(|l| l.push(t));
            } else {
                ui.push(Kind::Notice(t));
            }
        }
        // The thread made again: from its end, with what comes before it.
        "state" => {
            change(f.earlier, m["earlier"].as_i64().unwrap_or(0));
            change(f.first_turn, m["first_turn"].as_i64().unwrap_or(1));
            change(f.asked, None);
            change(f.phase, if m["running"] == true { Phase::Working } else { Phase::Idle });
            f.follow_on();
        }
        // A question sent from here is read where it is answered.
        "user" if m["replay"] != true && m["carried"] != true => f.follow_on(),
        "older" if f.asked.get_untracked().is_some() && m["before_turn"].as_i64() == f.asked.get_untracked() => {
            let mut older = super::stream::thread_of(ui, m);
            let earlier = m["earlier"].as_i64().unwrap_or(0);
            let from = kept_from(f.content.get_untracked(), f.row.get_untracked(), f.earlier.get_untracked(), earlier);
            change(f.earlier, earlier);
            change(f.first_turn, m["first_turn"].as_i64().unwrap_or(1));
            f.asked.set(None);
            if !older.is_empty() {
                f.above.set(Some(from));
                ui.items.update(|l| {
                    older.append(l);
                    *l = older;
                });
            }
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earlier_turns_keep_the_place_read_and_the_row_that_brought_the_last_of_them_goes() {
        assert_eq!(kept_from(2000.0, 18.0, 32, 2), 2000.0);
        assert_eq!(kept_from(2000.0, 18.0, 2, 0), 2000.0 - 18.0 - 16.0);
    }

    #[test]
    fn the_thread_follows_only_while_it_is_read_within_80_px_of_its_end() {
        assert!(near_end(920.0, 500.0, 1500.0), "80 px short of the end");
        assert!(!near_end(900.0, 500.0, 1500.0), "100 px up");
        assert!(near_end(0.0, 500.0, 300.0), "all of it seen");
        // It grows: to the new end while read there, nowhere when scrolled up.
        assert_eq!(scrolled(1000.0, true, None, 1800.0), Some(1800.0));
        assert_eq!(scrolled(200.0, false, None, 1800.0), None);
        // Turns put above: the place read is kept by what they add, whether following or not.
        assert_eq!(scrolled(0.0, true, Some(1500.0), 4000.0), Some(2500.0));
        assert_eq!(scrolled(40.0, false, Some(1500.0), 4000.0), Some(2540.0));
        // A bar over the box makes the viewport 120 px shorter: still followed. Scrolled up by the person: not.
        assert!(stays(true, 1000.0, 1000.0, 380.0, 1500.0));
        assert!(!stays(true, 1000.0, 700.0, 500.0, 1500.0));
        assert!(stays(false, 700.0, 960.0, 500.0, 1500.0), "back down at the end: followed again");
    }

    #[test]
    fn the_line_under_the_thread_follows_the_turn() {
        let next = |kind: &str, now| phase_of(&json!({"type": kind}), now);
        assert_eq!(next("turn_start", Phase::Idle), Some(Phase::Working));
        assert_eq!(next("text", Phase::Working), Some(Phase::Writing));
        assert_eq!(next("step_end", Phase::Writing), Some(Phase::Waiting));
        assert_eq!(next("tool_start", Phase::Waiting), Some(Phase::Calling));
        assert_eq!(next("ask", Phase::Writing), Some(Phase::Calling));
        assert_eq!(next("turn_end", Phase::Writing), Some(Phase::Idle));
        assert_eq!(next("text", Phase::Idle), None, "words outside a turn (a command's) change nothing");
        assert_eq!(next("status", Phase::Writing), None);
    }

    #[test]
    fn show_earlier_says_how_many_turns_are_left() {
        assert_eq!(earlier_words(1), "Show earlier (1 more turn)");
        assert_eq!(earlier_words(42), "Show earlier (42 more turns)");
    }
}
