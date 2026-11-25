//! The prompt's line editor, as a shell's is: a state machine over keys, drawn by the chat. Kept apart from the
//! terminal so every key can be tested.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a key did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing to do but draw again.
    Edited,
    /// Enter on a message: send it.
    Submit(String),
    /// Ctrl+C on a line with text: it is cleared.
    Cleared,
    /// Ctrl+C on an empty line (twice leaves), or Ctrl+D on an empty line.
    Interrupt,
    Exit,
    /// Ctrl+L.
    ClearScreen,
    /// Ctrl+Z: stop the program, as a shell does.
    Suspend,
    /// A key that does nothing here.
    Ignored,
}

#[derive(Debug, Default, Clone)]
pub struct Editor {
    text: Vec<char>,
    cursor: usize,
    /// What was sent in this folder, oldest first.
    history: Vec<String>,
    /// Where ↑ has gone back to, from the end.
    back: Option<usize>,
    /// What was being typed before ↑ went back.
    draft: Vec<char>,
}

impl Editor {
    pub fn new(history: Vec<String>) -> Self {
        Self { history, ..Default::default() }
    }

    pub fn text(&self) -> String {
        self.text.iter().collect()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.chars().collect();
        self.cursor = self.text.len();
        self.back = None;
    }

    pub fn remember(&mut self, line: &str) {
        if self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.to_string());
        }
    }

    /// Text pasted: its line breaks stay line breaks, never sending.
    pub fn paste(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        for c in s.chars() {
            self.insert(c);
        }
    }

    fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += 1;
    }

    fn word_left(&self) -> usize {
        let mut i = self.cursor;
        while i > 0 && !self.text[i - 1].is_alphanumeric() {
            i -= 1;
        }
        while i > 0 && self.text[i - 1].is_alphanumeric() {
            i -= 1;
        }
        i
    }

    fn word_right(&self) -> usize {
        let mut i = self.cursor;
        while i < self.text.len() && !self.text[i].is_alphanumeric() {
            i += 1;
        }
        while i < self.text.len() && self.text[i].is_alphanumeric() {
            i += 1;
        }
        i
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].iter().rposition(|c| *c == '\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..].iter().position(|c| *c == '\n').map(|i| self.cursor + i).unwrap_or(self.text.len())
    }

    /// ↑ in a message of several lines moves between its lines first; on the first line it goes back in history.
    fn up(&mut self) {
        let start = self.line_start();
        if start > 0 {
            let col = self.cursor - start;
            let prev_start = self.text[..start - 1].iter().rposition(|c| *c == '\n').map(|i| i + 1).unwrap_or(0);
            self.cursor = (prev_start + col).min(start - 1);
            return;
        }
        let n = self.history.len();
        let next = match self.back {
            None if n > 0 => {
                self.draft = self.text.clone();
                0
            }
            None => return,
            Some(b) if b + 1 < n => b + 1,
            Some(_) => return,
        };
        self.back = Some(next);
        self.text = self.history[n - 1 - next].chars().collect();
        self.cursor = self.text.len();
    }

    fn down(&mut self) {
        let end = self.line_end();
        if end < self.text.len() {
            let col = self.cursor - self.line_start();
            let next_start = end + 1;
            let next_end = self.text[next_start..]
                .iter()
                .position(|c| *c == '\n')
                .map(|i| next_start + i)
                .unwrap_or(self.text.len());
            self.cursor = (next_start + col).min(next_end);
            return;
        }
        match self.back {
            None => {}
            Some(0) => self.back_to_draft(),
            Some(b) => {
                self.back = Some(b - 1);
                let n = self.history.len();
                self.text = self.history[n - b].chars().collect();
                self.cursor = self.text.len();
            }
        }
    }

    fn back_to_draft(&mut self) {
        self.back = None;
        self.text = std::mem::take(&mut self.draft);
        self.cursor = self.text.len();
    }

    pub fn key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Enter if alt || k.modifiers.contains(KeyModifiers::SHIFT) => self.insert('\n'),
            KeyCode::Enter => {
                // A line ending in \ goes on to the next.
                if self.cursor == self.text.len() && self.text.last() == Some(&'\\') {
                    self.text.pop();
                    self.cursor -= 1;
                    self.insert('\n');
                    return Action::Edited;
                }
                let line = self.text();
                self.text.clear();
                self.cursor = 0;
                self.back = None;
                self.draft.clear();
                if line.trim().is_empty() {
                    return Action::Edited;
                }
                return Action::Submit(line);
            }
            KeyCode::Char('c') if ctrl => {
                if self.text.is_empty() {
                    return Action::Interrupt;
                }
                self.text.clear();
                self.cursor = 0;
                self.back = None;
                return Action::Cleared;
            }
            KeyCode::Char('d') if ctrl => {
                if self.text.is_empty() {
                    return Action::Exit;
                }
                if self.cursor < self.text.len() {
                    self.text.remove(self.cursor);
                }
            }
            KeyCode::Char('l') if ctrl => return Action::ClearScreen,
            KeyCode::Char('z') if ctrl => return Action::Suspend,
            KeyCode::Char('a') if ctrl => self.cursor = self.line_start(),
            KeyCode::Char('e') if ctrl => self.cursor = self.line_end(),
            KeyCode::Char('u') if ctrl => {
                let s = self.line_start();
                self.text.drain(s..self.cursor);
                self.cursor = s;
            }
            KeyCode::Char('k') if ctrl => {
                let e = self.line_end();
                self.text.drain(self.cursor..e);
            }
            KeyCode::Char('w') if ctrl => {
                let s = self.word_left();
                self.text.drain(s..self.cursor);
                self.cursor = s;
            }
            KeyCode::Char('b') if alt => self.cursor = self.word_left(),
            KeyCode::Char('f') if alt => self.cursor = self.word_right(),
            KeyCode::Char(c) if !ctrl => self.insert(c),
            KeyCode::Left if ctrl || alt => self.cursor = self.word_left(),
            KeyCode::Right if ctrl || alt => self.cursor = self.word_right(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.text.len()),
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::End => self.cursor = self.line_end(),
            KeyCode::Backspace if ctrl || alt => {
                let s = self.word_left();
                self.text.drain(s..self.cursor);
                self.cursor = s;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.text.remove(self.cursor);
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.text.len() {
                    self.text.remove(self.cursor);
                }
            }
            KeyCode::Up => self.up(),
            KeyCode::Down => self.down(),
            KeyCode::Esc => {
                if self.back.is_some() {
                    self.back_to_draft();
                }
            }
            KeyCode::Tab => self.insert(' '),
            _ => return Action::Ignored,
        }
        Action::Edited
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn typed(e: &mut Editor, s: &str) {
        for c in s.chars() {
            e.key(k(KeyCode::Char(c)));
        }
    }

    #[test]
    fn editing_keys() {
        let mut e = Editor::default();
        typed(&mut e, "hello world");
        e.key(ctrl('w'));
        assert_eq!(e.text(), "hello ");
        e.key(ctrl('a'));
        typed(&mut e, ">");
        assert_eq!(e.text(), ">hello ");
        e.key(k(KeyCode::End));
        e.key(k(KeyCode::Backspace));
        e.key(ctrl('u'));
        assert_eq!(e.text(), "");
        typed(&mut e, "a b");
        e.key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
        e.key(ctrl('k'));
        assert_eq!(e.text(), "a ");
    }

    #[test]
    fn enter_sends_and_a_backslash_continues() {
        let mut e = Editor::default();
        typed(&mut e, "line one\\");
        assert_eq!(e.key(k(KeyCode::Enter)), Action::Edited);
        typed(&mut e, "two");
        assert_eq!(e.key(k(KeyCode::Enter)), Action::Submit("line one\ntwo".into()));
        assert_eq!(e.text(), "");
        e.paste("a\r\nb");
        assert_eq!(e.text(), "a\nb");
        assert_eq!(e.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)), Action::Edited);
        assert_eq!(e.text(), "a\nb\n");
    }

    #[test]
    fn history_back_and_forth_keeps_the_draft() {
        let mut e = Editor::new(vec!["first".into(), "second".into()]);
        typed(&mut e, "draft");
        e.key(k(KeyCode::Up));
        assert_eq!(e.text(), "second");
        e.key(k(KeyCode::Up));
        assert_eq!(e.text(), "first");
        e.key(k(KeyCode::Up));
        assert_eq!(e.text(), "first");
        e.key(k(KeyCode::Down));
        assert_eq!(e.text(), "second");
        e.key(k(KeyCode::Down));
        assert_eq!(e.text(), "draft");
        e.key(k(KeyCode::Up));
        e.key(k(KeyCode::Esc));
        assert_eq!(e.text(), "draft");
    }

    #[test]
    fn arrows_move_between_lines_of_a_message_first() {
        let mut e = Editor::new(vec!["old".into()]);
        e.paste("ab\ncd");
        e.key(k(KeyCode::Up));
        assert_eq!((e.text().as_str(), e.cursor()), ("ab\ncd", 2));
        e.key(k(KeyCode::Up));
        assert_eq!(e.text(), "old");
    }

    #[test]
    fn ctrl_c_clears_then_interrupts_and_ctrl_d_exits() {
        let mut e = Editor::default();
        typed(&mut e, "x");
        assert_eq!(e.key(ctrl('c')), Action::Cleared);
        assert_eq!(e.key(ctrl('c')), Action::Interrupt);
        assert_eq!(e.key(ctrl('d')), Action::Exit);
    }
}
