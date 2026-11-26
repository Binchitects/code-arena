//! The IDE as a native app, drawn by Code Arena on the GPU (Floem), laid out and coloured as VS Code's workbench. It
//! speaks the very messages the browser page speaks, in-process: what the person does goes to the session's driver,
//! and what the session does comes back as JSON, one message at a time, into reactive signals, so a streamed token
//! changes one text run.

use std::rc::Rc;

use floem::IntoView;
use floem::reactive::{SignalGet, SignalTrack, SignalUpdate, create_effect, create_rw_signal};
use floem::window::WindowConfig;
use serde_json::{Value, json};

mod agent;
mod colors;
mod editor;
mod explorer;
mod icons;
mod layout;
mod model;
mod search;
mod state;
mod widgets;
mod workbench;

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
        ui.layout.track();
        ui.primary_width.track();
        ui.secondary_width.track();
        ui.panel_height.track();
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
    send(json!({"type": "hello"}));
    keep_view_state(ui, send.clone());
    workbench::view(ui, send)
}
