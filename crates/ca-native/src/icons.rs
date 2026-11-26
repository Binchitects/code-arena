//! VS Code's icons: the Codicons font, drawn as text in its own family.

use floem::views::{Decorators, Label, label};

static CODICON: &[u8] = include_bytes!("../assets/codicon.ttf");

/// Gives the window's text the Codicons font. Once, before the first view.
pub fn load() {
    floem::text::FONT_SYSTEM.lock().db_mut().load_font_data(CODICON.to_vec());
}

/// A codicon's character, by its name in VS Code.
pub fn char_of(name: &str) -> char {
    let code = match name {
        "files" => 0xeaf0,
        "search" => 0xea6d,
        "source-control" => 0xea68,
        "git-compare" => 0xeafd,
        "symbol-class" => 0xeb5b,
        "comment-discussion" => 0xeac7,
        "terminal" => 0xea85,
        "account" => 0xeb99,
        "settings-gear" => 0xeb51,
        "chevron-right" => 0xeab6,
        "chevron-down" => 0xeab4,
        "chevron-up" => 0xeab7,
        "close" => 0xea76,
        "ellipsis" => 0xea7c,
        "split-horizontal" => 0xeb56,
        "layout-sidebar-left" => 0xebf3,
        "layout-sidebar-left-off" => 0xec02,
        "layout-panel" => 0xebf2,
        "layout-panel-off" => 0xec01,
        "layout-sidebar-right" => 0xebf4,
        "layout-sidebar-right-off" => 0xec00,
        "layout" => 0xebeb,
        "git-branch" => 0xec6f,
        "error" => 0xea87,
        "warning" => 0xea6c,
        "bell" => 0xeaa2,
        "check" => 0xeab2,
        "sync" => 0xea77,
        "send" => 0xec0f,
        "debug-stop" => 0xead7,
        "new-file" => 0xea7f,
        "new-folder" => 0xea80,
        "refresh" => 0xeb37,
        "collapse-all" => 0xeac5,
        "add" => 0xea60,
        "trash" => 0xea81,
        "sparkle" => 0xec10,
        "chrome-minimize" => 0xeaba,
        "chrome-maximize" => 0xeab9,
        "chrome-restore" => 0xeabb,
        "chrome-close" => 0xeab8,
        "remote" => 0xeb3a,
        "arrow-left" => 0xea9b,
        "arrow-right" => 0xea9c,
        "history" => 0xea82,
        "pass" => 0xeba4,
        "loading" => 0xeb19,
        "tools" => 0xeb6d,
        "attach" => 0xec34,
        "circle-filled" => 0xea71,
        "circle-large-filled" => 0xebb4,
        "file" => 0xea7b,
        "screen-full" => 0xeb4c,
        "case-sensitive" => 0xeab1,
        "regex" => 0xeb38,
        "arrow-up" => 0xeaa1,
        "arrow-down" => 0xea9a,
        "symbol-method" => 0xea8c,
        "symbol-struct" => 0xea91,
        "symbol-enum" => 0xea95,
        "symbol-interface" => 0xeb61,
        "symbol-namespace" => 0xea8b,
        "symbol-constant" => 0xeb5d,
        "symbol-variable" => 0xea88,
        "symbol-string" => 0xeb8d,
        "symbol-field" => 0xeb5f,
        _ => 0xea74, // "info"
    };
    char::from_u32(code).unwrap_or('?')
}

/// A codicon at VS Code's size for it (16 px, 24 in the activity bar).
pub fn icon(name: &'static str, size: f32) -> Label {
    let c = char_of(name).to_string();
    label(move || c.clone()).style(move |s| {
        s.font_family("codicon".to_string()).font_size(size).line_height(1.0).width(size).height(size).justify_center()
    })
}
