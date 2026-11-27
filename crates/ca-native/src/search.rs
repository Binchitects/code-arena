//! The Search view, as VS Code's: the box with its Match Case and Regular Expression toggles, the count, then the
//! hits by file (the file's name, its folder dimmed, its count) and each line with the match lit; a click opens
//! the file at that line.

use floem::IntoView;
use floem::keyboard::{Key, NamedKey};
use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::style::CursorStyle;
use floem::views::{Decorators, container, dyn_stack, empty, h_stack, label, scroll, text_input, v_stack};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::state::{Found, Hit, Send, Ui};
use crate::widgets::{bold, toggle};

/// The hits by file, in the order the search found them.
pub fn by_file(hits: &[Hit]) -> Vec<(String, Vec<Hit>)> {
    let mut out: Vec<(String, Vec<Hit>)> = Vec::new();
    for h in hits {
        match out.last_mut() {
            Some((p, v)) if *p == h.path => v.push(h.clone()),
            _ => out.push((h.path.clone(), vec![h.clone()])),
        }
    }
    out
}

/// A line cut where the query is in it (the first time): before, the match, after. A regular expression is not
/// looked for here (the whole line is said as it is).
pub fn cut(text: &str, query: &str, case: bool, regex: bool) -> (String, String, String) {
    let text = text.trim_start();
    if regex || query.is_empty() {
        return (text.to_string(), String::new(), String::new());
    }
    let at = if case { text.find(query) } else { text.to_lowercase().find(&query.to_lowercase()) };
    match at {
        Some(i) if text.is_char_boundary(i) && text.is_char_boundary(i + query.len()) => {
            (text[..i].to_string(), text[i..i + query.len()].to_string(), text[i + query.len()..].to_string())
        }
        _ => (text.to_string(), String::new(), String::new()),
    }
}

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let focused = create_rw_signal(false);
    // Asked a moment after the last key, as VS Code searches while one types.
    let generation = create_rw_signal(0u64);
    {
        let send = send.clone();
        create_effect(move |first: Option<()>| {
            let (q, case, regex) = (ui.search_query.get(), ui.search_case.get(), ui.search_regex.get());
            if first.is_none() && q.is_empty() {
                return;
            }
            generation.update(|g| *g += 1);
            let g = generation.get_untracked();
            let send = send.clone();
            floem::action::exec_after(std::time::Duration::from_millis(300), move |_| {
                if generation.get_untracked() == g {
                    send(json!({"type": "search", "query": q, "case": case, "regex": regex}));
                }
            });
        });
    }
    let go = send.clone();
    let field = h_stack((
        text_input(ui.search_query)
            .placeholder("Search")
            .on_key_down(Key::Named(NamedKey::Enter), |m| m.is_empty(), move |_| {
                go(json!({"type": "search", "query": ui.search_query.get_untracked(), "case": ui.search_case.get_untracked(), "regex": ui.search_regex.get_untracked()}))
            })
            .on_event_cont(floem::event::EventListener::FocusGained, move |_| focused.set(true))
            .on_event_cont(floem::event::EventListener::FocusLost, move |_| focused.set(false))
            .style(move |s| {
                let c = t.get();
                s.flex_grow(1.0_f32)
                    .padding_horiz(4.0)
                    .border(0.0)
                    .background(Color::TRANSPARENT)
                    .color(c.input_fg)
                    .cursor_color(floem::peniko::Brush::Solid(c.cursor))
            }),
        toggle(t, "case-sensitive", ui.search_case, "Match Case"),
        toggle(t, "regex", ui.search_regex, "Use Regular Expression"),
    ))
    .style(move |s| {
        let c = t.get();
        s.margin_horiz(12.0)
            .margin_top(4.0)
            .height(26.0)
            .padding_right(3.0)
            .gap(2.0)
            .items_center()
            .border(1.0)
            .border_radius(2.0)
            .border_color(if focused.get() { c.focus } else { c.input_border })
            .background(c.input_bg)
    });
    let summary = label(move || {
        ui.search.with(|r| match r {
            None => String::new(),
            Some(r) if !r.error.is_empty() => r.error.clone(),
            Some(r) if r.hits.is_empty() => "No results found.".to_string(),
            Some(r) => {
                let n = r.hits.len();
                let files = by_file(&r.hits).len();
                let more = if r.more { " (more not shown)" } else { "" };
                format!(
                    "{n} result{} in {files} file{}{more}",
                    if n == 1 { "" } else { "s" },
                    if files == 1 { "" } else { "s" }
                )
            }
        })
    })
    .style(move |s| {
        s.padding_horiz(22.0)
            .padding_vert(6.0)
            .font_size(13.0)
            .color(t.get().desc)
            .apply_if(ui.search.with(|r| r.is_none()), |s| s.hide())
    });
    let results = dyn_stack(
        move || ui.search.with(|r| r.as_ref().map(|r| by_file(&r.hits)).unwrap_or_default()),
        |(p, v): &(String, Vec<Hit>)| (p.clone(), v.len()),
        move |(path, hits): (String, Vec<Hit>)| file_group(ui, send.clone(), path, hits),
    )
    .style(|s| s.flex_col().width_full());
    v_stack((field, summary, scroll(results).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full())))
        .style(|s| s.size_full().min_height(0.0))
}

fn file_group(ui: Ui, send: Send, path: String, hits: Vec<Hit>) -> impl IntoView {
    let t = ui.colors;
    let open = create_rw_signal(true);
    let (dir, name) = match path.rsplit_once('/') {
        Some((d, n)) => (d.to_string(), n.to_string()),
        None => (String::new(), path.clone()),
    };
    let count = hits.len();
    let head = h_stack((
        container(floem::views::dyn_container(
            move || open.get(),
            |o| icon(if o { "chevron-down" } else { "chevron-right" }, 16.0),
        )),
        label(move || name.clone()),
        label(move || dir.clone()).style(move |s| s.font_size(12.0).color(t.get().desc).text_ellipsis()),
        empty().style(|s| s.flex_grow(1.0_f32)),
        label(move || count.to_string()).style(move |s| {
            let c = t.get();
            s.font_size(11.0)
                .padding_horiz(5.0)
                .min_width(18.0)
                .height(16.0)
                .justify_center()
                .items_center()
                .border_radius(8.0)
                .background(c.badge_bg)
                .color(c.badge_fg)
        }),
    ))
    .on_click_stop(move |_| open.update(|o| *o = !*o))
    .style(move |s| {
        let c = t.get();
        s.height(22.0)
            .width_full()
            .items_center()
            .gap(6.0)
            .padding_left(8.0)
            .padding_right(12.0)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(c.list_hover))
    });
    let lines = dyn_stack(
        move || if open.get() { hits.clone().into_iter().enumerate().collect::<Vec<_>>() } else { vec![] },
        |(i, _): &(usize, Hit)| *i,
        move |(_, h): (usize, Hit)| {
            let send = send.clone();
            let (before, lit, after) = cut(
                &h.text,
                &ui.search_query.get_untracked(),
                ui.search_case.get_untracked(),
                ui.search_regex.get_untracked(),
            );
            let (p, line) = (h.path.clone(), h.line);
            h_stack((
                label(move || before.clone()).style(|s| s.text_ellipsis()),
                label(move || lit.clone()).style(move |s| {
                    let c = t.get();
                    bold(s).background(if c.dark {
                        Color::rgba8(0x9E, 0x6A, 0x03, 0x99)
                    } else {
                        Color::rgba8(0xEA, 0x5C, 0x00, 0x55)
                    })
                }),
                label(move || after.clone()).style(|s| s.text_ellipsis()),
            ))
            .on_click_stop(move |_| ui.open_at(&p, line, &send))
            .style(move |s| {
                let c = t.get();
                s.height(22.0)
                    .width_full()
                    .items_center()
                    .padding_left(38.0)
                    .padding_right(12.0)
                    .font_size(13.0)
                    .cursor(CursorStyle::Pointer)
                    .selectable(false)
                    .hover(|s| s.background(c.list_hover))
            })
        },
    )
    .style(|s| s.flex_col().width_full());
    v_stack((head, lines)).style(move |s| s.width_full().color(t.get().side_fg))
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are the Search view's, taken in. Never taken away from the
/// rest of the window (false).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    if m["type"] == "search" {
        if m["query"].as_str() != Some(ui.search_query.get_untracked().as_str()) {
            return false; // an answer to what was typed before
        }
        if ui.search_query.with_untracked(|q| q.is_empty()) {
            ui.search.set(None);
            return false;
        }
        let hits = m["hits"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|h| Hit {
                path: h["path"].as_str().unwrap_or("").into(),
                line: h["line"].as_u64().unwrap_or(1) as usize,
                text: h["text"].as_str().unwrap_or("").into(),
            })
            .collect();
        ui.search.set(Some(Found { hits, more: m["more"] == true, error: s("error") }));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(p: &str, line: usize) -> Hit {
        Hit { path: p.into(), line, text: String::new() }
    }

    #[test]
    fn hits_by_file_in_the_order_found() {
        let g = by_file(&[hit("a.rs", 1), hit("a.rs", 4), hit("b/c.rs", 2)]);
        assert_eq!(g.iter().map(|(p, v)| (p.as_str(), v.len())).collect::<Vec<_>>(), [("a.rs", 2), ("b/c.rs", 1)]);
    }

    #[test]
    fn a_line_cut_where_the_query_is() {
        assert_eq!(cut("    let Cart = 1;", "cart", false, false), ("let ".into(), "Cart".into(), " = 1;".into()));
        assert_eq!(cut("let Cart", "cart", true, false), ("let Cart".into(), String::new(), String::new()));
        assert_eq!(cut("a.b", "a.", false, true), ("a.b".into(), String::new(), String::new()));
    }
}
