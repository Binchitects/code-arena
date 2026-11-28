//! Tabs of files that are not text, as VS Code's media preview has them: a picture on a checkerboard, fitted to the
//! editor until a click shows it at its own size (and back), its size in pixels and in bytes in the status bar, and
//! drawn again when it changes on disk; a sound or a video by its name and size, with a button that opens it in
//! the system's player; an SVG's preview, from the button over its text; and a tab that says why a file could not
//! be opened (not text, larger than 5 MB, or what reading it said).
//!
//! All are tabs of the kind `media` ("media:<path>"), kept with the layout. A picture is decoded off the window's
//! thread, made into the copy that is drawn (at most 1024 px a side, its see-through parts on the checkerboard)
//! there, and drawn by a view of its own: floem's `img` would decode it again each time its tab is shown.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use base64::Engine;
use floem::context::PaintCx;
use floem::kurbo::Rect;
use floem::peniko::{self, Blob};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::CursorStyle;
use floem::views::{Decorators, container, dyn_container, empty, h_stack, label, scroll, v_stack};
use floem::{AnyView, IntoView, View, ViewId};
use floem_renderer::Renderer as _;
use serde_json::{Value, json};

use crate::editor::kinds;
use crate::icons::icon;
use crate::state::{Send, Tab, Ui};
use crate::statusbar;
use crate::widgets::button;

/// The kind of tab, and what its tabs' paths begin with.
pub const KIND: &str = "media";

/// The longest side of the copy of a picture that is drawn: the renderer keeps pictures in a texture it makes twice
/// as large as the largest it was given (with the glyphs, and another of the same size for their masks).
const MOST_DRAWN: u32 = 1024;
/// A picture larger than this a side is not drawn at all.
const MOST_SIDE: u32 = 32_768;
/// A picture of more pixels than this is not drawn at all (its decoding alone would take gigabytes).
const MOST_PIXELS: u64 = 100_000_000;
/// The side of a square of the checkerboard, in the drawn copy's pixels (VS Code's is 8 px).
const SQUARE: u32 = 8;

/// What a file is shown as, by its name's ending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Media {
    Picture,
    Svg,
    Sound,
    Video,
}

pub fn media_of(path: &str) -> Option<Media> {
    let ext = path.rsplit('/').next().unwrap_or(path).rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" => Some(Media::Picture),
        "svg" => Some(Media::Svg),
        "mp3" | "wav" | "ogg" | "oga" | "opus" | "flac" | "m4a" | "aac" | "weba" => Some(Media::Sound),
        "mp4" | "webm" | "ogv" | "mov" => Some(Media::Video),
        _ => None,
    }
}

/// Whether a file opens as media rather than as text (an SVG opens as text, its preview from a button).
pub fn opens_as_media(path: &str) -> bool {
    media_of(path).is_some_and(|m| m != Media::Svg)
}

fn name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// What the tab of a file that could not be opened says, by why (the `kind` of the session's `file` reply).
pub fn cannot_text(path: &str, kind: &str, error: &str) -> String {
    let name = name_of(path);
    match kind {
        "binary" => format!("{name} is not text (or not UTF-8)"),
        "too_large" => format!("{name} is larger than 5 MB"),
        _ => format!("{name} could not be opened: {error}"),
    }
}

/// A size in bytes as VS Code's status bar says it.
pub fn bytes(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if f < K {
        format!("{n}B")
    } else if f < K * K {
        format!("{:.2}KB", f / K)
    } else if f < K * K * K {
        format!("{:.2}MB", f / K / K)
    } else {
        format!("{:.2}GB", f / K / K / K)
    }
}

/// The size a picture of `w` × `h` is drawn at in a place of `cw` × `ch`: as large as fits, never larger than itself.
pub fn fitted(w: f64, h: f64, cw: f64, ch: f64) -> (f64, f64) {
    if w <= 0.0 || h <= 0.0 {
        return (0.0, 0.0);
    }
    let scale = (cw / w).min(ch / h).clamp(0.0, 1.0);
    (w * scale, h * scale)
}

/// A picture made ready to draw: its own size, and the copy that is drawn (RGBA, opaque, on the checkerboard).
#[derive(Debug)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub drawn: (u32, u32),
    pub rgba: Vec<u8>,
}

/// A picture's bytes decoded (by its file's ending: a GIF's first frame, an SVG drawn), made smaller when it is
/// very large, and laid on the checkerboard of these two colours.
pub fn decode(bytes: &[u8], path: &str, checker: [[u8; 3]; 2]) -> Result<Decoded, String> {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let (width, height, picture) = match ext.as_str() {
        "svg" => drawn_svg(bytes)?,
        "gif" => {
            let p = first_frame(bytes)?;
            (p.width(), p.height(), p)
        }
        _ => {
            let p = still(bytes)?;
            (p.width(), p.height(), p)
        }
    };
    let large = picture.width().max(picture.height()) > MOST_DRAWN;
    let picture = if large { picture.thumbnail(MOST_DRAWN, MOST_DRAWN) } else { picture };
    let mut rgba = picture.into_rgba8();
    for (x, y, px) in rgba.enumerate_pixels_mut() {
        let under = checker[((x / SQUARE + y / SQUARE) % 2) as usize];
        let a = u32::from(px[3]);
        for i in 0..3 {
            px[i] = ((u32::from(px[i]) * a + u32::from(under[i]) * (255 - a)) / 255) as u8;
        }
        px[3] = 255;
    }
    let drawn = rgba.dimensions();
    Ok(Decoded { width, height, drawn, rgba: rgba.into_raw() })
}

fn still(bytes: &[u8]) -> Result<image::DynamicImage, String> {
    let reader =
        image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let (w, h) = reader.into_dimensions().map_err(|e| e.to_string())?;
    if w.max(h) > MOST_SIDE || u64::from(w) * u64::from(h) > MOST_PIXELS {
        return Err(format!("{w} × {h} is too large to draw"));
    }
    image::load_from_memory(bytes).map_err(|e| e.to_string())
}

/// An SVG drawn (twice its own size, for a sharp picture on a dense screen), with its own size.
fn drawn_svg(bytes: &[u8]) -> Result<(u32, u32, image::DynamicImage), String> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_data(bytes, &svg_options()).map_err(|e| e.to_string())?;
    let size = tree.size();
    let (w, h) = (size.width().ceil().max(1.0), size.height().ceil().max(1.0));
    let scale = (MOST_DRAWN as f32 / w.max(h)).min(2.0);
    let (dw, dh) = ((w * scale).round().max(1.0) as u32, (h * scale).round().max(1.0) as u32);
    let mut pixmap = tiny_skia::Pixmap::new(dw, dh).ok_or("too large to draw")?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    let drawn = image::RgbaImage::from_raw(dw, dh, rgba).ok_or("it could not be drawn")?;
    Ok((w as u32, h as u32, image::DynamicImage::ImageRgba8(drawn)))
}

/// A GIF's first frame, laid where it is on its screen.
fn first_frame(bytes: &[u8]) -> Result<image::DynamicImage, String> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut d = options.read_info(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let (w, h) = (u32::from(d.width()), u32::from(d.height()));
    if w.max(h) > MOST_SIDE || u64::from(w) * u64::from(h) > MOST_PIXELS {
        return Err(format!("{w} × {h} is too large to draw"));
    }
    let mut screen = image::RgbaImage::new(w, h);
    if let Some(f) = d.read_next_frame().map_err(|e| e.to_string())? {
        for (i, px) in f.buffer.as_chunks::<4>().0.iter().enumerate() {
            let (x, y) =
                (u32::from(f.left) + i as u32 % u32::from(f.width), u32::from(f.top) + i as u32 / u32::from(f.width));
            if x < w && y < h {
                screen.put_pixel(x, y, image::Rgba([px[0], px[1], px[2], px[3]]));
            }
        }
    }
    Ok(image::DynamicImage::ImageRgba8(screen))
}

/// A picture ready to draw.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub size: u64,
    image: peniko::Image,
    /// What the renderer keeps its copy by: the file and when it changed.
    hash: Vec<u8>,
}

impl Picture {
    /// A decoded picture made ready to draw, kept by the renderer by this hash.
    pub(crate) fn new(d: Decoded, size: u64, hash: Vec<u8>) -> Picture {
        let image = peniko::Image::new(Blob::new(Arc::new(d.rgba)), peniko::Format::Rgba8, d.drawn.0, d.drawn.1);
        Picture { width: d.width, height: d.height, size, image, hash }
    }
}

/// The view that draws a picture into its place, as large as fits and never larger than itself.
pub(crate) fn drawn(picture: Rc<Picture>) -> impl View {
    Drawn { id: ViewId::new(), picture }
}

/// What an SVG's text is drawn with: the system's fonts, read once (on the first thread that draws one), its
/// `sans-serif` one of them (an SVG naming only fonts that are not here is drawn in it, its text not left out).
pub(crate) fn svg_options() -> resvg::usvg::Options<'static> {
    static FONTS: std::sync::OnceLock<Arc<resvg::usvg::fontdb::Database>> = std::sync::OnceLock::new();
    let fonts = FONTS.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_system_fonts();
        let families: Vec<String> =
            db.faces().filter_map(|f| f.families.first().map(|(name, _)| name.clone())).collect();
        let sans = ["Arial", "Liberation Sans", "DejaVu Sans", "Noto Sans", "Ubuntu", "Cantarell", "Segoe UI"]
            .into_iter()
            .map(str::to_string)
            .find(|want| families.iter().any(|f| f.eq_ignore_ascii_case(want)))
            .or_else(|| families.first().cloned());
        if let Some(sans) = sans {
            db.set_sans_serif_family(sans);
        }
        Arc::new(db)
    });
    resvg::usvg::Options { fontdb: fonts.clone(), ..Default::default() }
}

/// What a media tab shows.
#[derive(Clone)]
enum Item {
    Loading,
    Picture(Rc<Picture>),
    Sound(u64),
    Video(u64),
    Cannot(String),
}

/// The media tabs' states, by the file's path.
#[derive(Clone, Copy)]
struct State {
    /// What each shows, with a number that changes each time it is set (what its view is built again by).
    items: RwSignal<HashMap<String, (u64, Item)>>,
    /// The pictures shown at their own size (the rest are fitted to the editor).
    actual: RwSignal<HashSet<String>>,
    /// When each was last read, as the disk said.
    modified: RwSignal<HashMap<String, u64>>,
    count: RwSignal<u64>,
}

impl crate::ext::State for State {
    fn new(ui: Ui) -> Self {
        State {
            items: ui.scope.create_rw_signal(HashMap::new()),
            actual: ui.scope.create_rw_signal(HashSet::new()),
            modified: ui.scope.create_rw_signal(HashMap::new()),
            count: ui.scope.create_rw_signal(0),
        }
    }
}

impl State {
    fn set(self, path: &str, item: Item) {
        self.count.update(|n| *n += 1);
        let n = self.count.get_untracked();
        self.items.update(|m| {
            m.insert(path.to_string(), (n, item));
        });
    }
}

fn tab_of(path: &str) -> String {
    format!("{KIND}:{path}")
}

fn ask(send: &Send, path: &str, media: Option<Media>) {
    let data = matches!(media, Some(Media::Picture | Media::Svg));
    send(json!({"type": "media", "path": path, "data": data}));
}

/// A file opened as media: its tab shown, or made and its file asked for. False when it does not open as media.
pub fn open(ui: Ui, path: &str, send: &Send) -> bool {
    // A file whose tab says it could not be opened has that tab shown again.
    if ui.tab(&tab_of(path)).is_some() && !matches!(media_of(path), Some(Media::Svg)) {
        kinds::open(ui, &tab_of(path));
        return true;
    }
    if !opens_as_media(path) {
        return false;
    }
    show(ui, path, send);
    true
}

/// An SVG's preview opened (the button over its text).
pub fn preview(ui: Ui, path: &str, send: &Send) {
    show(ui, path, send);
}

fn show(ui: Ui, path: &str, send: &Send) {
    let id = tab_of(path);
    if ui.tab(&id).is_none() {
        ui.state::<State>().set(path, Item::Loading);
        ask(send, path, media_of(path));
    }
    kinds::open(ui, &id);
}

/// A file that could not be opened as text: a tab that says why, in place of a word in the status bar only.
pub fn cannot(ui: Ui, path: &str, kind: &str, error: &str) {
    ui.state::<State>().set(path, Item::Cannot(cannot_text(path, kind, error)));
    if ui.tab(&tab_of(path)).is_none() {
        kinds::open(ui, &tab_of(path));
    }
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    kinds::add(
        kinds::kind(
            KIND,
            |_ui, rest| match media_of(rest) {
                Some(Media::Svg) => format!("Preview {}", name_of(rest)),
                _ => name_of(rest).to_string(),
            },
            |ui, send, tab| view(ui, send, tab),
        )
        .icon("file-media")
        .kept(|ui, send, path| {
            let rest = kinds::split(path).map_or("", |(_, r)| r);
            match media_of(rest) {
                Some(_) => show(ui, rest, send),
                // A file that could not be opened is asked for again: it may be text now.
                // Its tab is made at once, so it keeps its place among the tabs.
                None => {
                    ui.state::<State>().set(rest, Item::Loading);
                    kinds::open(ui, path);
                    send(json!({"type": "open", "path": rest}));
                }
            }
        }),
    );
    statusbar::add("status.media.zoom", statusbar::Side::Right, 202, |ui, _send| {
        let t = ui.colors;
        let st = ui.state::<State>();
        let shown = move || shown_picture(ui).map(|p| st.actual.with(|a| a.contains(&p)));
        label(move || match shown() {
            Some(true) => "100%",
            _ => "Whole Image",
        })
        .on_click_stop(move |_| {
            if let Some(p) = shown_picture(ui) {
                toggle(st, &p);
            }
        })
        .style(move |s| statusbar::item(t, s).cursor(CursorStyle::Pointer).apply_if(shown().is_none(), |s| s.hide()))
        .into_any()
    });
    statusbar::add("status.media.size", statusbar::Side::Right, 204, |ui, _send| {
        let t = ui.colors;
        let text = move || {
            shown_item(ui).and_then(|i| match i {
                Item::Picture(p) => Some(bytes(p.size)),
                Item::Sound(n) | Item::Video(n) => Some(bytes(n)),
                _ => None,
            })
        };
        label(move || text().unwrap_or_default())
            .style(move |s| statusbar::item(t, s).apply_if(text().is_none(), |s| s.hide()))
            .into_any()
    });
    statusbar::add("status.media.dimensions", statusbar::Side::Right, 206, |ui, _send| {
        let t = ui.colors;
        let text = move || match shown_item(ui) {
            Some(Item::Picture(p)) => Some(format!("{} × {}", p.width, p.height)),
            _ => None,
        };
        label(move || text().unwrap_or_default())
            .style(move |s| statusbar::item(t, s).apply_if(text().is_none(), |s| s.hide()))
            .into_any()
    });
}

/// The shown tab's file, when it is a media tab.
fn shown_file(ui: Ui) -> Option<String> {
    ui.active
        .with(|a| a.as_deref().and_then(|p| p.strip_prefix(KIND)).and_then(|r| r.strip_prefix(':')).map(String::from))
}

fn shown_item(ui: Ui) -> Option<Item> {
    let path = shown_file(ui)?;
    ui.state::<State>().items.with(|m| m.get(&path).map(|(_, i)| i.clone()))
}

/// The shown tab's picture's file, when it shows one.
fn shown_picture(ui: Ui) -> Option<String> {
    matches!(shown_item(ui), Some(Item::Picture(_))).then(|| shown_file(ui)).flatten()
}

fn toggle(st: State, path: &str) {
    st.actual.update(|a| {
        if !a.remove(path) {
            a.insert(path.to_string());
        }
    });
}

fn view(ui: Ui, send: Send, tab: &Tab) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<State>();
    let path = tab.file.clone();
    let key = {
        let path = path.clone();
        create_memo(move |_| st.items.with(|m| m.get(&path).map_or(0, |(n, _)| *n)))
    };
    dyn_container(
        move || key.get(),
        move |_| {
            let item = st.items.with_untracked(|m| m.get(&path).map(|(_, i)| i.clone()));
            match item.unwrap_or(Item::Loading) {
                Item::Loading => empty().into_any(),
                Item::Picture(p) => picture(st, &path, p),
                Item::Sound(size) => player(ui, send.clone(), &path, "unmute", size),
                Item::Video(size) => player(ui, send.clone(), &path, "device-camera-video", size),
                Item::Cannot(text) => {
                    message(ui, "warning", label(move || text.clone()).into_any(), empty().into_any())
                }
            }
        },
    )
    .style(move |s| s.size_full().min_size(0.0, 0.0).background(t.get().editor_bg))
    .into_any()
}

/// A picture: fitted to the editor, or at its own size in a scroll; a click on it changes which.
fn picture(st: State, path: &str, p: Rc<Picture>) -> AnyView {
    let (path, key) = (path.to_string(), path.to_string());
    let actual = create_memo(move |_| st.actual.with(|a| a.contains(&key)));
    dyn_container(
        move || actual.get(),
        move |actual| {
            let (w, h) = (f64::from(p.width), f64::from(p.height));
            let on = path.clone();
            let drawn = drawn(p.clone())
                .on_click_stop(move |_| toggle(st, &on))
                .style(|s| s.cursor(CursorStyle::Pointer));
            if actual {
                scroll(
                    container(drawn.style(move |s| s.width(w).height(h).flex_shrink(0.0_f32)))
                        .style(|s| s.min_size_full().items_center().justify_center()),
                )
                // Out of the flow, so a picture larger than the editor does not make the editor larger.
                .style(|s| s.absolute().inset(0.0))
                .into_any()
            } else {
                drawn.style(|s| s.size_full()).into_any()
            }
        },
    )
    .style(|s| s.size_full().min_size(0.0, 0.0))
    .into_any()
}

/// A sound or a video: its name and size, and the button that opens it in the system's player.
fn player(ui: Ui, send: Send, path: &str, glyph: &'static str, size: u64) -> AnyView {
    let t = ui.colors;
    let text = format!("{} ({})", name_of(path), bytes(size));
    let file = path.to_string();
    let open = button(t, "Open in the system player", true, move || {
        send(json!({"type": "media_open", "path": file}));
    });
    message(ui, glyph, label(move || text.clone()).into_any(), open.into_any())
}

/// What a tab says in the middle of the editor: an icon, a line, and what can be done.
fn message(ui: Ui, glyph: &'static str, line: AnyView, act: AnyView) -> AnyView {
    let t = ui.colors;
    container(
        v_stack((h_stack((icon(glyph, 16.0), line)).style(|s| s.gap(8.0).items_center()), act))
            .style(|s| s.gap(12.0).items_center()),
    )
    .style(move |s| s.size_full().items_center().justify_center().font_size(13.0).color(t.get().fg))
    .into_any()
}

/// The view that draws a decoded picture into its place, as large as fits and never larger than itself.
struct Drawn {
    id: ViewId,
    picture: Rc<Picture>,
}

impl View for Drawn {
    fn id(&self) -> ViewId {
        self.id
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        let place = self.id.get_content_rect();
        let p = &self.picture;
        let (w, h) = fitted(f64::from(p.width), f64::from(p.height), place.width(), place.height());
        let (x, y) = (place.x0 + (place.width() - w) / 2.0, place.y0 + (place.height() - h) / 2.0);
        // Drawn at the origin of a moved drawing: the software renderer lays a picture from the origin, whatever
        // the place it is given.
        cx.save();
        cx.offset((x.round(), y.round()));
        cx.draw_img(floem_renderer::Img { img: p.image.clone(), hash: &p.hash }, Rect::new(0.0, 0.0, w, h));
        cx.restore();
    }
}

/// The session's messages that are this part's, taken in. Never taken away from the parts after it.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = ui.state::<State>();
    let path = m["path"].as_str().unwrap_or("").to_string();
    match m["type"].as_str().unwrap_or("") {
        "media" => {
            if ui.tab(&tab_of(&path)).is_none() {
                return false;
            }
            if let Some(e) = m["error"].as_str() {
                st.set(&path, Item::Cannot(format!("{} could not be opened: {e}", name_of(&path))));
                return false;
            }
            let size = m["size"].as_u64().unwrap_or(0);
            if let Some(at) = m["modified"].as_u64() {
                st.modified.update(|k| {
                    k.insert(path.clone(), at);
                });
            }
            match media_of(&path) {
                Some(Media::Sound) => st.set(&path, Item::Sound(size)),
                Some(Media::Video) => st.set(&path, Item::Video(size)),
                Some(Media::Picture | Media::Svg) => decode_off_thread(ui, st, path, m, size),
                None => {}
            }
        }
        "media_open" => {
            if let Some(e) = m["error"].as_str() {
                ui.message.set(format!("Could not open {path}: {e}"));
            }
        }
        // A picture or a preview whose file changed on disk is read again.
        "stats" => {
            for it in m["items"].as_array().into_iter().flatten() {
                let file = it["path"].as_str().unwrap_or("");
                let media = media_of(file);
                if !matches!(media, Some(Media::Picture | Media::Svg)) || ui.tab(&tab_of(file)).is_none() {
                    continue;
                }
                let changed = it["modified"]
                    .as_u64()
                    .is_some_and(|now| st.modified.with_untracked(|k| k.get(file).is_some_and(|known| *known != now)));
                if changed {
                    ask(send, file, media);
                }
            }
        }
        _ => {}
    }
    false
}

fn decode_off_thread(ui: Ui, st: State, path: String, m: &Value, size: u64) {
    let data = m["data"].as_str().unwrap_or("").to_string();
    let modified = m["modified"].as_u64().unwrap_or(0);
    let c = ui.colors.get_untracked();
    let rgb = |c: peniko::Color| [c.r, c.g, c.b];
    let checker = [rgb(c.editor_bg), if c.dark { [20, 20, 20] } else { [230, 230, 230] }];
    let hash = format!("{path}\0{modified}\0{}", c.dark).into_bytes();
    let file = path.clone();
    let done = floem::ext_event::create_ext_action(ui.scope, move |made: Result<Decoded, String>| {
        if ui.tab(&tab_of(&file)).is_none() {
            return;
        }
        match made {
            Ok(d) => {
                st.set(&file, Item::Picture(Rc::new(Picture::new(d, size, hash))));
            }
            Err(e) => st.set(&file, Item::Cannot(format!("{} could not be shown: {e}", name_of(&file)))),
        }
    });
    std::thread::spawn(move || {
        let made = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| e.to_string())
            .and_then(|bytes| decode(&bytes, &path, checker));
        done(made);
    });
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn sender() -> (Send, Rc<RefCell<Vec<Value>>>) {
        let said = Rc::new(RefCell::new(Vec::new()));
        let to = said.clone();
        (Rc::new(move |v: Value| to.borrow_mut().push(v)), said)
    }

    fn shown(ui: Ui, path: &str) -> Option<Item> {
        ui.state::<State>().items.with_untracked(|m| m.get(path).map(|(_, i)| i.clone()))
    }

    #[test]
    fn media_opens_in_a_tab_of_its_own_and_what_cannot_open_says_why() {
        let ui = Ui::new();
        let (send, said) = sender();
        register(ui, &send);
        ui.open("img/a.png", &send);
        ui.open("song.mp3", &send);
        assert_eq!(
            *said.borrow(),
            [
                json!({"type": "media", "path": "img/a.png", "data": true}),
                json!({"type": "media", "path": "song.mp3", "data": false}),
            ],
            "asked as media, not as text; a sound's bytes are not asked for"
        );
        assert!(ui.tab("media:img/a.png").is_some_and(|t| !t.is_file()) && ui.tab("img/a.png").is_none());
        assert_eq!(kinds::title(ui, &ui.tab("media:song.mp3").expect("tab")), "song.mp3");
        ui.take(
            &json!({"type": "media", "path": "song.mp3", "mime": "audio/mpeg", "size": 2048, "modified": 1}),
            &send,
        );
        assert!(matches!(shown(ui, "song.mp3"), Some(Item::Sound(2048))));
        // Not text: a tab that says so, in place of a word in the status bar only.
        ui.take(&json!({"type": "file", "path": "bin/tool", "error": "a binary file", "kind": "binary"}), &send);
        assert_eq!(ui.active.get_untracked().as_deref(), Some("media:bin/tool"));
        assert!(matches!(shown(ui, "bin/tool"), Some(Item::Cannot(t)) if t == "tool is not text (or not UTF-8)"));
        assert!(ui.message.get_untracked().is_empty());
        ui.active.set(Some("media:song.mp3".into()));
        said.borrow_mut().clear();
        ui.open("bin/tool", &send);
        assert_eq!(ui.active.get_untracked().as_deref(), Some("media:bin/tool"), "its tab shown again");
        assert!(said.borrow().is_empty(), "not asked for again");
        // An SVG's preview, drawn again when its file changes on disk.
        said.borrow_mut().clear();
        preview(ui, "logo.svg", &send);
        assert_eq!(kinds::title(ui, &ui.tab("media:logo.svg").expect("tab")), "Preview logo.svg");
        let data = base64::engine::general_purpose::STANDARD.encode("<svg/>");
        ui.take(&json!({"type": "media", "path": "logo.svg", "size": 6, "modified": 1, "data": data}), &send);
        ui.take(&json!({"type": "stats", "items": [{"path": "logo.svg", "modified": 1}]}), &send);
        ui.take(&json!({"type": "stats", "items": [{"path": "logo.svg", "modified": 2}]}), &send);
        let asked: Vec<Value> = said.borrow().iter().filter(|m| m["type"] == "media").cloned().collect();
        assert_eq!(asked.len(), 2, "asked once to open, once more when it changed: {asked:?}");
    }

    #[test]
    fn a_file_is_shown_by_its_ending() {
        assert_eq!(media_of("img/Logo.PNG"), Some(Media::Picture));
        assert_eq!(media_of("a.svg"), Some(Media::Svg));
        assert_eq!(media_of("song.oga"), Some(Media::Sound));
        assert_eq!(media_of("clip.mov"), Some(Media::Video));
        assert_eq!(media_of("src/main.rs"), None);
        assert_eq!(media_of("a.png/README"), None, "the name's ending, not a folder's");
        assert!(opens_as_media("a.webp") && !opens_as_media("a.svg"), "an SVG opens as text");
    }

    #[test]
    fn a_file_that_cannot_open_says_why() {
        assert_eq!(cannot_text("bin/tool", "binary", "a binary file"), "tool is not text (or not UTF-8)");
        assert_eq!(cannot_text("big.log", "too_large", "bigger than 5 MB"), "big.log is larger than 5 MB");
        assert_eq!(cannot_text("dev", "other", "not there"), "dev could not be opened: not there");
    }

    #[test]
    fn a_picture_fits_and_is_never_made_larger() {
        assert_eq!(fitted(200.0, 100.0, 100.0, 100.0), (100.0, 50.0));
        assert_eq!(fitted(100.0, 400.0, 500.0, 200.0), (50.0, 200.0));
        assert_eq!(fitted(16.0, 16.0, 500.0, 500.0), (16.0, 16.0));
        assert_eq!(fitted(0.0, 10.0, 5.0, 5.0), (0.0, 0.0));
        assert_eq!(
            (bytes(512), bytes(1536), bytes(3 * 1024 * 1024)),
            ("512B".into(), "1.50KB".into(), "3.00MB".into())
        );
    }

    #[test]
    fn a_picture_is_laid_on_the_checkerboard() {
        // A 2 × 1 PNG: one red pixel, one see-through.
        let mut png = Vec::new();
        let img = image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 0, 0, 0]).expect("raw");
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("png");
        let d = decode(&png, "a.png", [[30, 30, 30], [20, 20, 20]]).expect("decoded");
        assert_eq!((d.width, d.height, d.drawn), (2, 1, (2, 1)));
        assert_eq!(d.rgba, [255, 0, 0, 255, 30, 30, 30, 255]);
        assert!(decode(b"not a picture", "a.png", [[0; 3]; 2]).is_err());
    }

    #[test]
    fn an_svg_is_drawn_twice_its_size_in_its_own_colours() {
        let text = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="20"><rect width="10" height="20" fill="#f00"/></svg>"##;
        let d = decode(text.as_bytes(), "logo.svg", [[0; 3]; 2]).expect("drawn");
        assert_eq!((d.width, d.height, d.drawn), (10, 20, (20, 40)));
        assert_eq!(&d.rgba[..4], [255, 0, 0, 255]);
        assert!(decode(b"<svg", "bad.svg", [[0; 3]; 2]).is_err());
    }

    #[test]
    fn a_gifs_first_frame_is_drawn() {
        let mut out = Vec::new();
        {
            let mut e = gif::Encoder::new(&mut out, 2, 2, &[0, 0, 0, 0, 255, 0]).expect("encoder");
            let frame = gif::Frame {
                width: 2,
                height: 2,
                buffer: std::borrow::Cow::Borrowed(&[1, 1, 1, 0]),
                ..Default::default()
            };
            e.write_frame(&frame).expect("frame");
        }
        let d = decode(&out, "a.GIF", [[9, 9, 9], [9, 9, 9]]).expect("decoded");
        assert_eq!((d.width, d.height), (2, 2));
        assert_eq!(&d.rgba[..4], [0, 255, 0, 255]);
        assert_eq!(&d.rgba[12..], [0, 0, 0, 255]);
    }

    #[test]
    fn a_very_large_picture_is_drawn_from_a_smaller_copy() {
        let img = image::RgbaImage::from_pixel(5000, 10, image::Rgba([1, 2, 3, 255]));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("png");
        let d = decode(&png, "a.png", [[0; 3]; 2]).expect("decoded");
        assert_eq!((d.width, d.height), (5000, 10), "its own size is what is said");
        assert_eq!(d.drawn.0, MOST_DRAWN);
        assert_eq!(d.rgba.len() as u32, d.drawn.0 * d.drawn.1 * 4);
    }
}
