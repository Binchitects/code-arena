//! The Extensions view (Ctrl+Shift+X), as VS Code's: a search box over the list, then Installed (each extension's
//! icon, name, version, description and publisher, what it brings, what of it is not used here, a switch that turns
//! it on and off and Uninstall, asked first), Recommended (the project's `.vscode/extensions.json`) and VS Code on
//! This Computer (the extensions VS Code, VSCodium or VS Code - Insiders has here, each with Import, or Update when
//! a newer one is there). Each section folds. The title has Refresh, Clear Extensions Search Results and a "…" menu
//! with Install from VSIX…, which asks for the path of a `.vsix`. The search finds by name, id, publisher or
//! description, and `@installed`, `@enabled`, `@disabled`, `@recommended` and `@local` narrow it to one kind.
//!
//! The extensions' side is the driver's `ide_extensions.rs`; what installed extensions bring is put in force by
//! `extensions_apply.rs`. What every part has (`register`, `take`) is said in `ext.rs`; what a view has (`READY`,
//! `view`, `actions`), in `layout.rs`.

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use floem::IntoView;
use floem::event::EventListener;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_memo, create_rw_signal};
use floem::style::CursorStyle;
use floem::views::{
    Decorators, container, dyn_container, dyn_stack, empty, h_stack, label, scroll, text_input, tooltip, v_stack,
};
use floem::{AnyView, View as _, ViewId};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::layout::View;
use crate::media::Picture;
use crate::state::{Send, Ui};
use crate::widgets::{Theme, action, bold, caps, dialog, quick, toast};

/// Whether the view is in the window.
pub const READY: bool = true;

/// The commands of the view's title and its "…" menu, under VS Code's ids.
pub const INSTALL_VSIX: &str = "workbench.extensions.action.installVSIX";
pub const REFRESH: &str = "workbench.extensions.action.refreshExtension";
pub const CLEAR: &str = "workbench.extensions.action.clearExtensionsSearchResults";
pub const SHOW_INSTALLED: &str = "workbench.extensions.action.showInstalledExtensions";
pub const SHOW_RECOMMENDED: &str = "workbench.extensions.action.showRecommendedExtensions";
pub const MORE: &str = "codeArena.extensions.more";

/// The search box's words, as VS Code's.
const SEARCH_HINT: &str = "Search Extensions";

/// A path the driver reads: inside the project, from / or from ~/.
const PATH_RULE: &str = "inside the project, or from / or ~/";

/// An extension kept here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Ext {
    pub id: String,
    /// Its display name (its name when it has none).
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub description: String,
    /// Its icon's path inside it, fetched with `ext_file`.
    pub icon: Option<String>,
    pub enabled: bool,
    /// Where it came from: "vsix" or "vscode".
    pub source: String,
    /// What it brings, in a line ("2 color themes, snippets for rust").
    pub brings: String,
    /// What of it is not used here, and why.
    pub unused: Vec<String>,
}

/// An extension another editor has on this computer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Local {
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub description: String,
    /// The editor that has it ("VS Code", "VSCodium", "VS Code - Insiders").
    pub from: String,
    /// Whether anything of it can be used here; when not, `problem` says why.
    pub usable: bool,
    pub problem: Option<String>,
    /// The version kept here, when it was imported or installed.
    pub installed: Option<String>,
    pub brings: String,
}

/// What a copy on this computer can be asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Offer {
    Import,
    Update,
    /// The same or a newer one is kept here.
    Imported,
    /// Nothing of it would be used here.
    Unusable,
}

impl Local {
    pub fn offer(&self) -> Offer {
        match &self.installed {
            _ if !self.usable => Offer::Unusable,
            None => Offer::Import,
            Some(kept) if newer(&self.version, kept) => Offer::Update,
            Some(_) => Offer::Imported,
        }
    }
}

/// One of the project's recommendations.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rec {
    pub id: String,
    pub name: String,
    /// "installed", "built-in", "unsupported", "local" (it can be imported) or "unknown" (install its .vsix).
    pub status: String,
    pub note: String,
}

/// The project's recommendations as the driver said them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recs {
    /// Whether the project has a `.vscode/extensions.json`.
    pub file: bool,
    pub items: Vec<Rec>,
    pub error: Option<String>,
}

/// A section of the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Section {
    Installed,
    Recommended,
    Local,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Installed => "Installed",
            Section::Recommended => "Recommended",
            Section::Local => "VS Code on This Computer",
        }
    }
}

/// A row of the view, top to bottom. Each holds what it draws, so a row is drawn again only when that changed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Row {
    Head {
        section: Section,
        n: usize,
        open: bool,
    },
    Ext {
        e: Ext,
        busy: bool,
    },
    Rec {
        r: Rec,
        busy: bool,
    },
    Local {
        l: Local,
        busy: bool,
    },
    /// Words under a section: why it is empty, or what to do.
    Note {
        section: Section,
        text: String,
    },
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").trim().to_string()
}

fn texts(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

/// What an extension brings, in a line: its colour and file icon themes, snippets, languages, grammars, keys and
/// setting defaults (`keybindings` is a count, `configurationDefaults` an object, the rest lists).
pub fn brings(c: &Value) -> String {
    let n = |k: &str| c[k].as_array().map_or(0, Vec::len);
    let mut out = Vec::new();
    if n("themes") > 0 {
        out.push(plural(n("themes"), "color theme", "color themes"));
    }
    if n("iconThemes") > 0 {
        out.push(plural(n("iconThemes"), "file icon theme", "file icon themes"));
    }
    let mut langs: Vec<String> = c["snippets"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| text(&s["language"]))
        .filter(|l| !l.is_empty())
        .collect();
    langs.dedup();
    if !langs.is_empty() {
        out.push(format!("snippets for {}", langs.join(", ")));
    } else if n("snippets") > 0 {
        out.push("snippets".into());
    }
    if n("languages") > 0 {
        out.push(plural(n("languages"), "language", "languages"));
    }
    if n("grammars") > 0 {
        out.push(plural(n("grammars"), "grammar", "grammars"));
    }
    let keys = c["keybindings"].as_u64().map_or_else(|| n("keybindings"), |k| k as usize);
    if keys > 0 {
        out.push(plural(keys, "keybinding", "keybindings"));
    }
    if c["configurationDefaults"].as_object().is_some_and(|o| !o.is_empty()) {
        out.push("setting defaults".into());
    }
    out.join(", ")
}

/// A name to show: the display name, else the name, else the id.
fn shown_name(v: &Value) -> String {
    [&v["displayName"], &v["name"], &v["id"]].into_iter().map(text).find(|s| !s.is_empty()).unwrap_or_default()
}

fn ext_of(v: &Value) -> Ext {
    Ext {
        id: text(&v["id"]),
        name: shown_name(v),
        version: text(&v["version"]),
        publisher: text(&v["publisher"]),
        description: text(&v["description"]),
        icon: v["icon"].as_str().filter(|p| !p.is_empty()).map(String::from),
        enabled: v["enabled"].as_bool() != Some(false),
        source: text(&v["source"]),
        brings: brings(&v["contributes"]),
        unused: texts(&v["unused"]),
    }
}

/// The installed extensions, from `extensions{items}`.
pub fn items_of(m: &Value) -> Vec<Ext> {
    m["items"].as_array().into_iter().flatten().map(ext_of).filter(|e| !e.id.is_empty()).collect()
}

/// VS Code's extensions on this computer, from `ext_local{items}`.
pub fn local_of(m: &Value) -> Vec<Local> {
    let one = |v: &Value| Local {
        id: text(&v["id"]),
        name: shown_name(v),
        version: text(&v["version"]),
        publisher: text(&v["publisher"]),
        description: text(&v["description"]),
        from: text(&v["from"]),
        usable: v["usable"].as_bool() != Some(false),
        problem: v["problem"].as_str().map(String::from),
        installed: v["installed"].as_str().map(String::from),
        brings: brings(&v["contributes"]),
    };
    m["items"].as_array().into_iter().flatten().map(one).filter(|l| !l.id.is_empty()).collect()
}

/// The project's recommendations, from `ext_recommended`.
pub fn recs_of(m: &Value) -> Recs {
    let one = |v: &Value| Rec {
        id: text(&v["id"]),
        name: v["name"]
            .as_str()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(v["id"].as_str().unwrap_or(""))
            .into(),
        status: text(&v["status"]),
        note: text(&v["note"]),
    };
    Recs {
        file: m["file"].as_bool() == Some(true),
        items: m["recommendations"].as_array().into_iter().flatten().map(one).filter(|r| !r.id.is_empty()).collect(),
        error: m["error"].as_str().map(String::from),
    }
}

/// Whether version `a` is newer than `b`, as VS Code compares them: their numbers first (1.10 after 1.9), then a
/// release after its pre-release.
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| {
        let (main, pre) = v.split_once('-').map_or((v, None), |(m, p)| (m, Some(p.to_string())));
        let nums: Vec<u64> = main.split('.').map(|n| n.trim().parse().unwrap_or(0)).collect();
        (nums, pre)
    };
    let ((na, pa), (nb, pb)) = (parts(a.trim()), parts(b.trim()));
    let len = na.len().max(nb.len());
    for i in 0..len {
        let (x, y) = (na.get(i).copied().unwrap_or(0), nb.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    match (pa, pb) {
        (None, Some(_)) => true,
        (Some(x), Some(y)) => x > y,
        _ => false,
    }
}

/// What the search asks for: the kind it is narrowed to (`@installed` and the like), and the words.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Find {
    pub only: Option<&'static str>,
    pub words: String,
}

pub fn find_of(typed: &str) -> Find {
    let mut f = Find::default();
    let mut words = Vec::new();
    for w in typed.split_whitespace() {
        match w.to_lowercase().as_str() {
            "@installed" => f.only = Some("installed"),
            "@enabled" => f.only = Some("enabled"),
            "@disabled" => f.only = Some("disabled"),
            "@recommended" => f.only = Some("recommended"),
            "@local" => f.only = Some("local"),
            _ => words.push(w.to_lowercase()),
        }
    }
    f.words = words.join(" ");
    f
}

/// Whether every word is in one of the texts.
fn has(words: &str, of: &[&str]) -> bool {
    words.split_whitespace().all(|w| of.iter().any(|t| t.to_lowercase().contains(w)))
}

/// What the view shows, from what the driver said and what is asked: each section's head (none for a section that
/// has nothing to say while searching), its rows when it is open, and why one is empty.
pub fn rows(
    items: Option<&[Ext]>,
    recs: Option<&Recs>,
    local: Option<&[Local]>,
    typed: &str,
    folded: &BTreeSet<Section>,
    busy: &BTreeSet<String>,
) -> Vec<Row> {
    let find = find_of(typed);
    let searching = find.only.is_some() || !find.words.is_empty();
    let wanted = |s: Section| match find.only {
        None => true,
        Some("installed" | "enabled" | "disabled") => s == Section::Installed,
        Some("recommended") => s == Section::Recommended,
        Some(_) => s == Section::Local,
    };
    let mut out = Vec::new();
    let put = |out: &mut Vec<Row>, section: Section, found: Vec<Row>, note: Option<String>| {
        if searching && found.is_empty() {
            return;
        }
        let open = !folded.contains(&section);
        out.push(Row::Head { section, n: found.len(), open });
        if !open {
            return;
        }
        match note {
            Some(text) if found.is_empty() => out.push(Row::Note { section, text }),
            _ => out.extend(found),
        }
    };
    if wanted(Section::Installed) {
        let found: Vec<Row> = items
            .unwrap_or_default()
            .iter()
            .filter(|e| match find.only {
                Some("enabled") => e.enabled,
                Some("disabled") => !e.enabled,
                _ => true,
            })
            .filter(|e| has(&find.words, &[&e.name, &e.id, &e.publisher, &e.description]))
            .map(|e| Row::Ext { e: e.clone(), busy: busy.contains(&e.id) })
            .collect();
        let note = match items {
            None => "Reading the extensions\u{2026}",
            Some(_) => {
                "No extensions installed yet. Install one from its .vsix (Install from VSIX\u{2026}, in the \u{2026} \
                 menu), or import one VS Code has on this computer."
            }
        };
        put(&mut out, Section::Installed, found, Some(note.into()));
    }
    if wanted(Section::Recommended) {
        let r = recs.cloned().unwrap_or_default();
        let found: Vec<Row> = r
            .items
            .iter()
            .filter(|x| x.status != "installed")
            .filter(|x| has(&find.words, &[&x.name, &x.id, &x.note]))
            .map(|x| Row::Rec { r: x.clone(), busy: busy.contains(&x.id) })
            .collect();
        let note = match (&r.error, r.file) {
            (Some(e), _) => Some(e.clone()),
            (None, true) if !r.items.is_empty() => Some("Every extension the project recommends is installed.".into()),
            (None, true) => Some("The project's .vscode/extensions.json recommends no extension.".into()),
            // No file: the section is left out, as VS Code leaves it out when there is nothing to recommend.
            (None, false) => None,
        };
        if note.is_some() || !found.is_empty() {
            put(&mut out, Section::Recommended, found, note);
        }
    }
    if wanted(Section::Local) {
        let found: Vec<Row> = local
            .unwrap_or_default()
            .iter()
            .filter(|l| has(&find.words, &[&l.name, &l.id, &l.publisher, &l.description]))
            .map(|l| Row::Local { l: l.clone(), busy: busy.contains(&l.id) })
            .collect();
        let note = match local {
            None => "Looking for VS Code's extensions\u{2026}",
            Some(_) => "VS Code, VSCodium and VS Code - Insiders have no extension on this computer.",
        };
        put(&mut out, Section::Local, found, Some(note.into()));
    }
    out
}

/// The view's own state.
#[derive(Clone, Copy)]
struct Exts {
    items: RwSignal<Option<Vec<Ext>>>,
    recs: RwSignal<Option<Recs>>,
    local: RwSignal<Option<Vec<Local>>>,
    find: RwSignal<String>,
    folded: RwSignal<BTreeSet<Section>>,
    /// The extensions a change is asked for, until its reply.
    busy: RwSignal<BTreeSet<String>>,
    /// Icons drawn, by "id@version"; asked: the ones fetched or being fetched.
    icons: RwSignal<HashMap<String, Rc<Picture>>>,
    asked: RwSignal<BTreeSet<String>>,
    root: RwSignal<Option<ViewId>>,
}

impl crate::ext::State for Exts {
    fn new(ui: Ui) -> Self {
        let s = ui.scope;
        Exts {
            items: s.create_rw_signal(None),
            recs: s.create_rw_signal(None),
            local: s.create_rw_signal(None),
            find: s.create_rw_signal(String::new()),
            folded: s.create_rw_signal(BTreeSet::new()),
            busy: s.create_rw_signal(BTreeSet::new()),
            icons: s.create_rw_signal(HashMap::new()),
            asked: s.create_rw_signal(BTreeSet::new()),
            root: s.create_rw_signal(None),
        }
    }
}

fn state(ui: Ui) -> Exts {
    ui.state::<Exts>()
}

/// Asks for all the view shows.
fn ask_all(send: &Send) {
    send(json!({"type": "extensions"}));
    send(json!({"type": "ext_recommended"}));
    send(json!({"type": "ext_local"}));
}

fn set_busy(ui: Ui, id: &str, on: bool) {
    let st = state(ui);
    if st.busy.with_untracked(|b| b.contains(id) != on) {
        st.busy.update(|b| {
            if on {
                b.insert(id.to_string());
            } else {
                b.remove(id);
            }
        });
    }
}

/// Install from VSIX…: the path of a `.vsix`.
fn install_vsix(ui: Ui, send: &Send) {
    let send = send.clone();
    quick::input("Install from VSIX")
        .placeholder("extension.vsix")
        .prompt(&format!("The path of a .vsix: {PATH_RULE}"))
        .validate(|v| v.trim().is_empty().then(|| "A path is needed.".into()))
        .open(ui, move |path| {
            if let Some(path) = path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
                send(json!({"type": "ext_install", "path": path}));
            }
        });
}

fn uninstall(ui: Ui, send: &Send, e: &Ext) {
    let (send, id) = (send.clone(), e.id.clone());
    dialog::ask(&format!("Uninstall '{}'?", e.name))
        .detail("What it brings (its themes, snippets, keys and languages) goes with it.")
        .kind(dialog::Kind::Warning)
        .button("Uninstall")
        .destructive()
        .open(ui, move |a| {
            if a.yes() {
                set_busy(ui, &id, true);
                send(json!({"type": "ext_uninstall", "id": id}));
            }
        });
}

fn enable(ui: Ui, send: &Send, id: &str, on: bool) {
    set_busy(ui, id, true);
    send(json!({"type": "ext_enable", "id": id, "on": on}));
}

fn import(ui: Ui, send: &Send, id: &str) {
    set_busy(ui, id, true);
    send(json!({"type": "ext_import", "id": id}));
}

/// A reply's error, with what of the extension would not be used here.
fn refused(m: &Value) -> Option<String> {
    let e = m["error"].as_str()?;
    let unused = texts(&m["unused"]);
    Some(if unused.is_empty() { e.to_string() } else { format!("{e} ({})", unused.join("; ")) })
}

/// The search box over the list.
fn search_box(ui: Ui) -> impl IntoView {
    let (t, st) = (ui.colors, state(ui));
    let focused = create_rw_signal(false);
    text_input(st.find)
        .placeholder(SEARCH_HINT)
        .on_event_cont(EventListener::FocusGained, move |_| focused.set(true))
        .on_event_cont(EventListener::FocusLost, move |_| focused.set(false))
        .style(move |s| {
            let c = t.get();
            let s = s
                .width_full()
                .min_width(0.0)
                .height(26.0)
                .padding_horiz(6.0)
                .font_size(13.0)
                .border(1.0)
                .border_radius(2.0)
                .border_color(if focused.get() { c.focus } else { c.input_border })
                .background(c.input_bg)
                .color(c.input_fg)
                .cursor_color(floem::peniko::Brush::Solid(c.cursor));
            crate::widgets::steady_input(s, c.input_bg, Some(c.focus))
        })
}

/// The view's own content.
pub fn view(ui: Ui, send: Send) -> floem::AnyView {
    let (t, st) = (ui.colors, state(ui));
    if st.items.with_untracked(Option::is_none) {
        ask_all(&send);
    }
    let shown = create_memo(move |_| {
        st.items.with(|i| {
            st.recs.with(|r| {
                st.local.with(|l| {
                    st.find.with(|f| {
                        st.folded.with(|fo| st.busy.with(|b| rows(i.as_deref(), r.as_ref(), l.as_deref(), f, fo, b)))
                    })
                })
            })
        })
    });
    let none = create_memo(move |_| shown.with(Vec::is_empty));
    let body = dyn_container(
        move || none.get(),
        move |none| {
            if none {
                let words = move || format!("No extensions found for \u{201c}{}\u{201d}.", st.find.get().trim());
                return container(label(words).style(move |s| s.font_size(13.0).color(t.get().desc)))
                    .style(|s| s.padding_horiz(20.0).padding_vert(6.0).width_full().min_width(0.0))
                    .into_any();
            }
            let send = send.clone();
            scroll(
                dyn_stack(move || shown.get(), Clone::clone, move |r| row(ui, send.clone(), r))
                    .style(|s| s.flex_col().width_full().padding_bottom(10.0)),
            )
            .style(|s| s.size_full().min_height(0.0))
            .into_any()
        },
    )
    .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full().min_height(0.0));
    let top =
        container(search_box(ui)).style(|s| s.width_full().padding_horiz(12.0).padding_top(4.0).padding_bottom(6.0));
    let root = v_stack((top, body)).style(move |s| s.size_full().min_height(0.0).color(t.get().side_fg));
    let id = root.id();
    if st.root.get_untracked() != Some(id) {
        st.root.set(Some(id));
    }
    root.into_any()
}

/// A section's head: its chevron, its title in capitals and how many it has; a click folds it.
fn head(ui: Ui, section: Section, n: usize, open: bool) -> AnyView {
    let (t, st) = (ui.colors, state(ui));
    let title = section.title().to_uppercase();
    h_stack((
        icon(if open { "chevron-down" } else { "chevron-right" }, 16.0).style(move |s| s.color(t.get().icon)),
        label(move || title.clone()).style(move |s| bold(caps(s)).flex_shrink(1.0_f32).min_width(0.0).text_ellipsis()),
        empty().style(|s| s.flex_grow(1.0_f32)),
        count_badge(t, n),
    ))
    .on_click_stop(move |_| {
        st.folded.update(|f| {
            if !f.remove(&section) {
                f.insert(section);
            }
        })
    })
    .style(move |s| {
        let c = t.get();
        s.width_full()
            .height(22.0)
            .items_center()
            .gap(2.0)
            .padding_left(2.0)
            .padding_right(12.0)
            .color(c.section_fg)
            .background(c.section_bg)
            .border_top(1.0)
            .border_color(c.section_border)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
    })
    .into_any()
}

fn count_badge(t: Theme, n: usize) -> impl IntoView {
    label(move || n.to_string()).style(move |s| {
        let c = t.get();
        s.font_size(11.0)
            .padding_horiz(5.0)
            .height(16.0)
            .min_width(16.0)
            .items_center()
            .justify_center()
            .border_radius(8.0)
            .background(c.badge_bg)
            .color(c.badge_fg)
            .flex_shrink(0.0_f32)
    })
}

/// A small button of an extension's row (Uninstall, Import), as VS Code's `extensionButton`.
fn small_button(t: Theme, words: &'static str, primary: bool, on: impl Fn() + 'static) -> impl IntoView {
    label(move || words).on_click_stop(move |_| on()).style(move |s| {
        let c = t.get();
        let (bg, fg, hover) = if primary {
            (c.button_bg, c.button_fg, c.button_hover)
        } else {
            (c.button2_bg, c.button2_fg, c.button2_hover)
        };
        s.font_size(11.0)
            .padding_horiz(6.0)
            .height(18.0)
            .items_center()
            .border_radius(2.0)
            .background(bg)
            .color(fg)
            .flex_shrink(0.0_f32)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(hover))
    })
}

/// The switch that turns an extension on and off: lit while it is on.
fn switch(t: Theme, on: bool, busy: bool, flip: impl Fn() + 'static) -> impl IntoView {
    let knob = empty().style(move |s| {
        let c = t.get();
        s.width(10.0).height(10.0).border_radius(5.0).background(if on { c.button_fg } else { c.icon })
    });
    let track = container(knob)
        .on_click_stop(move |_| {
            if !busy {
                flip()
            }
        })
        .style(move |s| {
            let c = t.get();
            s.width(28.0)
                .height(16.0)
                .padding_horiz(3.0)
                .items_center()
                .border(1.0)
                .border_radius(8.0)
                .border_color(if on { c.button_bg } else { c.input_border })
                .background(if on { c.button_bg } else { c.input_bg })
                .apply_if(on, |s| s.justify_end())
                .flex_shrink(0.0_f32)
                .cursor(if busy { CursorStyle::Default } else { CursorStyle::Pointer })
        });
    tooltip(track, move || label(move || if on { "Disable" } else { "Enable" }))
}

/// An icon's place: the extension's own picture once fetched, else the Extensions codicon.
fn icon_box(ui: Ui, send: &Send, id: &str, version: &str, path: Option<&str>) -> AnyView {
    let (t, st) = (ui.colors, state(ui));
    let key = format!("{id}@{version}");
    if let Some(p) = path
        && st.asked.with_untracked(|a| !a.contains(&key))
    {
        st.asked.update(|a| {
            a.insert(key.clone());
        });
        send(json!({"type": "ext_file", "id": id, "path": p}));
    }
    let picture = create_memo(move |_| st.icons.with(|m| m.get(&key).cloned()));
    dyn_container(
        move || picture.get(),
        move |p| match p {
            Some(p) => crate::media::drawn(p, true).style(|s| s.size_full()).into_any(),
            None => icon("extensions", 32.0).style(move |s| s.color(t.get().icon)).into_any(),
        },
    )
    .style(|s| s.width(42.0).height(42.0).items_center().justify_center().flex_shrink(0.0_f32))
    .into_any()
}

/// The words of an extension's row: its name and version (and a tag), what it is, who made it, and its tooltip.
struct Words {
    name: String,
    version: String,
    tag: Option<&'static str>,
    middle: String,
    footer: String,
    tip: String,
}

/// An extension's three lines: its name and version (and a tag), what it is, and who made it with its actions.
fn card(ui: Ui, pic: AnyView, w: Words, actions: AnyView, dim: bool) -> AnyView {
    let t = ui.colors;
    let Words { name, version, tag, middle, footer, tip } = w;
    let title = h_stack((
        label(move || name.clone())
            .style(|s| bold(s.font_size(13.0)).text_ellipsis().min_width(0.0).flex_shrink(1.0_f32)),
        label(move || version.clone()).style(move |s| s.font_size(12.0).color(t.get().desc).flex_shrink(0.0_f32)),
        label(move || tag.unwrap_or("")).style(move |s| {
            let c = t.get();
            s.font_size(11.0)
                .padding_horiz(4.0)
                .border(1.0)
                .border_radius(3.0)
                .border_color(c.desc)
                .color(c.desc)
                .flex_shrink(0.0_f32)
                .apply_if(tag.is_none(), |s| s.hide())
        }),
    ))
    .style(|s| s.items_center().gap(6.0).min_width(0.0).width_full());
    let middle = label(move || middle.clone()).style(|s| s.font_size(13.0).text_ellipsis().min_width(0.0).width_full());
    let footer = h_stack((
        label(move || footer.clone())
            .style(move |s| s.font_size(12.0).color(t.get().desc).text_ellipsis().min_width(0.0).flex_shrink(1.0_f32)),
        empty().style(|s| s.flex_grow(1.0_f32)),
        actions,
    ))
    .style(|s| s.items_center().gap(6.0).min_width(0.0).width_full());
    let words = v_stack((title, middle, footer))
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).gap(2.0).justify_center());
    let line = h_stack((pic, words)).style(move |s| {
        s.width_full()
            .height(72.0)
            .items_center()
            .gap(12.0)
            .padding_left(16.0)
            .padding_right(12.0)
            .apply_if(dim, |s| s.color(t.get().desc))
            .hover(|s| s.background(t.get().list_hover))
    });
    tooltip(line, move || {
        let tip = tip.clone();
        label(move || tip.clone())
    })
    .style(|s| s.width_full())
    .into_any()
}

/// A row drawn.
fn row(ui: Ui, send: Send, r: Row) -> AnyView {
    let t = ui.colors;
    match r {
        Row::Head { section, n, open } => head(ui, section, n, open),
        Row::Note { section, text } => {
            let vsix = section == Section::Installed && text.starts_with("No extensions");
            let link = send.clone();
            let button = small_button(t, "Install from VSIX\u{2026}", true, move || install_vsix(ui, &link))
                .style(move |s| s.apply_if(!vsix, |s| s.hide()));
            v_stack((
                label(move || text.clone()).style(move |s| {
                    s.font_size(13.0)
                        .line_height(1.4)
                        .color(t.get().desc)
                        .text_overflow(floem::style::TextOverflow::Wrap)
                        .min_width(0.0)
                        .width_full()
                }),
                button,
            ))
            .style(|s| s.gap(8.0).width_full().min_width(0.0).padding_horiz(20.0).padding_vert(8.0).items_start())
            .into_any()
        }
        Row::Ext { e, busy } => {
            let pic = icon_box(ui, &send, &e.id, &e.version, e.icon.as_deref());
            let (on, id) = (e.enabled, e.id.clone());
            let flip = send.clone();
            let toggle = switch(t, on, busy, move || enable(ui, &flip, &id, !on));
            let (gone, ext) = (send.clone(), e.clone());
            let remove = tooltip(
                action(t, "trash", move || {
                    if !busy {
                        uninstall(ui, &gone, &ext)
                    }
                }),
                || label(|| "Uninstall"),
            );
            let unused = !e.unused.is_empty();
            let warn = tooltip(icon("warning", 14.0).style(move |s| s.color(t.get().warning).flex_shrink(0.0_f32)), {
                let all = format!("Not used here:\n{}", e.unused.join("\n"));
                move || {
                    let all = all.clone();
                    label(move || all.clone())
                }
            })
            .style(move |s| s.apply_if(!unused, |s| s.hide()));
            let actions = h_stack((warn, toggle, remove)).style(|s| s.items_center().gap(8.0).flex_shrink(0.0_f32));
            let middle = if e.description.is_empty() { e.brings.clone() } else { e.description.clone() };
            let footer = match (e.publisher.is_empty(), e.brings.is_empty() || e.description.is_empty()) {
                (true, _) => e.brings.clone(),
                (false, true) => e.publisher.clone(),
                (false, false) => format!("{} \u{b7} {}", e.publisher, e.brings),
            };
            let mut tip = format!("{}\n{} {}", e.name, e.id, e.version);
            if !e.brings.is_empty() {
                tip.push_str(&format!("\nBrings {}.", e.brings));
            }
            if e.source == "vscode" {
                tip.push_str("\nImported from VS Code.");
            }
            for u in &e.unused {
                tip.push_str(&format!("\nNot used here: {u}"));
            }
            let tag = (!e.enabled).then_some("Disabled");
            let w = Words { name: e.name, version: e.version, tag, middle, footer, tip };
            card(ui, pic, w, actions.into_any(), !e.enabled || busy)
        }
        Row::Rec { r, busy } => {
            let pic = icon_box(ui, &send, &r.id, "", None);
            let (go, id) = (send.clone(), r.id.clone());
            let act = match r.status.as_str() {
                "local" => small_button(t, "Import", true, move || {
                    if !busy {
                        import(ui, &go, &id)
                    }
                })
                .into_any(),
                "unknown" => {
                    small_button(t, "Install from VSIX\u{2026}", true, move || install_vsix(ui, &go)).into_any()
                }
                _ => empty().into_any(),
            };
            let tag = match r.status.as_str() {
                "built-in" => Some("Built in"),
                "unsupported" => Some("Not supported"),
                _ => None,
            };
            let tip = format!("{}\n{}\n{}", r.name, r.id, r.note);
            let w = Words { name: r.name, version: String::new(), tag, middle: r.note, footer: r.id, tip };
            card(ui, pic, w, act, busy)
        }
        Row::Local { l, busy } => {
            let pic = icon_box(ui, &send, &l.id, &l.version, None);
            let (go, id) = (send.clone(), l.id.clone());
            let offer = l.offer();
            let act = match offer {
                Offer::Import | Offer::Update => {
                    let words = if offer == Offer::Import { "Import" } else { "Update" };
                    small_button(t, words, true, move || {
                        if !busy {
                            import(ui, &go, &id)
                        }
                    })
                    .into_any()
                }
                Offer::Imported => {
                    label(|| "Imported").style(move |s| s.font_size(12.0).color(t.get().desc)).into_any()
                }
                Offer::Unusable => icon("warning", 14.0).style(move |s| s.color(t.get().warning)).into_any(),
            };
            let middle = match (&l.problem, l.description.is_empty()) {
                (Some(p), _) if !l.usable => p.clone(),
                (_, true) => l.brings.clone(),
                _ => l.description.clone(),
            };
            let footer =
                if l.publisher.is_empty() { l.from.clone() } else { format!("{} \u{b7} {}", l.publisher, l.from) };
            let mut tip = format!("{}\n{} {} ({})", l.name, l.id, l.version, l.from);
            if let Some(kept) = &l.installed {
                tip.push_str(&format!("\nKept here: {kept}."));
            }
            if !l.brings.is_empty() {
                tip.push_str(&format!("\nBrings {}.", l.brings));
            }
            if let Some(p) = &l.problem {
                tip.push_str(&format!("\n{p}"));
            }
            let w = Words { name: l.name, version: l.version, tag: None, middle, footer, tip };
            card(ui, pic, w, act, !l.usable || busy)
        }
    }
}

/// Opens the title's "…" menu at the top right of the view.
fn more(ui: Ui, send: &Send) {
    use crate::widgets::popup;
    let Some(view) = state(ui).root.get_untracked() else { return };
    let r = view.layout_rect();
    let rows = crate::skills_view::more_rows(crate::menus::rows(ui, crate::menus::Menu::ViewTitle(View::Extensions)));
    popup::open(ui, popup::At::point(view, r.x1 - 8.0, r.y0), popup::of_rows(ui, send, rows));
}

/// The search set to `text`, the view shown.
fn search_for(ui: Ui, text: &str) {
    let st = state(ui);
    if st.find.with_untracked(|f| f != text) {
        st.find.set(text.to_string());
    }
    ui.reveal(View::Extensions);
}

/// The actions in its title: none of its own (they are the title menu's).
pub fn actions(_ui: Ui, _send: Send) -> Option<floem::AnyView> {
    None
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::menus::{Menu, item};
    add(command(INSTALL_VSIX, "Extensions", "Install from VSIX...", install_vsix));
    add(command(REFRESH, "Extensions", "Refresh", |_, send| ask_all(send)));
    add(command(CLEAR, "Extensions", "Clear Extensions Search Results", |ui, _| search_for(ui, "")));
    add(command(SHOW_INSTALLED, "Extensions", "Show Installed Extensions", |ui, _| search_for(ui, "@installed")));
    add(command(SHOW_RECOMMENDED, "Extensions", "Show Recommended Extensions", |ui, _| search_for(ui, "@recommended")));
    add(command(MORE, "Extensions", "More Actions...", more).hidden());
    let at = Menu::ViewTitle(View::Extensions);
    let title = |id: &str, icon: &'static str, order: i32| item(id).icon(icon).group("navigation").order(order);
    crate::menus::add(at, title(REFRESH, "refresh", 1));
    crate::menus::add(at, title(CLEAR, "clear-all", 2));
    crate::menus::add(at, title(MORE, "ellipsis", 3));
    crate::menus::add(at, item(SHOW_INSTALLED).group("1_show").order(1));
    crate::menus::add(at, item(SHOW_RECOMMENDED).group("1_show").order(2));
    crate::menus::add(at, item(INSTALL_VSIX).group("2_install").order(1));
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it. None is
/// taken away: `extensions` and `ext_file` are read by `extensions_apply` too.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = state(ui);
    let id = m["id"].as_str().unwrap_or("");
    match m["type"].as_str().unwrap_or("") {
        "extensions" => {
            let items = items_of(m);
            let was = st.items.get_untracked();
            if was.as_ref() != Some(&items) {
                // What is kept here changed: what the project recommends and what may be imported say so too.
                let ids = |l: &[Ext]| l.iter().map(|e| (e.id.clone(), e.version.clone())).collect::<BTreeSet<_>>();
                if was.as_deref().is_some_and(|w| ids(w) != ids(&items)) {
                    send(json!({"type": "ext_recommended"}));
                    send(json!({"type": "ext_local"}));
                }
                st.items.set(Some(items));
            }
        }
        "ext_recommended" => {
            let recs = recs_of(m);
            if st.recs.with_untracked(|r| r.as_ref() != Some(&recs)) {
                st.recs.set(Some(recs));
            }
        }
        "ext_local" => {
            let local = local_of(m);
            if st.local.with_untracked(|l| l.as_ref() != Some(&local)) {
                st.local.set(Some(local));
            }
        }
        "ext_install" => match refused(m) {
            Some(e) => {
                toast::error(ui, &format!("Could not install {}: {e}", m["path"].as_str().unwrap_or("the .vsix")));
            }
            None => {
                let name = shown_name(&m["item"]);
                toast::info(ui, &format!("Completed installing {name} extension from VSIX."));
            }
        },
        "ext_import" => {
            set_busy(ui, id, false);
            match refused(m) {
                Some(e) => toast::error(ui, &format!("Could not import {id}: {e}")),
                None => toast::info(
                    ui,
                    &format!("Imported {} {}.", shown_name(&m["item"]), m["item"]["version"].as_str().unwrap_or("")),
                ),
            };
        }
        "ext_uninstall" => {
            set_busy(ui, id, false);
            if let Some(e) = refused(m) {
                toast::error(ui, &format!("Could not uninstall {id}: {e}"));
            }
        }
        "ext_enable" => {
            set_busy(ui, id, false);
            if let Some(e) = refused(m) {
                toast::error(ui, &e);
            }
        }
        "ext_file" => {
            // An icon this view asked for: its key is the extension's id and the version kept.
            let version = st.items.with_untracked(|i| {
                i.as_ref()?
                    .iter()
                    .find(|e| e.id == id && e.icon.as_deref() == m["path"].as_str())
                    .map(|e| e.version.clone())
            });
            if let Some(version) = version
                && m["error"].is_null()
            {
                let key = format!("{id}@{version}");
                let url =
                    format!("data:{};base64,{}", m["mime"].as_str().unwrap_or(""), m["data"].as_str().unwrap_or(""));
                let icons = st.icons;
                let k = key.clone();
                crate::media::small_picture(ui, &url, &format!("extension\0{key}"), move |p| {
                    if let Some(p) = p {
                        icons.update(|m| {
                            m.insert(k, p);
                        });
                    }
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

    fn installed() -> Vec<Ext> {
        items_of(&json!({"type": "extensions", "items": [
            {"id": "acme.night", "name": "night", "displayName": "Night", "version": "1.2.0", "publisher": "acme",
             "description": "A dark theme", "icon": "icon.png", "enabled": true, "source": "vsix",
             "contributes": {"themes": [{"label": "Night"}, {"label": "Night Blue"}],
                             "snippets": [{"language": "rust"}, {"language": "rust"}], "keybindings": 1,
                             "configurationDefaults": {"editor.tabSize": 2}},
             "unused": ["grammars: kept, not used yet"]},
            {"id": "acme.keys", "name": "keys", "displayName": null, "version": "0.1.0", "publisher": "acme",
             "description": "", "icon": null, "enabled": false, "source": "vscode",
             "contributes": {"keybindings": 0, "languages": [{"id": "x"}]}, "unused": []}
        ]}))
    }

    #[test]
    fn an_item_says_what_it_brings_and_whether_it_is_on() {
        let l = installed();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].name, "Night");
        assert_eq!(l[0].brings, "2 color themes, snippets for rust, 1 keybinding, setting defaults");
        assert_eq!(l[0].icon.as_deref(), Some("icon.png"));
        assert_eq!(l[0].unused, vec!["grammars: kept, not used yet".to_string()]);
        assert!(l[0].enabled);
        assert_eq!(l[1].name, "keys", "no display name: its name");
        assert_eq!(l[1].brings, "1 language");
        assert!(!l[1].enabled);
        assert_eq!(brings(&json!({})), "");
    }

    #[test]
    fn versions_compare_by_their_numbers_then_a_release_after_its_pre_release() {
        assert!(newer("1.10.0", "1.9.0"));
        assert!(!newer("1.9.0", "1.10.0"));
        assert!(!newer("1.2.0", "1.2.0"));
        assert!(newer("1.2.0", "1.2.0-beta"));
        assert!(newer("2", "1.9.9"));
    }

    #[test]
    fn a_copy_on_this_computer_is_imported_updated_or_left() {
        let l = local_of(&json!({"type": "ext_local", "items": [
            {"id": "a.b", "name": "b", "displayName": "B", "version": "1.0.0", "from": "VS Code", "usable": true,
             "problem": null, "installed": null},
            {"id": "a.c", "name": "c", "version": "2.0.0", "from": "VSCodium", "usable": true, "installed": "1.0.0"},
            {"id": "a.d", "name": "d", "version": "1.0.0", "from": "VS Code", "usable": true, "installed": "1.0.0"},
            {"id": "a.e", "name": "e", "version": "1.0.0", "from": "VS Code", "usable": false,
             "problem": "It is all code."}
        ]}));
        let offers: Vec<Offer> = l.iter().map(Local::offer).collect();
        assert_eq!(offers, vec![Offer::Import, Offer::Update, Offer::Imported, Offer::Unusable]);
        assert_eq!(l[3].problem.as_deref(), Some("It is all code."));
    }

    fn recs() -> Recs {
        recs_of(&json!({"type": "ext_recommended", "file": true, "recommendations": [
            {"id": "acme.night", "status": "installed", "note": "Installed here (1.2.0)."},
            {"id": "rust-lang.rust-analyzer", "status": "built-in", "note": "Code Arena reads Rust itself."},
            {"id": "x.y", "status": "local", "name": "Y", "note": "VS Code has it on this computer (1.0.0): import it."}
        ], "unwanted": []}))
    }

    fn heads(r: &[Row]) -> Vec<(Section, usize, bool)> {
        r.iter()
            .filter_map(|r| match r {
                Row::Head { section, n, open } => Some((*section, *n, *open)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_sections_list_what_is_installed_recommended_and_on_this_computer() {
        let (i, r, none, idle) = (installed(), recs(), BTreeSet::new(), BTreeSet::new());
        let local: Vec<Local> = Vec::new();
        let all = rows(Some(&i), Some(&r), Some(&local), "", &none, &idle);
        assert_eq!(
            heads(&all),
            vec![(Section::Installed, 2, true), (Section::Recommended, 2, true), (Section::Local, 0, true)],
            "an installed recommendation is not recommended again"
        );
        assert!(matches!(all.last(), Some(Row::Note { section: Section::Local, .. })), "why it is empty");
        assert_eq!(rec_name(&all), "Y");
        // Folded: its head only.
        let folded = BTreeSet::from([Section::Installed]);
        let some = rows(Some(&i), Some(&r), Some(&local), "", &folded, &idle);
        assert!(matches!(some[1], Row::Head { section: Section::Recommended, .. }));
        // Busy: the row says so.
        let busy = BTreeSet::from(["acme.keys".to_string()]);
        let b = rows(Some(&i), Some(&r), Some(&local), "", &none, &busy);
        assert!(b.iter().any(|r| matches!(r, Row::Ext { e, busy: true } if e.id == "acme.keys")));
    }

    fn rec_name(r: &[Row]) -> String {
        r.iter()
            .filter_map(|r| match r {
                Row::Rec { r, .. } if r.status == "local" => Some(r.name.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn nothing_installed_says_how_to_install_and_no_file_has_no_recommended_section() {
        let (none, idle) = (BTreeSet::new(), BTreeSet::new());
        let no_file = recs_of(&json!({"type": "ext_recommended", "file": false, "recommendations": []}));
        let r = rows(Some(&[]), Some(&no_file), None, "", &none, &idle);
        assert_eq!(heads(&r), vec![(Section::Installed, 0, true), (Section::Local, 0, true)]);
        match &r[1] {
            Row::Note { section: Section::Installed, text } => assert!(text.contains("Install from VSIX")),
            other => panic!("{other:?}"),
        }
        // Not read yet: it says it is reading.
        let r = rows(None, None, None, "", &none, &idle);
        assert!(matches!(&r[1], Row::Note { text, .. } if text.starts_with("Reading")));
    }

    #[test]
    fn the_search_finds_by_words_and_narrows_by_kind() {
        let (i, r, none, idle) = (installed(), recs(), BTreeSet::new(), BTreeSet::new());
        let found = rows(Some(&i), Some(&r), Some(&[]), "dark", &none, &idle);
        assert_eq!(heads(&found), vec![(Section::Installed, 1, true)], "sections with nothing found are left out");
        let off = rows(Some(&i), Some(&r), Some(&[]), "@disabled", &none, &idle);
        assert!(matches!(&off[1], Row::Ext { e, .. } if e.id == "acme.keys"));
        assert_eq!(off.len(), 2);
        let rec = rows(Some(&i), Some(&r), Some(&[]), "@recommended rust", &none, &idle);
        assert_eq!(heads(&rec), vec![(Section::Recommended, 1, true)]);
        assert!(rows(Some(&i), Some(&r), Some(&[]), "nothing-like-it", &none, &idle).is_empty());
        assert_eq!(find_of("@Installed  Night"), Find { only: Some("installed"), words: "night".into() });
    }

    #[test]
    fn a_refusal_says_what_would_not_be_used() {
        assert_eq!(refused(&json!({"error": "No."})).as_deref(), Some("No."));
        assert_eq!(
            refused(&json!({"error": "Nothing usable.", "unused": ["a", "b"]})).as_deref(),
            Some("Nothing usable. (a; b)")
        );
        assert_eq!(refused(&json!({"id": "x"})), None);
    }

    #[test]
    fn its_commands_are_in_the_palette_and_its_title() {
        let ui = Ui::new();
        crate::layout::also_ready(&[View::Extensions]);
        let send: Send = Rc::new(|_| {});
        register(ui, &send);
        for id in [INSTALL_VSIX, REFRESH, CLEAR, SHOW_INSTALLED, SHOW_RECOMMENDED] {
            assert!(crate::commands::find(id).is_some(), "{id}");
        }
        let rows =
            crate::skills_view::more_rows(crate::menus::rows(ui, crate::menus::Menu::ViewTitle(View::Extensions)));
        assert!(!rows.is_empty(), "the \u{2026} menu has Install from VSIX");
    }

    #[test]
    fn replies_are_taken_in_and_set_only_when_they_change() {
        let ui = Ui::new();
        let said = Rc::new(std::cell::RefCell::new(Vec::<Value>::new()));
        let to = said.clone();
        let send: Send = Rc::new(move |v| to.borrow_mut().push(v));
        let list = json!({"type": "extensions", "items": [{"id": "a.b", "version": "1.0.0"}]});
        assert!(!take(ui, &list, &send), "left to extensions_apply too");
        assert_eq!(state(ui).items.get_untracked().map(|i| i.len()), Some(1));
        assert!(said.borrow().is_empty(), "the first list asks for nothing more");
        take(ui, &json!({"type": "extensions", "items": []}), &send);
        let asked: Vec<String> = said.borrow().iter().map(|m| text(&m["type"])).collect();
        assert_eq!(asked, vec!["ext_recommended", "ext_local"], "what is kept changed");
        set_busy(ui, "a.b", true);
        take(ui, &json!({"type": "ext_enable", "id": "a.b", "on": false, "error": "No."}), &send);
        assert!(state(ui).busy.get_untracked().is_empty());
    }
}
