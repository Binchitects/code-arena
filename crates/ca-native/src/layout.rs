//! The window's views: what each is called, its icon, its keys. Where they are is the dock tree's (`dock.rs`).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Files,
    Search,
    Changes,
    Outline,
    Agent,
    Terminal,
}

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
