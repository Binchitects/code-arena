//! The editor's settings put in force, as VS Code's: read for the file's language (its `[lang]` block wins), and
//! applied to every editor on screen as soon as a settings file changes, with no tab opened again.
//!
//! Applied:
//!
//! - `editor.fontFamily`, `editor.fontSize`, `editor.fontWeight`, `editor.lineHeight` (0: from the font size, below
//!   8: a multiple of it), and the editor's font zoom (View: Editor Font Zoom In, Out, Reset; Ctrl and the mouse
//!   wheel with `editor.mouseWheelZoom`), each step 10% of the font size and the line height, as VS Code's.
//! - `editor.wordWrap` off, on, wordWrapColumn (at `editor.wordWrapColumn`), bounded (at the column or the editor's
//!   width, whichever is less); View: Toggle Word Wrap (Alt+Z) turns it on or off for the file, not the setting.
//! - `editor.rulers`: a line at each column, in `editorRuler.foreground`, down the file's lines.
//! - `editor.lineNumbers` on and off (off: the numbers are not drawn, their column stays).
//! - `editor.renderWhitespace` none, boundary, trailing, all; View: Toggle Render Whitespace writes none or all.
//! - `editor.renderLineHighlight` none, gutter, line, all.
//! - `editor.cursorBlinking` solid (the caret stays); the others blink.
//! - `editor.guides.indentation`, `editor.scrollBeyondLastLine`.
//! - `editor.emptySelectionClipboard`: off, Ctrl+C and Ctrl+X with nothing selected do nothing (on, they take the
//!   line, which is what the editor does).
//! - View: Toggle Minimap, Toggle Sticky Scroll, Toggle Breadcrumbs write the person's setting
//!   (`editor.minimap.enabled`, `editor.stickyScroll.enabled`, `breadcrumbs.enabled`); the breadcrumbs follow it.
//!
//! Not applied, because Floem's editor cannot do them (the setting is kept, and nothing pretends to follow it):
//!
//! - `editor.renderWhitespace` selection (the default): Floem marks whitespace by line, not by selection, so it is
//!   drawn as none.
//! - `editor.lineNumbers` relative and interval: Floem numbers lines relatively only in its modal (vim) mode; they
//!   are drawn as on.
//! - `editor.cursorStyle`: Floem draws one caret, the line; block, underline and the thin ones are drawn as line.
//! - `editor.cursorBlinking` smooth, phase, expand: Floem's caret blinks on and off only.
//! - `editor.cursorSmoothCaretAnimation`, `editor.smoothScrolling`: Floem moves the caret and scrolls without
//!   animation.
//! - `editor.fontLigatures` off: Floem's text shaping always joins a font's ligatures and takes no font features.
//! - `editor.letterSpacing`: Floem's text attributes have no letter spacing.
//! - `editor.multiCursorModifier` ctrlCmd: Floem adds a cursor on Alt and a click (alt, the default), and on
//!   nothing else.
//! - `editor.renderControlCharacters`, `editor.dragAndDrop`, `editor.copyWithSyntaxHighlighting`: Floem's editor
//!   draws control characters as the font has them, does not drag a selection, and copies plain text.
//! - `editor.linkedEditing` is the language servers' (it needs the tag pairs a server says).
//!
//! What every part has (`register`, `take`) is said in `ext.rs`.

use std::collections::HashSet;
use std::rc::Rc;

use floem::event::{Event, EventPropagation};
use floem::keyboard::{Key, Modifiers};
use floem::peniko::Color;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::style::Style;
use floem::text::{Attrs, AttrsList, FamilyOwned, TextLayout, Weight};
use floem::views::Decorators;
use floem::views::editor::Editor;
use floem::views::editor::core::cursor::CursorMode;
use floem::views::editor::text::{RenderWhitespace, WrapMethod};
use floem::views::text_editor::EditorCustomStyle;
use floem::{AnyView, IntoView, View, ViewId};
use serde_json::{Value, json};

use crate::colors::Colors;
use crate::highlight::Colours;
use crate::settings;
use crate::state::{Send, Ui};
use crate::widgets::MONO_FONT;

/// VS Code's ids of the commands of this part.
pub const TOGGLE_WORD_WRAP: &str = "editor.action.toggleWordWrap";
pub const ZOOM_IN: &str = "editor.action.fontZoomIn";
pub const ZOOM_OUT: &str = "editor.action.fontZoomOut";
pub const ZOOM_RESET: &str = "editor.action.fontZoomReset";

/// The editor's font: what its lines are laid out with.
#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    /// The families, as CSS names them, the editor's own last.
    pub family: String,
    pub size: f32,
    pub weight: u16,
    pub line_height: f32,
}

impl Font {
    /// Its families, as the text is laid out with.
    pub fn families(&self) -> Vec<FamilyOwned> {
        FamilyOwned::parse_list(&self.family).collect()
    }

    /// The width of one of its characters (the editor's fonts are fixed width), by which columns are found.
    pub fn char_width(&self) -> f64 {
        let family = self.families();
        let attrs = Attrs::new().family(&family).font_size(self.size).weight(Weight(self.weight));
        let mut layout = TextLayout::new();
        layout.set_text("0000000000", AttrsList::new(attrs));
        let w = layout.size().width / 10.0;
        if w > 0.0 { w } else { f64::from(self.size) * 0.6 }
    }
}

/// How the editor's lines wrap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wrap {
    Off,
    /// At the editor's width.
    On,
    /// At a column.
    Column(usize),
    /// At a column, or the editor's width when that is less.
    Bounded(usize),
}

/// What a file's editor is drawn with, from the settings for its language.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub font: Font,
    pub wrap: Wrap,
    /// The rulers' columns.
    pub rulers: Vec<usize>,
    pub line_numbers: bool,
    pub whitespace: RenderWhitespace,
    /// The current line's background in the gutter, and across the text.
    pub highlight_gutter: bool,
    pub highlight_line: bool,
    /// The caret does not blink.
    pub solid_caret: bool,
    pub indent_guides: bool,
    pub scroll_beyond_last_line: bool,
    pub wheel_zoom: bool,
    pub empty_selection_copies: bool,
}

/// The size a font zoom level gives (VS Code's: each level 10% of the size, from -5 to 20).
pub fn zoomed(size: f32, level: i32) -> f32 {
    (size * (1.0 + 0.1 * level.clamp(-5, 20) as f32)).max(1.0)
}

/// `editor.fontSize`: from 6 to 100, the default where it is not a number.
pub fn font_size(v: &Value) -> f32 {
    v.as_f64().map_or(13.0, |n| n.clamp(6.0, 100.0) as f32)
}

/// `editor.fontWeight`: normal, bold, or a number from 1 to 1000 (as text or a number).
pub fn font_weight(v: &Value) -> u16 {
    let n = match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => match s.trim() {
            "normal" => Some(400.0),
            "bold" => Some(700.0),
            other => other.parse().ok(),
        },
        _ => None,
    };
    n.map_or(400, |n| n.clamp(1.0, 1000.0) as u16)
}

/// `editor.fontFamily`: the families written, then the editor's own; none written, the editor's own.
pub fn font_family(v: &Value) -> String {
    match v.as_str().map(str::trim) {
        Some(f) if !f.is_empty() => format!("{f}, {}", *MONO_FONT),
        _ => MONO_FONT.clone(),
    }
}

/// `editor.lineHeight` for a font size, as VS Code: 0 works it out from the size, below 8 it is a multiple of the
/// size, else pixels (8 at least).
pub fn line_height(v: &Value, size: f32) -> f32 {
    let h = v.as_f64().unwrap_or(0.0) as f32;
    let px = if h <= 0.0 {
        size * 1.35
    } else if h < 8.0 {
        size * h
    } else {
        h
    };
    px.max(8.0).round()
}

/// `editor.wordWrap` with `editor.wordWrapColumn`.
pub fn wrap(mode: &Value, column: &Value) -> Wrap {
    let col = column.as_u64().map_or(80, |c| c.max(1) as usize);
    match mode.as_str() {
        Some("on") => Wrap::On,
        Some("wordWrapColumn") => Wrap::Column(col),
        Some("bounded") => Wrap::Bounded(col),
        _ => Wrap::Off,
    }
}

/// Toggle Word Wrap's: a file whose lines wrap stops wrapping them; one whose lines do not wraps them at the width.
pub fn toggled(w: Wrap) -> Wrap {
    if w == Wrap::Off { Wrap::On } else { Wrap::Off }
}

/// `editor.rulers`: its columns (VS Code's `{ "column": 80 }` too), in order.
pub fn rulers(v: &Value) -> Vec<usize> {
    let mut cols: Vec<usize> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r.as_f64().or_else(|| r["column"].as_f64()))
        .filter(|c| *c >= 0.0)
        .map(|c| c as usize)
        .collect();
    cols.sort_unstable();
    cols.dedup();
    cols
}

/// `editor.renderWhitespace` as Floem draws it (selection: none, see the module's doc).
pub fn whitespace(v: &Value) -> RenderWhitespace {
    match v.as_str() {
        Some("all") => RenderWhitespace::All,
        Some("boundary") => RenderWhitespace::Boundary,
        Some("trailing") => RenderWhitespace::Trailing,
        _ => RenderWhitespace::None,
    }
}

/// `editor.renderLineHighlight`: the gutter's and the line's.
pub fn line_highlight(v: &Value) -> (bool, bool) {
    match v.as_str() {
        Some("none") => (false, false),
        Some("gutter") => (true, false),
        Some("all") => (true, true),
        _ => (false, true),
    }
}

/// The window's font zoom and the files Toggle Word Wrap turned (neither is a setting: as VS Code's, they last
/// while the window is open).
#[derive(Clone, Copy)]
pub struct Zoom {
    pub level: RwSignal<i32>,
    pub toggled: RwSignal<HashSet<String>>,
}

impl crate::ext::State for Zoom {
    fn new(ui: Ui) -> Self {
        Zoom { level: ui.scope.create_rw_signal(0), toggled: ui.scope.create_rw_signal(HashSet::new()) }
    }
}

/// The font zoom one step in (1), out (-1), or back (0).
pub fn zoom(ui: Ui, step: i32) {
    let level = ui.state::<Zoom>().level;
    let next = if step == 0 { 0 } else { (level.get_untracked() + step).clamp(-5, 20) };
    if next != level.get_untracked() {
        level.set(next);
    }
}

/// The options of a file's editor (none: an editor of no file), followed: read again when the settings, the zoom
/// or the file's toggled wrap change.
pub fn read(ui: Ui, path: Option<&str>) -> Options {
    let lang = path.map(|p| crate::language::id(ui, p));
    let z = ui.state::<Zoom>();
    let level = z.level.get();
    let size = font_size(&settings::setting(ui, "editor.fontSize", lang));
    let font = Font {
        family: font_family(&settings::setting(ui, "editor.fontFamily", lang)),
        // Whole pixels, as the editor lays its text out with: the columns of the rulers and of a bounded wrap are
        // found at the very size drawn.
        size: zoomed(size, level).round(),
        weight: font_weight(&settings::setting(ui, "editor.fontWeight", lang)),
        line_height: zoomed(line_height(&settings::setting(ui, "editor.lineHeight", lang), size), level).round(),
    };
    let mut wrap =
        wrap(&settings::setting(ui, "editor.wordWrap", lang), &settings::setting(ui, "editor.wordWrapColumn", lang));
    if path.is_some_and(|p| z.toggled.with(|t| t.contains(p))) {
        wrap = toggled(wrap);
    }
    let (highlight_gutter, highlight_line) = line_highlight(&settings::setting(ui, "editor.renderLineHighlight", lang));
    Options {
        font,
        wrap,
        rulers: rulers(&settings::setting(ui, "editor.rulers", lang)),
        line_numbers: settings::setting(ui, "editor.lineNumbers", lang).as_str() != Some("off"),
        whitespace: whitespace(&settings::setting(ui, "editor.renderWhitespace", lang)),
        highlight_gutter,
        highlight_line,
        solid_caret: settings::setting(ui, "editor.cursorBlinking", lang).as_str() == Some("solid"),
        indent_guides: settings::flag(ui, "editor.guides.indentation", lang),
        scroll_beyond_last_line: settings::flag(ui, "editor.scrollBeyondLastLine", lang),
        wheel_zoom: settings::flag(ui, "editor.mouseWheelZoom", lang),
        empty_selection_copies: settings::flag(ui, "editor.emptySelectionClipboard", lang),
    }
}

/// The options of the file a view shows, behind a memo: what is built from them hears only of a change of theirs.
/// Made once per view that shows files (a group, a settings tab), in that view's scope.
pub fn memo(ui: Ui, path: impl Fn() -> Option<String> + 'static) -> Memo<Options> {
    create_memo(move |_| read(ui, path().as_deref()))
}

/// The editor's style from its options and the theme's colours: the wrap (bounded at `width`, the editor's own),
/// whitespace, guides, the line numbers and the current line.
pub fn style(s: EditorCustomStyle, o: &Options, c: &Colors, width: f64) -> EditorCustomStyle {
    let (wrap, dim, accent, gutter, line) = parts(o, c, width);
    s.gutter_dim_color(dim)
        .gutter_accent_color(accent)
        .gutter_current_color(gutter)
        .current_line_color(line)
        .wrap_method(wrap)
        .render_white_space(o.whitespace)
        .indent_guide(o.indent_guides)
        .scroll_beyond_last_line(o.scroll_beyond_last_line)
}

/// The same as `style`, for an editor drawn by its own parts (a group's, with the folds): set on its gutter's and
/// its view's classes.
pub fn look(s: Style, o: &Options, c: &Colors, width: f64) -> Style {
    use floem::views::editor::gutter::{DimColor, GutterClass};
    use floem::views::editor::view::EditorViewClass;
    use floem::views::editor::{
        CurrentLineColor, RenderWhitespaceProp, ScrollBeyondLastLine, ShowIndentGuide, WrapProp,
    };
    let (wrap, dim, accent, gutter, line) = parts(o, c, width);
    let (whitespace, guides, beyond) = (o.whitespace, o.indent_guides, o.scroll_beyond_last_line);
    let s = s.class(GutterClass, move |s| s.color(accent).set(DimColor, Some(dim)).set(CurrentLineColor, Some(gutter)));
    s.class(EditorViewClass, move |s| {
        s.set(CurrentLineColor, Some(line))
            .set(WrapProp, wrap)
            .set(RenderWhitespaceProp, whitespace)
            .set(ShowIndentGuide, guides)
            .set(ScrollBeyondLastLine, beyond)
    })
}

/// The wrap, the line numbers' colours (dim, accent), the gutter's current line and the current line's colour.
fn parts(o: &Options, c: &Colors, width: f64) -> (WrapMethod, Color, Color, Color, Color) {
    let wrap = match o.wrap {
        Wrap::Off => WrapMethod::None,
        Wrap::On => WrapMethod::EditorWidth,
        Wrap::Column(col) => WrapMethod::WrapColumn { col },
        Wrap::Bounded(col) => {
            let at = col as f64 * o.font.char_width();
            if width > 0.0 && width < at { WrapMethod::EditorWidth } else { WrapMethod::WrapColumn { col } }
        }
    };
    let gutter = if o.highlight_gutter { c.line_highlight } else { c.editor_bg };
    // Hidden numbers are drawn in the colour under them, not a transparent one: the software renderer draws a
    // glyph's colour without its alpha, so a transparent number came out black.
    let (dim, accent) = if o.line_numbers { (c.line_number, c.line_number_active) } else { (c.editor_bg, gutter) };
    let line = if o.highlight_line { c.line_highlight } else { Color::TRANSPARENT };
    (wrap, dim, accent, gutter, line)
}

/// An editor made to follow its options: its font and rulers laid out again when they change (the style is
/// `style`'s), its caret kept on when it does not blink, and Ctrl and the wheel zooming its font. The view given
/// is returned inside the one that takes the wheel.
pub fn follow(
    ui: Ui,
    view: impl IntoView + 'static,
    editor: &Editor,
    colours: Rc<Colours>,
    o: Memo<Options>,
) -> AnyView {
    let (here, c) = (editor.clone(), ui.colors);
    let first = o.get_untracked();
    colours.set_look(&first.font, &first.rulers, c.get_untracked().editor_ruler);
    create_effect(move |before: Option<(Font, Vec<usize>)>| {
        let (font, rulers) = o.with(|o| (o.font.clone(), o.rulers.clone()));
        let now = (font, rulers);
        if before.as_ref().is_some_and(|b| *b != now) {
            colours.set_look(&now.0, &now.1, c.get_untracked().editor_ruler);
            here.update_styling(colours.clone());
        }
        now
    });
    let hidden = editor.cursor_info.hidden;
    create_effect(move |_| {
        if hidden.get() && o.with(|o| o.solid_caret) {
            hidden.set(false);
        }
    });
    let id = ViewId::new();
    id.set_children(vec![view.into_any()]);
    Wheel { id, ui, on: o }.style(|s| s.size_full()).into_any()
}

/// Takes Ctrl and the wheel before the editor scrolls, when `editor.mouseWheelZoom` is on.
struct Wheel {
    id: ViewId,
    ui: Ui,
    on: Memo<Options>,
}

impl View for Wheel {
    fn id(&self) -> ViewId {
        self.id
    }

    fn event_before_children(&mut self, _: &mut floem::context::EventCx, event: &Event) -> EventPropagation {
        if let Event::PointerWheel(w) = event
            && w.modifiers.control()
            && self.on.with_untracked(|o| o.wheel_zoom)
            && w.delta.y != 0.0
        {
            zoom(self.ui, if w.delta.y < 0.0 { 1 } else { -1 });
            return EventPropagation::Stop;
        }
        EventPropagation::Continue
    }
}

/// The editor's own keys of its options: Alt+Z toggles the file's wrap (taken here, before the editor types a z);
/// with `editor.emptySelectionClipboard` off, Ctrl+C and Ctrl+X on a caret with nothing selected are taken and do
/// nothing.
/// Nothing is held while the wrap is toggled: the editor's options are read again from it.
pub fn key(ui: Ui, editor: RwSignal<Editor>, o: Memo<Options>, key: &Key, mods: Modifiers) -> bool {
    let only = |m: Modifiers| mods & (Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT | Modifiers::META) == m;
    let is = |k: &str| matches!(key, Key::Character(c) if c.eq_ignore_ascii_case(k));
    if only(Modifiers::ALT) && is("z") {
        toggle_wrap(ui);
        return true;
    }
    only(Modifiers::CONTROL)
        && (is("c") || is("x"))
        && !o.with_untracked(|o| o.empty_selection_copies)
        && editor
            .with_untracked(|e| e.cursor.with_untracked(|c| matches!(&c.mode, CursorMode::Insert(s) if s.is_caret())))
}

/// Toggle Word Wrap: the file on screen wraps, or stops wrapping, whatever its setting says.
fn toggle_wrap(ui: Ui) {
    let Some(path) = ui.live.with_untracked(|l| l.as_ref().map(|(p, _)| p.clone())) else { return };
    ui.state::<Zoom>().toggled.update(|t| {
        if !t.remove(&path) {
            t.insert(path);
        }
    });
}

/// A boolean setting of the person's turned over, written to their settings.json.
fn toggle(send: &Send, key: &str, now: bool) {
    send(json!({"type": "set_setting", "key": key, "value": !now}));
}

/// What it adds to the window's registries, once at start: the View commands of the editor's options.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    let file = |ui: Ui| ui.live.with_untracked(|l| l.is_some());
    add(command(TOGGLE_WORD_WRAP, "View", "Toggle Word Wrap", |ui, _| toggle_wrap(ui)).keys("Alt+Z").when(file));
    add(command("editor.action.toggleRenderWhitespace", "View", "Toggle Render Whitespace", |ui, send| {
        let shown = settings::now(ui, "editor.renderWhitespace", None).as_str() != Some("none");
        send(
            json!({"type": "set_setting", "key": "editor.renderWhitespace", "value": if shown { "none" } else { "all" }}),
        );
    }));
    add(command("editor.action.toggleMinimap", "View", "Toggle Minimap", |ui, send| {
        toggle(send, "editor.minimap.enabled", settings::now(ui, "editor.minimap.enabled", None) == Value::Bool(true));
    }));
    add(command("editor.action.toggleStickyScroll", "View", "Toggle Sticky Scroll", |ui, send| {
        let on = settings::now(ui, "editor.stickyScroll.enabled", None) == Value::Bool(true);
        toggle(send, "editor.stickyScroll.enabled", on);
    }));
    add(command("breadcrumbs.toggle", "View", "Toggle Breadcrumbs", |ui, send| {
        toggle(send, "breadcrumbs.enabled", settings::now(ui, "breadcrumbs.enabled", None) == Value::Bool(true));
    }));
    add(command(ZOOM_IN, "View", "Editor Font Zoom In", |ui, _| zoom(ui, 1)));
    add(command(ZOOM_OUT, "View", "Editor Font Zoom Out", |ui, _| zoom(ui, -1)));
    add(command(ZOOM_RESET, "View", "Editor Font Zoom Reset", |ui, _| zoom(ui, 0)));
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// A window with the settings and this part in it, and what it tells the session.
    fn window() -> (Ui, Send, Rc<RefCell<Vec<Value>>>) {
        let ui = Ui::new();
        let told = Rc::new(RefCell::new(Vec::new()));
        let send: Send = {
            let to = told.clone();
            Rc::new(move |v: Value| to.borrow_mut().push(v))
        };
        ui.out.set(Some(send.clone()));
        settings::register(ui, &send);
        register(ui, &send);
        (ui, send, told)
    }

    /// The session says the person's settings, and those of languages.
    fn say(ui: Ui, send: &Send, values: Value, by_lang: Value) {
        let m = json!({"type": "settings", "values": values, "by_lang": by_lang, "sources": {}, "problems": [],
            "files": {"user": "/u/settings.json", "project": "/w/.code-arena/settings.json", "workspace": "/w/.vscode/settings.json"}});
        settings::take(ui, &m, send);
    }

    #[test]
    fn each_setting_is_read_as_vs_code_reads_it() {
        assert_eq!(font_size(&json!(20)), 20.0);
        assert_eq!(font_size(&json!(2)), 6.0);
        assert_eq!(font_size(&json!("big")), 13.0);
        assert_eq!(font_weight(&json!("bold")), 700);
        assert_eq!(font_weight(&json!("300")), 300);
        assert_eq!(font_weight(&json!(5000)), 1000);
        assert_eq!(font_weight(&Value::Null), 400);
        assert!(font_family(&json!("Fira Code")).starts_with("Fira Code, "));
        assert_eq!(font_family(&json!("  ")), *MONO_FONT);
        assert_eq!(line_height(&json!(0), 14.0), 19.0);
        assert_eq!(line_height(&json!(1.5), 14.0), 21.0);
        assert_eq!(line_height(&json!(24), 14.0), 24.0);
        assert_eq!(line_height(&json!(2), 3.0), 8.0);
        assert_eq!(wrap(&json!("off"), &json!(80)), Wrap::Off);
        assert_eq!(wrap(&json!("on"), &json!(80)), Wrap::On);
        assert_eq!(wrap(&json!("wordWrapColumn"), &json!(100)), Wrap::Column(100));
        assert_eq!(wrap(&json!("bounded"), &Value::Null), Wrap::Bounded(80));
        assert_eq!(toggled(Wrap::Column(80)), Wrap::Off);
        assert_eq!(toggled(Wrap::Off), Wrap::On);
        assert_eq!(rulers(&json!([120, {"column": 80, "color": "#f00"}, 80, -1])), vec![80, 120]);
        assert!(rulers(&json!("80")).is_empty());
        assert_eq!(whitespace(&json!("all")), RenderWhitespace::All);
        assert_eq!(whitespace(&json!("boundary")), RenderWhitespace::Boundary);
        assert_eq!(whitespace(&json!("trailing")), RenderWhitespace::Trailing);
        assert_eq!(whitespace(&json!("selection")), RenderWhitespace::None);
        assert_eq!(line_highlight(&json!("line")), (false, true));
        assert_eq!(line_highlight(&json!("all")), (true, true));
        assert_eq!(line_highlight(&json!("gutter")), (true, false));
        assert_eq!(line_highlight(&json!("none")), (false, false));
        assert_eq!(zoomed(10.0, 2), 12.0);
        assert_eq!(zoomed(10.0, -9), 5.0);
    }

    #[test]
    fn alt_z_toggles_the_files_wrap_and_copy_with_nothing_selected_follows_its_setting() {
        // A font to lay text out with, where the machine has none.
        crate::icons::load();
        let (ui, _, _) = window();
        let ed = floem::views::text_editor("one\ntwo\n");
        ui.live.set(Some(("a.rs".into(), ed.editor().clone())));
        let editor = ui.scope.create_rw_signal(ed.editor().clone());
        let (ui2, off) = (ui, ui.scope.create_rw_signal(false));
        let o = memo(ui, move || Some("a.rs".into()));
        let o2 = create_memo(move |_| Options { empty_selection_copies: !off.get(), ..read(ui2, Some("a.rs")) });
        let ch = |c: &str| Key::Character(c.into());
        assert!(key(ui, editor, o, &ch("z"), Modifiers::ALT));
        assert_eq!(read(ui, Some("a.rs")).wrap, Wrap::On);
        assert!(key(ui, editor, o, &ch("Z"), Modifiers::ALT));
        assert_eq!(read(ui, Some("a.rs")).wrap, Wrap::Off);
        assert!(!key(ui, editor, o, &ch("z"), Modifiers::ALT | Modifiers::SHIFT));
        // On (the default), the editor copies the caret's line; off, nothing is copied.
        assert!(!key(ui, editor, o, &ch("c"), Modifiers::CONTROL));
        assert!(!key(ui, editor, o2, &ch("c"), Modifiers::CONTROL));
        off.set(true);
        assert!(key(ui, editor, o2, &ch("c"), Modifiers::CONTROL) && key(ui, editor, o2, &ch("x"), Modifiers::CONTROL));
        assert!(!key(ui, editor, o2, &ch("v"), Modifiers::CONTROL));
    }

    #[test]
    fn hidden_line_numbers_are_drawn_in_the_gutters_own_colour() {
        let (ui, send, _) = window();
        say(ui, &send, json!({"editor.lineNumbers": "off"}), json!({}));
        let c = ui.colors.get_untracked();
        let (_, dim, accent, gutter, _) = parts(&read(ui, None), &c, 0.0);
        assert_eq!((dim, accent), (c.editor_bg, gutter));
        assert_ne!(dim, Color::TRANSPARENT);
    }

    #[test]
    fn a_font_size_between_pixels_is_drawn_at_the_nearest() {
        let (ui, send, _) = window();
        say(ui, &send, json!({"editor.fontSize": 15.5}), json!({}));
        assert_eq!(read(ui, None).font.size, 16.0);
        zoom(ui, -1);
        assert_eq!(read(ui, None).font.size, 14.0);
    }

    #[test]
    fn the_options_follow_the_settings_the_languages_own_first_the_zoom_and_the_files_toggled_wrap() {
        let (ui, send, _) = window();
        let rust = Some("src/a.rs".to_string());
        let path = ui.scope.create_rw_signal(rust.clone());
        let o = memo(ui, move || path.get());
        // Before the session says anything: the defaults.
        let first = o.get_untracked();
        assert_eq!((first.font.size, first.font.line_height, first.wrap), (13.0, 20.0, Wrap::Off));
        assert!(first.line_numbers && first.highlight_line && !first.highlight_gutter && first.rulers.is_empty());
        say(
            ui,
            &send,
            json!({"editor.fontSize": 20, "editor.fontWeight": "bold", "editor.lineHeight": 30, "editor.wordWrap": "on",
                "editor.rulers": [80], "editor.lineNumbers": "off", "editor.renderWhitespace": "all",
                "editor.renderLineHighlight": "all", "editor.cursorBlinking": "solid", "editor.guides.indentation": false,
                "editor.scrollBeyondLastLine": false, "editor.mouseWheelZoom": true, "editor.emptySelectionClipboard": false}),
            json!({"rust": {"editor.fontSize": 16, "editor.wordWrap": "bounded", "editor.wordWrapColumn": 60}}),
        );
        let now = o.get_untracked();
        assert_eq!((now.font.size, now.font.weight, now.font.line_height), (16.0, 700, 30.0));
        assert_eq!((now.wrap, now.rulers.clone()), (Wrap::Bounded(60), vec![80]));
        assert!(!now.line_numbers && now.whitespace == RenderWhitespace::All);
        assert!(now.highlight_gutter && now.highlight_line && now.solid_caret);
        assert!(!now.indent_guides && !now.scroll_beyond_last_line && now.wheel_zoom && !now.empty_selection_copies);
        // Another file, of no language block: the person's own.
        path.set(Some("notes.md".into()));
        assert_eq!((o.get_untracked().font.size, o.get_untracked().wrap), (20.0, Wrap::On));
        // The zoom grows the font and the line by a tenth a step; reset, they are the settings' again.
        zoom(ui, 1);
        zoom(ui, 1);
        assert_eq!((o.get_untracked().font.size, o.get_untracked().font.line_height), (24.0, 36.0));
        zoom(ui, 0);
        assert_eq!(o.get_untracked().font.size, 20.0);
        // Toggle Word Wrap is the file's: this one stops wrapping, the other wraps as before.
        ui.state::<Zoom>().toggled.update(|t| {
            t.insert("notes.md".into());
        });
        assert_eq!(o.get_untracked().wrap, Wrap::Off);
        path.set(rust);
        assert_eq!(o.get_untracked().wrap, Wrap::Bounded(60));
    }

    #[test]
    fn the_view_toggles_write_the_persons_setting_and_the_zoom_is_bounded() {
        let (ui, send, told) = window();
        say(ui, &send, json!({"editor.minimap.enabled": false, "editor.renderWhitespace": "selection"}), json!({}));
        for id in ["editor.action.toggleMinimap", "editor.action.toggleStickyScroll", "breadcrumbs.toggle"] {
            assert!(crate::commands::run(ui, &send, id), "{id}");
        }
        assert!(crate::commands::run(ui, &send, "editor.action.toggleRenderWhitespace"));
        {
            let told = told.borrow();
            let set: Vec<&Value> = told.iter().filter(|m| m["type"] == "set_setting").collect();
            assert_eq!(set[0], &json!({"type": "set_setting", "key": "editor.minimap.enabled", "value": true}));
            assert_eq!(set[1], &json!({"type": "set_setting", "key": "editor.stickyScroll.enabled", "value": false}));
            assert_eq!(set[2], &json!({"type": "set_setting", "key": "breadcrumbs.enabled", "value": false}));
            assert_eq!(set[3], &json!({"type": "set_setting", "key": "editor.renderWhitespace", "value": "none"}));
        }
        assert_eq!(crate::commands::keys_of(TOGGLE_WORD_WRAP), "Alt+Z");
        // With no file on screen, there is nothing to toggle.
        assert!(!crate::commands::run(ui, &send, TOGGLE_WORD_WRAP));
        for _ in 0..30 {
            assert!(crate::commands::run(ui, &send, ZOOM_IN));
        }
        assert_eq!(ui.state::<Zoom>().level.get_untracked(), 20);
        assert!(crate::commands::run(ui, &send, ZOOM_RESET));
        assert_eq!(ui.state::<Zoom>().level.get_untracked(), 0);
    }
}
