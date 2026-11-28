//! Pictures, sounds and videos a tool made, under its card in the thread: a picture by a small copy that opens a
//! viewer over the whole window (fitted, zoomed with + − 0 or Ctrl+wheel, the call's other pictures by the arrows,
//! saved elsewhere in the project with Save As), a sound or a video by a card that opens the system's player; and,
//! while a tool that draws or films runs, a line that says so.
//!
//! What a tool made is kept under `.code-arena/media/`, and its call names those files (`Tool::media`). A file is
//! asked for only once its card is drawn, and once: what came is kept by its path. What every part has
//! (`register`, `take`) is said in `ext.rs`.

use std::collections::HashMap;
use std::rc::Rc;

use base64::Engine;
use floem::event::{Event, EventListener};
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::style::{CursorStyle, FlexWrap, TextOverflow};
use floem::views::{Decorators, container, dyn_container, empty, h_stack, h_stack_from_iter, label, scroll, v_stack};
use floem::{AnyView, IntoView, View};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::layers::{self, Place};
use crate::media::{self as files, Media, Picture};
use crate::model::Tool;
use crate::state::{Send, Ui};
use crate::widgets::{action, quick};

/// Where a tool's results are kept, from the project's top.
const MADE: &str = ".code-arena/media/";
/// The viewer's zoom steps, as VS Code's picture preview has them.
const ZOOMS: [f64; 18] = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.5, 2.0, 3.0, 5.0, 7.0, 10.0, 15.0, 20.0];
/// The small copy's place under a card.
const THUMB: (f64, f64) = (160.0, 120.0);

/// The files a tool made that a result names (`…/.code-arena/media/NAME`, either slash), as
/// `.code-arena/media/NAME`, each once, in the order said.
pub fn found(text: &str) -> Vec<String> {
    let mut at: Vec<(usize, String)> = Vec::new();
    for mark in [MADE.to_string(), MADE.replace('/', "\\")] {
        for (i, _) in text.match_indices(&mark) {
            let name: String = text[i + mark.len()..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
                .collect();
            let name = name.trim_end_matches('.');
            let key = format!("{MADE}{name}");
            if name.contains('.') && !name.starts_with('.') && !at.iter().any(|(_, k)| *k == key) {
                at.push((i, key));
            }
        }
    }
    at.sort_by_key(|(i, _)| *i);
    at.into_iter().map(|(_, k)| k).collect()
}

/// What a running call says it is making, by its name: a tool that draws, or one that films.
pub fn making(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    let makes = has(&["make", "generate", "create", "render", "gen_", "_gen", "text_to", "imagine"]);
    if has(&["video", "movie", "film", "animation"]) && (makes || has(&["animate"])) {
        Some("Making the video, a few minutes…")
    } else if (makes && has(&["image", "picture", "photo", "drawing", "art"])) || has(&["draw", "paint"]) {
        Some("Drawing the picture…")
    } else {
        None
    }
}

/// The zoom one step in (`up`) or out from `now`, within VS Code's steps.
pub fn zoom_step(now: f64, up: bool) -> f64 {
    let e = 1e-6;
    if up {
        ZOOMS.iter().copied().find(|z| *z > now + e).unwrap_or(ZOOMS[ZOOMS.len() - 1])
    } else {
        ZOOMS.iter().rev().copied().find(|z| *z < now - e).unwrap_or(ZOOMS[0])
    }
}

/// The picture `by` places from `at` among `n`, round from the last to the first.
pub fn next(at: usize, n: usize, by: isize) -> usize {
    if n == 0 { 0 } else { (at as isize + by).rem_euclid(n as isize) as usize }
}

/// A file's name, the last part of its path.
fn name_of(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// What came of a file asked for.
#[derive(Clone)]
enum Got {
    Asked,
    Picture(Rc<Picture>),
    /// A sound or a video, or a file of another kind: its size.
    Size(u64),
    Cannot(String),
}

/// The viewer: the pictures it goes through and the one shown.
#[derive(Clone, PartialEq)]
struct Viewing {
    all: Vec<String>,
    at: usize,
}

/// This part's state: what came of each file, and the viewer.
#[derive(Clone, Copy)]
struct State {
    got: RwSignal<HashMap<String, Got>>,
    viewing: RwSignal<Option<Viewing>>,
    /// The viewer's zoom (none: fitted to the window).
    zoom: RwSignal<Option<f64>>,
}

impl crate::ext::State for State {
    fn new(ui: Ui) -> Self {
        State {
            got: ui.scope.create_rw_signal(HashMap::new()),
            viewing: ui.scope.create_rw_signal(None),
            zoom: ui.scope.create_rw_signal(None),
        }
    }
}

impl State {
    fn got(self, path: &str) -> Option<Got> {
        self.got.with(|g| g.get(path).cloned())
    }

    fn set(self, path: &str, got: Got) {
        self.got.update(|g| {
            g.insert(path.to_string(), got);
        });
    }

    /// The file asked for, once: a picture's bytes, a sound's or a video's size.
    fn ask(self, send: &Send, path: &str) {
        if self.got.with_untracked(|g| g.contains_key(path)) {
            return;
        }
        self.set(path, Got::Asked);
        let data = matches!(files::media_of(path), Some(Media::Picture | Media::Svg));
        send(json!({"type": "media", "path": path, "data": data}));
    }

    fn open(self, all: Vec<String>, at: usize) {
        if self.zoom.get_untracked().is_some() {
            self.zoom.set(None);
        }
        self.viewing.set(Some(Viewing { all, at }));
    }

    fn go(self, by: isize) {
        let Some(v) = self.viewing.get_untracked() else { return };
        if v.all.len() > 1 {
            if self.zoom.get_untracked().is_some() {
                self.zoom.set(None);
            }
            self.viewing.set(Some(Viewing { at: next(v.at, v.all.len(), by), ..v }));
        }
    }

    fn shown(self) -> Option<String> {
        self.viewing.with(|v| v.as_ref().and_then(|v| v.all.get(v.at).cloned()))
    }
}

/// What it adds to the window's registries, once at start: the viewer's layer, under the quick input (so Save
/// As asks over it).
pub fn register(_ui: Ui, _send: &Send) {
    layers::add(Place::Over, "media.viewer", 22, viewer);
}

/// The session's messages that are this part's, taken in: a file asked for from a card, a picture saved. Never
/// taken away from the parts after it (a media tab of the same file reads the same reply).
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let st = ui.state::<State>();
    let path = m["path"].as_str().unwrap_or("").to_string();
    match m["type"].as_str().unwrap_or("") {
        "media" if matches!(st.got.with_untracked(|g| g.get(&path).cloned()), Some(Got::Asked)) => {
            if let Some(e) = m["error"].as_str() {
                st.set(&path, Got::Cannot(e.to_string()));
            } else if matches!(files::media_of(&path), Some(Media::Picture | Media::Svg)) {
                decode(ui, st, path, m);
            } else {
                st.set(&path, Got::Size(m["size"].as_u64().unwrap_or(0)));
            }
        }
        "media_save" if path.starts_with(MADE) => {
            let to = m["to"].as_str().unwrap_or("");
            ui.message.set(match m["error"].as_str() {
                Some(e) => format!("Could not save {to}: {e}"),
                None => format!("Saved {to}"),
            });
        }
        _ => {}
    }
    false
}

/// A picture's bytes decoded off the window's thread, then kept ready to draw.
fn decode(ui: Ui, st: State, path: String, m: &Value) {
    let data = m["data"].as_str().unwrap_or("").to_string();
    let size = m["size"].as_u64().unwrap_or(0);
    let checker = files::checker(ui);
    let hash = format!("made\0{path}\0{}\0{}", m["modified"], checker[0][0]).into_bytes();
    let file = path.clone();
    let done = floem::ext_event::create_ext_action(ui.scope, move |made: Result<files::Decoded, String>| {
        st.set(
            &file,
            match made {
                Ok(d) => Got::Picture(Rc::new(Picture::of(d, size, hash))),
                Err(e) => Got::Cannot(e),
            },
        );
    });
    std::thread::spawn(move || {
        let made = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| e.to_string())
            .and_then(|bytes| files::decode(&bytes, &path, checker));
        done(made);
    });
}

/// Under a call's card: what it is making while it runs, then what it made.
pub(super) fn under(ui: Ui, send: Send, me: Memo<Tool>) -> impl IntoView {
    let t = ui.colors;
    let running = ui.running;
    let shown = create_memo(move |_| {
        me.with(|m| {
            if !m.done && running.get() && m.status != "waiting" {
                (making(&m.name), Vec::new())
            } else if m.done && m.ok {
                (None, m.media.clone())
            } else {
                (None, Vec::new())
            }
        })
    });
    dyn_container(
        move || shown.get(),
        move |(words, all)| match words {
            Some(w) => h_stack((icon("loading", 14.0), label(move || w)))
                .style(move |s| s.gap(6.0).items_center().padding_horiz(8.0).font_size(12.0).color(t.get().desc))
                .into_any(),
            None if all.is_empty() => empty().into_any(),
            None => made(ui, send.clone(), all).into_any(),
        },
    )
    .style(|s| s.width_full().min_width(0.0))
}

/// What a call made, side by side, wrapped: its pictures small, its sounds and videos as cards.
fn made(ui: Ui, send: Send, all: Vec<String>) -> impl IntoView {
    let st = ui.state::<State>();
    for p in &all {
        st.ask(&send, p);
    }
    let pictures: Vec<String> =
        all.iter().filter(|p| matches!(files::media_of(p), Some(Media::Picture | Media::Svg))).cloned().collect();
    h_stack_from_iter(all.into_iter().map(move |path| {
        let send = send.clone();
        let pictures = pictures.clone();
        match files::media_of(&path) {
            Some(Media::Picture | Media::Svg) => thumb(ui, path, pictures).into_any(),
            kind => {
                let glyph = match kind {
                    Some(Media::Sound) => "unmute",
                    Some(Media::Video) => "device-camera-video",
                    _ => "file",
                };
                player(ui, send, path, glyph).into_any()
            }
        }
    }))
    .style(|s| s.gap(6.0).flex_wrap(FlexWrap::Wrap).width_full().min_width(0.0))
}

/// A picture's small copy: a click opens it in the viewer, among the call's other pictures.
fn thumb(ui: Ui, path: String, pictures: Vec<String>) -> impl IntoView {
    let t = ui.colors;
    let st = ui.state::<State>();
    let key = path.clone();
    let ready = create_memo(move |_| match st.got(&key) {
        Some(Got::Picture(p)) => Ok(Some(p)),
        Some(Got::Cannot(e)) => Err(e),
        _ => Ok(None),
    });
    let name = name_of(&path).to_string();
    let at = pictures.iter().position(|p| *p == path).unwrap_or(0);
    dyn_container(
        move || ready.get().map(|p| p.map(|p| Rc::as_ptr(&p) as usize)),
        move |_| match ready.get_untracked() {
            Ok(Some(p)) => files::drawn(p, false).style(|s| s.size_full()).into_any(),
            Ok(None) => icon("file-media", 16.0).into_any(),
            Err(e) => {
                let said = format!("{name}: {e}");
                label(move || said.clone())
                    .style(|s| s.font_size(11.0).padding(6.0).text_overflow(TextOverflow::Wrap))
                    .into_any()
            }
        },
    )
    .on_click_stop(move |_| st.open(pictures.clone(), at))
    .style(move |s| {
        let c = t.get();
        s.width(THUMB.0)
            .height(THUMB.1)
            .items_center()
            .justify_center()
            .border(1.0)
            .border_color(c.widget_border)
            .border_radius(4.0)
            .background(c.code)
            .color(c.desc)
            .cursor(CursorStyle::Pointer)
            .hover(|s| s.border_color(c.focus))
    })
}

/// A sound's, a video's or another file's card: its name and size; a click opens it in the system's player.
fn player(ui: Ui, send: Send, path: String, glyph: &'static str) -> impl IntoView {
    let t = ui.colors;
    let st = ui.state::<State>();
    let key = path.clone();
    let size = create_memo(move |_| match st.got(&key) {
        Some(Got::Size(n)) => files::bytes(n),
        Some(Got::Cannot(e)) => e,
        _ => String::new(),
    });
    let name = name_of(&path).to_string();
    h_stack((
        icon(glyph, 16.0),
        v_stack((
            label(move || name.clone()).style(move |s| s.color(t.get().fg).text_ellipsis().min_width(0.0)),
            label(move || size.get()).style(|s| s.font_size(11.0)),
        ))
        .style(|s| s.min_width(0.0).flex_shrink(1.0_f32)),
        icon("link-external", 14.0),
    ))
    .on_click_stop(move |_| send(json!({"type": "media_open", "path": path})))
    .style(move |s| {
        let c = t.get();
        s.gap(8.0)
            .items_center()
            .padding_horiz(10.0)
            .padding_vert(6.0)
            .max_width(280.0)
            .border(1.0)
            .border_color(c.widget_border)
            .border_radius(4.0)
            .color(c.desc)
            .cursor(CursorStyle::Pointer)
            .hover(|s| s.background(c.list_hover))
    })
}

/// The viewer over the whole window: the picture fitted or zoomed, its bar above it, the arrows beside it.
fn viewer(ui: Ui, send: Send) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<State>();
    let open = create_memo(move |_| st.viewing.with(Option::is_some));
    let shown = create_memo(move |_| st.shown());
    let picture = create_memo(move |_| {
        shown.get().and_then(|p| match st.got(&p) {
            Some(Got::Picture(p)) => Some(p),
            _ => None,
        })
    });
    let count = create_memo(move |_| st.viewing.with(|v| v.as_ref().map_or((0, 0), |v| (v.at + 1, v.all.len()))));
    // The zoom the picture is drawn at: its own when zoomed, else what fits the window.
    let scale = move || {
        st.zoom.get().unwrap_or_else(|| {
            picture.with(|p| {
                p.as_ref().map_or(1.0, |p| {
                    let win = crate::widgets::window_size(ui, None);
                    let (w, h) = (f64::from(p.width).max(1.0), f64::from(p.height).max(1.0));
                    ((win.width - 120.0) / w).min((win.height - 120.0) / h).clamp(0.01, 1.0)
                })
            })
        })
    };
    let zoom = move |up: Option<bool>| match up {
        None => st.zoom.set(None),
        Some(up) => {
            let now = scale();
            st.zoom.set(Some(zoom_step(now, up)));
        }
    };
    let close = move || st.viewing.set(None);
    let save_send = send.clone();
    let layer_id = std::rc::Rc::new(std::cell::Cell::new(None::<floem::ViewId>));
    let save_layer = layer_id.clone();
    let save = move || {
        let Some(path) = shown.get_untracked() else { return };
        let send = save_send.clone();
        let mut ask = quick::input("Save As")
            .value(name_of(&path))
            .prompt("Where in the project to save the picture")
            .validate(|v| v.trim().is_empty().then(|| "A path is needed.".into()));
        if let Some(id) = save_layer.get() {
            ask = ask.back(id);
        }
        ask.open(ui, move |to| {
            if let Some(to) = to.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
                send(json!({"type": "media_save", "path": path, "to": to}));
            }
        });
    };
    let bar = h_stack((
        label(move || shown.get().map(|p| name_of(&p).to_string()).unwrap_or_default())
            .style(move |s| s.color(t.get().fg).text_ellipsis().min_width(0.0)),
        label(move || {
            let (at, n) = count.get();
            if n > 1 { format!("{at} of {n}") } else { String::new() }
        })
        .style(|s| s.font_size(12.0)),
        label(move || {
            picture
                .with(|p| p.as_ref().map(|p| format!("{} × {}  {}", p.width, p.height, files::bytes(p.size))))
                .unwrap_or_default()
        })
        .style(|s| s.font_size(12.0)),
        empty().style(|s| s.flex_grow(1.0_f32)),
        label(move || if st.zoom.get().is_some() { format!("{:.0}%", scale() * 100.0) } else { "Whole Image".into() })
            .style(|s| s.font_size(12.0).min_width(80.0)),
        action(t, "zoom-out", move || zoom(Some(false))),
        action(t, "zoom-in", move || zoom(Some(true))),
        action(t, "screen-full", move || zoom(None)),
        action(t, "save-as", save.clone()),
        {
            let send = send.clone();
            action(t, "link-external", move || {
                if let Some(path) = shown.get_untracked() {
                    send(json!({"type": "media_open", "path": path}));
                }
            })
        },
        action(t, "close", close),
    ))
    .style(move |s| {
        let c = t.get();
        s.gap(10.0)
            .items_center()
            .padding_horiz(12.0)
            .height(36.0)
            .width_full()
            .min_width(0.0)
            .flex_shrink(0.0_f32)
            .background(c.editor_bg)
            .color(c.desc)
            .border_bottom(1.0)
            .border_color(c.widget_border)
    });
    let picture_key = create_memo(move |_| (picture.get().map(|p| Rc::as_ptr(&p) as usize), st.zoom.get()));
    let body = dyn_container(
        move || picture_key.get(),
        move |(_, z)| match (picture.get_untracked(), z) {
            (None, _) => match shown.get_untracked().and_then(|p| st.got(&p)) {
                Some(Got::Cannot(e)) => label(move || e.clone()).into_any(),
                _ => icon("loading", 16.0).into_any(),
            },
            (Some(p), None) => files::drawn(p, false).style(|s| s.size_full()).into_any(),
            (Some(p), Some(z)) => {
                let (w, h) = (f64::from(p.width) * z, f64::from(p.height) * z);
                scroll(
                    container(files::drawn(p, true).style(move |s| s.width(w).height(h).flex_shrink(0.0_f32)))
                        .style(|s| s.min_size_full().items_center().justify_center()),
                )
                .style(|s| s.absolute().inset(0.0))
                .into_any()
            }
        },
    )
    .style(|s| s.size_full().min_size(0.0, 0.0).items_center().justify_center());
    let arrow = move |glyph: &'static str, by: isize| {
        container(icon(glyph, 24.0)).on_click_stop(move |_| st.go(by)).style(move |s| {
            let c = t.get();
            s.width(40.0)
                .height(40.0)
                .items_center()
                .justify_center()
                .border_radius(20.0)
                .flex_shrink(0.0_f32)
                .color(c.fg)
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.background(c.toolbar_hover))
                .apply_if(count.get().1 < 2, |s| s.hide())
        })
    };
    let middle = h_stack((
        arrow("chevron-left", -1),
        container(body).style(|s| s.flex_grow(1.0_f32).height_full().min_size(0.0, 0.0)),
        arrow("chevron-right", 1),
    ))
    .style(|s| s.flex_grow(1.0_f32).width_full().min_size(0.0, 0.0).items_center().padding(12.0).gap(8.0));
    let layer = v_stack((bar, middle)).keyboard_navigable();
    let id = layer.id();
    layer_id.set(Some(id));
    create_effect(move |was: Option<bool>| {
        let now = open.get();
        if now && was != Some(true) {
            id.request_focus();
        }
        now
    });
    layer
        .on_event_stop(EventListener::PointerWheel, move |e| {
            if let Event::PointerWheel(w) = e
                && (w.modifiers.contains(Modifiers::CONTROL) || w.modifiers.contains(Modifiers::META))
                && w.delta.y != 0.0
            {
                zoom(Some(w.delta.y < 0.0));
            }
        })
        .on_event_cont(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return };
            match &k.key.logical_key {
                Key::Named(NamedKey::Escape) => close(),
                Key::Named(NamedKey::ArrowLeft) => st.go(-1),
                Key::Named(NamedKey::ArrowRight) => st.go(1),
                Key::Character(c) if c == "+" || c == "=" => zoom(Some(true)),
                Key::Character(c) if c == "-" => zoom(Some(false)),
                Key::Character(c) if c == "0" => zoom(None),
                _ => {}
            }
        })
        .style(move |s| s.absolute().inset(0.0).background(t.get().editor_bg).apply_if(!open.get(), |s| s.hide()))
        .into_any()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[test]
    fn what_a_tool_made_is_read_from_its_result() {
        let text = "(Saved image/png as /home/a/p/.code-arena/media/make_image-20261128-101500.png: tell the person \
                    where it is.)\n(Saved video/mp4 as C:\\p\\.code-arena\\media\\film-1-2.mp4: tell…)\n\
                    again .code-arena/media/make_image-20261128-101500.png. and .code-arena/media/.gitignore";
        assert_eq!(found(text), [".code-arena/media/make_image-20261128-101500.png", ".code-arena/media/film-1-2.mp4"]);
        assert!(found("nothing made").is_empty());
        let t = Tool::from_json(&json!({"name": "x", "text": text}), true);
        assert_eq!(t.media.len(), 2, "read from the result when the session does not say");
        let t = Tool::from_json(&json!({"name": "x", "text": "", "media": [".code-arena/media/a.wav"]}), true);
        assert_eq!(t.media, [".code-arena/media/a.wav"], "what the session says, past the result's cut");
    }

    #[test]
    fn a_tool_that_draws_or_films_says_so_while_it_runs() {
        assert_eq!(making("make_image"), Some("Drawing the picture…"));
        assert_eq!(making("arena__generate_picture"), Some("Drawing the picture…"));
        assert_eq!(making("draw"), Some("Drawing the picture…"));
        assert_eq!(making("generate_video"), Some("Making the video, a few minutes…"));
        assert_eq!(making("read_image"), None, "reading a picture makes none");
        assert_eq!(making("read_file"), None);
    }

    #[test]
    fn the_viewer_zooms_by_vs_codes_steps_and_goes_round() {
        assert_eq!(zoom_step(1.0, true), 1.5);
        assert_eq!(zoom_step(1.0, false), 0.9);
        assert_eq!(zoom_step(0.33, true), 0.4, "from a fitted zoom, the next step");
        assert_eq!(zoom_step(0.33, false), 0.3);
        assert_eq!((zoom_step(20.0, true), zoom_step(0.1, false)), (20.0, 0.1), "kept within the steps");
        assert_eq!((next(0, 3, -1), next(2, 3, 1), next(1, 3, 1), next(0, 0, 1)), (2, 0, 2, 0));
    }

    #[test]
    fn a_file_is_asked_for_once_and_its_reply_kept() {
        let ui = Ui::new();
        let said = Rc::new(RefCell::new(Vec::new()));
        let to = said.clone();
        let send: Send = Rc::new(move |v: Value| to.borrow_mut().push(v));
        let st = ui.state::<State>();
        st.ask(&send, ".code-arena/media/a.png");
        st.ask(&send, ".code-arena/media/a.png");
        st.ask(&send, ".code-arena/media/b.mp3");
        assert_eq!(
            *said.borrow(),
            [
                json!({"type": "media", "path": ".code-arena/media/a.png", "data": true}),
                json!({"type": "media", "path": ".code-arena/media/b.mp3", "data": false}),
            ]
        );
        take(ui, &json!({"type": "media", "path": ".code-arena/media/b.mp3", "size": 2048}), &send);
        assert!(matches!(st.got(".code-arena/media/b.mp3"), Some(Got::Size(2048))));
        take(ui, &json!({"type": "media", "path": ".code-arena/media/a.png", "error": "not there"}), &send);
        assert!(matches!(st.got(".code-arena/media/a.png"), Some(Got::Cannot(e)) if e == "not there"));
        // A reply for a file no card asked for is left to the media tabs.
        take(ui, &json!({"type": "media", "path": "img/c.png", "size": 1}), &send);
        assert!(st.got("img/c.png").is_none());
        // The viewer goes through the call's pictures, round.
        st.open(vec!["a.png".into(), "b.png".into()], 1);
        st.zoom.set(Some(2.0));
        st.go(1);
        assert_eq!((st.shown().as_deref(), st.zoom.get_untracked()), (Some("a.png"), None), "fitted again");
        take(ui, &json!({"type": "media_save", "path": ".code-arena/media/a.png", "to": "art/a.png"}), &send);
        assert_eq!(ui.message.get_untracked(), "Saved art/a.png");
    }
}
