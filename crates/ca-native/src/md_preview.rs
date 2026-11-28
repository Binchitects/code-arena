//! A Markdown file's preview, as VS Code's: Markdown: Open Preview (Ctrl+Shift+V) shows it in a tab of the group
//! the file is in, Open Preview to the Side (Ctrl+K V) in the group beside it, and Show Source (Ctrl+Shift+V in the
//! preview) goes back to the file. The breadcrumbs of a Markdown file have the Open Preview to the Side button.
//!
//! The tab (`preview:PATH`, "Preview NAME") draws the file's text with the answers' renderer (`prose.rs`): its
//! headings, lists, quotes, code blocks with their bar, tables. It follows what is typed in the file's editor, read
//! again 150 ms after the last key, and read off the window's thread; a preview whose file is not open (one kept
//! from the last time) reads the file from the disk. Its pictures (`![alt](img/a.png)`, read from the file's folder)
//! are asked of the session as media and drawn as large as they are, never wider than the preview. A link to a
//! file opens it (a Markdown file in its own preview, as VS Code's `markdown.preview.openMarkdownLinks`), a link to
//! `#a-heading` scrolls to that heading (named as GitHub names them), an address opens in the system's browser.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use base64::Engine;
use floem::kurbo::Point;
use floem::reactive::{RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::views::{Decorators, dyn_container, empty, h_stack, label, scroll};
use floem::{AnyView, IntoView, View, ViewId};
use serde_json::{Value, json};

use crate::citations::{self, Ref, Target};
use crate::commands::{self, command};
use crate::editor::{breadcrumbs, groups, kinds};
use crate::icons::icon;
use crate::markdown::{Block, parse};
use crate::media::{self, Picture};
use crate::prose::{self, Doc};
use crate::state::{Send, Ui};

/// What a preview's tab path begins with.
const KIND: &str = "preview";
/// How long after the last key the preview reads the file again.
const WAIT: Duration = Duration::from_millis(150);

pub const SHOW: &str = "markdown.showPreview";
pub const TO_SIDE: &str = "markdown.showPreviewToSide";
pub const SOURCE: &str = "markdown.showSource";

/// Whether a file is Markdown, by its name's ending.
pub fn is_markdown(path: &str) -> bool {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "md" | "markdown" | "mdown" | "mkd" | "mkdn")
}

/// A heading's name in a link (`#getting-started`), as GitHub makes it: in small letters, its spaces made dashes,
/// all but letters, digits, dashes and underscores left out.
pub fn slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

/// Each heading of a document, by its block's place: its name in a link, `-1`, `-2` after one already taken.
pub fn slugs(blocks: &[Rc<Block>]) -> Vec<(usize, String)> {
    let mut taken: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for (n, b) in blocks.iter().enumerate() {
        let Block::Heading(_, t) = &**b else { continue };
        let base = slug(&t.text);
        let mut name = base.clone();
        while taken.contains_key(&name) {
            let k = taken.entry(base.clone()).or_insert(0);
            *k += 1;
            name = format!("{base}-{k}");
        }
        taken.insert(name.clone(), 0);
        out.push((n, name));
    }
    out
}

/// The folder a file is in (none: the project's top).
fn folder_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// A path written in a document, read from its folder (`/` first: from the project's top): `..` and `.` taken out.
/// None when it leaves the project.
pub fn joined(folder: &str, rel: &str) -> Option<String> {
    let rel = rel.replace('\\', "/");
    let mut parts: Vec<&str> = match rel.strip_prefix('/') {
        Some(_) => Vec::new(),
        None => folder.split('/').filter(|p| !p.is_empty()).collect(),
    };
    for p in rel.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// An address's `%XX` read as the bytes they stand for.
fn unescape(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 2 < b.len())
            .then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
            .flatten();
        match hex {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Whether an address has a scheme (`https:`, `mailto:`, `data:`): it is no file's path.
fn has_scheme(href: &str) -> bool {
    href.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1 && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// Where a link of a document in `folder` leads: one of its headings (`#name`), an address the browser opens, a
/// file read from its folder (at a line with `#L12`). None for anything else (`javascript:`, a path out of the
/// project).
pub fn target(folder: &str, href: &str) -> Option<Target> {
    let h = href.trim();
    let lower = h.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") {
        return Some(Target::Web(h.to_string()));
    }
    if has_scheme(h) {
        return None;
    }
    let (path, fragment) = h.split_once('#').unwrap_or((h, ""));
    if path.is_empty() {
        return (!fragment.is_empty()).then(|| Target::Heading(unescape(fragment).to_lowercase()));
    }
    let line = fragment.strip_prefix('L').and_then(|l| l.split('-').next()?.parse().ok());
    let path = joined(folder, &unescape(path))?;
    Some(Target::File(Ref { path, line, ..Ref::default() }))
}

/// The file a picture of a document in `folder` is (none: an address on the web, which is not fetched).
pub fn picture_path(folder: &str, src: &str) -> Option<String> {
    let s = src.trim();
    if has_scheme(s) || s.starts_with("//") {
        return None;
    }
    let path = s.split(['#', '?']).next().unwrap_or("");
    joined(folder, &unescape(path))
}

/// A picture of a preview, as far as it has come.
#[derive(Clone, PartialEq)]
enum Pic {
    Loading,
    Shown(Rc<Picture>),
    Failed,
}

/// One file's preview: what it shows, and what its view keeps while the tab is not shown.
#[derive(Clone, Copy)]
struct Entry {
    blocks: RwSignal<Vec<Rc<Block>>>,
    /// The number of the last read asked for, and of the last one shown (an older read that comes later is not).
    asked: RwSignal<u64>,
    shown: RwSignal<u64>,
    /// Whether the file was asked for from the disk (its editor not open).
    read: RwSignal<bool>,
    /// Each block's view, by its place, and the view they are in: where a heading is.
    views: RwSignal<HashMap<usize, ViewId>>,
    content: RwSignal<Option<ViewId>>,
    /// Where the preview is scrolled to (kept while its tab is not shown), and where it is to go.
    top: RwSignal<f64>,
    goto: RwSignal<Option<Point>>,
}

/// The previews and their pictures, by their files' paths.
#[derive(Clone, Copy)]
struct State {
    entries: RwSignal<HashMap<String, Entry>>,
    pictures: RwSignal<HashMap<String, Pic>>,
}

impl crate::ext::State for State {
    fn new(ui: Ui) -> Self {
        State { entries: ui.scope.create_rw_signal(HashMap::new()), pictures: ui.scope.create_rw_signal(HashMap::new()) }
    }
}

fn entry(ui: Ui, file: &str) -> Entry {
    let st = ui.state::<State>();
    if let Some(e) = st.entries.with_untracked(|m| m.get(file).copied()) {
        return e;
    }
    let s = ui.scope;
    let e = Entry {
        blocks: s.create_rw_signal(Vec::new()),
        asked: s.create_rw_signal(0),
        shown: s.create_rw_signal(0),
        read: s.create_rw_signal(false),
        views: s.create_rw_signal(HashMap::new()),
        content: s.create_rw_signal(None),
        top: s.create_rw_signal(0.0),
        goto: s.create_rw_signal(None),
    };
    st.entries.update(|m| {
        m.insert(file.to_string(), e);
    });
    e
}

fn tab_of(file: &str) -> String {
    format!("{KIND}:{file}")
}

/// A Markdown text read into blocks off the window's thread, shown when it comes unless a later read was shown.
fn parse_off(ui: Ui, e: Entry, n: u64, text: impl FnOnce() -> String + std::marker::Send + 'static) {
    let done = floem::ext_event::create_ext_action(ui.scope, move |blocks: Vec<Block>| {
        if e.shown.try_get_untracked().is_none_or(|s| s > n) {
            return;
        }
        e.shown.set(n);
        let blocks: Vec<Rc<Block>> = blocks.into_iter().map(Rc::new).collect();
        if e.blocks.with_untracked(|b| *b != blocks) {
            e.blocks.set(blocks);
        }
    });
    std::thread::spawn(move || done(parse(&text())));
}

/// The next read's number.
fn next(e: Entry) -> u64 {
    e.asked.update(|n| *n += 1);
    e.asked.get_untracked()
}

/// The preview of a file follows its editor's text while its view is there: read at once, then 150 ms after each
/// change. With no editor of it open, the file is read from the disk once.
fn follow(ui: Ui, send: &Send, file: &str, e: Entry) {
    let (file, send) = (file.to_string(), send.clone());
    create_effect(move |before: Option<bool>| {
        let doc = ui.tabs.with(|tabs| tabs.iter().find(|t| t.path == file && t.is_file()).map(|t| t.doc.clone()));
        let Some(doc) = doc else {
            if !e.read.get_untracked() && e.shown.get_untracked() == 0 {
                e.read.set(true);
                send(json!({"type": "media", "path": file, "data": true}));
            }
            return false;
        };
        floem::views::editor::text::Document::cache_rev(&*doc).track();
        let n = next(e);
        let wait = if before == Some(true) { WAIT } else { Duration::ZERO };
        floem::action::exec_after(wait, move |_| {
            if e.asked.try_get_untracked() == Some(n) {
                let text = floem::views::editor::text::Document::text(&*doc);
                parse_off(ui, e, n, move || text.to_string());
            }
        });
        true
    });
}

/// A file's preview opened in the focused group, or shown where it is.
pub fn open(ui: Ui, file: &str) {
    entry(ui, file);
    kinds::open(ui, &tab_of(file));
}

/// A file's preview opened in the group beside the focused one (made when there is none).
pub fn open_to_side(ui: Ui, send: &Send, file: &str) {
    let id = tab_of(file);
    entry(ui, file);
    let from = groups::focused(ui);
    let made = ui.tab(&id).is_none();
    if made {
        // Made in this group behind what it shows, then moved to the next.
        let shown = ui.active.get_untracked();
        kinds::open(ui, &id);
        if shown.is_some() {
            ui.active.set(shown);
        }
    }
    groups::open_to_side(ui, &id, send);
    if made && groups::focused(ui) != from {
        groups::change(ui, |m, _| m.drop_tab(from, &id));
    }
}

/// The Markdown file shown in the focused group, as text.
fn markdown_shown(ui: Ui) -> Option<String> {
    let path = ui.active.get_untracked()?;
    (is_markdown(&path) && ui.tab(&path).is_some_and(|t| t.is_file())).then_some(path)
}

/// The file whose preview is shown in the focused group.
fn preview_shown(ui: Ui) -> Option<String> {
    let path = ui.active.get_untracked()?;
    kinds::split(&path).filter(|(k, _)| *k == KIND).map(|(_, rest)| rest.to_string())
}

/// What a click on a link of `file`'s preview does.
fn go(ui: Ui, send: &Send, file: &str, e: Entry, to: &Target) {
    match to {
        Target::Heading(name) => {
            let blocks = e.blocks.get_untracked();
            let Some(n) = slugs(&blocks).into_iter().find(|(_, s)| s == name).map(|(n, _)| n) else { return };
            let (Some(view), Some(content)) =
                (e.views.with_untracked(|v| v.get(&n).copied()), e.content.get_untracked())
            else {
                return;
            };
            let y = view.layout_rect().y0 - content.layout_rect().y0;
            e.goto.set(Some(Point::new(0.0, y.max(0.0))));
        }
        Target::File(r) if is_markdown(&r.path) && r.line.is_none() => {
            // In the group of the preview clicked.
            groups::focus(ui, groups::holder(ui, &tab_of(file)));
            entry(ui, &r.path);
            kinds::open(ui, &tab_of(&r.path));
        }
        Target::File(r) => match r.line {
            Some(line) => ui.open_at(&r.path, line, send),
            None => ui.open(&r.path, send),
        },
        Target::Web(_) => citations::open(ui, to),
    }
}

/// A picture of a preview: asked of the session the first time, drawn as large as it is and never wider than the
/// preview; its words in its place while it cannot be shown.
fn picture(ui: Ui, send: &Send, folder: &str, src: &str, alt: &str) -> AnyView {
    let t = ui.colors;
    let alt = alt.to_string();
    let words = move |alt: String| {
        h_stack((icon("file-media", 14.0), label(move || alt.clone())))
            .style(move |s| s.gap(4.0).items_center().font_size(13.0).color(t.get().desc))
            .into_any()
    };
    let Some(path) = picture_path(folder, src) else { return words(alt) };
    let st = ui.state::<State>();
    if !st.pictures.with_untracked(|p| p.contains_key(&path)) {
        st.pictures.update(|p| {
            p.insert(path.clone(), Pic::Loading);
        });
        send(json!({"type": "media", "path": path, "data": true}));
    }
    let now = create_memo(move |_| st.pictures.with(|p| p.get(&path).cloned()));
    dyn_container(
        move || now.get(),
        move |pic| match pic {
            Some(Pic::Shown(p)) => {
                let (w, h) = (f64::from(p.width.max(1)), f64::from(p.height.max(1)));
                media::drawn(p, true)
                    .style(move |s| s.width(w).max_width_full().aspect_ratio((w / h) as f32).flex_shrink(1.0_f32))
                    .into_any()
            }
            Some(Pic::Failed) => words(alt.clone()),
            _ => empty().into_any(),
        },
    )
    .style(|s| s.max_width_full())
    .into_any()
}

/// What a preview's tab draws: the file's text as the answers' renderer draws it, on the editor's background,
/// scrolled where it was left.
fn view(ui: Ui, send: Send, file: &str) -> AnyView {
    let t = ui.colors;
    let e = entry(ui, file);
    follow(ui, &send, file, e);
    let folder = folder_of(file).to_string();
    let (to, pics, here) = (send.clone(), send.clone(), file.to_string());
    let picture_folder = folder.clone();
    let doc = Doc {
        target: Box::new(move |href| target(&folder, href)),
        follow: Box::new(move |link| go(ui, &to, &here, e, link)),
        picture: Box::new(move |src, alt| picture(ui, &pics, &picture_folder, src, alt)),
        placed: Box::new(move |n, id| {
            e.views.update(|v| {
                v.insert(n, id);
            })
        }),
    };
    let blocks = create_memo(move |_| e.blocks.get());
    let content = prose::document(ui, blocks, doc).style(|s| s.width_full().padding_horiz(26.0).padding_vert(16.0));
    e.content.set(Some(content.id()));
    let back = e.top.get_untracked();
    if back > 0.0 {
        e.goto.set(Some(Point::new(0.0, back)));
    }
    scroll(content)
        .scroll_to(move || e.goto.get())
        .on_scroll(move |r| {
            if e.top.get_untracked() != r.y0 {
                e.top.set(r.y0);
            }
        })
        .style(move |s| s.size_full().min_size(0.0, 0.0).background(t.get().editor_bg).color(t.get().fg))
        .into_any()
}

/// The commands, the preview's kind of tab and the breadcrumbs' button.
pub fn register(_ui: Ui, _send: &Send) {
    kinds::add(
        kinds::kind(
            KIND,
            |_ui, rest| format!("Preview {}", rest.rsplit('/').next().unwrap_or(rest)),
            |ui, send, tab| view(ui, send, &tab.file),
        )
        .icon("open-preview")
        .kept(|ui, _send, path| {
            if let Some((_, rest)) = kinds::split(path) {
                open(ui, rest);
            }
        }),
    );
    commands::add(
        command(SHOW, "Markdown", "Open Preview", |ui, _| {
            if let Some(file) = markdown_shown(ui) {
                open(ui, &file);
            }
        })
        .keys("Ctrl+Shift+V")
        .when(|ui| markdown_shown(ui).is_some()),
    );
    commands::add(
        command(TO_SIDE, "Markdown", "Open Preview to the Side", |ui, send| {
            if let Some(file) = markdown_shown(ui) {
                open_to_side(ui, send, &file);
            }
        })
        .keys("Ctrl+K V")
        .when(|ui| markdown_shown(ui).is_some()),
    );
    commands::add(
        command(SOURCE, "Markdown", "Show Source", |ui, send| {
            if let Some(file) = preview_shown(ui) {
                ui.open(&file, send);
            }
        })
        .keys("Ctrl+Shift+V")
        .when(|ui| preview_shown(ui).is_some()),
    );
    breadcrumbs::end("open-preview", "Open Preview to the Side", |_ui, file| is_markdown(file), |ui, send, file| {
        open_to_side(ui, send, file)
    });
}

/// The pictures and the files on disk the previews asked for, as media. Never taken away from the parts after it.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] != "media" {
        return false;
    }
    let st = ui.state::<State>();
    let path = m["path"].as_str().unwrap_or("").to_string();
    let data = m["data"].as_str().unwrap_or("").to_string();
    // A preview's file read from the disk, while no editor of it is open.
    if let Some(e) = st.entries.with_untracked(|s| s.get(&path).copied()).filter(|e| e.read.get_untracked()) {
        e.read.set(false);
        if m["error"].is_null() && e.shown.get_untracked() == 0 {
            let n = next(e);
            parse_off(ui, e, n, move || {
                let bytes = base64::engine::general_purpose::STANDARD.decode(data).unwrap_or_default();
                String::from_utf8_lossy(&bytes).into_owned()
            });
        }
        return false;
    }
    if !st.pictures.with_untracked(|p| p.contains_key(&path)) {
        return false;
    }
    if !m["error"].is_null() {
        st.pictures.update(|p| {
            p.insert(path, Pic::Failed);
        });
        return false;
    }
    let size = m["size"].as_u64().unwrap_or(0);
    let checker = media::checker(ui);
    let hash = format!("preview\0{path}\0{}\0{}", m["modified"], ui.colors.get_untracked().dark).into_bytes();
    let file = path.clone();
    let done = floem::ext_event::create_ext_action(ui.scope, move |made: Result<media::Decoded, String>| {
        let pic = match made {
            Ok(d) => Pic::Shown(Rc::new(Picture::of(d, size, hash))),
            Err(_) => Pic::Failed,
        };
        st.pictures.update(|p| {
            p.insert(file, pic);
        });
    });
    std::thread::spawn(move || {
        let made = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| e.to_string())
            .and_then(|bytes| media::decode(&bytes, &path, checker));
        done(made);
    });
    false
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::markdown::inline;

    fn file(path: &str) -> Option<Target> {
        Some(Target::File(Ref { path: path.into(), ..Ref::default() }))
    }

    #[test]
    fn a_heading_is_named_in_a_link_as_github_names_it() {
        assert_eq!(slug("Getting Started"), "getting-started");
        assert_eq!(slug(" What's new in 2.0? "), "whats-new-in-20");
        assert_eq!(slug("snake_case and-dash"), "snake_case-and-dash");
        assert_eq!(slug("Über Größe"), "über-größe", "letters beyond ASCII stay");
        let blocks: Vec<Rc<Block>> = [
            Block::Heading(1, inline("Intro")),
            Block::Paragraph(inline("text")),
            Block::Heading(2, inline("Intro")),
            Block::Heading(2, inline("Intro")),
            Block::Heading(3, inline("Intro-1")),
        ]
        .into_iter()
        .map(Rc::new)
        .collect();
        assert_eq!(
            slugs(&blocks),
            [(0, "intro".into()), (2, "intro-1".into()), (3, "intro-2".into()), (4, "intro-1-1".into())],
            "a name taken gets the next number, and a heading named as a numbered one is numbered itself"
        );
    }

    #[test]
    fn a_link_is_read_from_the_document_s_folder() {
        assert_eq!(target("docs", "guide.md"), file("docs/guide.md"));
        let at_line = Target::File(Ref { path: "src/main.rs".into(), line: Some(12), ..Ref::default() });
        assert_eq!(target("docs/a", "../../src/main.rs#L12"), Some(at_line));
        assert_eq!(target("docs", "/README.md"), file("README.md"), "from the project's top");
        assert_eq!(target("docs", "./my%20notes.md#usage"), file("docs/my notes.md"), "another file's heading is its file");
        assert_eq!(target("docs", "#Getting-Started"), Some(Target::Heading("getting-started".into())));
        assert_eq!(target("", "https://x.dev/a"), Some(Target::Web("https://x.dev/a".into())));
        assert_eq!(target("", "mailto:a@b.c"), Some(Target::Web("mailto:a@b.c".into())));
        assert_eq!(target("", "javascript:alert(1)"), None);
        assert_eq!(target("", "../out.md"), None, "out of the project");
        assert_eq!(target("", "#"), None);
    }

    #[test]
    fn a_picture_is_a_file_of_the_project_or_nothing_fetched() {
        assert_eq!(picture_path("docs", "img/a.png").as_deref(), Some("docs/img/a.png"));
        assert_eq!(picture_path("docs", "../logo.svg?v=2").as_deref(), Some("logo.svg"));
        assert_eq!(picture_path("docs", "https://x.dev/badge.svg"), None);
        assert_eq!(picture_path("docs", "//cdn.x/a.png"), None);
        assert_eq!(picture_path("docs", "data:image/png;base64,AAAA"), None);
        assert!(is_markdown("README.md") && is_markdown("a/B.MARKDOWN"));
        assert!(!is_markdown("a.mdx") && !is_markdown("md"));
    }

    fn sender() -> (Send, Rc<RefCell<Vec<Value>>>) {
        let said = Rc::new(RefCell::new(Vec::new()));
        let to = said.clone();
        (Rc::new(move |v: Value| to.borrow_mut().push(v)), said)
    }

    #[test]
    fn the_preview_opens_beside_its_file_and_goes_back_to_it() {
        let ui = Ui::new();
        let (send, _) = sender();
        groups::register(ui, &send);
        register(ui, &send);
        ui.take(&json!({"type": "file", "path": "docs/README.md", "text": "# Hi\n", "modified": 1}), &send);
        ui.take(&json!({"type": "file", "path": "a.rs", "text": "x\n", "modified": 1}), &send);
        assert!(!commands::find(SHOW).expect("there").offered(ui), "only for a Markdown file");
        ui.open("docs/README.md", &send);
        assert!(commands::run(ui, &send, SHOW));
        let tab = ui.tab("preview:docs/README.md").expect("its tab");
        assert!(!tab.is_file() && ui.active.get_untracked().as_deref() == Some("preview:docs/README.md"));
        assert_eq!(kinds::title(ui, &tab), "Preview README.md");
        assert_eq!(kinds::icon(&tab), "open-preview");
        assert!(commands::run(ui, &send, SOURCE), "Show Source");
        assert_eq!(ui.active.get_untracked().as_deref(), Some("docs/README.md"));
        ui.close("preview:docs/README.md");
        // To the side: in a group of its own, the file staying where it was.
        assert!(commands::run(ui, &send, TO_SIDE));
        let m = groups::groups(ui).model.get_untracked();
        assert_eq!(m.groups.len(), 2);
        assert!(!m.groups[0].tabs.contains(&"preview:docs/README.md".to_string()));
        assert_eq!(m.groups[0].active.as_deref(), Some("docs/README.md"));
        assert_eq!(m.groups[1].tabs, ["preview:docs/README.md".to_string()]);
        assert_eq!(ui.active.get_untracked().as_deref(), Some("preview:docs/README.md"));
        assert!(kinds::kept(ui).contains(&"preview:docs/README.md".to_string()), "kept for the next start");
    }

    #[test]
    fn a_preview_s_pictures_and_its_file_on_disk_are_asked_as_media() {
        let ui = Ui::new();
        let (send, said) = sender();
        register(ui, &send);
        let e = entry(ui, "docs/a.md");
        e.read.set(true);
        let _ = picture(ui, &send, "docs", "img/p.png", "a picture");
        let _ = picture(ui, &send, "docs", "img/p.png", "again");
        let _ = picture(ui, &send, "docs", "https://x.dev/b.png", "web");
        let asked = json!({"type": "media", "path": "docs/img/p.png", "data": true});
        assert_eq!(*said.borrow(), [asked], "once, and none of the web");
        // A picture that cannot be read shows its words.
        take(ui, &json!({"type": "media", "path": "docs/img/p.png", "error": "not there"}), &send);
        let st = ui.state::<State>();
        assert!(st.pictures.with_untracked(|p| p.get("docs/img/p.png") == Some(&Pic::Failed)));
        // The file's text read from the disk is wanted no more once it came.
        take(ui, &json!({"type": "media", "path": "docs/a.md", "data": "IyBIaQo="}), &send);
        assert!(!e.read.get_untracked() && e.asked.get_untracked() == 1);
    }
}
