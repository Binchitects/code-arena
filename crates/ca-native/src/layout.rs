//! The workbench's layout as VS Code's, the very rules of the page's `web/src/layout.ts`: views in three containers
//! (the primary side bar, the secondary side bar, the panel), each with the view it shows and whether it is shown.

use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Files,
    Search,
    Changes,
    Outline,
    Agent,
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Where {
    Primary,
    Secondary,
    Panel,
}

pub const WHERE: [Where; 3] = [Where::Primary, Where::Secondary, Where::Panel];
pub const VIEWS: [View; 6] = [View::Files, View::Search, View::Changes, View::Outline, View::Agent, View::Terminal];

impl View {
    pub fn id(self) -> &'static str {
        match self {
            View::Files => "files",
            View::Search => "search",
            View::Changes => "changes",
            View::Outline => "outline",
            View::Agent => "agent",
            View::Terminal => "terminal",
        }
    }

    pub fn from_id(id: &str) -> Option<View> {
        VIEWS.into_iter().find(|v| v.id() == id)
    }

    /// Its title as VS Code writes a view container's (the side bar's title shows it in capitals).
    pub fn title(self) -> &'static str {
        match self {
            View::Files => "Explorer",
            View::Search => "Search",
            View::Changes => "Changes",
            View::Outline => "Outline",
            View::Agent => "Agent",
            View::Terminal => "Terminal",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            View::Files => "files",
            View::Search => "search",
            View::Changes => "source-control",
            View::Outline => "symbol-class",
            View::Agent => "comment-discussion",
            View::Terminal => "terminal",
        }
    }

    pub fn keys(self) -> &'static str {
        match self {
            View::Files => "Ctrl+Shift+E",
            View::Search => "Ctrl+Shift+F",
            View::Agent => "Ctrl+Alt+I",
            View::Terminal => "Ctrl+`",
            _ => "",
        }
    }
}

impl Where {
    fn id(self) -> &'static str {
        match self {
            Where::Primary => "primary",
            Where::Secondary => "secondary",
            Where::Panel => "panel",
        }
    }

    fn at(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub views: [Vec<View>; 3],
    pub active: [Option<View>; 3],
    pub shown: [bool; 3],
    pub right: bool,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout {
            views: [
                vec![View::Files, View::Search, View::Changes, View::Outline],
                vec![View::Agent],
                vec![View::Terminal],
            ],
            active: [Some(View::Files), Some(View::Agent), Some(View::Terminal)],
            shown: [true, true, true],
            right: false,
        }
    }
}

impl Layout {
    pub fn of(&self, w: Where) -> &[View] {
        &self.views[w.at()]
    }

    pub fn active(&self, w: Where) -> Option<View> {
        self.active[w.at()]
    }

    pub fn shown(&self, w: Where) -> bool {
        self.shown[w.at()]
    }

    /// The container a view is in.
    pub fn where_is(&self, v: View) -> Option<Where> {
        WHERE.into_iter().find(|w| self.of(*w).contains(&v))
    }

    /// `v` moved to `to` (before the view at `index`, else last): shown there; where it was shows another.
    pub fn moved(&self, v: View, to: Where, index: Option<usize>) -> Layout {
        let mut n = self.clone();
        let mut index = index;
        if let Some(from) = n.where_is(v) {
            let f = from.at();
            let at = n.views[f].iter().position(|x| *x == v).unwrap_or(0);
            n.views[f].remove(at);
            if from == to
                && let Some(i) = index
                && i > at
            {
                index = Some(i - 1);
            }
            if n.active[f] == Some(v) {
                n.active[f] = n.views[f].get(at.min(n.views[f].len().saturating_sub(1))).copied();
            }
            if n.views[f].is_empty() {
                n.shown[f] = false;
            }
        }
        let t = to.at();
        let i = index.unwrap_or(n.views[t].len()).min(n.views[t].len());
        n.views[t].insert(i, v);
        n.active[t] = Some(v);
        n.shown[t] = true;
        n
    }

    /// `v` on screen: its container shown, and it the one shown in it.
    pub fn revealed(&self, v: View) -> Layout {
        let mut n = self.clone();
        if let Some(w) = n.where_is(v) {
            n.active[w.at()] = Some(v);
            n.shown[w.at()] = true;
        }
        n
    }

    /// An activity bar's click (and a view's key): shown, or hidden when it is shown already.
    pub fn view_toggled(&self, v: View) -> Layout {
        match self.where_is(v) {
            Some(w) if self.shown(w) && self.active(w) == Some(v) => {
                let mut n = self.clone();
                n.shown[w.at()] = false;
                n
            }
            Some(_) => self.revealed(v),
            None => self.clone(),
        }
    }

    pub fn toggled(&self, w: Where) -> Layout {
        let mut n = self.clone();
        let i = w.at();
        n.shown[i] = !n.shown[i] && !n.views[i].is_empty();
        if n.shown[i] && n.active[i].is_none() {
            n.active[i] = n.views[i].first().copied();
        }
        n
    }

    /// As the page keeps it.
    pub fn to_json(&self) -> Value {
        let ids = |w: Where| self.of(w).iter().map(|v| v.id()).collect::<Vec<_>>();
        let mut active = serde_json::Map::new();
        let mut shown = serde_json::Map::new();
        for w in WHERE {
            active.insert(w.id().into(), self.active(w).map(|v| json!(v.id())).unwrap_or(Value::Null));
            shown.insert(w.id().into(), json!(self.shown(w)));
        }
        json!({
            "primary": ids(Where::Primary), "secondary": ids(Where::Secondary), "panel": ids(Where::Panel),
            "active": active, "shown": shown, "side": if self.right { "right" } else { "left" }, "panelAt": "bottom",
        })
    }

    /// A layout kept before (or by another version): every view once, in a container; what it lacks from the defaults.
    pub fn normalize(saved: &Value) -> Layout {
        let d = Layout::default();
        if !saved.is_object() {
            return d;
        }
        let mut n = Layout { views: [vec![], vec![], vec![]], active: [None; 3], shown: [false; 3], right: false };
        let mut seen = Vec::new();
        for w in WHERE {
            for v in saved[w.id()].as_array().into_iter().flatten().filter_map(|x| x.as_str().and_then(View::from_id)) {
                if !seen.contains(&v) {
                    seen.push(v);
                    n.views[w.at()].push(v);
                }
            }
        }
        for w in WHERE {
            for v in d.of(w) {
                if !seen.contains(v) {
                    n.views[w.at()].push(*v);
                }
            }
        }
        for w in WHERE {
            let i = w.at();
            let a = saved["active"][w.id()].as_str().and_then(View::from_id);
            n.active[i] = a.filter(|a| n.views[i].contains(a)).or(n.views[i].first().copied());
            let has = !n.views[i].is_empty();
            n.shown[i] = saved["shown"][w.id()].as_bool().map(|s| s && has).unwrap_or(has);
        }
        n.right = saved["side"] == "right";
        n
    }
}

#[cfg(test)]
mod tests {
    use super::View::*;
    use super::Where::*;
    use super::*;

    #[test]
    fn a_view_moves_to_another_container_shown_there_where_it_was_shows_the_next() {
        let mut l = Layout::default().moved(Search, Panel, None);
        assert_eq!(l.of(Panel), [Terminal, Search]);
        assert_eq!(l.active(Panel), Some(Search));
        assert_eq!(l.of(Primary), [Files, Changes, Outline]);
        assert_eq!(l.active(Primary), Some(Files));
        l = l.revealed(Changes).moved(Changes, Secondary, Some(0));
        assert_eq!(l.of(Secondary), [Changes, Agent]);
        assert_eq!(l.active(Primary), Some(Outline));
        l = l.moved(Agent, Panel, None).moved(Changes, Primary, None);
        assert!(l.of(Secondary).is_empty());
        assert!(!l.shown(Secondary));
        assert_eq!(l.where_is(Agent), Some(Panel));
    }

    #[test]
    fn moved_within_its_container_to_its_new_place() {
        assert_eq!(Layout::default().moved(Files, Primary, Some(3)).of(Primary), [Search, Changes, Files, Outline]);
        assert_eq!(Layout::default().moved(Outline, Primary, Some(0)).of(Primary), [Outline, Files, Search, Changes]);
    }

    #[test]
    fn the_activity_bars_click_shows_a_view_or_hides_the_side_bar_when_it_is_shown() {
        let mut l = Layout::default().view_toggled(Search);
        assert_eq!(l.active(Primary), Some(Search));
        assert!(l.shown(Primary));
        l = l.view_toggled(Search);
        assert!(!l.shown(Primary));
        assert!(l.toggled(Primary).shown(Primary));
    }

    #[test]
    fn a_kept_layout_is_read_kindly() {
        let l = Layout::normalize(&json!({
            "primary": ["files", "bogus", "files"], "secondary": ["terminal"], "panel": [],
            "active": {"secondary": "terminal"}, "shown": {"panel": true}, "side": "right", "panelAt": "sideways",
        }));
        assert_eq!(l.of(Primary), [Files, Search, Changes, Outline]);
        assert_eq!(l.of(Secondary), [Terminal, Agent]);
        assert!(l.of(Panel).is_empty());
        assert!(!l.shown(Panel), "an empty container is not shown");
        assert!(l.right);
        assert_eq!(Layout::normalize(&Value::Null), Layout::default());
        assert_eq!(Layout::normalize(&Layout::default().to_json()), Layout::default());
    }
}
