//! The IDE as a native app, drawn by Code Arena on the GPU (Floem). It speaks the very messages the browser page
//! speaks, in-process: what the person does goes to the session's driver, and what the turn does comes back as JSON,
//! one message at a time, into reactive signals, so a streamed token changes one text run.
//!
//! This first slice is Agent mode: the conversation (answers streamed, a turn's tool calls in one line that opens, the
//! agent's questions as cards), the box, the bars (plan, goal, jobs) and the status line, in the theme in force.

use std::rc::Rc;

use floem::IntoView;
use floem::event::Event;
use floem::keyboard::{Key, NamedKey};
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_rw_signal};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{Decorators, button, container, dyn_stack, empty, h_stack, label, scroll, text_input, v_stack};
use floem::window::WindowConfig;
use serde_json::{Value, json};

mod model;

pub use model::summary;
use model::{Item, Kind, Tool};

/// What joins the native window to the session's driver: what the person does goes out on `send` (as the page's
/// messages), and what the session does comes in on `recv` (as the page receives it).
pub struct Hub {
    pub send: tokio::sync::mpsc::UnboundedSender<(u64, Value)>,
    pub recv: tokio::sync::broadcast::Receiver<String>,
}

/// The native window's own number among the driver's faces (a page's are counted from 1).
const NATIVE: u64 = 0;

fn colour(hex: &str) -> Color {
    Color::parse(hex).unwrap_or(Color::rgb8(0x80, 0x80, 0x80))
}

/// Everything the window shows, as signals.
#[derive(Clone, Copy)]
struct Ui {
    items: RwSignal<Vec<Item>>,
    next: RwSignal<u64>,
    running: RwSignal<bool>,
    model: RwSignal<String>,
    mode: RwSignal<String>,
    folder: RwSignal<String>,
    branch: RwSignal<String>,
    theme: RwSignal<ca_themes::Palette>,
    themes: RwSignal<Vec<ca_themes::Theme>>,
    choice: RwSignal<String>,
    plan: RwSignal<Option<(String, usize, usize)>>,
    goal: RwSignal<Option<String>>,
    jobs: RwSignal<Vec<(u64, String)>>,
    draft: RwSignal<String>,
}

impl Ui {
    fn new() -> Ui {
        let themes = ca_themes::built_in();
        let start = ca_themes::chosen(&themes, "", true);
        Ui {
            items: create_rw_signal(Vec::new()),
            next: create_rw_signal(1),
            running: create_rw_signal(false),
            model: create_rw_signal(String::new()),
            mode: create_rw_signal(String::new()),
            folder: create_rw_signal(String::new()),
            branch: create_rw_signal(String::new()),
            theme: create_rw_signal(ca_themes::palette(&start)),
            themes: create_rw_signal(themes),
            choice: create_rw_signal(String::new()),
            plan: create_rw_signal(None),
            goal: create_rw_signal(None),
            jobs: create_rw_signal(Vec::new()),
            draft: create_rw_signal(String::new()),
        }
    }

    fn id(&self) -> u64 {
        let n = self.next.get_untracked();
        self.next.set(n + 1);
        n
    }

    fn push(&self, kind: Kind) {
        let id = self.id();
        self.items.update(|l| l.push(Item { id, kind }));
    }

    /// The answer being written: the last item when it is one, else a new one.
    fn current(&self) -> Item {
        if let Some(last) = self.items.with_untracked(|l| l.last().cloned())
            && matches!(last.kind, Kind::Assistant { .. })
        {
            return last;
        }
        let item = Item { id: self.id(), kind: Kind::assistant() };
        self.items.update(|l| l.push(item.clone()));
        item
    }

    fn apply_theme(&self) {
        let t = ca_themes::chosen(&self.themes.get_untracked(), &self.choice.get_untracked(), true);
        self.theme.set(ca_themes::palette(&t));
    }

    /// One message from the session, as the page takes it.
    fn take(&self, m: &Value, send: &Rc<dyn Fn(Value)>) {
        let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
        match m["type"].as_str().unwrap_or("") {
            "state" => {
                self.model.set(s("model"));
                self.mode.set(s("mode"));
                self.folder.set(s("folder"));
                self.branch.set(s("branch"));
                self.running.set(m["running"] == true);
                self.choice.set(s("theme"));
                self.apply_theme();
                let mut items = Vec::new();
                for msg in m["messages"].as_array().into_iter().flatten() {
                    let text = msg["text"].as_str().unwrap_or("").to_string();
                    match msg["role"].as_str() {
                        Some("user") if !text.starts_with("(Code Arena") => {
                            items.push(Item { id: self.id(), kind: Kind::User(text) })
                        }
                        Some("assistant") => {
                            let tools: Vec<Tool> = msg["tools"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|t| t.as_str())
                                .map(|n| Tool {
                                    name: n.into(),
                                    subject: String::new(),
                                    done: true,
                                    ok: true,
                                    text: String::new(),
                                })
                                .collect();
                            let last_is_answer =
                                matches!(items.last(), Some(Item { kind: Kind::Assistant { .. }, .. }));
                            if !last_is_answer {
                                items.push(Item { id: self.id(), kind: Kind::assistant() });
                            }
                            if let Some(Item { kind: Kind::Assistant { text: t, tools: ts, .. }, .. }) = items.last() {
                                if !text.is_empty() {
                                    t.update(|x| {
                                        if !x.is_empty() {
                                            x.push_str("\n\n");
                                        }
                                        x.push_str(&text)
                                    });
                                }
                                ts.update(|x| x.extend(tools));
                            }
                        }
                        _ => {}
                    }
                }
                self.items.set(items);
                send(json!({"type": "themes"}));
            }
            "status" => {
                self.model.set(s("model"));
                self.mode.set(s("mode"));
                self.running.set(m["running"] == true);
                if m["branch"].is_string() {
                    self.branch.set(s("branch"));
                }
            }
            "theme" => {
                self.choice.set(s("theme"));
                self.apply_theme();
            }
            "themes" => {
                let mut all = ca_themes::built_in();
                for t in m["items"].as_array().into_iter().flatten() {
                    if let Some(id) = t["id"].as_str() {
                        all.push(ca_themes::Theme { id: id.into(), json: t.clone() });
                    }
                }
                self.themes.set(all);
                self.apply_theme();
            }
            "user" => self.push(Kind::User(s("text"))),
            "turn_start" => self.running.set(true),
            "text" => {
                if let Kind::Assistant { text, .. } = self.current().kind {
                    text.update(|t| t.push_str(m["delta"].as_str().unwrap_or("")));
                }
            }
            "tool_start" => {
                if let Kind::Assistant { tools, .. } = self.current().kind {
                    tools.update(|t| {
                        t.push(Tool {
                            name: s("name"),
                            subject: s("subject"),
                            done: false,
                            ok: true,
                            text: String::new(),
                        })
                    });
                }
            }
            "tool_end" => {
                if let Kind::Assistant { tools, .. } = self.current().kind {
                    tools.update(|ts| {
                        let (name, subject) = (s("name"), s("subject"));
                        match ts.iter_mut().rev().find(|t| !t.done && t.name == name && t.subject == subject) {
                            Some(t) => {
                                t.done = true;
                                t.ok = m["ok"] == true;
                                t.text = s("text");
                            }
                            None => ts.push(Tool { name, subject, done: true, ok: m["ok"] == true, text: s("text") }),
                        }
                    });
                }
            }
            "notice" => {
                let t = s("text");
                if !(t.starts_with("› ") && t.ends_with("(sent while it worked)")) {
                    self.push(Kind::Notice(t));
                }
            }
            "ask" => self.push(Kind::Ask {
                id: m["id"].as_u64().unwrap_or(0),
                tool: s("tool"),
                subject: s("subject"),
                answer: create_rw_signal(None),
            }),
            "answered" => {
                let id = m["id"].as_u64().unwrap_or(0);
                self.items.with_untracked(|l| {
                    for i in l {
                        if let Kind::Ask { id: a, answer, .. } = &i.kind
                            && *a == id
                        {
                            answer.set(Some(s("answer")));
                        }
                    }
                });
            }
            "queued" => self.push(Kind::Queued(s("text"))),
            "turn_end" => {
                self.running.set(false);
                self.items.with_untracked(|l| {
                    for i in l {
                        if let Kind::Ask { answer, .. } = &i.kind
                            && answer.get_untracked().is_none()
                        {
                            answer.set(Some("gone".into()));
                        }
                    }
                });
                if m["ok"] != true {
                    if m["stopped"] == true {
                        self.push(Kind::Notice("Stopped.".into()));
                    } else {
                        self.push(Kind::Error(m["error"].as_str().unwrap_or("The turn failed.").to_string()));
                    }
                }
            }
            "tasks" => {
                self.plan.set(m["plan"]["steps"].as_array().map(|steps| {
                    let done = steps.iter().filter(|st| st["status"] == "done").count();
                    (m["plan"]["title"].as_str().unwrap_or("").to_string(), done, steps.len())
                }));
                self.goal.set(
                    m["goal"]["text"].as_str().map(|g| format!("{g} · {}", m["goal"]["status"].as_str().unwrap_or(""))),
                );
                self.jobs.set(
                    m["jobs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|j| j["running"] == true)
                        .map(|j| (j["id"].as_u64().unwrap_or(0), j["command"].as_str().unwrap_or("").to_string()))
                        .collect(),
                );
            }
            "behind" => send(json!({"type": "state"})),
            _ => {}
        }
    }
}

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
    let config = WindowConfig::default().title("Code Arena").size((1100.0, 760.0));
    floem::Application::new().window(move |_| app(out, rx), Some(config)).run();
}

fn app(out: tokio::sync::mpsc::UnboundedSender<(u64, Value)>, rx: crossbeam_channel::Receiver<Value>) -> impl IntoView {
    let ui = Ui::new();
    let send: Rc<dyn Fn(Value)> = Rc::new(move |v: Value| {
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
    let pal = ui.theme;
    v_stack((thread(ui, send.clone()), bars(ui, send.clone()), input_box(ui, send.clone()), status_line(ui))).style(
        move |s| {
            let p = pal.get();
            s.size_full().background(colour(&p.bg)).color(colour(&p.text)).font_size(13.0)
        },
    )
}

fn thread(ui: Ui, send: Rc<dyn Fn(Value)>) -> impl IntoView {
    let pal = ui.theme;
    let follow = create_rw_signal(0.0f32);
    create_effect(move |_| {
        // Kept at the end as the thread grows.
        ui.items.with(|l| l.len());
        follow.set(100.0);
    });
    scroll(
        dyn_stack(move || ui.items.get(), |i: &Item| i.id, move |i: Item| item_view(ui, i, send.clone()))
            .style(|s| s.flex_col().width_full().padding(16.0).gap(10.0)),
    )
    .scroll_to_percent(move || follow.get())
    .style(move |s| s.flex_grow(1.0_f32).width_full().background(colour(&pal.get().bg)))
}

fn item_view(ui: Ui, i: Item, send: Rc<dyn Fn(Value)>) -> impl IntoView {
    let pal = ui.theme;
    match i.kind {
        Kind::User(text) | Kind::Queued(text) => {
            container(label(move || text.clone()).style(|s| s.text_overflow(TextOverflow::Wrap)))
                .style(move |s| {
                    let p = pal.get();
                    s.padding_horiz(12.0)
                        .padding_vert(8.0)
                        .border_radius(10.0)
                        .background(colour(&p.hover))
                        .max_width_pct(85.0)
                        .margin_left(floem::unit::PxPctAuto::Auto)
                })
                .into_any()
        }
        Kind::Assistant { text, tools, open } => v_stack((
            label(move || {
                let t = tools.get();
                if t.is_empty() {
                    String::new()
                } else {
                    format!("{} {}", if open.get() { "▾" } else { "▸" }, summary(&t))
                }
            })
            .on_click_stop(move |_| open.update(|o| *o = !*o))
            .style(move |s| {
                let p = pal.get();
                s.color(colour(&p.muted))
                    .cursor(CursorStyle::Pointer)
                    .apply_if(tools.with(|t| t.is_empty()), |s| s.hide())
            }),
            dyn_stack(
                move || if open.get() { tools.get().into_iter().enumerate().collect::<Vec<_>>() } else { vec![] },
                |(n, t): &(usize, Tool)| (*n, t.done),
                move |(_, t): (usize, Tool)| {
                    v_stack((
                        label(move || format!("{}  {}", t.name, t.subject)).style(move |s| {
                            let p = pal.get();
                            s.font_family("monospace".to_string()).color(colour(if t.ok {
                                &p.muted
                            } else {
                                &p.danger
                            }))
                        }),
                        label({
                            let x = t.text.clone();
                            move || x.clone()
                        })
                        .style(move |s| {
                            let p = pal.get();
                            s.font_family("monospace".to_string())
                                .font_size(12.0)
                                .padding(8.0)
                                .border_radius(6.0)
                                .background(colour(&p.code))
                                .text_overflow(TextOverflow::Wrap)
                                .apply_if(t.text.is_empty(), |s| s.hide())
                        }),
                    ))
                    .style(|s| s.gap(4.0).padding_left(16.0))
                },
            )
            .style(|s| s.flex_col().gap(6.0)),
            label(move || text.get()).style(move |s| {
                s.text_overflow(TextOverflow::Wrap).apply_if(text.with(|t| t.is_empty()), |s| s.hide())
            }),
        ))
        .style(|s| s.gap(6.0).width_full())
        .into_any(),
        Kind::Notice(text) => {
            label(move || text.clone()).style(move |s| s.color(colour(&pal.get().muted)).font_size(12.0)).into_any()
        }
        Kind::Error(text) => label(move || text.clone())
            .style(move |s| s.color(colour(&pal.get().danger)).text_overflow(TextOverflow::Wrap))
            .into_any(),
        Kind::Ask { id, tool, subject, answer } => {
            let reply = move |a: &'static str| {
                let send = send.clone();
                move |_: &Event| send(json!({"type": "answer", "id": id, "answer": a}))
            };
            v_stack((
                h_stack((
                    label(move || tool.clone())
                        .style(move |s| s.color(colour(&pal.get().muted)).font_family("monospace".to_string())),
                    label(move || subject.clone())
                        .style(|s| s.font_family("monospace".to_string()).text_overflow(TextOverflow::Wrap)),
                ))
                .style(|s| s.gap(8.0)),
                h_stack((
                    button(label(|| "Allow")).on_click_stop(reply("yes")).style(move |s| {
                        let p = pal.get();
                        s.background(colour(&p.button))
                            .color(colour(&p.button_text))
                            .border(0.0)
                            .border_radius(4.0)
                            .padding_horiz(12.0)
                    }),
                    button(label(|| "Always, this session")).on_click_stop(reply("always")),
                    button(label(|| "Deny")).on_click_stop(reply("no")),
                ))
                .style(move |s| s.gap(8.0).apply_if(answer.get().is_some(), |s| s.hide())),
                label(move || match answer.get().as_deref() {
                    Some("no") => "Denied".to_string(),
                    Some("always") => "Allowed for this session".to_string(),
                    Some("yes") => "Allowed".to_string(),
                    Some("gone") => "No longer asked: the turn ended".to_string(),
                    _ => String::new(),
                })
                .style(move |s| s.color(colour(&pal.get().muted)).apply_if(answer.get().is_none(), |s| s.hide())),
            ))
            .style(move |s| {
                let p = pal.get();
                s.gap(10.0)
                    .padding(12.0)
                    .border(1.0)
                    .border_color(colour(&p.accent))
                    .border_radius(8.0)
                    .background(colour(&p.panel))
            })
            .into_any()
        }
    }
}

fn bars(ui: Ui, send: Rc<dyn Fn(Value)>) -> impl IntoView {
    let pal = ui.theme;
    let bar = move |s: floem::style::Style| {
        let p = pal.get();
        s.padding_horiz(10.0)
            .padding_vert(6.0)
            .border(1.0)
            .border_color(colour(&p.line))
            .border_radius(8.0)
            .background(colour(&p.panel))
    };
    v_stack((
        label(move || ui.plan.get().map(|(t, d, n)| format!("Plan  {t} · {d} of {n} done")).unwrap_or_default())
            .style(move |s| bar(s).apply_if(ui.plan.get().is_none(), |s| s.hide())),
        label(move || ui.goal.get().map(|g| format!("Goal  {g}")).unwrap_or_default())
            .style(move |s| bar(s).apply_if(ui.goal.get().is_none(), |s| s.hide())),
        dyn_stack(
            move || ui.jobs.get(),
            |(id, _): &(u64, String)| *id,
            move |(id, cmd): (u64, String)| {
                let send = send.clone();
                h_stack((
                    label(move || format!("Job {id}  {cmd}")).style(|s| s.flex_grow(1.0_f32)),
                    button(label(|| "Stop")).on_click_stop(move |_| send(json!({"type": "stop_job", "id": id}))),
                ))
                .style(move |s| bar(s).items_center())
            },
        )
        .style(|s| s.flex_col().gap(6.0)),
    ))
    .style(|s| s.gap(6.0).padding_horiz(16.0))
}

fn input_box(ui: Ui, send: Rc<dyn Fn(Value)>) -> impl IntoView {
    let pal = ui.theme;
    let submit = {
        let send = send.clone();
        move || {
            let text = ui.draft.get_untracked().trim().to_string();
            if !text.is_empty() {
                send(json!({"type": "send", "text": text}));
                ui.draft.set(String::new());
            }
        }
    };
    let enter = submit.clone();
    let stop = send.clone();
    h_stack((
        text_input(ui.draft)
            .placeholder("Ask Code Arena…  (/ for commands)")
            .on_key_down(Key::Named(NamedKey::Enter), |m| m.is_empty(), move |_| enter())
            .on_key_down(Key::Named(NamedKey::Escape), |_| true, move |_| stop(json!({"type": "stop"})))
            .style(move |s| {
                let p = pal.get();
                s.flex_grow(1.0_f32).padding(8.0).border(0.0).background(colour(&p.input)).color(colour(&p.text))
            }),
        button(label(move || if ui.running.get() { "Stop" } else { "Send" }))
            .on_click_stop(move |_| {
                if ui.running.get_untracked() {
                    send(json!({"type": "stop"}));
                } else {
                    submit();
                }
            })
            .style(move |s| {
                let p = pal.get();
                s.background(colour(&p.button))
                    .color(colour(&p.button_text))
                    .border(0.0)
                    .border_radius(6.0)
                    .padding_horiz(14.0)
                    .padding_vert(6.0)
            }),
    ))
    .style(move |s| {
        let p = pal.get();
        s.margin(16.0)
            .padding(6.0)
            .gap(8.0)
            .items_center()
            .border(1.0)
            .border_color(colour(&p.line))
            .border_radius(10.0)
            .background(colour(&p.input))
    })
}

fn status_line(ui: Ui) -> impl IntoView {
    let pal = ui.theme;
    h_stack((
        label(move || ui.model.get()),
        label(move || ui.mode.get()),
        label(move || ui.folder.get().rsplit('/').next().unwrap_or("").to_string()),
        label(move || if ui.branch.get().is_empty() { String::new() } else { format!("⎇ {}", ui.branch.get()) }),
        empty().style(|s| s.flex_grow(1.0_f32)),
        label(move || if ui.running.get() { "Working…" } else { "" }),
    ))
    .style(move |s| {
        let p = pal.get();
        s.width_full()
            .gap(14.0)
            .padding_horiz(10.0)
            .padding_vert(3.0)
            .font_size(12.0)
            .background(colour(&p.status))
            .color(colour(&p.status_fg))
    })
}
