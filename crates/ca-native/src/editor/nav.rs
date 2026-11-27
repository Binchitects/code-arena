//! Going about the tabs, as VS Code: Go Back and Go Forward (Alt+Left, Alt+Right) through the last 50 places the
//! caret was, and Reopen Closed Editor (Ctrl+Shift+T) for the last 20 tabs closed.
//!
//! A place is a tab and a line. The caret moved within ten lines of the place it is at changes that place rather
//! than add one (typing adds none); a tab shown, or the caret taken further, adds one, and the places ahead of the
//! one gone back to are dropped. A tab just shown is put back where it was left a moment after: what the caret does
//! in that moment (and while a place gone back to is being reached) is that place's, not a new one.

use std::time::{Duration, Instant};

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use serde_json::Value;

use crate::commands::{self, command};
use crate::state::{Send, Ui};

/// The places kept, and the closed tabs.
pub const PLACES: usize = 50;
pub const CLOSED: usize = 20;
/// How far the caret moves within a place.
const NEAR: usize = 10;
/// How long a place just come to takes what the caret does as its own.
const SETTLE: Duration = Duration::from_millis(400);

/// The places the caret was, the one it is at, and until when what the caret does is that place's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    pub places: Vec<(String, usize)>,
    pub at: usize,
    /// Until when the place it is at takes the caret's moves in its tab; true while a place gone back (or forward)
    /// to is reached, when the other tabs' moves count for nothing.
    hold: Option<(Instant, bool)>,
}

impl History {
    /// The caret in a tab at a line (from 1); none: the tab shown, its line not known yet.
    pub fn visit(&mut self, path: &str, line: Option<usize>, now: Instant) {
        let held = self.hold.filter(|(until, _)| now < *until);
        if let Some((p, l)) = self.places.get_mut(self.at)
            && p == path
        {
            match line {
                Some(n) if held.is_some() || l.abs_diff(n) <= NEAR => *l = n,
                Some(_) => self.push(path, line, now),
                None => {}
            }
            return;
        }
        if held.is_some_and(|(_, going)| going) {
            return;
        }
        self.push(path, line, now);
    }

    fn push(&mut self, path: &str, line: Option<usize>, now: Instant) {
        self.places.truncate(self.at + 1);
        self.places.push((path.to_string(), line.unwrap_or(1)));
        if self.places.len() > PLACES {
            self.places.remove(0);
        }
        self.at = self.places.len() - 1;
        self.hold = line.is_none().then_some((now + SETTLE, false));
    }

    /// The place before the one it is at (Go Back), gone to.
    pub fn back(&mut self, now: Instant) -> Option<(String, usize)> {
        if self.at == 0 || self.places.is_empty() {
            return None;
        }
        self.at -= 1;
        self.hold = Some((now + SETTLE, true));
        self.places.get(self.at).cloned()
    }

    /// The place after (Go Forward), gone to.
    pub fn forward(&mut self, now: Instant) -> Option<(String, usize)> {
        if self.at + 1 >= self.places.len() {
            return None;
        }
        self.at += 1;
        self.hold = Some((now + SETTLE, true));
        self.places.get(self.at).cloned()
    }

    pub fn can_back(&self) -> bool {
        self.at > 0 && !self.places.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        self.at + 1 < self.places.len()
    }

    /// A file moved: its places follow it.
    pub fn rename(&mut self, from: &str, to: &str) {
        for (p, _) in &mut self.places {
            if p == from {
                *p = to.to_string();
            }
        }
    }
}

/// The tabs closed, the last first, at most `CLOSED`; a tab opened again is no longer among them.
pub fn closed(list: &mut Vec<String>, path: &str) {
    list.retain(|p| p != path);
    list.insert(0, path.to_string());
    list.truncate(CLOSED);
}

/// The tabs gone from the window's tabs (closed, not moved): those of `old` not in `now`, when there are fewer now.
pub fn gone(old: &[String], now: &[String]) -> Vec<String> {
    if now.len() >= old.len() {
        return Vec::new();
    }
    old.iter().filter(|p| !now.contains(p)).cloned().collect()
}

#[derive(Clone, Copy)]
pub struct Nav {
    pub history: RwSignal<History>,
    pub closed: RwSignal<Vec<String>>,
    /// The window's tabs as they were last (to see which closed).
    was: RwSignal<Vec<String>>,
}

impl crate::ext::State for Nav {
    fn new(ui: Ui) -> Self {
        Nav {
            history: ui.scope.create_rw_signal(History::default()),
            closed: ui.scope.create_rw_signal(Vec::new()),
            was: ui.scope.create_rw_signal(Vec::new()),
        }
    }
}

/// A place gone to: its tab shown (opened again when it was closed) and its line.
fn go(ui: Ui, send: &Send, place: Option<(String, usize)>) {
    let Some((path, line)) = place else { return };
    if ui.tab(&path).is_some() || !path.contains(':') {
        ui.open_at(&path, line, send);
    }
}

/// What it adds to the window's registries, once at start: Go Back, Go Forward, Reopen Closed Editor, and what
/// follows the tab shown, the caret and the tabs closed.
pub fn register(ui: Ui, _send: &Send) {
    let n = ui.state::<Nav>();
    // The tab shown: a place (its line comes with its caret).
    ui.scope.create_effect(move |_| {
        if let Some(p) = ui.active.get() {
            n.history.update(|h| h.visit(&p, None, Instant::now()));
        }
    });
    // The caret of the tab shown: its place changed, or a new one.
    ui.scope.create_effect(move |_| {
        let (line, _) = ui.caret.get();
        let path = ui.live.with_untracked(|l| l.as_ref().map(|(p, _)| p.clone()));
        if let Some(p) = path.filter(|p| ui.active.with_untracked(|a| a.as_deref() == Some(p.as_str()))) {
            n.history.update(|h| h.visit(&p, Some(line), Instant::now()));
        }
    });
    // The tabs closed, kept to be opened again (the files' and the files' changes' only: a kind's tab is opened
    // its own way).
    ui.scope.create_effect(move |_| {
        let now: Vec<String> = ui.tabs.with(|t| t.iter().map(|t| t.path.clone()).collect());
        let old = n.was.get_untracked();
        let gone = gone(&old, &now);
        if !gone.is_empty() {
            n.closed.update(|c| {
                for p in gone.iter().filter(|p| !p.starts_with("changes:") && !p.contains("://")) {
                    closed(c, p);
                }
            });
        }
        n.was.set(now);
    });
    let go_cat = "Go";
    commands::add(
        command("workbench.action.navigateBack", go_cat, "Go Back", |ui, send| {
            let place = ui.state::<Nav>().history.try_update(|h| h.back(Instant::now())).flatten();
            go(ui, send, place);
        })
        .keys("Alt+Left")
        .enabled(|ui| ui.state::<Nav>().history.with_untracked(History::can_back)),
    );
    commands::add(
        command("workbench.action.navigateForward", go_cat, "Go Forward", |ui, send| {
            let place = ui.state::<Nav>().history.try_update(|h| h.forward(Instant::now())).flatten();
            go(ui, send, place);
        })
        .keys("Alt+Right")
        .enabled(|ui| ui.state::<Nav>().history.with_untracked(History::can_forward)),
    );
    commands::add(
        command("workbench.action.reopenClosedEditor", "View", "Reopen Closed Editor", |ui, send| {
            let n = ui.state::<Nav>();
            let open = |p: &String| ui.tab(p).is_some();
            let next = n.closed.try_update(|c| {
                c.retain(|p| !open(p));
                (!c.is_empty()).then(|| c.remove(0))
            });
            if let Some(path) = next.flatten() {
                ui.open(&path, send);
            }
        })
        .keys("Ctrl+Shift+T"),
    );
    crate::menus::add(
        crate::menus::Menu::TitleBar(crate::menus::GO),
        crate::menus::item("workbench.action.navigateBack").group("1_history_nav").order(1),
    );
    crate::menus::add(
        crate::menus::Menu::TitleBar(crate::menus::GO),
        crate::menus::item("workbench.action.navigateForward").group("1_history_nav").order(2),
    );
}

/// The session's messages that are this part's: a file moved takes its places with it. Never taken away.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "renamed"
        && m["error"].is_null()
        && let (Some(from), Some(to)) = (m["path"].as_str(), m["to"].as_str())
    {
        ui.state::<Nav>().history.update(|h| h.rename(from, to));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(h: &History) -> Vec<(&str, usize)> {
        h.places.iter().map(|(p, l)| (p.as_str(), *l)).collect()
    }

    #[test]
    fn the_caret_near_its_place_moves_it_and_further_or_in_another_tab_adds_one() {
        let t = Instant::now();
        let later = t + Duration::from_secs(5);
        let mut h = History::default();
        h.visit("a.rs", None, t);
        // Put back where it was left a moment after it was shown: still its first place.
        h.visit("a.rs", Some(40), t + Duration::from_millis(50));
        h.visit("a.rs", Some(45), later);
        assert_eq!(places(&h), [("a.rs", 45)]);
        h.visit("a.rs", Some(200), later);
        h.visit("b.rs", None, later);
        h.visit("b.rs", Some(3), later);
        assert_eq!(places(&h), [("a.rs", 45), ("a.rs", 200), ("b.rs", 3)]);
        assert_eq!(h.at, 2);
    }

    #[test]
    fn back_and_forward_go_through_the_places_and_a_new_one_drops_those_ahead() {
        let t = Instant::now();
        let mut h = History::default();
        for (p, l) in [("a.rs", 1), ("b.rs", 1), ("c.rs", 1)] {
            h.visit(p, Some(l), t);
        }
        assert_eq!(h.back(t), Some(("b.rs".into(), 1)));
        // Reaching it: the tab left and the caret's first moves change nothing.
        h.visit("c.rs", Some(9), t + Duration::from_millis(10));
        h.visit("b.rs", None, t + Duration::from_millis(10));
        h.visit("b.rs", Some(30), t + Duration::from_millis(20));
        assert_eq!(places(&h), [("a.rs", 1), ("b.rs", 30), ("c.rs", 1)]);
        assert_eq!(h.back(t), Some(("a.rs".into(), 1)));
        assert_eq!(h.back(t), None);
        assert_eq!(h.forward(t), Some(("b.rs".into(), 30)));
        assert!(h.can_forward());
        let later = t + Duration::from_secs(5);
        h.visit("d.rs", None, later);
        assert_eq!(places(&h), [("a.rs", 1), ("b.rs", 30), ("d.rs", 1)]);
        assert!(!h.can_forward());
    }

    #[test]
    fn at_most_fifty_places_are_kept() {
        let t = Instant::now();
        let mut h = History::default();
        for i in 0..60 {
            h.visit(&format!("{i}.rs"), Some(1), t);
        }
        assert_eq!(h.places.len(), PLACES);
        assert_eq!(h.places[0].0, "10.rs");
        assert_eq!(h.at, PLACES - 1);
        h.rename("59.rs", "x.rs");
        assert_eq!(h.places[PLACES - 1].0, "x.rs");
    }

    #[test]
    fn the_closed_tabs_are_the_last_twenty_and_a_move_is_not_a_close() {
        let mut list = Vec::new();
        for i in 0..25 {
            closed(&mut list, &format!("{i}.rs"));
        }
        closed(&mut list, "20.rs");
        assert_eq!(list.len(), CLOSED);
        assert_eq!(list[0], "20.rs");
        assert_eq!(list[1], "24.rs");
        let s = |v: &[&str]| v.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        assert_eq!(gone(&s(&["a", "b", "c"]), &s(&["a", "c"])), ["b"]);
        assert!(gone(&s(&["a", "b"]), &s(&["a", "z"])).is_empty());
    }

    #[test]
    fn going_back_shows_the_tab_and_reopen_brings_a_closed_one_back() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let ui = Ui::new();
        let asked = Rc::new(RefCell::new(Vec::<Value>::new()));
        let send: Send = {
            let asked = asked.clone();
            Rc::new(move |v| asked.borrow_mut().push(v))
        };
        register(ui, &send);
        ui.take(&serde_json::json!({"type": "file", "path": "a.rs", "text": "a\n", "modified": 1}), &send);
        ui.take(&serde_json::json!({"type": "file", "path": "b.rs", "text": "b\n", "modified": 1}), &send);
        ui.active.set(Some("b.rs".into()));
        let n = ui.state::<Nav>();
        assert!(n.history.with_untracked(History::can_back));
        commands::run(ui, &send, "workbench.action.navigateBack");
        assert_eq!(ui.active.get_untracked().as_deref(), Some("a.rs"));
        ui.close_now("b.rs");
        assert_eq!(n.closed.get_untracked(), ["b.rs"]);
        commands::run(ui, &send, "workbench.action.reopenClosedEditor");
        assert_eq!(asked.borrow().last().and_then(|m| m["path"].as_str()), Some("b.rs"));
    }
}
