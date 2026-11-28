//! Diagrams in an answer: a fenced `mermaid` (or `mmd`) block drawn as a picture where it is written, as the old
//! Code Arena's chat draws one. While the block is still being written it is code; once its fence is closed the
//! driver is asked to draw it (`diagram{id, kind, source, dark}`, by mermaid-cli or a headless Chrome) and its SVG
//! is drawn off the window's thread, fitted to the answer's width. Its bar's Show Code and Show Diagram change which
//! is shown, and Open in Browser opens a page that draws it. One that does not parse shows its code under Mermaid's
//! error; where nothing draws diagrams, the code says so, and Open in Browser still shows it.
//!
//! Each diagram is drawn once for a source and a theme: every block of the same diagram (an answer drawn again, a
//! session opened again) shows the same drawing.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo, create_rw_signal};
use floem::style::TextOverflow;
use floem::views::{Decorators, container, dyn_container, empty, h_stack, label};
use floem::{AnyView, IntoView};
use serde_json::{Value, json};

use crate::media::{self, Picture};
use crate::prose::{Drawing, bar_action, code_with};
use crate::state::{Send, Ui};

/// The diagram kind a fence's language names, if it names one.
pub fn kind_of(lang: &str) -> Option<&'static str> {
    match lang {
        "mermaid" | "mmd" => Some("mermaid"),
        _ => None,
    }
}

/// A diagram's drawn picture: the same one only as itself.
#[derive(Clone)]
struct Drawn(Rc<Picture>);

impl PartialEq for Drawn {
    fn eq(&self, other: &Drawn) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for Drawn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Drawn({} × {})", self.0.width, self.0.height)
    }
}

/// What is known of a diagram's drawing.
#[derive(Clone, Debug, PartialEq)]
enum Shown {
    /// Asked of the driver, not answered yet.
    Asked,
    Picture(Drawn),
    /// It could not be drawn: why (Mermaid's error, for one that does not parse).
    Failed(String),
    /// Nothing here draws diagrams: what to install.
    Missing(String),
}

/// The window's diagrams, by the key their drawing was asked by.
#[derive(Clone, Copy)]
struct State {
    drawings: RwSignal<HashMap<String, RwSignal<Shown>>>,
}

impl crate::ext::State for State {
    fn new(ui: Ui) -> Self {
        State { drawings: ui.scope.create_rw_signal(HashMap::new()) }
    }
}

thread_local! {
    /// What asks the driver, kept at start.
    static SEND: RefCell<Option<Send>> = const { RefCell::new(None) };
}

fn ask(m: Value) {
    if let Some(send) = SEND.with(|s| s.borrow().clone()) {
        send(m);
    }
}

/// The key a diagram's drawing is asked and kept by: its kind, its source, and the theme it is drawn for (dark or
/// light, and the background it is laid on).
fn key_of(kind: &str, source: &str, dark: bool, background: [u8; 3]) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (kind, source, dark, background).hash(&mut h);
    format!("{kind}-{:016x}", h.finish())
}

/// The theme a diagram is drawn for now: dark or not, and the background it is laid on.
fn theme(ui: Ui) -> (bool, [u8; 3]) {
    ui.colors.with(|c| (c.dark, [c.editor_bg.r, c.editor_bg.g, c.editor_bg.b]))
}

/// A diagram's drawing, asked of the driver the first time it is wanted.
fn drawing(ui: Ui, kind: &'static str, source: &str) -> RwSignal<Shown> {
    let st = ui.state::<State>();
    let (dark, background) = theme(ui);
    let key = key_of(kind, source, dark, background);
    if let Some(found) = st.drawings.with_untracked(|d| d.get(&key).copied()) {
        return found;
    }
    let shown = ui.scope.create_rw_signal(Shown::Asked);
    st.drawings.update(|d| {
        d.insert(key.clone(), shown);
    });
    ask(json!({"type": "diagram", "id": key, "kind": kind, "source": source, "dark": dark}));
    shown
}

/// What a diagram's block shows under its bar.
#[derive(Clone, Debug, PartialEq)]
enum Over {
    Nothing,
    Drawing,
    Picture(Drawn),
    Note { text: String, missing: bool },
}

/// What is under the bar: while the code is not asked for, the drawing (or that it is being drawn); always, why it
/// could not be drawn.
fn over_of(shown: Option<&Shown>, showing_code: bool) -> Over {
    match shown {
        None => Over::Nothing,
        Some(Shown::Asked) if !showing_code => Over::Drawing,
        Some(Shown::Picture(p)) if !showing_code => Over::Picture(p.clone()),
        Some(Shown::Failed(e)) => Over::Note { text: e.clone(), missing: false },
        Some(Shown::Missing(e)) => Over::Note { text: e.clone(), missing: true },
        Some(_) => Over::Nothing,
    }
}

/// A diagram's block: its code's box, with the drawing in the lines' place once its fence is closed.
pub fn block(ui: Ui, lang: &str, name: &str, text: impl Fn() -> String + 'static, closed: Memo<bool>) -> impl IntoView {
    let t = ui.colors;
    let kind = kind_of(lang).unwrap_or("mermaid");
    let source = create_memo(move |_| text());
    let showing_code = create_rw_signal(false);
    // Asked once its fence is closed, and again for another theme.
    let shown = create_memo(move |_| {
        if !closed.get() {
            return None;
        }
        theme(ui);
        Some(source.with(|s| drawing(ui, kind, s)).get())
    });
    let over = create_memo(move |_| shown.with(|s| over_of(s.as_ref(), showing_code.get())));
    let drawn = create_memo(move |_| over.with(|o| matches!(o, Over::Drawing | Over::Picture(_))));
    let has_picture = create_memo(move |_| shown.with(|s| matches!(s, Some(Shown::Picture(_)))));
    let open = move || {
        let dark = theme(ui).0;
        let id = source.with_untracked(|s| key_of(kind, s, dark, theme(ui).1));
        ask(json!({"type": "diagram_open", "id": id, "kind": kind, "source": source.get_untracked(), "dark": dark}));
    };
    let actions = h_stack((
        container(bar_action(ui, "code", || "Show Code".into(), move || showing_code.set(true)))
            .style(move |s| s.apply_if(!has_picture.get() || showing_code.get(), |s| s.hide())),
        container(bar_action(ui, "preview", || "Show Diagram".into(), move || showing_code.set(false)))
            .style(move |s| s.apply_if(!has_picture.get() || !showing_code.get(), |s| s.hide())),
        bar_action(ui, "link-external", || "Open in Browser".into(), open),
    ))
    .style(|s| s.gap(2.0).items_center());
    let under = dyn_container(
        move || over.get(),
        move |o| match o {
            Over::Nothing => empty().style(|s| s.hide()).into_any(),
            Over::Drawing => label(|| "Drawing the diagram...")
                .style(move |s| s.padding_horiz(12.0).padding_vert(8.0).font_size(13.0).color(t.get().desc))
                .into_any(),
            Over::Picture(p) => picture(p.0),
            Over::Note { text, missing } => {
                let glyph = if missing { "info" } else { "error" };
                h_stack((
                    crate::icons::icon(glyph, 16.0).style(move |s| {
                        let c = t.get();
                        s.flex_shrink(0.0_f32).color(if missing { c.desc } else { c.error })
                    }),
                    label(move || text.clone()).style(move |s| {
                        s.flex_grow(1.0_f32)
                            .flex_basis(0.0)
                            .min_width(0.0)
                            .text_overflow(TextOverflow::Wrap)
                            .color(if missing { t.get().desc } else { t.get().error })
                    }),
                ))
                .style(move |s| {
                    s.width_full()
                        .gap(6.0)
                        .padding_horiz(12.0)
                        .padding_vert(6.0)
                        .font_size(12.0)
                        .border_bottom(1.0)
                        .border_color(t.get().widget_border)
                })
                .into_any()
            }
        },
    )
    .style(|s| s.width_full().min_width(0.0))
    .into_any();
    code_with(ui, lang, name, move || source.get(), Some(Drawing { actions: actions.into_any(), over: under, drawn }))
}

/// The tallest a diagram is drawn in an answer; a taller one is made smaller to fit.
const MOST_HEIGHT: f64 = 800.0;

/// A diagram drawn at its own size, made smaller to fit the answer's width.
fn picture(p: Rc<Picture>) -> AnyView {
    let (w, h) = (f64::from(p.width).max(1.0), f64::from(p.height).max(1.0));
    container(
        media::drawn(p)
            .style(move |s| s.width_full().max_width(w).max_height(MOST_HEIGHT).aspect_ratio((w / h) as f32)),
    )
    .style(|s| s.width_full().min_width(0.0).justify_center().padding(12.0))
    .into_any()
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, send: &Send) {
    SEND.with(|s| *s.borrow_mut() = Some(send.clone()));
}

/// The session's messages that are this part's, taken in: a diagram's drawing, drawn off the window's thread.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] != "diagram" {
        return false;
    }
    let key = m["id"].as_str().unwrap_or("").to_string();
    let Some(shown) = ui.state::<State>().drawings.with_untracked(|d| d.get(&key).copied()) else { return true };
    let error = m["error"].as_str().unwrap_or("The diagram could not be drawn.").to_string();
    let Some(svg) = m["svg"].as_str().map(str::to_string) else {
        shown.set(if m["missing"] == true { Shown::Missing(error) } else { Shown::Failed(error) });
        return true;
    };
    let under = theme(ui).1;
    let done = floem::ext_event::create_ext_action(ui.scope, move |made: Result<media::Decoded, String>| {
        shown.set(match made {
            Ok(d) => Shown::Picture(Drawn(Rc::new(Picture::new(d, 0, key.into_bytes())))),
            Err(e) => Shown::Failed(format!("The diagram could not be drawn: {e}")),
        });
    });
    std::thread::spawn(move || done(media::decode(svg.as_bytes(), "diagram.svg", [under, under])));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mermaid_fence_is_a_diagram_and_others_are_code() {
        assert_eq!((kind_of("mermaid"), kind_of("mmd")), (Some("mermaid"), Some("mermaid")));
        assert_eq!((kind_of("rust"), kind_of("")), (None, None));
    }

    #[test]
    fn a_diagram_is_kept_by_its_source_and_its_theme() {
        let k = |s: &str, dark: bool| key_of("mermaid", s, dark, [30, 30, 30]);
        assert_eq!(k("graph TD; A-->B", true), k("graph TD; A-->B", true));
        assert_ne!(k("graph TD; A-->B", true), k("graph TD; A-->C", true));
        assert_ne!(k("graph TD; A-->B", true), k("graph TD; A-->B", false));
        assert_ne!(k("a", true), key_of("mermaid", "a", true, [0, 0, 0]));
    }

    #[test]
    fn the_code_is_shown_until_it_is_drawn_or_asked_for_and_why_it_was_not_drawn_always() {
        assert_eq!(over_of(None, false), Over::Nothing);
        assert_eq!(over_of(Some(&Shown::Asked), false), Over::Drawing);
        assert_eq!(over_of(Some(&Shown::Asked), true), Over::Nothing);
        let failed = Shown::Failed("Parse error on line 2".into());
        assert_eq!(over_of(Some(&failed), true), Over::Note { text: "Parse error on line 2".into(), missing: false });
        let missing = Shown::Missing("install mermaid-cli".into());
        assert_eq!(over_of(Some(&missing), false), Over::Note { text: "install mermaid-cli".into(), missing: true });
    }

    #[test]
    fn mermaids_own_svg_is_drawn_with_its_labels() {
        let svg = include_str!("diagrams/flowchart.svg");
        assert!(!svg.contains("<foreignObject"), "labels as SVG text");
        let d = media::decode(svg.as_bytes(), "diagram.svg", [[30, 30, 30]; 2]).expect("drawn");
        assert_eq!((d.width, d.height), (360, 406));
        // Its labels are kept as text drawn in a font of the system's, where the system has one.
        let options = media::svg_options();
        if options.fontdb.is_empty() {
            return;
        }
        fn texts(g: &resvg::usvg::Group) -> usize {
            g.children()
                .iter()
                .map(|n| match n {
                    resvg::usvg::Node::Group(g) => texts(g),
                    resvg::usvg::Node::Text(t) => usize::from(t.flattened().bounding_box().width() > 0.0),
                    _ => 0,
                })
                .sum()
        }
        let tree = resvg::usvg::Tree::from_data(svg.as_bytes(), &options).expect("parsed");
        // Start, Is it?, OK, Find out, Yes, No.
        assert!(texts(tree.root()) >= 6, "{} labels drawn", texts(tree.root()));
    }
}
