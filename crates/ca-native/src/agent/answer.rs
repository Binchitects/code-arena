//! An answer in the thread: its calls, each a card (`tools.rs`), then its words. Over them its thinking, as VS
//! Code's chat draws it: "Thinking… 12 s" open while the model thinks (its newest words in view, with a "Skip
//! thinking" pill), folded to "Thought for 12 s" when the answer begins, "Thought process" in a thread read again.
//! Under a finished answer its footer: copy it, the model, how long the turn took, the tokens it read and wrote
//! (exact in its tooltip), and its cost when known.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use floem::IntoView;
use floem::action::exec_after;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, container, dyn_container, dyn_stack, empty, h_stack, label, scroll, tooltip, v_stack};
use serde_json::{Value, json};

use super::slots::{self, ItemSlot};
use crate::icons::icon;
use crate::model::{Answer, Item, Kind, Thought, Usage};
use crate::state::{Send, Ui};

pub(super) fn view(ui: Ui, answer: Answer, send: Send) -> impl IntoView {
    let Answer { text, tools, .. } = answer;
    v_stack((
        super::tools::list(ui, send, tools).style(move |s| s.apply_if(tools.with(|t| t.is_empty()), |s| s.hide())),
        crate::prose::answer(ui, text),
    ))
    .style(|s| s.gap(8.0).width_full())
}

/// What it adds to the window's registries, once at start: the thinking over an answer, the footer under it.
pub fn register(_ui: Ui, _send: &Send) {
    slots::add_item(ItemSlot::AnswerTop, "thinking", 10, |ui, send, item| match &item.kind {
        Kind::Assistant(answer) => thinking(ui, send, *answer).into_any(),
        _ => empty().into_any(),
    });
    slots::add_item(ItemSlot::AnswerFoot, "footer", 20, |ui, _send, item| match &item.kind {
        Kind::Assistant(answer) => footer(ui, *answer).into_any(),
        _ => empty().into_any(),
    });
}

/// The session's messages that are an answer's (its words and thinking as they come, how long it thought, its
/// turn's end), taken in. Never taken away from the rest of the window (false).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    match m["type"].as_str().unwrap_or("") {
        "text" => {
            if let Kind::Assistant(answer) = ui.current().kind {
                answer.text.update(|t| t.push_str(m["delta"].as_str().unwrap_or("")));
            }
        }
        "reasoning" => {
            if let Kind::Assistant(answer) = ui.current().kind {
                let delta = m["delta"].as_str().unwrap_or("");
                // Thinking again after a call: its words go on after those before, and the block ticks again.
                let again = answer.thought.with_untracked(Option::is_some);
                answer.reasoning.update(|r| {
                    if again && !r.is_empty() {
                        r.push_str("\n\n");
                    }
                    r.push_str(delta)
                });
                if again {
                    answer.thought.set(None);
                }
            }
        }
        "thought" => {
            if let Some(answer) = last_answer(ui) {
                let ms = m["ms"].as_u64().unwrap_or(0);
                answer.thought.set(Some(Thought { ms, cut_short: m["cut_short"] == true }));
            }
        }
        "turn_end" => {
            ui.running.set(false);
            send(json!({"type": "changes"}));
            if let Some(answer) = last_answer(ui) {
                // A thinking the session did not say the end of (a face from before) ends with the turn.
                if answer.thought.with_untracked(Option::is_none) && answer.reasoning.with_untracked(|r| !r.is_empty())
                {
                    answer.thought.set(Some(Thought { ms: 0, cut_short: m["ok"] != true }));
                }
                if m["ok"] == true {
                    finished(answer, &m["usage"], m["model"].as_str());
                }
            }
            ui.items.with_untracked(|l| {
                for i in l {
                    if let Kind::Ask { answer, .. } = &i.kind
                        && answer.get_untracked().is_none()
                    {
                        answer.set(Some("gone".into()));
                    }
                }
            });
            if m["ok"] != true {
                if m["stopped"] == true {
                    ui.push(Kind::Notice("Stopped.".into()));
                } else {
                    ui.push(Kind::Error(m["error"].as_str().unwrap_or("The turn failed.").to_string()));
                }
            }
        }
        _ => {}
    }
    false
}

/// The answer of the last question: the last answer in the thread, none when a request comes after it.
fn last_answer(ui: Ui) -> Option<Answer> {
    ui.items.with_untracked(|l| {
        l.iter().rev().take_while(|i| !matches!(i.kind, Kind::User(_))).find_map(|i| match i.kind {
            Kind::Assistant(a) => Some(a),
            _ => None,
        })
    })
}

/// What its turn used, set on a finished answer: `{prompt, cached, completion, ms, model, cost?}` as the session
/// says it at the turn's end and keeps it with the thread. The model, when not in it, as said beside it.
fn finished(answer: Answer, usage: &Value, model: Option<&str>) {
    let Some(u) = Usage::from_json(usage) else { return };
    let model = usage["model"].as_str().or(model).unwrap_or("");
    if !model.is_empty() {
        answer.model.set(model.to_string());
    }
    answer.usage.set(Some(u));
}

/// An answer's step the session kept, put after the items before it: into the answer it carries on, or a new one.
pub(super) fn kept(ui: Ui, msg: &Value, items: &mut Vec<Item>, running: bool) {
    let text = msg["text"].as_str().unwrap_or("").to_string();
    let reasoning = msg["reasoning"].as_str().unwrap_or("");
    let tools = super::tools::kept(msg, running);
    if !matches!(items.last(), Some(Item { kind: Kind::Assistant(_), .. })) {
        items.push(Item { id: ui.id(), kind: Kind::assistant(ui.scope) });
    }
    if let Some(Item { kind: Kind::Assistant(answer), .. }) = items.last() {
        let Answer { text: t, tools: ts, .. } = *answer;
        if !text.is_empty() {
            t.update(|x| {
                if !x.is_empty() {
                    x.push_str("\n\n");
                }
                x.push_str(&text)
            });
        }
        ts.update(|x| x.extend(tools));
        // Its thinking, ended: "Thought process" (how long it took is not kept).
        if !reasoning.is_empty() {
            answer.reasoning.update(|r| {
                if !r.is_empty() {
                    r.push_str("\n\n");
                }
                r.push_str(reasoning)
            });
            answer.thought.set(Some(Thought::default()));
        }
        finished(*answer, &msg["usage"], None);
    }
}

/// How long, as the chat says it: "12 s", "2 min 5 s".
pub fn duration(ms: u64) -> String {
    let s = ms.div_ceil(1000).max(u64::from(ms > 0));
    if s < 60 { format!("{s} s") } else { format!("{} min {} s", s / 60, s % 60) }
}

/// The block's title while the model thinks: "Thinking… 12 s".
pub fn thinking_title(secs: u64) -> String {
    format!("Thinking\u{2026} {secs} s")
}

/// The block's title when the thinking ended: "Thought for 12 s", ", cut short" when it was; "Thought process"
/// when how long is not known (a thread read again).
pub fn thought_title(t: Thought) -> String {
    if t.ms == 0 {
        return "Thought process".into();
    }
    let mut s = format!("Thought for {}", duration(t.ms));
    if t.cut_short {
        s.push_str(", cut short");
    }
    s
}

/// Tokens in short: 950, 8.6K, 12K, 1.2M.
pub fn tokens(n: u64) -> String {
    let short = |v: f64, unit: &str| {
        let s = if v < 10.0 { format!("{v:.1}") } else { format!("{v:.0}") };
        format!("{}{unit}", s.trim_end_matches(".0"))
    };
    match n {
        0..1000 => n.to_string(),
        1000..1_000_000 => short(n as f64 / 1000.0, "K"),
        _ => short(n as f64 / 1_000_000.0, "M"),
    }
}

/// A count with its thousands marked: 8,600.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The tokens in the footer: "8.6K in (73% cached) · 1.2K out".
pub fn usage_line(u: &Usage) -> String {
    let cached = (u.cached * 100).checked_div(u.prompt).filter(|_| u.cached > 0);
    let cached = cached.map(|p| format!(" ({p}% cached)")).unwrap_or_default();
    format!("{} in{cached} \u{b7} {} out", tokens(u.prompt), tokens(u.completion))
}

/// The same, exact, for its tooltip: "8,600 tokens in, 6,278 of them cached; 1,200 tokens out".
pub fn usage_tip(u: &Usage) -> String {
    let cached = if u.cached > 0 { format!(", {} of them cached", thousands(u.cached)) } else { String::new() };
    format!("{} tokens in{cached}; {} tokens out", thousands(u.prompt), thousands(u.completion))
}

/// A cost: "$0.012", "$1.25".
pub fn cost(c: f64) -> String {
    if c < 1.0 { format!("${c:.3}") } else { format!("${c:.2}") }
}

/// The footer's line after the copy button: "model · 12 s · 8.6K in (73% cached) · 1.2K out · $0.012".
pub fn footer_line(model: &str, u: &Usage) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !model.is_empty() {
        parts.push(model.to_string());
    }
    if u.ms > 0 {
        parts.push(duration(u.ms));
    }
    if u.prompt + u.completion > 0 {
        parts.push(usage_line(u));
    }
    if let Some(c) = u.cost {
        parts.push(cost(c));
    }
    parts.join(" \u{b7} ")
}

/// How often the thinking's seconds are looked at (they are set only when they change).
const TICK: Duration = Duration::from_millis(250);

/// The answer's thinking: hidden while it has none. Its title ticks while the model thinks, and the block is open
/// then with its newest words in view; when the answer begins it folds, and opens again on a click.
fn thinking(ui: Ui, send: Send, answer: Answer) -> impl IntoView {
    let t = ui.colors;
    let Answer { reasoning, thought, .. } = answer;
    let has = create_memo(move |_| reasoning.with(|r| !r.is_empty()));
    let now = create_memo(move |_| has.get() && thought.with(Option::is_none) && ui.running.get());
    // The window's signals: a timer still pending after the answer is gone finds them.
    let secs = ui.scope.create_rw_signal(0u64);
    let generation = ui.scope.create_rw_signal(0u64);
    let skipped = ui.scope.create_rw_signal(false);
    let open = ui.scope.create_rw_signal(false);
    // How long it thought before in this turn, and since when it thinks now.
    let before = Rc::new(Cell::new(0u64));
    let since = Rc::new(Cell::new(Instant::now()));
    {
        let (before, since) = (before.clone(), since.clone());
        create_effect(move |was: Option<bool>| {
            let thinks = now.get();
            if let Some(t) = thought.get_untracked() {
                before.set(t.ms);
            }
            if thinks && was != Some(true) {
                since.set(Instant::now());
                generation.update(|g| *g += 1);
                let ms = before.get();
                secs.set(ms / 1000);
                tick(secs, generation, generation.get_untracked(), now, ms, since.get());
            }
            thinks
        });
    }
    let title = create_memo(move |_| match thought.get() {
        Some(t) if !now.get() => thought_title(t),
        _ if skipped.get() => "Answering now\u{2026}".into(),
        _ => thinking_title(secs.get()),
    });
    let shown = create_memo(move |_| now.get() || open.get());
    let skip = label(|| "Skip thinking")
        .on_click_stop(move |_| {
            skipped.set(true);
            send(json!({"type": "skip"}));
        })
        .style(move |s| {
            let c = t.get();
            s.font_size(11.0)
                .padding_horiz(8.0)
                .padding_vert(1.0)
                .border(1.0)
                .border_radius(10.0)
                .border_color(c.widget_border)
                .color(c.fg)
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.background(c.toolbar_hover))
                .apply_if(!now.get() || skipped.get(), |s| s.hide())
        });
    let head = h_stack((
        dyn_container(move || shown.get(), |o| icon(if o { "chevron-down" } else { "chevron-right" }, 16.0)),
        label(move || title.get()).style(|s| s.flex_shrink(1.0_f32).min_width(0.0).text_ellipsis()),
        empty().style(|s| s.flex_grow(1.0_f32)),
        skip,
    ))
    .on_click_stop(move |_| {
        if !now.get_untracked() {
            open.update(|o| *o = !*o);
        }
    })
    .style(move |s| {
        let c = t.get();
        s.gap(4.0)
            .items_center()
            .width_full()
            .min_width(0.0)
            .padding_vert(2.0)
            .font_size(13.0)
            .color(c.desc)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.color(c.fg))
    });
    // Its words a paragraph a view, so a word streamed lays out its own paragraph again and not those before.
    let paragraphs = create_memo(move |_| reasoning.with(|r| r.split("\n\n").count()));
    let paragraph = move |n: usize| {
        label(move || reasoning.with(|r| r.split("\n\n").nth(n).unwrap_or("").trim().to_string()))
            .style(|s| s.text_overflow(TextOverflow::Wrap).line_height(1.5).width_full().min_width(0.0))
    };
    let words = dyn_stack(move || 0..paragraphs.get(), |n| *n, paragraph)
        .style(|s| s.flex_col().gap(8.0).width_full().min_width(0.0));
    let body = scroll(words)
        .scroll_to_percent(move || {
            // The newest words in view while it thinks.
            reasoning.with(|r| r.len());
            if now.get_untracked() { 100.0 } else { 0.0 }
        })
        .style(move |s| {
            let c = t.get();
            s.max_height(240.0)
                .width_full()
                .min_width(0.0)
                .margin_left(7.0)
                .padding_left(12.0)
                .border_left(1.0)
                .border_color(c.widget_border)
                .font_size(13.0)
                .color(c.desc)
                .apply_if(!shown.get(), |s| s.hide())
        });
    v_stack((head, body)).style(move |s| s.gap(4.0).width_full().min_width(0.0).margin_bottom(8.0).apply_if(!has.get(), |s| s.hide()))
}

/// The thinking's seconds, moved on while it thinks and this round of it is the newest.
fn tick(secs: RwSignal<u64>, generation: RwSignal<u64>, g: u64, now: floem::reactive::Memo<bool>, before: u64, since: Instant) {
    exec_after(TICK, move |_| {
        if generation.get_untracked() != g || !now.get_untracked() {
            return;
        }
        let s = (before + since.elapsed().as_millis() as u64) / 1000;
        if secs.get_untracked() != s {
            secs.set(s);
        }
        tick(secs, generation, g, now, before, since);
    });
}

/// Under a finished answer: copy it, then the model, the time, the tokens (exact in the tooltip) and the cost.
/// Hidden until its turn has ended.
fn footer(ui: Ui, answer: Answer) -> impl IntoView {
    let t = ui.colors;
    let Answer { text, usage, model, .. } = answer;
    let copied = ui.scope.create_rw_signal(false);
    let copy = container(dyn_container(move || copied.get(), |c| icon(if c { "check" } else { "copy" }, 14.0)))
        .on_click_stop(move |_| {
            let _ = floem::Clipboard::set_contents(text.get_untracked());
            copied.set(true);
            exec_after(Duration::from_millis(1500), move |_| copied.set(false));
        })
        .style(move |s| {
            let c = t.get();
            s.width(22.0)
                .height(22.0)
                .items_center()
                .justify_center()
                .border_radius(5.0)
                .color(c.icon)
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.background(c.toolbar_hover))
        });
    let copy = tooltip(copy, || label(|| "Copy"));
    let line = create_memo(move |_| usage.get().map(|u| model.with(|m| footer_line(m, &u))).unwrap_or_default());
    let tip = create_memo(move |_| usage.get().map(|u| usage_tip(&u)).unwrap_or_default());
    let words = tooltip(
        label(move || line.get()).style(|s| s.text_ellipsis().min_width(0.0).flex_shrink(1.0_f32)),
        move || label(move || tip.get()),
    );
    h_stack((copy, words)).style(move |s| {
        s.gap(4.0)
            .items_center()
            .margin_top(6.0)
            .font_size(12.0)
            .color(t.get().desc)
            .min_width(0.0)
            .apply_if(usage.with(Option::is_none) || text.with(|x| x.is_empty()), |s| s.hide())
    })
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

    #[test]
    fn times_and_thinking_titles_read_as_the_chat_says_them() {
        assert_eq!(duration(12_000), "12 s");
        assert_eq!(duration(400), "1 s");
        assert_eq!(duration(125_000), "2 min 5 s");
        assert_eq!(thinking_title(12), "Thinking\u{2026} 12 s");
        assert_eq!(thought_title(Thought { ms: 12_000, cut_short: false }), "Thought for 12 s");
        assert_eq!(thought_title(Thought { ms: 3_200, cut_short: true }), "Thought for 4 s, cut short");
        assert_eq!(thought_title(Thought::default()), "Thought process");
    }

    #[test]
    fn tokens_are_short_in_the_footer_and_exact_in_its_tooltip() {
        assert_eq!((tokens(950), tokens(8_600), tokens(12_400), tokens(1_000), tokens(1_230_000)), (
            "950".into(),
            "8.6K".into(),
            "12K".into(),
            "1K".into(),
            "1.2M".into()
        ));
        let u = Usage { prompt: 8_600, cached: 6_278, completion: 1_200, ms: 12_000, cost: Some(0.012) };
        assert_eq!(usage_line(&u), "8.6K in (73% cached) \u{b7} 1.2K out");
        assert_eq!(usage_tip(&u), "8,600 tokens in, 6,278 of them cached; 1,200 tokens out");
        assert_eq!(footer_line("qwen", &u), "qwen \u{b7} 12 s \u{b7} 8.6K in (73% cached) \u{b7} 1.2K out \u{b7} $0.012");
        // Nothing cached, no cost, nothing read: each left out.
        let none = Usage { prompt: 0, cached: 0, completion: 0, ms: 0, cost: None };
        assert_eq!(usage_line(&none), "0 in \u{b7} 0 out");
        assert_eq!(footer_line("", &none), "");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(cost(1.254), "$1.25");
    }

    /// The last answer's thinking, how long it took, and its footer's line.
    fn last(ui: Ui) -> (String, Option<Thought>, String) {
        let a = last_answer(ui).expect("an answer");
        let line = a.usage.get_untracked().map(|u| footer_line(&a.model.get_untracked(), &u)).unwrap_or_default();
        (a.reasoning.get_untracked(), a.thought.get_untracked(), line)
    }

    #[test]
    fn a_streamed_answer_thinks_then_ends_with_the_footer_its_thread_read_again_shows() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let take = |m: Value| ui.take(&m, &send);
        take(json!({"type": "user", "text": "hi", "q": 1}));
        take(json!({"type": "turn_start"}));
        take(json!({"type": "reasoning", "delta": "Say it "}));
        take(json!({"type": "reasoning", "delta": "kindly."}));
        assert_eq!(last(ui).1, None, "thinking still");
        take(json!({"type": "thought", "ms": 2_400, "cut_short": true}));
        take(json!({"type": "text", "delta": "Hello."}));
        let usage = json!({"prompt": 8_600, "cached": 6_278, "completion": 1_200, "ms": 12_000, "model": "qwen"});
        take(json!({"type": "turn_end", "ok": true, "ms": 12_000, "model": "qwen", "usage": usage}));
        let live = last(ui);
        assert_eq!(live.0, "Say it kindly.");
        assert_eq!(live.1.map(thought_title).as_deref(), Some("Thought for 3 s, cut short"));
        assert_eq!(live.2, "qwen \u{b7} 12 s \u{b7} 8.6K in (73% cached) \u{b7} 1.2K out");
        // The same answer as the session kept it: its thinking ended, with no time, and the same footer.
        take(json!({"type": "state", "messages": [
            {"role": "user", "text": "hi", "q": 1},
            {"role": "assistant", "text": "Hello.", "reasoning": "Say it kindly.", "usage": usage},
        ]}));
        let again = last(ui);
        assert_eq!((again.0.as_str(), again.1.map(thought_title)), ("Say it kindly.", Some("Thought process".into())));
        assert_eq!(again.2, live.2);
    }

    #[test]
    fn thinking_again_after_a_call_goes_on_under_the_same_block() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        let take = |m: Value| ui.take(&m, &send);
        take(json!({"type": "user", "text": "hi", "q": 1}));
        take(json!({"type": "reasoning", "delta": "First."}));
        take(json!({"type": "thought", "ms": 1_000, "cut_short": false}));
        take(json!({"type": "reasoning", "delta": "Then."}));
        assert_eq!(last(ui).0, "First.\n\nThen.");
        assert_eq!(last(ui).1, None);
        // A failed turn that never said the thinking ended folds it all the same, with no footer.
        take(json!({"type": "turn_end", "ok": false, "error": "no"}));
        assert_eq!(last(ui).1, Some(Thought { ms: 0, cut_short: true }));
        assert_eq!(last(ui).2, "");
    }
}
