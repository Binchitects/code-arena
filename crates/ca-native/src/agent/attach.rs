//! Files attached to a message, as Arena's web chat attaches them: the paperclip in the box's toolbar (the system's
//! own dialog), a file dropped on the box from the system, or files copied in a file manager and pasted (Ctrl+V).
//! A drag from the Explorer stays a path in the box. Each file is sent to Arena as soon as it is added; over the
//! box's text it is a chip: a picture as itself, another file by its name with "Uploading N%" and a bar while it
//! goes, then its size (", cut to fit" when Arena kept only part of its text), or what went wrong ("Larger than
//! 20 MB."). Its X takes it away. "MODEL cannot see images: it gets their names only." when the model chosen does
//! not see pictures. The message waits for every upload; files alone are sent as "Look at these files.". At most
//! ten go with one message, and there is no paperclip without Arena.
//!
//! Under a question, the files that went with it: pictures as small pictures, the rest as chips.
//!
//! The session's messages (see `ide_attach.rs`): `attach{key, name, from}`, `attach_progress{key, pct}`,
//! `attached_file{key, id, name, size, kind, cut, thumb?}` or `{key, error}`, `attach_pick{key}` →
//! `attach_picked{key, paths, error?}`, and `state.attach` / `attach_limits{arena, most_bytes}`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use floem::IntoView;
use floem::keyboard::{Key, Modifiers};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo};
use floem::style::{CursorStyle, FlexWrap, Position, TextOverflow};
use floem::views::{Decorators, container, dyn_container, dyn_stack, empty, h_stack, label, tooltip, v_stack};
use serde_json::{Value, json};

use super::slots::{self, ItemSlot, Slot};
use crate::chat_bridge;
use crate::icons::icon;
use crate::media::{Picture, drawn, small_picture};
use crate::model::{Attachment, Kind};
use crate::state::{Send, Ui};

/// The most files one message carries.
pub const MOST_FILES: usize = 10;

/// The most one file may hold until the session says Arena's limit (Arena's own default).
const DEFAULT_MOST: u64 = 20 * 1024 * 1024;

/// A picture's chip, a side.
const PICTURE: f64 = 64.0;

/// What is sent when only files are.
pub const FILES_ALONE: &str = "Look at these files.";

/// A file on its way with the next message.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Upload {
    /// What the session's replies about it carry.
    pub key: String,
    pub name: String,
    pub size: u64,
    pub image: bool,
    pub pct: u64,
    /// Arena has it.
    pub done: Option<Attachment>,
    pub error: Option<String>,
}

impl Upload {
    fn busy(&self) -> bool {
        self.done.is_none() && self.error.is_none()
    }
}

/// A size as the chips say it: "512 B", "2.4 KB", "20 MB".
pub fn size_text(n: u64) -> String {
    let short = |v: f64, unit: &str| {
        let s = format!("{v:.1}");
        format!("{} {unit}", s.strip_suffix(".0").unwrap_or(&s))
    };
    match n {
        n if n >= 1024 * 1024 => short(n as f64 / (1024.0 * 1024.0), "MB"),
        n if n >= 1024 => short(n as f64 / 1024.0, "KB"),
        n => format!("{n} B"),
    }
}

/// What a chip says under its name.
pub fn line_of(u: &Upload) -> String {
    match (&u.error, &u.done) {
        (Some(e), _) => e.clone(),
        (None, Some(a)) => {
            let size = if a.size > 0 { a.size } else { u.size };
            format!("{}{}", size_text(size), if a.cut { ", cut to fit" } else { "" })
        }
        (None, None) => format!("Uploading {}%", u.pct),
    }
}

/// Whether a file's name says it is a picture this window draws.
pub fn is_picture(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
}

/// The files a paste holds when it is files copied in a file manager (`file://` URIs, one a line, with "copy" or
/// "cut" before them as GNOME's has it); none when it is anything else.
pub fn pasted_files(text: &str) -> Vec<PathBuf> {
    let mut lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if matches!(lines.first(), Some(&("copy" | "cut"))) {
        lines.remove(0);
    }
    let paths: Vec<PathBuf> =
        lines.iter().filter_map(|l| l.strip_prefix("file://")).map(|p| PathBuf::from(unescape(p))).collect();
    if paths.is_empty() || paths.len() != lines.len() { vec![] } else { paths }
}

/// A URI's path with its `%XX` made the bytes they stand for.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(n) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(n);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The part's state in a window.
#[derive(Clone, Copy)]
struct Attach {
    uploads: RwSignal<Vec<Upload>>,
    /// What the last message took, to put back if the session refuses it.
    taken: RwSignal<Vec<Upload>>,
    arena: RwSignal<bool>,
    most: RwSignal<u64>,
    /// A line under the chips: too many files, the dialog not found.
    note: RwSignal<String>,
    next: RwSignal<u64>,
    /// This window's part of a key: another window's replies are not its.
    stamp: RwSignal<u64>,
    /// Small pictures decoded, by the file's id in Arena, and those being decoded.
    pictures: RwSignal<HashMap<String, Rc<Picture>>>,
    decoding: RwSignal<HashSet<String>>,
}

impl crate::ext::State for Attach {
    fn new(ui: Ui) -> Self {
        let s = ui.scope;
        let stamp =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
        Attach {
            uploads: s.create_rw_signal(Vec::new()),
            taken: s.create_rw_signal(Vec::new()),
            arena: s.create_rw_signal(false),
            most: s.create_rw_signal(DEFAULT_MOST),
            note: s.create_rw_signal(String::new()),
            next: s.create_rw_signal(1),
            stamp: s.create_rw_signal(stamp),
            pictures: s.create_rw_signal(HashMap::new()),
            decoding: s.create_rw_signal(HashSet::new()),
        }
    }
}

fn key_of(me: Attach, what: &str) -> String {
    let n = me.next.get_untracked();
    me.next.set(n + 1);
    format!("{:x}-{what}{n}", me.stamp.get_untracked())
}

fn mine(me: Attach, key: &str) -> bool {
    key.starts_with(&format!("{:x}-", me.stamp.get_untracked()))
}

fn say_note(me: Attach, text: &str) {
    if me.note.with_untracked(|n| n != text) {
        me.note.set(text.to_string());
    }
}

/// A file of this machine attached: sent up unless it is one too many or too large.
pub fn add_path(ui: Ui, send: &Send, path: &Path) {
    let me = ui.state::<Attach>();
    if !me.arena.get_untracked() {
        return;
    }
    if me.uploads.with_untracked(Vec::len) >= MOST_FILES {
        say_note(me, &format!("At most {MOST_FILES} files go with one message."));
        return;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let key = key_of(me, "f");
    let mut up = Upload { key: key.clone(), name: name.clone(), image: is_picture(&name), ..Default::default() };
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => up.error = Some("A folder cannot be attached.".into()),
        Ok(m) if m.len() > me.most.get_untracked() => {
            up.size = m.len();
            up.error = Some(format!("Larger than {}.", size_text(me.most.get_untracked())));
        }
        Ok(m) => up.size = m.len(),
        Err(e) => up.error = Some(format!("It could not be read: {e}")),
    }
    if up.error.is_none() {
        send(json!({"type": "attach", "key": key, "name": name, "from": path.to_string_lossy()}));
    }
    me.uploads.update(|u| u.push(up));
}

fn remove(ui: Ui, key: &str) {
    let me = ui.state::<Attach>();
    me.uploads.update(|u| u.retain(|x| x.key != key));
    if me.uploads.with_untracked(Vec::is_empty) {
        say_note(me, "");
    }
}

/// A file's small picture asked for once: decoded off the window's thread.
fn want_picture(ui: Ui, a: &Attachment) {
    let me = ui.state::<Attach>();
    if a.thumb.is_empty() || a.id.is_empty() {
        return;
    }
    if me.pictures.with_untracked(|p| p.contains_key(&a.id)) || me.decoding.with_untracked(|d| d.contains(&a.id)) {
        return;
    }
    me.decoding.update(|d| {
        d.insert(a.id.clone());
    });
    let id = a.id.clone();
    small_picture(ui, &a.thumb, &a.id, move |p| {
        if let Some(p) = p {
            me.pictures.update(|all| {
                all.insert(id, p);
            });
        }
    });
}

/// A small picture of a file once it is decoded, else what `other` draws.
fn picture_or(ui: Ui, id: String, other: impl Fn() -> floem::AnyView + 'static) -> impl IntoView {
    let me = ui.state::<Attach>();
    let at = id.clone();
    let has = create_memo(move |_| me.pictures.with(|p| p.get(&at).map(|p| Rc::as_ptr(p) as usize)));
    dyn_container(
        move || has.get(),
        move |_| match me.pictures.with_untracked(|p| p.get(&id).cloned()) {
            Some(p) => drawn(p, false).style(|s| s.width(PICTURE).height(PICTURE)).into_any(),
            None => other(),
        },
    )
}

/// A file's chip by its name: its icon, its name, and a line under it.
fn named(ui: Ui, name: String, line: String, error: bool) -> floem::AnyView {
    let t = ui.colors;
    h_stack((
        icon("file", 16.0).style(move |s| s.color(t.get().desc).flex_shrink(0.0_f32)),
        v_stack((
            label(move || name.clone()).style(|s| s.max_width(176.0).text_ellipsis()),
            tooltip(
                label({
                    let line = line.clone();
                    move || line.clone()
                })
                .style(move |s| {
                    let c = t.get();
                    s.font_size(11.0).max_width(176.0).text_ellipsis().color(if error { c.error } else { c.desc })
                }),
                // The line is cut to the chip; the whole of it (an upload's error, mostly) on hover.
                move || {
                    let line = line.clone();
                    label(move || line.clone()).style(|s| s.max_width(360.0).text_overflow(TextOverflow::Wrap))
                },
            ),
        )),
    ))
    .style(|s| s.gap(8.0).items_center().padding_vert(4.0).padding_left(8.0).padding_right(28.0))
    .into_any()
}

/// A chip over the box: the file, how far it is, its X.
fn chip(ui: Ui, u: Upload) -> impl IntoView {
    let t = ui.colors;
    let key = u.key.clone();
    let busy = u.busy();
    let error = u.error.is_some();
    let pct = u.pct as f64;
    let thumb = u.done.as_ref().filter(|a| u.image && !a.thumb.is_empty()).cloned();
    let body = match thumb {
        Some(a) => {
            want_picture(ui, &a);
            let (name, line) = (u.name.clone(), line_of(&u));
            picture_or(ui, a.id.clone(), move || named(ui, name.clone(), line.clone(), false)).into_any()
        }
        None => named(ui, u.name.clone(), line_of(&u), error),
    };
    let bar = container(empty().style(move |s| s.height_full().width_pct(pct).background(t.get().progress))).style(
        move |s| {
            s.position(Position::Absolute)
                .inset_left(0.0)
                .inset_right(0.0)
                .inset_bottom(0.0)
                .height(2.0)
                .background(t.get().widget_border)
                .apply_if(!busy, |s| s.hide())
        },
    );
    let name = u.name.clone();
    let x = tooltip(
        container(icon("close", 12.0)).on_click_stop(move |_| remove(ui, &key)).style(move |s| {
            let c = t.get();
            s.width(18.0)
                .height(18.0)
                .items_center()
                .justify_center()
                .border_radius(9.0)
                .color(c.desc)
                .background(c.input_bg.multiply_alpha(0.9))
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.color(c.fg).background(c.toolbar_hover))
        }),
        move || {
            let name = name.clone();
            label(move || format!("Remove {name}"))
        },
    )
    // Placed on the hover box itself: a box of no size laid out in the row would get no clicks for what it holds.
    .style(|s| s.position(Position::Absolute).inset_top(3.0).inset_right(3.0));
    container((body, bar, x)).style(move |s| {
        let c = t.get();
        s.position(Position::Relative)
            .border(1.0)
            .border_radius(6.0)
            .border_color(if error { c.error.multiply_alpha(0.5) } else { c.widget_border })
            .background(c.fg.multiply_alpha(0.04))
            .font_size(12.0)
            .color(c.fg)
    })
}

/// The chips over the box, the line for a model that does not see, and the note.
fn chips(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let me = ui.state::<Attach>();
    let list = dyn_stack(
        move || me.uploads.get(),
        |u: &Upload| (u.key.clone(), u.pct, u.done.is_some(), u.error.clone()),
        move |u| chip(ui, u),
    )
    .style(|s| s.flex_row().flex_wrap(FlexWrap::Wrap).gap(8.0));
    let blind = create_memo(move |_| {
        let model = ui.model.get();
        let pictures = me.uploads.with(|u| u.iter().any(|u| u.image && u.error.is_none()));
        (pictures && !super::pickers::sees(ui, &model)).then_some(model)
    });
    let blind_line = h_stack((
        icon("eye-closed", 14.0),
        label(move || format!("{} cannot see images: it gets their names only.", blind.get().unwrap_or_default())),
    ))
    .style(move |s| {
        s.gap(6.0)
            .items_center()
            .font_size(11.0)
            .color(t.get().desc)
            .apply_if(blind.with(Option::is_none), |s| s.hide())
    });
    let note = label(move || me.note.get()).style(move |s| {
        s.font_size(12.0)
            .line_height(1.4)
            .min_width(0.0)
            .text_overflow(TextOverflow::Wrap)
            .color(t.get().error)
            .apply_if(me.note.with(String::is_empty), |s| s.hide())
    });
    let empty_all = create_memo(move |_| me.uploads.with(Vec::is_empty) && me.note.with(String::is_empty));
    v_stack((list, blind_line, note))
        .style(move |s| s.gap(6.0).margin_bottom(6.0).width_full().apply_if(empty_all.get(), |s| s.hide()))
}

/// The paperclip in the box's toolbar: the system's dialog for choosing files.
fn clip(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let me = ui.state::<Attach>();
    let button = container(icon("attach", 16.0))
        .on_click_stop(move |_| {
            let key = key_of(me, "pick");
            send(json!({"type": "attach_pick", "key": key}));
        })
        .style(move |s| {
            let c = t.get();
            s.width(22.0)
                .height(22.0)
                .items_center()
                .justify_center()
                .border_radius(4.0)
                .color(c.icon)
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.background(c.toolbar_hover))
        });
    tooltip(button, || {
        label(|| "Attach files: Word, Excel, PowerPoint, PDF, text, code, images (Arena reads them; a model that sees gets the pictures)")
    })
    .style(move |s| s.apply_if(!me.arena.get(), |s| s.hide()))
}

/// The files that went with a question, under its bubble.
fn sent_files(ui: Ui, files: Vec<Attachment>) -> impl IntoView {
    let t = ui.colors;
    let views: Vec<floem::AnyView> = files
        .into_iter()
        .map(|a| {
            let line = format!("{}{}", size_text(a.size), if a.cut { ", cut to fit" } else { "" });
            let name = a.name.clone();
            let inner = if a.kind == "image" && !a.thumb.is_empty() {
                want_picture(ui, &a);
                let (n, l) = (name.clone(), line.clone());
                picture_or(ui, a.id.clone(), move || named(ui, n.clone(), l.clone(), false)).into_any()
            } else {
                named(ui, name.clone(), line, false)
            };
            tooltip(
                container(inner).style(move |s| {
                    let c = t.get();
                    s.border(1.0).border_radius(6.0).border_color(c.widget_border).font_size(12.0).color(c.fg)
                }),
                move || {
                    let name = name.clone();
                    label(move || name.clone())
                },
            )
            .into_any()
        })
        .collect();
    floem::views::h_stack_from_iter(views)
        .style(|s| s.flex_wrap(FlexWrap::Wrap).gap(6.0).justify_end().margin_top(4.0).max_width_pct(85.0))
}

/// A message on its way: the files Arena has go with it, and it is given words when it has none.
fn fill(ui: Ui, msg: &mut Value) {
    let me = ui.state::<Attach>();
    let all = me.uploads.get_untracked();
    let done: Vec<Upload> = all.iter().filter(|u| u.done.is_some()).cloned().collect();
    if done.is_empty() {
        return;
    }
    msg["attachments"] = done.iter().filter_map(|u| u.done.as_ref()).map(|a| json!(a.id)).collect();
    if msg["text"].as_str().is_none_or(|t| t.trim().is_empty()) {
        msg["text"] = json!(FILES_ALONE);
    }
    me.taken.set(done);
    me.uploads.set(Vec::new());
    say_note(me, "");
}

/// A message the session refused: its files go back over the box.
fn put_back(ui: Ui, msg: &Value) {
    let me = ui.state::<Attach>();
    let ids: Vec<&str> = msg["attachments"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if ids.is_empty() {
        return;
    }
    let back: Vec<Upload> = me
        .taken
        .get_untracked()
        .into_iter()
        .filter(|u| u.done.as_ref().is_some_and(|a| ids.contains(&a.id.as_str())))
        .collect();
    me.uploads.update(|u| {
        let rest = std::mem::take(u);
        *u = back.into_iter().chain(rest).collect();
    });
}

/// Ctrl+V with files copied in a file manager attaches them; anything else is the box's paste.
fn paste_key(ui: Ui, send: &Send, key: &Key, mods: Modifiers) -> bool {
    let ctrl =
        mods.intersects(Modifiers::CONTROL | Modifiers::META) && !mods.intersects(Modifiers::ALT | Modifiers::SHIFT);
    if !ctrl || !matches!(key, Key::Character(c) if c.eq_ignore_ascii_case("v")) {
        return false;
    }
    if !ui.state::<Attach>().arena.get_untracked() {
        return false;
    }
    let Ok(text) = floem::Clipboard::get_contents() else { return false };
    let files = pasted_files(&text);
    if files.is_empty() || !files.iter().all(|f| f.is_file()) {
        return false;
    }
    for f in files {
        add_path(ui, send, &f);
    }
    true
}

/// What it adds to the window's registries, once at start: the chips over the box, the paperclip, what goes with a
/// message, a drop on the box, the paste of files, and the files under a question.
pub fn register(_ui: Ui, _send: &Send) {
    slots::add(Slot::BoxTop, "attachments", 30, |ui, _| chips(ui).into_any());
    slots::add(Slot::BoxRight, "attach", 20, |ui, send| clip(ui, send).into_any());
    slots::add_item(ItemSlot::QuestionFoot, "attachments", 20, |ui, _, item| match &item.kind {
        Kind::User(q) if !q.attachments.is_empty() => sent_files(ui, q.attachments.clone()).into_any(),
        _ => empty().style(|s| s.hide()).into_any(),
    });
    chat_bridge::sent_with(
        "attachments",
        30,
        chat_bridge::with(fill)
            .more(|ui| ui.state::<Attach>().uploads.with(|u| u.iter().any(|u| u.done.is_some())))
            .ready(|ui| ui.state::<Attach>().uploads.with(|u| !u.iter().any(Upload::busy)))
            .back(put_back),
    );
    chat_bridge::on_drop(add_path);
    chat_bridge::on_key("attachments.paste", 5, paste_key);
}

/// The limits as the session says them: whether there is Arena, and the most one file may hold.
fn limits(me: Attach, v: &Value) {
    let arena = v["arena"] == true;
    if me.arena.get_untracked() != arena {
        me.arena.set(arena);
    }
    if let Some(most) = v["most_bytes"].as_u64().filter(|n| *n > 0)
        && me.most.get_untracked() != most
    {
        me.most.set(most);
    }
}

/// The session's messages that are this part's, taken in: the limits, how far each upload is, what Arena made of
/// it, and the files chosen in the dialog.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let me = ui.state::<Attach>();
    let key = m["key"].as_str().unwrap_or("");
    match m["type"].as_str().unwrap_or("") {
        "state" => {
            if m.get("attach").is_some() {
                limits(me, &m["attach"]);
            }
            false
        }
        "attach_limits" => {
            limits(me, m);
            true
        }
        "attach_progress" => {
            let pct = m["pct"].as_u64().unwrap_or(0).min(99);
            if me.uploads.with_untracked(|u| u.iter().any(|x| x.key == key && x.busy() && x.pct != pct)) {
                me.uploads.update(|u| {
                    if let Some(x) = u.iter_mut().find(|x| x.key == key) {
                        x.pct = pct;
                    }
                });
            }
            true
        }
        "attached_file" => {
            if me.uploads.with_untracked(|u| u.iter().any(|x| x.key == key && x.busy())) {
                let error = m["error"].as_str().map(str::to_string);
                let done = error.is_none().then(|| Attachment::from_json(m));
                me.uploads.update(|u| {
                    if let Some(x) = u.iter_mut().find(|x| x.key == key) {
                        x.pct = 100;
                        if let Some(a) = &done {
                            x.image = x.image || a.kind == "image";
                        }
                        x.error = error;
                        x.done = done;
                    }
                });
            }
            true
        }
        "attach_picked" => {
            if mine(me, key) {
                if let Some(e) = m["error"].as_str() {
                    say_note(me, e);
                }
                for p in m["paths"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    add_path(ui, send, Path::new(p));
                }
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn upload(key: &str) -> Upload {
        Upload { key: key.into(), name: "notes.md".into(), size: 2048, ..Default::default() }
    }

    #[test]
    fn a_chip_says_how_far_it_is_then_its_size_or_what_went_wrong() {
        let mut u = upload("k");
        u.pct = 42;
        assert_eq!(line_of(&u), "Uploading 42%");
        u.done = Some(Attachment { id: "a".into(), size: 2048, cut: true, ..Default::default() });
        assert_eq!(line_of(&u), "2 KB, cut to fit");
        u.error = Some("Larger than 20 MB.".into());
        assert_eq!(line_of(&u), "Larger than 20 MB.");
        assert_eq!(
            (size_text(512), size_text(2500), size_text(20 * 1024 * 1024)),
            ("512 B".into(), "2.4 KB".into(), "20 MB".into())
        );
        assert!(is_picture("a.PNG") && !is_picture("a.svg") && !is_picture("notes"));
    }

    #[test]
    fn files_copied_in_a_file_manager_are_pasted_as_files_and_other_text_is_not() {
        assert_eq!(
            pasted_files("copy\nfile:///home/a/My%20Notes.md\nfile:///tmp/b.png"),
            [PathBuf::from("/home/a/My Notes.md"), PathBuf::from("/tmp/b.png")]
        );
        assert_eq!(pasted_files("file:///tmp/%C3%A9.txt\n"), [PathBuf::from("/tmp/é.txt")]);
        assert!(pasted_files("just words").is_empty());
        assert!(pasted_files("file:///a\nand words").is_empty());
        assert!(pasted_files("").is_empty());
    }

    #[test]
    fn a_file_goes_up_and_with_the_message_which_waits_for_every_upload() {
        let ui = Ui::new();
        let said = Rc::new(RefCell::new(Vec::<Value>::new()));
        let to = said.clone();
        let send: Send = Rc::new(move |v| to.borrow_mut().push(v));
        register(ui, &send);
        let dir = std::env::temp_dir().join(format!("attach-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("as the test makes it");
        let file = dir.join("notes.md");
        std::fs::write(&file, "# Notes").expect("as the test makes it");
        // No Arena: nothing is attached.
        add_path(ui, &send, &file);
        assert!(said.borrow().is_empty());
        assert!(take(ui, &json!({"type": "attach_limits", "arena": true, "most_bytes": 4}), &send));
        add_path(ui, &send, &file);
        let me = ui.state::<Attach>();
        assert_eq!(me.uploads.get_untracked()[0].error.as_deref(), Some("Larger than 4 B."));
        assert!(said.borrow().is_empty(), "too large: not sent");
        remove(ui, &me.uploads.get_untracked()[0].key);
        take(ui, &json!({"type": "state", "attach": {"arena": true, "most_bytes": 1024}}), &send);
        add_path(ui, &send, &file);
        let key = said.borrow()[0]["key"].as_str().expect("as the test makes it").to_string();
        assert_eq!(said.borrow()[0]["from"], file.to_string_lossy().as_ref());
        assert!(!chat_bridge::ready(ui) && !chat_bridge::more(ui), "an upload on its way holds the message");
        take(ui, &json!({"type": "attach_progress", "key": key, "pct": 40}), &send);
        assert_eq!(line_of(&me.uploads.get_untracked()[0]), "Uploading 40%");
        // Another window's reply is not this one's.
        take(ui, &json!({"type": "attached_file", "key": "other", "id": "x", "name": "x"}), &send);
        assert!(me.uploads.get_untracked()[0].busy());
        take(
            ui,
            &json!({"type": "attached_file", "key": key, "id": "f1", "name": "notes.md", "size": 7, "kind": "text", "cut": false}),
            &send,
        );
        assert!(chat_bridge::ready(ui) && chat_bridge::more(ui));
        let mut msg = json!({"type": "send", "text": ""});
        chat_bridge::fill(ui, &mut msg);
        assert_eq!((msg["text"].as_str(), msg["attachments"].clone()), (Some(FILES_ALONE), json!(["f1"])));
        assert!(me.uploads.get_untracked().is_empty());
        chat_bridge::came_back(ui, &msg);
        assert_eq!(me.uploads.get_untracked().len(), 1, "a refused message gives its files back");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn at_most_ten_files_wait_for_one_message() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        take(ui, &json!({"type": "attach_limits", "arena": true, "most_bytes": 1024}), &send);
        for _ in 0..12 {
            add_path(ui, &send, Path::new("/nowhere/at/all.txt"));
        }
        let me = ui.state::<Attach>();
        assert_eq!(me.uploads.get_untracked().len(), MOST_FILES);
        assert_eq!(me.note.get_untracked(), "At most 10 files go with one message.");
    }
}
