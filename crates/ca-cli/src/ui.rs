//! What the program says on the terminal: progress on stderr (dimmed, coloured when it is a terminal), the answer
//! on stdout.

use std::io::{IsTerminal, Write};

#[derive(Debug, Clone, Copy)]
pub struct Ui {
    pub color: bool,
}

impl Ui {
    pub fn new(color_setting: bool, no_color_env: bool) -> Self {
        Self { color: color_setting && !no_color_env && std::io::stderr().is_terminal() }
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

    /// A line of progress on stderr.
    pub fn say(&self, text: &str) {
        let mut e = std::io::stderr().lock();
        let _ = writeln!(e, "{text}");
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
}
