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
use floem::text::{Attrs, AttrsList, FamilyOwned, TextLayout, Weight};
use floem::views::editor::Editor;
use floem::views::editor::core::cursor::CursorMode;
use floem::views::editor::text::{RenderWhitespace, WrapMethod};
use floem::views::Decorators;
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

/// VS Code's language id of a file, by its name, as a `[lang]` block of the settings names it.
pub fn language(path: &str) -> Option<&'static str> {
    if path.starts_with("user:") {
        // The person's settings, keybindings and snippets: JSON with comments.
        return Some("jsonc");
    }
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    match name.as_str() {
        "dockerfile" => return Some("dockerfile"),
        "makefile" | "gnumakefile" => return Some("makefile"),
        "cmakelists.txt" => return Some("cmake"),
        "settings.json" | "keybindings.json" | "tsconfig.json" | "jsconfig.json" => return Some("jsonc"),
        _ => {}
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e)?;
    Some(match ext {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "json" => "json",
        "jsonc" | "code-snippets" | "code-workspace" => "jsonc",
        "md" | "markdown" => "markdown",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" => "scss",
        "less" => "less",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "cs" => "csharp",
        "sh" | "bash" | "zsh" => "shellscript",
        "ps1" => "powershell",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "xml" | "svg" => "xml",
        "sql" => "sql",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "lua" => "lua",
        "dart" => "dart",
        "vue" => "vue",
        "txt" => "plaintext",
        _ => return None,
    })
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
    let lang = path.and_then(language);
    let z = ui.state::<Zoom>();
    let level = z.level.get();
    let size = font_size(&settings::setting(ui, "editor.fontSize", lang));
    let font = Font {
        family: font_family(&settings::setting(ui, "editor.fontFamily", lang)),
        size: zoomed(size, level),
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
    let wrap = match o.wrap {
        Wrap::Off => WrapMethod::None,
        Wrap::On => WrapMethod::EditorWidth,
        Wrap::Column(col) => WrapMethod::WrapColumn { col },
        Wrap::Bounded(col) => {
            let at = col as f64 * o.font.char_width();
            if width > 0.0 && width < at { WrapMethod::EditorWidth } else { WrapMethod::WrapColumn { col } }
        }
    };
    let (dim, accent) = if o.line_numbers { (c.line_number, c.line_number_active) } else { (Color::TRANSPARENT, Color::TRANSPARENT) };
    s.gutter_dim_color(dim)
        .gutter_accent_color(accent)
        .gutter_current_color(if o.highlight_gutter { c.line_highlight } else { c.editor_bg })
        .current_line_color(if o.highlight_line { c.line_highlight } else { Color::TRANSPARENT })
        .wrap_method(wrap)
        .render_white_space(o.whitespace)
        .indent_guide(o.indent_guides)
        .scroll_beyond_last_line(o.scroll_beyond_last_line)
}

/// An editor made to follow its options: its font and rulers laid out again when they change (the style is
/// `style`'s), its caret kept on when it does not blink, and Ctrl and the wheel zooming its font. The view given
/// is returned inside the one that takes the wheel.
pub fn follow(ui: Ui, view: impl IntoView + 'static, editor: &Editor, colours: Rc<Colours>, o: Memo<Options>) -> AnyView {
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

/// The editor's own keys of its options: with `editor.emptySelectionClipboard` off, Ctrl+C and Ctrl+X on a caret
/// with nothing selected are taken and do nothing.
pub fn key(editor: &Editor, o: &Options, key: &Key, mods: Modifiers) -> bool {
    let ctrl = mods.contains(Modifiers::CONTROL) && !mods.intersects(Modifiers::ALT | Modifiers::SHIFT);
    let copy = matches!(key, Key::Character(c) if c.eq_ignore_ascii_case("c") || c.eq_ignore_ascii_case("x"));
    ctrl && copy && !o.empty_selection_copies && editor.cursor.with_untracked(|c| matches!(&c.mode, CursorMode::Insert(s) if s.is_caret()))
}

/// A boolean setting of the person's turned over, written to their settings.json.
fn toggle(send: &Send, key: &str, now: bool) {
    send(json!({"type": "set_setting", "key": key, "value": !now}));
}

/// What it adds to the window's registries, once at start: the View commands of the editor's options.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    let file = |ui: Ui| ui.live.with_untracked(|l| l.is_some());
    add(command(TOGGLE_WORD_WRAP, "View", "Toggle Word Wrap", |ui, _| {
        let Some(path) = ui.live.with_untracked(|l| l.as_ref().map(|(p, _)| p.clone())) else { return };
        ui.state::<Zoom>().toggled.update(|t| {
            if !t.remove(&path) {
                t.insert(path);
            }
        });
    })
    .keys("Alt+Z")
    .when(file));
    add(command("editor.action.toggleRenderWhitespace", "View", "Toggle Render Whitespace", |ui, send| {
        let shown = settings::now(ui, "editor.renderWhitespace", None).as_str() != Some("none");
        send(json!({"type": "set_setting", "key": "editor.renderWhitespace", "value": if shown { "none" } else { "all" }}));
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
