//! The terminal, as VS Code's: the session's own shell (the driver's terminal on `ca-pty`, the very one the page
//! gets), its output understood by `alacritty_terminal`'s emulator and drawn by the app in the theme's terminal
//! colours, the keys sent as xterm's, the screen fitted to the panel.

use std::cell::RefCell;
use std::rc::Rc;

use alacritty_terminal::event::{Event as TermEvent, EventListener as TermListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor, Processor, Rgb};
use floem::keyboard::{Key, Modifiers, NamedKey};
use serde_json::Value;

/// The native window's terminals are numbered from here, apart from the page's (which count from 1).
pub const FIRST_ID: u64 = 1001;

/// A run of cells drawn alike: its text, its colours (0xRRGGBB, the background none when it is the terminal's
/// own), bold, italic, underlined.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Run {
    pub text: String,
    pub fg: u32,
    pub bg: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// What the screen shows: its rows of runs, and where the caret is (none when hidden, or scrolled back).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Screen {
    pub rows: Vec<Vec<Run>>,
    pub cursor: Option<(usize, usize)>,
}

/// The theme's terminal colours: the 16 of ANSI, the foreground, the background, the caret.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub ansi: [u32; 16],
    pub fg: u32,
    pub bg: u32,
    pub cursor: u32,
}

impl Palette {
    /// VS Code's own terminal colours, from the theme's `terminal.*` ids (VS Code's defaults where it has none).
    pub fn of(json: &Value, dark: bool) -> Palette {
        const NAMES: [&str; 16] = [
            "Black",
            "Red",
            "Green",
            "Yellow",
            "Blue",
            "Magenta",
            "Cyan",
            "White",
            "BrightBlack",
            "BrightRed",
            "BrightGreen",
            "BrightYellow",
            "BrightBlue",
            "BrightMagenta",
            "BrightCyan",
            "BrightWhite",
        ];
        const DARK: [u32; 16] = [
            0x000000, 0xCD3131, 0x0DBC79, 0xE5E510, 0x2472C8, 0xBC3FBC, 0x11A8CD, 0xE5E5E5, 0x666666, 0xF14C4C,
            0x23D18B, 0xF5F543, 0x3B8EEA, 0xD670D6, 0x29B8DB, 0xE5E5E5,
        ];
        const LIGHT: [u32; 16] = [
            0x000000, 0xCD3131, 0x00BC00, 0x949800, 0x0451A5, 0xBC05BC, 0x0598BC, 0x555555, 0x666666, 0xCD3131,
            0x14CE14, 0xB5BA00, 0x0451A5, 0xBC05BC, 0x0598BC, 0xA5A5A5,
        ];
        let c = &json["colors"];
        let get =
            |id: &str| c[id].as_str().and_then(crate::colors::hex).map(|x| u32::from_be_bytes([0, x.r, x.g, x.b]));
        let mut ansi = if dark { DARK } else { LIGHT };
        for (i, n) in NAMES.iter().enumerate() {
            if let Some(v) = get(&format!("terminal.ansi{n}")) {
                ansi[i] = v;
            }
        }
        let fg = get("terminal.foreground").unwrap_or(if dark { 0xCCCCCC } else { 0x333333 });
        let bg = get("terminal.background").or_else(|| get("panel.background")).unwrap_or(if dark {
            0x1E1E1E
        } else {
            0xFFFFFF
        });
        let cursor = get("terminalCursor.foreground").unwrap_or(fg);
        Palette { ansi, fg, bg, cursor }
    }

    fn rgb(r: Rgb) -> u32 {
        u32::from_be_bytes([0, r.r, r.g, r.b])
    }

    /// One of xterm's 256 colours: the 16, the 6×6×6 cube, the 24 greys.
    fn indexed(&self, i: u8) -> u32 {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let v = |n: u8| if n == 0 { 0 } else { 55 + 40 * n as u32 };
                let n = i - 16;
                (v(n / 36) << 16) | (v((n / 6) % 6) << 8) | v(n % 6)
            }
            _ => {
                let g = 8 + 10 * (i as u32 - 232);
                (g << 16) | (g << 8) | g
            }
        }
    }

    fn colour(&self, c: AnsiColor, term_colours: &alacritty_terminal::term::color::Colors) -> u32 {
        match c {
            AnsiColor::Spec(rgb) => Palette::rgb(rgb),
            AnsiColor::Indexed(i) => term_colours[i as usize].map(Palette::rgb).unwrap_or_else(|| self.indexed(i)),
            AnsiColor::Named(n) => {
                if let Some(set) = term_colours[n] {
                    return Palette::rgb(set);
                }
                match n {
                    NamedColor::Foreground | NamedColor::BrightForeground => self.fg,
                    NamedColor::Background => self.bg,
                    NamedColor::Cursor => self.cursor,
                    NamedColor::DimForeground => dim(self.fg),
                    n if (n as usize) < 16 => self.ansi[n as usize],
                    n => {
                        // The dim ones (DimBlack…DimWhite) after the named.
                        let at = (n as usize).saturating_sub(NamedColor::DimBlack as usize).min(7);
                        dim(self.ansi[at])
                    }
                }
            }
        }
    }
}

fn dim(c: u32) -> u32 {
    let [_, r, g, b] = c.to_be_bytes();
    let d = |x: u8| (x as u32 * 2 / 3) as u8;
    u32::from_be_bytes([0, d(r), d(g), d(b)])
}

/// What the emulator asks to write back (an answer to a query of the program's, as where the caret is).
#[derive(Clone, Default)]
struct Answers(Rc<RefCell<Vec<String>>>);

impl TermListener for Answers {
    fn send_event(&self, e: TermEvent) {
        if let TermEvent::PtyWrite(s) = e {
            self.0.borrow_mut().push(s);
        }
    }
}

/// One terminal's emulator: the output fed in, its screen read out.
pub struct Emulator {
    term: Term<Answers>,
    parser: Processor,
    answers: Answers,
}

impl Emulator {
    pub fn new(columns: usize, lines: usize) -> Emulator {
        let answers = Answers::default();
        let config = Config { scrolling_history: 10_000, ..Config::default() };
        let term = Term::new(config, &TermSize::new(columns.max(2), lines.max(1)), answers.clone());
        Emulator { term, parser: Processor::new(), answers }
    }

    /// The program's output; what the emulator answers it, to be written back.
    pub fn feed(&mut self, text: &str) -> Vec<String> {
        self.parser.advance(&mut self.term, text.as_bytes());
        std::mem::take(&mut *self.answers.0.borrow_mut())
    }

    pub fn resize(&mut self, columns: usize, lines: usize) {
        self.term.resize(TermSize::new(columns.max(2), lines.max(1)));
    }

    /// Back into what scrolled off (more lines up), or forward.
    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn screen(&self, p: &Palette) -> Screen {
        let grid = self.term.grid();
        let offset = grid.display_offset() as i32;
        let colours = self.term.colors();
        let content = self.term.renderable_content();
        let cursor = (content.cursor.shape != CursorShape::Hidden && offset == 0)
            .then(|| (content.cursor.point.line.0.max(0) as usize, content.cursor.point.column.0));
        let mut rows = Vec::with_capacity(self.term.screen_lines());
        for l in 0..self.term.screen_lines() as i32 {
            let row = &grid[Line(l - offset)];
            let mut runs: Vec<Run> = Vec::new();
            for col in 0..self.term.columns() {
                let cell = &row[Column(col)];
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                let (mut fg, mut bg) = (p.colour(cell.fg, colours), p.colour(cell.bg, colours));
                if cell.flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if cell.flags.contains(Flags::DIM) {
                    fg = dim(fg);
                }
                let bold = cell.flags.contains(Flags::BOLD);
                // Bold in one of the first 8 is drawn bright, as xterm (and VS Code) draw it.
                if bold
                    && let AnsiColor::Named(n) = cell.fg
                    && (n as usize) < 8
                {
                    fg = p.ansi[n as usize + 8];
                }
                let ch = if cell.flags.contains(Flags::HIDDEN) { ' ' } else { cell.c };
                let run = Run {
                    text: String::new(),
                    fg,
                    bg: (bg != p.bg).then_some(bg),
                    bold,
                    italic: cell.flags.contains(Flags::ITALIC),
                    underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                };
                // The caret's cell a run of its own, so it can be drawn over.
                let at_cursor = cursor == Some((l as usize, col));
                match runs.last_mut() {
                    Some(last)
                        if !at_cursor
                            && !last.text.is_empty()
                            && (last.fg, last.bg, last.bold, last.italic, last.underline)
                                == (run.fg, run.bg, run.bold, run.italic, run.underline)
                            && cursor != Some((l as usize, col.saturating_sub(1))) =>
                    {
                        last.text.push(ch)
                    }
                    _ => runs.push(Run { text: ch.to_string(), ..run }),
                }
            }
            // Trailing blanks of the terminal's own background are not drawn.
            while runs.last().is_some_and(|r| r.bg.is_none() && r.text.trim().is_empty() && !r.underline)
                && cursor.is_none_or(|(cl, _)| cl != l as usize)
            {
                runs.pop();
            }
            rows.push(runs);
        }
        Screen { rows, cursor }
    }
}

/// A key as xterm sends it (none: a key the terminal does not take, as Ctrl+Shift+C).
pub fn key_bytes(key: &Key, text: Option<&str>, m: Modifiers, mode: TermMode) -> Option<String> {
    let ctrl = m.contains(Modifiers::CONTROL);
    let alt = m.contains(Modifiers::ALT);
    let app = mode.contains(TermMode::APP_CURSOR);
    let arrow = |c: char| Some(if app { format!("\x1bO{c}") } else { format!("\x1b[{c}") });
    let s = match key {
        Key::Named(NamedKey::Enter) => Some("\r".to_string()),
        Key::Named(NamedKey::Backspace) => Some(if ctrl { "\x08" } else { "\x7f" }.to_string()),
        Key::Named(NamedKey::Tab) => Some(if m.contains(Modifiers::SHIFT) { "\x1b[Z" } else { "\t" }.to_string()),
        Key::Named(NamedKey::Escape) => Some("\x1b".to_string()),
        Key::Named(NamedKey::ArrowUp) => arrow('A'),
        Key::Named(NamedKey::ArrowDown) => arrow('B'),
        Key::Named(NamedKey::ArrowRight) => arrow('C'),
        Key::Named(NamedKey::ArrowLeft) => arrow('D'),
        Key::Named(NamedKey::Home) => arrow('H'),
        Key::Named(NamedKey::End) => arrow('F'),
        Key::Named(NamedKey::Delete) => Some("\x1b[3~".to_string()),
        Key::Named(NamedKey::Insert) => Some("\x1b[2~".to_string()),
        Key::Named(NamedKey::PageUp) => Some("\x1b[5~".to_string()),
        Key::Named(NamedKey::PageDown) => Some("\x1b[6~".to_string()),
        Key::Named(NamedKey::Space) => Some(if ctrl { "\0" } else { " " }.to_string()),
        Key::Character(c) if ctrl && !m.contains(Modifiers::SHIFT) => {
            let ch = c.chars().next()?.to_ascii_lowercase();
            match ch {
                'a'..='z' => Some(((ch as u8 - b'a' + 1) as char).to_string()),
                '[' => Some("\x1b".into()),
                '\\' => Some("\x1c".into()),
                ']' => Some("\x1d".into()),
                _ => None,
            }
        }
        Key::Character(_) if ctrl => None,
        _ => text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)).map(str::to_string),
    }?;
    Some(if alt && !s.starts_with('\x1b') { format!("\x1b{s}") } else { s })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        Palette::of(&serde_json::json!({"colors": {}}), true)
    }

    #[test]
    fn output_is_drawn_in_its_colours_and_the_caret_where_it_is() {
        let mut e = Emulator::new(20, 3);
        e.feed("ok \x1b[31mred\x1b[0m\r\n$ ");
        let s = e.screen(&palette());
        let first: Vec<_> = s.rows[0].iter().map(|r| (r.text.as_str(), r.fg)).collect();
        assert_eq!(first, [("ok ", 0xCCCCCC), ("red", 0xCD3131)]);
        assert_eq!(s.cursor, Some((1, 2)));
        assert_eq!(s.rows[1][0].text, "$ ");
    }

    #[test]
    fn a_program_asking_where_the_caret_is_gets_its_answer() {
        let mut e = Emulator::new(20, 3);
        assert_eq!(e.feed("ab\x1b[6n"), ["\x1b[1;3R"]);
    }

    #[test]
    fn keys_as_xterm_sends_them() {
        let none = Modifiers::empty();
        let k = |c: &str| Key::Character(c.into());
        assert_eq!(key_bytes(&k("c"), Some("c"), Modifiers::CONTROL, TermMode::empty()).as_deref(), Some("\x03"));
        assert_eq!(key_bytes(&Key::Named(NamedKey::ArrowUp), None, none, TermMode::empty()).as_deref(), Some("\x1b[A"));
        assert_eq!(
            key_bytes(&Key::Named(NamedKey::ArrowUp), None, none, TermMode::APP_CURSOR).as_deref(),
            Some("\x1bOA")
        );
        assert_eq!(key_bytes(&k("C"), Some("C"), Modifiers::CONTROL | Modifiers::SHIFT, TermMode::empty()), None);
        assert_eq!(key_bytes(&k("é"), Some("é"), none, TermMode::empty()).as_deref(), Some("é"));
    }

    #[test]
    fn xterms_256_colours() {
        let p = palette();
        assert_eq!(p.indexed(196), 0xFF0000);
        assert_eq!(p.indexed(232), 0x080808);
        assert_eq!(p.indexed(1), 0xCD3131);
    }
}

// ---- The view, and what keeps the terminal while the panel shows another view. ----

use floem::event::{Event, EventListener, EventPropagation};
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, create_rw_signal};
use floem::views::{Decorators, dyn_stack, h_stack, label, v_stack};
use floem::{IntoView, View};
use serde_json::json;

use crate::state::{Send, Ui};
use crate::widgets::MONO_FONT;

/// The terminal's cell, in pixels (VS Code's: 14 px text, its line 17 px high).
const FONT: f32 = 14.0;
const LINE: f64 = 17.0;

thread_local! {
    static EMULATOR: RefCell<Option<(u64, Emulator)>> = const { RefCell::new(None) };
    static CELL_WIDTH: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

/// How wide a cell of the monospace font is.
fn cell_width() -> f64 {
    CELL_WIDTH.with(|w| {
        if w.get() == 0.0 {
            let family: Vec<floem::text::FamilyOwned> = floem::text::FamilyOwned::parse_list(&MONO_FONT).collect();
            let mut l = floem::text::TextLayout::new();
            let attrs = floem::text::Attrs::new().family(&family).font_size(FONT);
            l.set_text(&"M".repeat(40), floem::text::AttrsList::new(attrs));
            w.set((l.size().width / 40.0).max(1.0));
        }
        w.get()
    })
}

/// The terminal's state as the window shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Not started yet (it starts when the panel first shows it, at its size).
    None,
    Running(u64),
    /// The shell ended, with its code; a key starts another.
    Ended(u64, Option<i64>),
}

fn palette(ui: Ui) -> Palette {
    Palette::of(&ui.theme_json(), ui.colors.get_untracked().dark)
}

/// Output of a terminal of the window's (others are the page's, and not drawn here).
pub fn output(ui: Ui, id: u64, text: &str, send: &Send) {
    let answers = EMULATOR.with(|e| match &mut *e.borrow_mut() {
        Some((n, em)) if *n == id => Some(em.feed(text)),
        _ => None,
    });
    let Some(answers) = answers else { return };
    for a in answers {
        send(json!({"type": "term_in", "id": id, "data": a}));
    }
    redraw(ui);
}

pub fn ended(ui: Ui, id: u64, code: Option<i64>) {
    if ui.term.get_untracked() == State::Running(id) {
        ui.term.set(State::Ended(id, code));
    }
}

fn redraw(ui: Ui) {
    let p = palette(ui);
    if let Some(s) = EMULATOR.with(|e| e.borrow().as_ref().map(|(_, em)| em.screen(&p))) {
        ui.term_screen.set(s);
    }
}

/// A shell started at the panel's size (another number each time, so an old one's last output is not taken).
fn start(ui: Ui, send: &Send, columns: usize, lines: usize) {
    let id = match ui.term.get_untracked() {
        State::None => FIRST_ID,
        State::Running(n) | State::Ended(n, _) => n + 1,
    };
    EMULATOR.with(|e| *e.borrow_mut() = Some((id, Emulator::new(columns, lines))));
    ui.term.set(State::Running(id));
    send(json!({"type": "term_open", "id": id, "cols": columns, "rows": lines}));
    redraw(ui);
}

fn hex(c: u32) -> Color {
    let [_, r, g, b] = c.to_be_bytes();
    Color::rgb8(r, g, b)
}

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let focused = create_rw_signal(false);
    let size: RwSignal<(usize, usize)> = create_rw_signal((0, 0));
    let on_resize = {
        let send = send.clone();
        move |r: floem::kurbo::Rect| {
            let columns = ((r.width() - 20.0) / cell_width()).floor().max(2.0) as usize;
            let lines = ((r.height() - 4.0) / LINE).floor().max(1.0) as usize;
            if size.get_untracked() == (columns, lines) {
                return;
            }
            size.set((columns, lines));
            match ui.term.get_untracked() {
                State::None => start(ui, &send, columns, lines),
                State::Running(id) => {
                    EMULATOR.with(|e| {
                        if let Some((_, em)) = &mut *e.borrow_mut() {
                            em.resize(columns, lines);
                        }
                    });
                    send(json!({"type": "term_resize", "id": id, "cols": columns, "rows": lines}));
                    redraw(ui);
                }
                State::Ended(..) => {}
            }
        }
    };
    let keys = send.clone();
    let rows = dyn_stack(
        move || {
            let s = ui.term_screen.get();
            let cursor = s.cursor;
            s.rows.into_iter().enumerate().map(move |(i, r)| (i, r, cursor.filter(|c| c.0 == i))).collect::<Vec<_>>()
        },
        |(i, r, c): &(usize, Vec<Run>, Option<(usize, usize)>)| (*i, r.clone(), *c),
        move |(_, runs, cursor): (usize, Vec<Run>, Option<(usize, usize)>)| {
            let mut col = 0;
            let cells: Vec<(Run, bool)> = runs
                .into_iter()
                .map(|r| {
                    let here = cursor.is_some_and(|(_, c)| c == col);
                    col += r.text.chars().count();
                    (r, here)
                })
                .collect();
            h_stack((dyn_stack(
                move || cells.clone().into_iter().enumerate().collect::<Vec<_>>(),
                |(i, _)| *i,
                move |(_, (run, caret))| {
                    let text = run.text.clone();
                    label(move || text.clone()).style(move |s| {
                        let c = t.get();
                        let p_cursor = c.cursor;
                        let s = s.height(LINE).line_height(1.0).color(hex(run.fg)).selectable(false);
                        let s = match run.bg {
                            Some(bg) => s.background(hex(bg)),
                            None => s,
                        };
                        let s = s
                            .apply_if(run.bold, |s| s.font_weight(floem::text::Weight::BOLD))
                            .apply_if(run.italic, |s| s.font_style(floem::text::Style::Italic));
                        if caret {
                            if focused.get() {
                                s.background(p_cursor).color(c.terminal_bg)
                            } else {
                                s.border(1.0).border_color(p_cursor)
                            }
                        } else {
                            s
                        }
                    })
                },
            )
            .style(|s| s.flex_row()),))
            .style(|s| s.height(LINE))
        },
    )
    .style(|s| s.flex_col());
    let ended_line = label(move || match ui.term.get() {
        State::Ended(_, Some(code)) => {
            format!("The terminal process terminated with exit code: {code}. Press any key to start a new one.")
        }
        State::Ended(..) => "The terminal process terminated. Press any key to start a new one.".to_string(),
        _ => String::new(),
    })
    .style(move |s| {
        s.padding_top(6.0).color(t.get().desc).apply_if(!matches!(ui.term.get(), State::Ended(..)), |s| s.hide())
    });
    let body = v_stack((rows, ended_line));
    let id = body.id();
    body.keyboard_navigable()
        .on_click_stop(move |_| id.request_focus())
        .on_event_cont(EventListener::FocusGained, move |_| focused.set(true))
        .on_event_cont(EventListener::FocusLost, move |_| focused.set(false))
        .on_resize(on_resize)
        .on_event(EventListener::PointerWheel, move |e| {
            if let Event::PointerWheel(w) = e {
                let lines = (-w.delta.y / LINE).round() as i32;
                EMULATOR.with(|e| {
                    if let Some((_, em)) = &mut *e.borrow_mut() {
                        em.scroll(if lines == 0 { -w.delta.y.signum() as i32 } else { lines });
                    }
                });
                redraw(ui);
            }
            EventPropagation::Stop
        })
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            let m = k.modifiers;
            let ctrl = m.contains(Modifiers::CONTROL);
            // The workbench's own keys stay the workbench's, as VS Code skips them in the shell.
            if let Key::Character(c) = &k.key.logical_key
                && ctrl
                && matches!(c.to_lowercase().as_str(), "b" | "j" | "`" | "p" | "w")
            {
                return EventPropagation::Continue;
            }
            match ui.term.get_untracked() {
                State::Ended(..) => {
                    let (c, l) = size.get_untracked();
                    start(ui, &keys, c, l);
                    return EventPropagation::Stop;
                }
                State::None => return EventPropagation::Continue,
                State::Running(_) => {}
            }
            let State::Running(id) = ui.term.get_untracked() else { return EventPropagation::Continue };
            let mode = EMULATOR.with(|e| e.borrow().as_ref().map(|(_, em)| em.mode())).unwrap_or(TermMode::empty());
            // Ctrl+Shift+V pastes, as in VS Code's terminal (bracketed when the program asks for it).
            if ctrl
                && m.contains(Modifiers::SHIFT)
                && matches!(&k.key.logical_key, Key::Character(c) if c.eq_ignore_ascii_case("v"))
            {
                if let Ok(text) = floem::Clipboard::get_contents() {
                    let data = if mode.contains(TermMode::BRACKETED_PASTE) {
                        format!("\x1b[200~{text}\x1b[201~")
                    } else {
                        text.replace("\r\n", "\r").replace('\n', "\r")
                    };
                    keys(json!({"type": "term_in", "id": id, "data": data}));
                }
                return EventPropagation::Stop;
            }
            match key_bytes(&k.key.logical_key, k.key.text.as_deref(), m, mode) {
                Some(data) => {
                    // Typing goes back to the end, as in VS Code.
                    EMULATOR.with(|e| {
                        if let Some((_, em)) = &mut *e.borrow_mut() {
                            em.scroll(i32::MIN / 2);
                        }
                    });
                    keys(json!({"type": "term_in", "id": id, "data": data}));
                    EventPropagation::Stop
                }
                None => EventPropagation::Continue,
            }
        })
        .style(move |s| {
            let c = t.get();
            s.size_full()
                .padding_horiz(10.0)
                .padding_top(2.0)
                .font_family(MONO_FONT.clone())
                .font_size(FONT)
                .background(c.terminal_bg)
                .color(c.terminal_fg)
                .cursor(floem::style::CursorStyle::Text)
        })
}

/// The theme changed: the screen drawn again in its colours.
pub fn recolour(ui: Ui) {
    redraw(ui);
}
