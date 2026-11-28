//! What the program says on the terminal: progress on stderr (dimmed, coloured when it is a terminal), the answer
//! on stdout. Where there is no terminal to say it on (the IDE), a [`Capture`] collects the lines instead, and
//! whoever asked for it says them its own way (a notice in the thread).

use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

/// Whether the terminal is in raw mode (the chat): lines must then end with `\r\n`.
static RAW: AtomicBool = AtomicBool::new(false);

pub fn set_raw(on: bool) {
    RAW.store(on, Ordering::SeqCst);
}

/// Text as the terminal needs it now: `\n` as `\r\n` in raw mode.
pub fn term(text: &str) -> std::borrow::Cow<'_, str> {
    if RAW.load(Ordering::SeqCst) {
        std::borrow::Cow::Owned(text.replace("\r\n", "\n").replace('\n', "\r\n"))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// Writes to stdout as the terminal needs it.
pub fn out(text: &str) {
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(term(text).as_bytes());
    let _ = o.flush();
}

#[derive(Debug, Clone, Copy)]
pub struct Ui {
    pub color: bool,
    /// Which [`Capture`] takes what is said (0: none, it goes to stderr).
    capture: u32,
}

/// What each capture was told so far, by its number. A number and not the lines themselves, so that a `Ui` stays
/// a plain copyable value that closures and other threads may hold.
fn captured() -> &'static Mutex<HashMap<u32, Vec<String>>> {
    static ALL: OnceLock<Mutex<HashMap<u32, Vec<String>>>> = OnceLock::new();
    ALL.get_or_init(Mutex::default)
}

/// A `Ui` whose lines are kept instead of written to the terminal, for as long as this lives: what a command
/// written for the terminal says can then be shown elsewhere. Its `ui` has no colours.
#[allow(dead_code)] // For the IDE's commands: none is answered through it yet.
pub struct Capture {
    pub ui: Ui,
}

#[allow(dead_code)] // As `Capture`.
impl Capture {
    /// The lines said so far, taken out (a second call gives what was said since).
    pub fn lines(&self) -> Vec<String> {
        captured().lock().ok().and_then(|mut all| all.get_mut(&self.ui.capture).map(std::mem::take)).unwrap_or_default()
    }

    /// The lines said so far as one text, taken out.
    pub fn text(&self) -> String {
        self.lines().join("\n")
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if let Ok(mut all) = captured().lock() {
            all.remove(&self.ui.capture);
        }
    }
}

impl Ui {
    pub fn new(color_setting: bool, no_color_env: bool) -> Self {
        Self { color: color_setting && !no_color_env && std::io::stderr().is_terminal(), capture: 0 }
    }

    /// A `Ui` that keeps what it is told ([`Capture::lines`]) instead of writing it to the terminal.
    #[allow(dead_code)] // As `Capture`.
    pub fn capture() -> Capture {
        static NEXT: AtomicU32 = AtomicU32::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed).max(1);
        if let Ok(mut all) = captured().lock() {
            all.insert(id, Vec::new());
        }
        Capture { ui: Ui { color: false, capture: id } }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() }
    }

    pub fn dim(&self, text: &str) -> String {
        self.paint("2", text)
    }
    pub fn bold(&self, text: &str) -> String {
        self.paint("1", text)
    }
    pub fn green(&self, text: &str) -> String {
        self.paint("32", text)
    }
    pub fn red(&self, text: &str) -> String {
        self.paint("31", text)
    }
    pub fn yellow(&self, text: &str) -> String {
        self.paint("33", text)
    }

    /// A line of progress on stderr (or to the capture this `Ui` belongs to; nowhere once that capture is gone).
    pub fn say(&self, text: &str) {
        if self.capture != 0 {
            if let Ok(mut all) = captured().lock()
                && let Some(lines) = all.get_mut(&self.capture)
            {
                lines.push(text.to_string());
            }
            return;
        }
        crate::log::line(text);
        let mut e = std::io::stderr().lock();
        let _ = e.write_all(term(&format!("{text}\n")).as_bytes());
        let _ = e.flush();
    }
}

/// 32768 → "32,768".
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 12400 → "12.4k".
pub fn short(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1_000.0).replace(".0k", "k"),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0).replace(".0M", "M"),
    }
}

/// Asks a question on the terminal and reads a line.
pub fn ask_line(question: &str) -> std::io::Result<String> {
    let mut e = std::io::stderr().lock();
    write!(e, "{question}")?;
    e.flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_as_people_read_them() {
        assert_eq!(thousands(32768), "32,768");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000_000), "1,000,000");
        assert_eq!(short(12_400), "12.4k");
        assert_eq!(short(131_000), "131k");
        assert_eq!(short(1_500_000), "1.5M");
    }

    #[test]
    fn a_capture_keeps_what_is_said_plain_and_each_its_own() {
        let (one, two) = (Ui::capture(), Ui::capture());
        let ui = one.ui;
        // A copy in another thread says to the same capture.
        std::thread::spawn(move || ui.say(&ui.red("from a thread"))).join().expect("joined");
        one.ui.say(&one.ui.bold("second"));
        two.ui.say("the other");
        assert_eq!(one.lines(), ["from a thread", "second"], "no colours, in order");
        assert_eq!(one.lines(), Vec::<String>::new(), "taken out once");
        one.ui.say("third");
        assert_eq!(one.text(), "third");
        assert_eq!(two.text(), "the other");
        // Once the capture is gone, what its `Ui` is told goes nowhere (and not to the terminal).
        let late = two.ui;
        drop(two);
        late.say("lost");
        assert!(captured().lock().expect("lock").get(&late.capture).is_none());
    }
}
