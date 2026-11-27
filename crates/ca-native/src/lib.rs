//! The IDE as a native app, drawn by Code Arena on the GPU (Floem), laid out and coloured as VS Code's workbench. It
//! speaks the very messages the browser page speaks, in-process: what the person does goes to the session's driver,
//! and what the session does comes back as JSON, one message at a time, into reactive signals, so a streamed token
//! changes one text run.

use std::rc::Rc;

use floem::IntoView;
use floem::reactive::{SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::window::WindowConfig;
use serde_json::{Value, json};
// Kept for the parts to come (see Cargo.toml): named here so the build does not say they are not used.
use {base64 as _, globset as _, regex as _, similar as _};

mod accessibility;
mod accounts;
mod activity_bar;
mod agent;
mod assistants;
mod automation;
mod badges;
mod banners;
mod bidi;
mod changes;
mod chat_bridge;
mod checklists;
mod citations;
mod colors;
mod commands;
mod compare;
mod complete;
mod context_panel;
mod debug;
mod debug_console;
mod decor;
mod diagrams;
mod diff;
mod dock;
mod docking;
mod editor;
mod explorer;
mod explorer_decor;
mod ext;
mod extensions_apply;
mod extensions_view;
mod format;
mod harness;
mod help;
mod help_text;
mod highlight;
mod icons;
mod icons_table;
mod indent;
mod inline_complete;
mod keymap;
mod keys_hook;
mod language;
mod layers;
mod layout;
mod layout_commands;
mod loops;
mod lsp;
mod manage;
mod markdown;
mod mcp;
mod md_preview;
mod media;
mod memory_view;
mod menus;
mod merge_editor;
mod merge_inline;
mod model;
mod notebook;
mod notify;
mod open_folder;
mod outline;
mod output;
mod palette;
mod peek;
mod plan_view;
mod ports;
mod problems;
mod project_tab;
mod projects_view;
mod prose;
mod providers;
mod quick_modes;
mod review;
mod run_debug;
mod save;
mod scm;
mod search;
mod sessions;
mod sessions_arena;
mod settings;
mod skills_view;
mod snippets;
mod state;
mod status_items;
mod statusbar;
mod tasks_panel;
mod tasks_run;
mod terminal;
mod theme_menu;
mod tools_dialog;
mod trust;
mod update;
mod vm_login;
mod widgets;
mod workbench;
mod workspaces;

pub use model::summary;
use state::{Send, Ui};

/// What joins the native window to the session's driver: what the person does goes out on `send` (as the page's
/// messages), and what the session does comes in on `recv` (as the page receives it).
pub struct Hub {
    pub send: tokio::sync::mpsc::UnboundedSender<(u64, Value)>,
    pub recv: tokio::sync::broadcast::Receiver<String>,
}

/// The native window's own number among the driver's faces (a page's are counted from 1).
const NATIVE: u64 = 0;

/// Opens the native window and runs until it is closed. The messages come in from a thread of their own.
pub fn launch(hub: Hub) {
    let Hub { send: out, mut recv } = hub;
    let (tx, rx) = crossbeam_channel::unbounded::<Value>();
    std::thread::spawn(move || {
        loop {
            match recv.blocking_recv() {
                Ok(text) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&text)
                        && tx.send(v).is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if tx.send(json!({"type": "behind"})).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    icons::load();
    // Which fonts this machine has, found before the first view is drawn (the font system is busy then).
    std::sync::LazyLock::force(&widgets::UI_FONT);
    std::sync::LazyLock::force(&widgets::MONO_FONT);
    // VS Code's own title bar, drawn by the app (the system's frame is off), as VS Code's "custom" title bar.
    let config = WindowConfig::default().title("Code Arena").size((1280.0, 800.0)).undecorated(true);
    floem::Application::new().window(move |_| app(out, rx), Some(config)).run();
}

/// The layout and the parts' sizes kept for the folder when they change: a moment after the last change (a sash
/// dragged sends one, not one a pixel), and after the turn when one is running (the driver keeps it then).
fn keep_view_state(ui: Ui, send: Send) {
    let generation = create_rw_signal(0u64);
    let waiting = create_rw_signal(false);
    create_effect(move |_| {
        ui.dock.track();
        ui.hidden.track();
        ui.sizes.with(|m| m.values().for_each(|s| s.track()));
        ui.tabs.track();
        ui.active.track();
        if ui.loaded.get_untracked() {
            generation.update(|g| *g += 1);
            waiting.set(true);
        }
    });
    create_effect(move |_| {
        let g = generation.get();
        let running = ui.running.get();
        if !waiting.get_untracked() || running {
            return;
        }
        let send = send.clone();
        floem::action::exec_after(std::time::Duration::from_millis(500), move |_| {
            if generation.get_untracked() == g && !ui.running.get_untracked() && waiting.get_untracked() {
                waiting.set(false);
                send(json!({"type": "keep_view_state", "state": ui.view_state()}));
            }
        });
    });
}

/// What is on screen is watched on disk, as the page has it watched: the open files and the Explorer's open folders
/// (the session says when one changes, and lists a changed folder again).
fn watch_disk(ui: Ui, send: Send) {
    create_effect(move |_| {
        let files: Vec<String> = ui.tabs.with(|t| t.iter().filter(|t| t.is_file()).map(|t| t.path.clone()).collect());
        let mut folders: Vec<String> = ui.expanded.with(|e| e.iter().cloned().collect());
        folders.sort();
        folders.insert(0, String::new());
        if ui.loaded.get() {
            send(json!({"type": "watch", "files": files, "folders": folders}));
        }
    });
}

fn app(out: tokio::sync::mpsc::UnboundedSender<(u64, Value)>, rx: crossbeam_channel::Receiver<Value>) -> impl IntoView {
    let ui = Ui::new();
    let send: Send = Rc::new(move |v: Value| {
        let _ = out.send((NATIVE, v));
    });
    let incoming = create_rw_signal::<Option<Value>>(None);
    floem::ext_event::update_signal_from_channel(incoming.write_only(), rx);
    {
        let send = send.clone();
        create_effect(move |_| {
            if let Some(m) = incoming.get() {
                ui.take(&m, &send);
            }
        });
    }
    ui.out.set(Some(send.clone()));
    send(json!({"type": "hello"}));
    docking::set_send(send.clone());
    // Every part's commands, menus and status bar items, before anything is drawn from them.
    ext::register(ui, &send);
    keep_view_state(ui, send.clone());
    watch_disk(ui, send.clone());
    workbench::view(ui, send)
}
