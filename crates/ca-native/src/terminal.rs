//! The terminal, as VS Code's: the session's own shell (the driver's terminal on `ca-pty`, the very one the page
//! gets), its output understood by `alacritty_terminal`'s emulator and drawn by the app in the theme's terminal
//! colours, the keys sent as xterm's, the screen fitted to the panel. A window has several of them, each its own
//! emulator, listed at the panel's right when there are two or more; the mouse selects in the one shown.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use alacritty_terminal::event::{Event as TermEvent, EventListener as TermListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor, Processor, Rgb};
use floem::keyboard::{Key, Modifiers, NamedKey};
use serde_json::Value;

/// The native window's terminals are numbered from here, apart from the page's (which count from 1).
pub const FIRST_ID: u64 = 1001;

/// A run of cells drawn alike: its text, its colours (0xRRGGBB, the background none when it is the terminal's
/// own), bold, italic, underlined, and whether it is in the selection (drawn over the selection's background).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Run {
    pub text: String,
    pub fg: u32,
    pub bg: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub selected: bool,
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

    /// Back into what scrolled off (more lines up), or forward. Output that comes while it is scrolled back
    /// leaves the lines shown where they are (the emulator moves its offset with what scrolls off).
    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    /// To the end of the output; whether it was scrolled back.
    pub fn bottom(&mut self) -> bool {
        let back = self.term.grid().display_offset() != 0;
        self.term.scroll_display(Scroll::Bottom);
        back
    }

    /// A cell of the screen as shown (its row from the top, its column) in the emulator's own lines, which count
    /// what scrolled off as lines above 0. A place past the last column is the right side of the last cell.
    fn point(&self, line: usize, column: usize, side: Side) -> (Point, Side) {
        let l = line.min(self.term.screen_lines() - 1) as i32 - self.term.grid().display_offset() as i32;
        let last = self.term.columns() - 1;
        if column > last {
            (Point::new(Line(l), Column(last)), Side::Right)
        } else {
            (Point::new(Line(l), Column(column)), side)
        }
    }

    /// A selection begun at a cell: of cells (a drag), of the word there (a double-click), of its line (a triple).
    pub fn select(&mut self, kind: SelectionType, line: usize, column: usize, side: Side) {
        let (p, side) = self.point(line, column, side);
        self.term.selection = Some(Selection::new(kind, p, side));
    }

    /// The selection taken as far as a cell.
    pub fn drag(&mut self, line: usize, column: usize, side: Side) {
        let (p, side) = self.point(line, column, side);
        if let Some(s) = &mut self.term.selection {
            s.update(p, side);
        }
    }

    /// The text selected (none when nothing is: a click alone selects nothing).
    pub fn selected(&self) -> Option<String> {
        self.term.selection_to_string().filter(|s| !s.is_empty())
    }

    /// The selection dropped; whether there was one.
    pub fn deselect(&mut self) -> bool {
        self.term.selection.take().is_some()
    }

    /// VS Code's Terminal: Clear: what scrolled off and the lines of the screen are gone, the caret's line (the
    /// prompt) kept as the first. A program on the alternate screen (an editor, a pager) keeps its screen.
    pub fn clear(&mut self) {
        if self.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        self.term.selection = None;
        self.term.scroll_display(Scroll::Bottom);
        let at = self.term.grid().cursor.point;
        let (row, column) = (at.line.0.max(0) as usize, at.column.0);
        let mut text = String::new();
        // What is under the caret's line erased, the caret put back where it was.
        if row + 1 < self.term.screen_lines() {
            text.push_str(&format!("\x1b[{};1H\x1b[J\x1b[{};{}H", row + 2, row + 1, column + 1));
        }
        // The caret's line scrolled to the top and the caret with it, then what scrolled off dropped.
        if row > 0 {
            text.push_str(&format!("\x1b[{row}S\x1b[{row}A"));
        }
        text.push_str("\x1b[3J");
        self.feed(&text);
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
        let selection = self.term.selection.as_ref().and_then(|s| s.to_range(&self.term));
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
                    selected: selection.is_some_and(|s| s.contains(Point::new(Line(l - offset), Column(col)))),
                };
                // The caret's cell a run of its own, so it can be drawn over.
                let at_cursor = cursor == Some((l as usize, col));
                match runs.last_mut() {
                    Some(last)
                        if !at_cursor
                            && !last.text.is_empty()
                            && (last.fg, last.bg, last.bold, last.italic, last.underline, last.selected)
                                == (run.fg, run.bg, run.bold, run.italic, run.underline, run.selected)
                            && cursor != Some((l as usize, col.saturating_sub(1))) =>
                    {
                        last.text.push(ch)
                    }
                    _ => runs.push(Run { text: ch.to_string(), ..run }),
                }
            }
            // Trailing blanks of the terminal's own background are not drawn.
            while runs.last().is_some_and(|r| r.bg.is_none() && !r.selected && r.text.trim().is_empty() && !r.underline)
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

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages of a terminal (what it wrote, that it ended), taken in. Never taken away (false).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    match m["type"].as_str().unwrap_or("") {
        "term_out" => output(ui, m["id"].as_u64().unwrap_or(0), m["data"].as_str().unwrap_or(""), send),
        "term_exit" => ended(ui, m["id"].as_u64().unwrap_or(0), m["code"].as_i64()),
        _ => {}
    }
    false
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

    fn text(s: &Screen, row: usize) -> String {
        s.rows[row].iter().map(|r| r.text.as_str()).collect::<String>().trim_end().to_string()
    }

    #[test]
    fn a_drag_selects_cells_a_double_click_a_word_and_both_are_drawn_selected() {
        let mut e = Emulator::new(20, 3);
        e.feed("cargo build --all\r\nnext");
        assert_eq!(e.selected(), None);
        // A click alone selects nothing.
        e.select(SelectionType::Simple, 0, 2, Side::Left);
        assert_eq!(e.selected(), None);
        e.drag(0, 7, Side::Right);
        assert_eq!(e.selected().as_deref(), Some("rgo bu"));
        let s = e.screen(&palette());
        let first: Vec<_> = s.rows[0].iter().map(|r| (r.text.trim_end(), r.selected)).collect();
        assert_eq!(first, [("ca", false), ("rgo bu", true), ("ild --all", false)]);
        // Over two lines, the first to its end.
        e.drag(1, 1, Side::Right);
        assert_eq!(e.selected().as_deref(), Some("rgo build --all\nne"));
        e.select(SelectionType::Semantic, 0, 8, Side::Left);
        assert_eq!(e.selected().as_deref(), Some("build"));
        e.select(SelectionType::Lines, 1, 0, Side::Left);
        assert_eq!(e.selected().as_deref(), Some("next\n"));
        assert!(e.deselect());
        assert!(e.screen(&palette()).rows.iter().flatten().all(|r| !r.selected));
    }

    #[test]
    fn output_that_streams_leaves_what_was_scrolled_back_to_where_it_is() {
        let mut e = Emulator::new(20, 4);
        for i in 0..20 {
            e.feed(&format!("line {i}\r\n"));
        }
        e.scroll(5);
        let before = e.screen(&palette());
        assert_eq!(before.cursor, None);
        let top = text(&before, 0);
        for i in 20..60 {
            e.feed(&format!("line {i}\r\n"));
        }
        let after = e.screen(&palette());
        assert_eq!(text(&after, 0), top);
        assert_eq!(after.rows, before.rows);
        assert!(e.bottom());
        assert_eq!(text(&e.screen(&palette()), 2), "line 59");
    }

    #[test]
    fn a_selection_follows_its_text_as_the_output_scrolls() {
        let mut e = Emulator::new(20, 4);
        e.feed("one\r\ntwo\r\n");
        e.select(SelectionType::Semantic, 1, 1, Side::Left);
        e.feed("three\r\nfour\r\nfive\r\n");
        assert_eq!(e.selected().as_deref(), Some("two"));
    }

    #[test]
    fn clear_keeps_the_prompt_and_drops_the_rest() {
        let mut e = Emulator::new(20, 4);
        for i in 0..10 {
            e.feed(&format!("line {i}\r\n"));
        }
        e.feed("$ ls");
        e.scroll(3);
        e.clear();
        let s = e.screen(&palette());
        assert_eq!(text(&s, 0), "$ ls");
        assert!((1..4).all(|r| text(&s, r).is_empty()));
        assert_eq!(s.cursor, Some((0, 4)));
        // Nothing is left to scroll back to.
        e.scroll(5);
        assert_eq!(text(&e.screen(&palette()), 0), "$ ls");
        // The prompt on the first line, with output under it (a program drew below the caret).
        let mut e = Emulator::new(20, 4);
        e.feed("$ \x1b7\r\nunder\x1b8");
        e.clear();
        let s = e.screen(&palette());
        assert_eq!((text(&s, 0), text(&s, 1), s.cursor), ("$".to_string(), String::new(), Some((0, 2))));
    }

    /// What a redraw costs: the screen of a 200×60 terminal full of coloured text read out, as each output and
    /// each move of a drag reads it. Printed (`--nocapture`), and bounded loosely so a slow machine passes.
    #[test]
    fn the_screen_of_a_large_terminal_is_read_out_fast() {
        let mut e = Emulator::new(200, 60);
        for l in 0..200 {
            let mut line = String::new();
            for w in 0..40 {
                line.push_str(&format!("\x1b[{}mw{:03} ", 31 + (l + w) % 7, w));
            }
            e.feed(&format!("{line}\r\n"));
        }
        e.select(SelectionType::Simple, 5, 10, Side::Left);
        e.drag(40, 100, Side::Right);
        let p = palette();
        let rounds = 200;
        let from = std::time::Instant::now();
        let mut runs = 0;
        for _ in 0..rounds {
            runs += e.screen(&p).rows.iter().map(Vec::len).sum::<usize>();
        }
        let each = from.elapsed() / rounds;
        println!("screen() of 200x60, {} runs: {each:?}", runs / rounds as usize);
        assert!(each < std::time::Duration::from_millis(50), "{each:?}");
    }

    #[test]
    fn xterms_256_colours() {
        let p = palette();
        assert_eq!(p.indexed(196), 0xFF0000);
        assert_eq!(p.indexed(232), 0x080808);
        assert_eq!(p.indexed(1), 0xCD3131);
    }
}

// ---- The view, and what keeps the terminals while the panel shows another view. ----

use floem::event::{Event, EventListener, EventPropagation};
use floem::peniko::Color;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::style::CursorStyle;
use floem::views::{Decorators, container, dyn_stack, h_stack, label, v_stack};
use floem::{IntoView, View};
use serde_json::json;

use crate::icons::icon;
use crate::state::{Send, Ui};
use crate::widgets::{MONO_FONT, action};

/// The terminal's cell, in pixels (VS Code's: 14 px text, its line 17 px high).
const FONT: f32 = 14.0;
const LINE: f64 = 17.0;
/// The room around the screen: 10 px at each side, 2 px above.
const PAD_X: f64 = 10.0;
const PAD_TOP: f64 = 2.0;
/// VS Code's terminal tabs list: 120 px wide, its rows 22 px high.
const TABS: f64 = 120.0;
const TAB: f64 = 22.0;
/// The size a terminal starts at when the panel has not been laid out yet (it is fitted once it is).
const UNFITTED: (usize, usize) = (80, 24);

thread_local! {
    /// The window's emulators, by their terminals' numbers.
    static EMULATORS: RefCell<HashMap<u64, Emulator>> = RefCell::new(HashMap::new());
    static CELL_WIDTH: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
    /// The theme's terminal colours, read from it once (a redraw is not to read the theme again).
    static PALETTE: RefCell<Option<Palette>> = const { RefCell::new(None) };
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

/// The state of the terminal the panel shows.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// None yet (the first starts when the panel first shows, at its size), or every one was killed.
    None,
    Running(u64),
    /// The shell ended, with its code; a key starts another in its place.
    Ended(u64, Option<i64>),
}

/// A terminal of the window's, as its row in the tabs list shows it: its number, the shell's name, and how it
/// ended once it has (with its code, when it gave one).
#[derive(Clone, Debug, PartialEq)]
pub struct Shell {
    pub id: u64,
    pub name: String,
    pub ended: Option<Option<i64>>,
}

impl Shell {
    fn state(&self) -> State {
        match self.ended {
            None => State::Running(self.id),
            Some(code) => State::Ended(self.id, code),
        }
    }
}

/// What a terminal's tab is called: the shell's name, as VS Code's ("bash", "zsh"), as the session says it.
fn shell_name(ui: Ui) -> String {
    let s = ui.shell.get_untracked();
    if s.is_empty() { "bash".to_string() } else { s }
}

fn palette(ui: Ui) -> Palette {
    PALETTE.with(|p| {
        p.borrow_mut().get_or_insert_with(|| Palette::of(&ui.theme_json(), ui.colors.get_untracked().dark)).clone()
    })
}

/// The terminal the panel shows.
fn shown(ui: Ui) -> Option<u64> {
    match ui.term.get_untracked() {
        State::None => None,
        State::Running(n) | State::Ended(n, _) => Some(n),
    }
}

/// Something done with a terminal's emulator (none: the window has no such terminal).
fn with<R>(id: u64, f: impl FnOnce(&mut Emulator) -> R) -> Option<R> {
    EMULATORS.with(|e| e.borrow_mut().get_mut(&id).map(f))
}

/// Output of a terminal of the window's (others are the page's, or one that was killed, and not drawn here).
pub fn output(ui: Ui, id: u64, text: &str, send: &Send) {
    let Some(answers) = with(id, |em| em.feed(text)) else { return };
    for a in answers {
        send(json!({"type": "term_in", "id": id, "data": a}));
    }
    if shown(ui) == Some(id) {
        redraw(ui);
    }
}

pub fn ended(ui: Ui, id: u64, code: Option<i64>) {
    if !ui.terms.with_untracked(|l| l.iter().any(|s| s.id == id && s.ended.is_none())) {
        return;
    }
    ui.terms.update(|l| {
        if let Some(s) = l.iter_mut().find(|s| s.id == id) {
            s.ended = Some(code);
        }
    });
    if shown(ui) == Some(id) {
        ui.term.set(State::Ended(id, code));
    }
}

/// The screen of the terminal shown, read out again (the rows that did not change are not drawn again).
fn redraw(ui: Ui) {
    let p = palette(ui);
    let s = shown(ui).and_then(|id| with(id, |em| em.screen(&p))).unwrap_or_default();
    if ui.term_screen.with_untracked(|old| *old != s) {
        ui.term_screen.set(s);
    }
}

/// A shell started at the panel's size and shown, last in the list or in the place of one that ended (another
/// number each time, so an old one's last output is not taken).
fn open(ui: Ui, send: &Send, over: Option<u64>) {
    let id = ui.term_next.get_untracked();
    ui.term_next.set(id + 1);
    let (columns, lines) = match ui.term_size.get_untracked() {
        (0, _) | (_, 0) => UNFITTED,
        size => size,
    };
    EMULATORS.with(|e| {
        let mut e = e.borrow_mut();
        if let Some(old) = over {
            e.remove(&old);
        }
        e.insert(id, Emulator::new(columns, lines));
    });
    if let Some(old) = over {
        send(json!({"type": "term_close", "id": old}));
    }
    let shell = Shell { id, name: shell_name(ui), ended: None };
    ui.terms.update(|l| match over.and_then(|old| l.iter().position(|s| s.id == old)) {
        Some(at) => l[at] = shell,
        None => l.push(shell),
    });
    ui.term.set(State::Running(id));
    send(json!({"type": "term_open", "id": id, "cols": columns, "rows": lines}));
    redraw(ui);
}

/// VS Code's New Terminal (Ctrl+Shift+`): another shell, shown and given the keys.
pub fn new_terminal(ui: Ui, send: &Send) {
    open(ui, send, None);
    ui.term_focus.update(|n| *n += 1);
}

/// A terminal of the list shown in the panel, and given the keys.
fn show(ui: Ui, id: u64) {
    let Some(state) = ui.terms.with_untracked(|l| l.iter().find(|s| s.id == id).map(Shell::state)) else { return };
    ui.term.set(state);
    redraw(ui);
    ui.term_focus.update(|n| *n += 1);
}

/// VS Code's Kill Terminal: the terminal shown is closed, and the one after it (or the last) shown.
pub fn kill(ui: Ui, send: &Send) {
    if let Some(id) = shown(ui) {
        close(ui, send, id);
    }
}

fn close(ui: Ui, send: &Send, id: u64) {
    let Some(at) = ui.terms.with_untracked(|l| l.iter().position(|s| s.id == id)) else { return };
    EMULATORS.with(|e| e.borrow_mut().remove(&id));
    send(json!({"type": "term_close", "id": id}));
    ui.terms.update(|l| {
        l.remove(at);
    });
    if shown(ui) != Some(id) {
        return;
    }
    match ui.terms.with_untracked(|l| l.get(at).or(l.last()).map(|s| s.id)) {
        Some(next) => show(ui, next),
        None => {
            ui.term.set(State::None);
            redraw(ui);
        }
    }
}

/// VS Code's Terminal: Clear, on the terminal shown.
pub fn clear(ui: Ui) {
    if let Some(id) = shown(ui) {
        with(id, Emulator::clear);
        redraw(ui);
    }
}

/// The selection of the terminal shown put on the clipboard; whether there was one.
fn copy(ui: Ui) -> bool {
    let Some(text) = shown(ui).and_then(|id| with(id, |em| em.selected())).flatten() else { return false };
    let _ = floem::Clipboard::set_contents(text);
    true
}

/// The clipboard's text typed into the shell (bracketed when the program asks for it).
fn paste(ui: Ui, send: &Send) {
    let State::Running(id) = ui.term.get_untracked() else { return };
    let Ok(text) = floem::Clipboard::get_contents() else { return };
    if text.is_empty() {
        return;
    }
    let mode = with(id, |em| em.mode()).unwrap_or(TermMode::empty());
    let data = if mode.contains(TermMode::BRACKETED_PASTE) {
        format!("\x1b[200~{text}\x1b[201~")
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r")
    };
    typed(ui, id);
    send(json!({"type": "term_in", "id": id, "data": data}));
}

/// Something typed into a terminal: it goes back to the end and its selection clears, as in VS Code.
fn typed(ui: Ui, id: u64) {
    if with(id, |em| em.deselect() | em.bottom()) == Some(true) {
        redraw(ui);
    }
}

fn hex(c: u32) -> Color {
    let [_, r, g, b] = c.to_be_bytes();
    Color::rgb8(r, g, b)
}

/// The cell under a place in the screen's view: its row, its column, and the half of the cell pointed at.
fn cell_at(p: floem::kurbo::Point) -> (usize, usize, Side) {
    let x = ((p.x - PAD_X) / cell_width()).max(0.0);
    let line = ((p.y - PAD_TOP) / LINE).floor().max(0.0) as usize;
    (line, x as usize, if x.fract() < 0.5 { Side::Left } else { Side::Right })
}

/// The panel's view: the screen of the terminal shown and, when there are two or more, the list of them at its
/// right.
pub fn view(ui: Ui, send: Send) -> impl IntoView {
    h_stack((screen_view(ui, send.clone()), tabs(ui, send))).style(|s| s.size_full())
}

/// VS Code's panel title actions for the terminal: New Terminal, Kill Terminal.
pub fn actions(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let new = send.clone();
    h_stack((action(t, "add", move || new_terminal(ui, &new)), action(t, "trash", move || kill(ui, &send))))
        .style(|s| s.items_center().gap(4.0))
}

/// VS Code's terminal tabs list: a row for each terminal (the terminal codicon, the shell's name), the one shown
/// lit, a trash action on the row pointed at.
fn tabs(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    dyn_stack(
        move || ui.terms.get(),
        |s: &Shell| (s.id, s.name.clone()),
        move |shell: Shell| {
            let id = shell.id;
            let name = shell.name;
            let over = create_rw_signal(false);
            let send = send.clone();
            let trash = container(icon("trash", 16.0)).on_click_stop(move |_| close(ui, &send, id)).style(move |s| {
                let c = t.get();
                s.width(20.0)
                    .height(20.0)
                    .flex_shrink(0.0_f32)
                    .items_center()
                    .justify_center()
                    .border_radius(5.0)
                    .color(c.icon)
                    .hover(|s| s.background(c.toolbar_hover))
                    .apply_if(!over.get(), |s| s.hide())
            });
            let text = label(move || name.clone())
                .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).text_ellipsis().selectable(false));
            h_stack((icon("terminal", 16.0), text, trash))
                .on_click_stop(move |_| show(ui, id))
                .on_event_cont(EventListener::PointerEnter, move |_| over.set(true))
                .on_event_cont(EventListener::PointerLeave, move |_| over.set(false))
                .style(move |s| {
                    let c = t.get();
                    let lit = matches!(ui.term.get(), State::Running(n) | State::Ended(n, _) if n == id);
                    s.height(TAB)
                        .width_full()
                        .items_center()
                        .gap(6.0)
                        .padding_left(10.0)
                        .padding_right(4.0)
                        .font_size(13.0)
                        .color(c.fg)
                        .cursor(CursorStyle::Pointer)
                        .apply_if(lit, |s| s.background(c.list_inactive))
                        .apply_if(!lit, |s| s.hover(|s| s.background(c.list_hover)))
                })
        },
    )
    .style(move |s| {
        let c = t.get();
        s.flex_col()
            .width(TABS)
            .height_full()
            .flex_shrink(0.0_f32)
            .border_left(1.0)
            .border_color(c.panel_border)
            .background(c.terminal_bg)
            .apply_if(ui.terms.with(|l| l.len() < 2), |s| s.hide())
    })
}

/// The screen of the terminal shown: its rows, the keys it takes, the mouse's selection.
fn screen_view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let focused = create_rw_signal(false);
    // The cell a drag has reached (none: no button is held).
    let drag = create_rw_signal(None::<(usize, usize, Side)>);
    let on_resize = {
        let send = send.clone();
        move |r: floem::kurbo::Rect| {
            if r.width() < 1.0 || r.height() < 1.0 {
                return;
            }
            let columns = ((r.width() - 2.0 * PAD_X) / cell_width()).floor().max(2.0) as usize;
            let lines = ((r.height() - 2.0 * PAD_TOP) / LINE).floor().max(1.0) as usize;
            if ui.term_size.get_untracked() == (columns, lines) {
                return;
            }
            ui.term_size.set((columns, lines));
            // The first terminal starts with the panel's first size; after that every one is fitted to it.
            if ui.term_next.get_untracked() == FIRST_ID {
                open(ui, &send, None);
                return;
            }
            for shell in ui.terms.get_untracked() {
                with(shell.id, |em| em.resize(columns, lines));
                if shell.ended.is_none() {
                    send(json!({"type": "term_resize", "id": shell.id, "cols": columns, "rows": lines}));
                }
            }
            redraw(ui);
        }
    };
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
                        let s = match (run.selected, run.bg) {
                            (true, _) => s.background(c.terminal_selection),
                            (_, Some(bg)) => s.background(hex(bg)),
                            _ => s,
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
    // The keys given to the terminal when one is made or chosen (not when the view is first drawn).
    create_effect(move |drawn: Option<()>| {
        ui.term_focus.get();
        if drawn.is_some() {
            id.request_focus();
        }
    });
    let (keys, mouse) = (send.clone(), send);
    body.keyboard_navigable()
        .on_event_cont(EventListener::FocusGained, move |_| focused.set(true))
        .on_event_cont(EventListener::FocusLost, move |_| focused.set(false))
        .on_resize(on_resize)
        .on_event(EventListener::PointerDown, move |e| {
            let Event::PointerDown(p) = e else { return EventPropagation::Continue };
            id.request_focus();
            let Some(shell) = shown(ui) else { return EventPropagation::Stop };
            // A right-click copies the selection when there is one (and drops it), and pastes when there is none.
            if p.button.is_secondary() {
                if copy(ui) {
                    with(shell, |em| em.deselect());
                    redraw(ui);
                } else {
                    paste(ui, &mouse);
                }
                return EventPropagation::Stop;
            }
            if !p.button.is_primary() {
                return EventPropagation::Continue;
            }
            // A press begins a selection of cells, a second one of the word there, a third of the line; the
            // moves that follow, wherever the pointer goes, take it further.
            let (line, column, side) = cell_at(p.pos);
            let kind = match p.count {
                2 => SelectionType::Semantic,
                3 => SelectionType::Lines,
                _ => SelectionType::Simple,
            };
            with(shell, |em| em.select(kind, line, column, side));
            drag.set(Some((line, column, side)));
            id.request_active();
            redraw(ui);
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::PointerMove, move |e| {
            let (Event::PointerMove(p), Some(last), Some(shell)) = (e, drag.get_untracked(), shown(ui)) else { return };
            let at = cell_at(p.pos);
            if at != last {
                drag.set(Some(at));
                with(shell, |em| em.drag(at.0, at.1, at.2));
                redraw(ui);
            }
        })
        .on_event_cont(EventListener::PointerUp, move |_| drag.set(None))
        .on_event(EventListener::PointerWheel, move |e| {
            if let (Event::PointerWheel(w), Some(shell)) = (e, shown(ui)) {
                let lines = (-w.delta.y / LINE).round() as i32;
                with(shell, |em| em.scroll(if lines == 0 { -w.delta.y.signum() as i32 } else { lines }));
                redraw(ui);
            }
            EventPropagation::Stop
        })
        .on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            let m = k.modifiers;
            let ctrl = m.contains(Modifiers::CONTROL);
            let letter = |l: &str| matches!(&k.key.logical_key, Key::Character(c) if c.eq_ignore_ascii_case(l));
            // The workbench's own keys stay the workbench's, as VS Code skips them in the shell.
            if crate::keys_hook::terminal(&k.key.logical_key, m) {
                return EventPropagation::Continue;
            }
            // Ctrl+Shift+C copies the selection, as in VS Code's terminal.
            if ctrl && m.contains(Modifiers::SHIFT) && letter("c") {
                copy(ui);
                return EventPropagation::Stop;
            }
            let held = matches!(
                &k.key.logical_key,
                Key::Named(NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Super | NamedKey::Meta)
            );
            let id = match ui.term.get_untracked() {
                State::Running(id) => id,
                _ if held => return EventPropagation::Continue,
                // Any key starts another shell in the place of one that ended, or when every one was killed.
                State::Ended(old, _) => {
                    open(ui, &keys, Some(old));
                    return EventPropagation::Stop;
                }
                State::None => {
                    open(ui, &keys, None);
                    return EventPropagation::Stop;
                }
            };
            // Ctrl+Shift+V pastes, as in VS Code's terminal.
            if ctrl && m.contains(Modifiers::SHIFT) && letter("v") {
                paste(ui, &keys);
                return EventPropagation::Stop;
            }
            let mode = with(id, |em| em.mode()).unwrap_or(TermMode::empty());
            match key_bytes(&k.key.logical_key, k.key.text.as_deref(), m, mode) {
                Some(data) => {
                    typed(ui, id);
                    keys(json!({"type": "term_in", "id": id, "data": data}));
                    EventPropagation::Stop
                }
                None => EventPropagation::Continue,
            }
        })
        .style(move |s| {
            let c = t.get();
            s.flex_grow(1.0_f32)
                .flex_basis(0.0)
                .min_width(0.0)
                .height_full()
                .padding_horiz(PAD_X)
                .padding_top(PAD_TOP)
                .font_family(MONO_FONT.clone())
                .font_size(FONT)
                .background(c.terminal_bg)
                .color(c.terminal_fg)
                .cursor(CursorStyle::Text)
        })
}

/// The theme changed: the screen drawn again in its colours.
pub fn recolour(ui: Ui) {
    PALETTE.with(|p| *p.borrow_mut() = None);
    redraw(ui);
}

#[cfg(test)]
mod window_tests {
    use super::*;
    use floem::reactive::SignalGet;

    fn first(ui: Ui) -> String {
        ui.term_screen
            .get_untracked()
            .rows
            .first()
            .map(|r| r.iter().map(|x| x.text.as_str()).collect::<String>().trim_end().to_string())
            .unwrap_or_default()
    }

    #[test]
    fn a_window_keeps_several_terminals_and_shows_one() {
        let ui = Ui::new();
        let sent = Rc::new(RefCell::new(Vec::new()));
        let send: Send = {
            let sent = sent.clone();
            Rc::new(move |v: serde_json::Value| sent.borrow_mut().push(v))
        };
        let of = |kind: &str| -> Vec<u64> {
            sent.borrow().iter().filter(|v| v["type"] == kind).filter_map(|v| v["id"].as_u64()).collect()
        };
        new_terminal(ui, &send);
        new_terminal(ui, &send);
        let (a, b) = (FIRST_ID, FIRST_ID + 1);
        assert_eq!(of("term_open"), [a, b]);
        assert_eq!(sent.borrow()[0]["cols"], 80);
        assert_eq!(ui.term.get_untracked(), State::Running(b));
        assert_eq!(ui.term_focus.get_untracked(), 2);
        // Each has its own screen; the output of the one not shown is kept for when it is.
        output(ui, a, "first", &send);
        assert_eq!(first(ui), "");
        output(ui, b, "second", &send);
        assert_eq!(first(ui), "second");
        show(ui, a);
        assert_eq!((ui.term.get_untracked(), first(ui)), (State::Running(a), "first".to_string()));
        // One that ends while another is shown says so once it is shown; a key's new shell takes its place.
        ended(ui, b, Some(3));
        assert_eq!(ui.term.get_untracked(), State::Running(a));
        show(ui, b);
        assert_eq!(ui.term.get_untracked(), State::Ended(b, Some(3)));
        open(ui, &send, Some(b));
        let c = FIRST_ID + 2;
        assert_eq!(ui.terms.get_untracked().iter().map(|s| s.id).collect::<Vec<_>>(), [a, c]);
        assert_eq!(ui.term.get_untracked(), State::Running(c));
        // Clear is the shown one's, and what was typed stays the first line.
        output(ui, c, "one\r\ntwo\r\n$ ", &send);
        clear(ui);
        assert_eq!(first(ui), "$");
        // Killed: closed in the session, its late output not taken, the one beside it shown.
        kill(ui, &send);
        assert_eq!(of("term_close"), [b, c]);
        output(ui, c, "late", &send);
        assert_eq!((ui.term.get_untracked(), first(ui)), (State::Running(a), "first".to_string()));
        kill(ui, &send);
        assert_eq!(ui.term.get_untracked(), State::None);
        assert!(ui.terms.get_untracked().is_empty());
        assert_eq!(ui.term_screen.get_untracked(), Screen::default());
        // Another after every one was killed takes a number none had.
        new_terminal(ui, &send);
        assert_eq!(ui.term.get_untracked(), State::Running(FIRST_ID + 3));
    }
}
