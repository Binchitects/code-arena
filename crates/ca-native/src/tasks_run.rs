//! Tasks (tasks.json and the project's own): Run Task, Run Build Task (Ctrl+Shift+B), Run Test Task, Terminate
//! Task and the build configuration, as VS Code's Terminal menu has them. The session lists and runs them
//! (`ide_tasks` in the driver); each runs in a terminal tab of its own, "Task - LABEL", used again by its next run.
//!
//! How a run is asked for:
//!
//! 1. `task_run{label | group, context}`: `context` is the editor's file (from the project's top), its caret's line
//!    (from 1) and the text selected, for `${file}`, `${lineNumber}` and `${selectedText}`.
//! 2. The session may answer `task_needs{label, inputs}`: the `${input:id}` it needs. [`ask_inputs`] asks each in
//!    turn, in the quick input, as VS Code does: a `promptString` in an input box (dots for one with
//!    `password`), a `pickString` in a pick of its options, a `pickProcess` in a pick of the processes the session
//!    lists in its options. The answers go back in `task_run{inputs: {id: answer}}`; Escape on any one asks for
//!    nothing more and runs nothing. Run and debug's launch configurations ask theirs the same way.
//! 3. `task_untrusted{label, commands}`: the folder is in Restricted Mode, so the commands it would run are shown
//!    and the person says whether they may (trusting the folder, or this once: `allow`).
//! 4. `task_pick{group, items}`: no default in the group, so the person picks which one.
//! 5. `task_failed{error}` is said in a toast. `task_started{terminal, replaces}` puts the new run's tab where the
//!    last run's was.
//!
//! What every part has (`register`, `take`) is said in `ext.rs`.

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::buffer::rope_text::RopeText;
use serde_json::{Map, Value, json};

use crate::commands::{add, command};
use crate::menus::{Menu, item};
use crate::state::{Send, Ui};
use crate::widgets::dialog::{self, Kind};
use crate::widgets::{quick, toast};

pub const RUN_TASK: &str = "workbench.action.tasks.runTask";
pub const RUN_BUILD: &str = "workbench.action.tasks.build";
pub const RUN_TEST: &str = "workbench.action.tasks.test";
pub const TERMINATE: &str = "workbench.action.tasks.terminate";
pub const SHOW_RUNNING: &str = "workbench.action.tasks.showTasks";
pub const BUILD_CONFIGURATION: &str = "codeArena.selectBuildConfiguration";

/// The most text of the selection sent for `${selectedText}`.
const MOST_SELECTED: usize = 64 * 1024;

/// This part's state in a window.
#[derive(Clone, Copy)]
struct Tasks {
    /// The last `task_run` sent, whose context and inputs an answer that asks for more is sent again with.
    asked: RwSignal<Value>,
    /// The folder's build configuration, as the session last said it.
    build: RwSignal<String>,
}

impl crate::ext::State for Tasks {
    fn new(ui: Ui) -> Self {
        Tasks { asked: ui.scope.create_rw_signal(Value::Null), build: ui.scope.create_rw_signal("Debug".into()) }
    }
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    let list = |asked: &'static str| move |_: Ui, send: &Send| send(json!({"type": "task_list", "asked": asked}));
    add(command(RUN_TASK, "Tasks", "Run Task", list("run")));
    add(command(RUN_BUILD, "Tasks", "Run Build Task", |ui, send| run(ui, send, json!({"group": "build"})))
        .keys("Ctrl+Shift+B"));
    add(command(RUN_TEST, "Tasks", "Run Test Task", |ui, send| run(ui, send, json!({"group": "test"}))));
    add(command(TERMINATE, "Tasks", "Terminate Task", list("terminate")));
    add(command(SHOW_RUNNING, "Tasks", "Show Running Tasks", list("running")));
    add(command(BUILD_CONFIGURATION, "Tasks", "Select Build Configuration", list("build")));
    let bar = |c: &str| item(c);
    let terminal = Menu::TitleBar(crate::menus::TERMINAL);
    crate::menus::add(terminal.clone(), bar(RUN_TASK).title("Run Task...").group("3_run").order(1));
    crate::menus::add(terminal.clone(), bar(RUN_BUILD).title("Run Build Task...").group("3_run").order(2));
    crate::menus::add(terminal.clone(), bar(RUN_TEST).title("Run Test Task...").group("3_run").order(3));
    crate::menus::add(terminal.clone(), bar(SHOW_RUNNING).title("Show Running Tasks...").group("4_tasks").order(1));
    crate::menus::add(terminal.clone(), bar(TERMINATE).title("Terminate Task...").group("4_tasks").order(2));
    crate::menus::add(
        terminal,
        bar(BUILD_CONFIGURATION).title("Select Build Configuration...").group("5_configure").order(1),
    );
}

/// The editor's file, caret line and selection, for the variables that read them.
fn context(ui: Ui) -> Value {
    let Some((path, e)) = ui.live.get_untracked() else { return json!({}) };
    let Some(tab) = ui.tab(&path).filter(|t| t.is_file()) else { return json!({}) };
    let offset = e.cursor.with_untracked(|c| c.offset());
    let (a, b) = e.cursor.with_untracked(|c| c.get_selection().unwrap_or((offset, offset)));
    let (from, to) = (a.min(b), a.max(b).min(a.min(b) + MOST_SELECTED));
    let text = e.rope_text();
    let selected = if to <= text.len() { text.slice_to_cow(from..to).to_string() } else { String::new() };
    json!({"file": tab.file, "line": e.line_of_offset(offset) + 1, "selected": selected})
}

/// Asks the session to run a task (`{label}`) or a group's (`{group}`), with the editor as it is now.
fn run(ui: Ui, send: &Send, what: Value) {
    let mut m = what;
    m["type"] = json!("task_run");
    m["context"] = context(ui);
    ui.state::<Tasks>().asked.set(m.clone());
    send(m);
}

/// The last run asked for, sent again for `label` with `more` laid over it (its inputs, `allow`).
fn again(ui: Ui, send: &Send, label: &str, more: Value) {
    let mut m = ui.state::<Tasks>().asked.get_untracked();
    if !m.is_object() {
        m = json!({"type": "task_run", "context": context(ui)});
    }
    if let Some(o) = m.as_object_mut() {
        o.remove("group");
    }
    m["label"] = json!(label);
    for (k, v) in more.as_object().into_iter().flatten() {
        m[k.as_str()] = v.clone();
    }
    ui.state::<Tasks>().asked.set(m.clone());
    send(m);
}

/// One input to ask for, as the quick input asks it.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    /// A line of text: what is asked, its first text, whether it is a secret.
    Text { id: String, prompt: String, value: String, password: bool },
    /// One of a list: its title, its options (label, value, detail) and the one lit at first.
    Pick { id: String, title: String, options: Vec<(String, String, String)>, active: String },
}

/// An input of tasks.json's (or launch.json's), as it is asked; none for a kind that is not asked for here.
pub fn ask_of(input: &Value) -> Option<Ask> {
    let id = input["id"].as_str().unwrap_or("").to_string();
    let words = input["description"].as_str().unwrap_or("").to_string();
    let default = match &input["default"] {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    };
    let option = |o: &Value| match o {
        Value::String(s) => Some((s.clone(), s.clone(), String::new())),
        Value::Object(o) => {
            let value = o.get("value").and_then(Value::as_str)?.to_string();
            let label = o.get("label").and_then(Value::as_str).unwrap_or(&value).to_string();
            Some((label, value, o.get("description").and_then(Value::as_str).unwrap_or("").to_string()))
        }
        _ => None,
    };
    let options = || input["options"].as_array().into_iter().flatten().filter_map(option).collect::<Vec<_>>();
    match input["type"].as_str()? {
        "promptString" => {
            Some(Ask::Text { id, prompt: words, value: default, password: input["password"] == true })
        }
        "pickString" => Some(Ask::Pick { id, title: words, options: options(), active: default }),
        "pickProcess" => {
            let title = if words.is_empty() { "Select the process to attach to".to_string() } else { words };
            Some(Ask::Pick { id, title, options: options(), active: String::new() })
        }
        _ => None,
    }
}

/// Asks the inputs one after another; `done` is given every answer by id, or none once one was not answered.
pub fn ask_inputs(ui: Ui, inputs: &[Value], done: impl FnOnce(Option<Map<String, Value>>) + 'static) {
    let asks: Vec<Ask> = inputs.iter().filter_map(ask_of).collect();
    next(ui, asks, 0, Map::new(), Box::new(done));
}

type Done = Box<dyn FnOnce(Option<Map<String, Value>>)>;

fn next(ui: Ui, asks: Vec<Ask>, at: usize, mut answers: Map<String, Value>, done: Done) {
    let Some(ask) = asks.get(at).cloned() else { return done(Some(answers)) };
    match ask {
        Ask::Text { id, prompt, value, password } => {
            // As VS Code's: no title, the input's description said under the box.
            let mut q = quick::input("").value(&value).prompt(&prompt);
            if password {
                q = q.password();
            }
            q.open(ui, move |typed| match typed {
                Some(text) => {
                    answers.insert(id, json!(text));
                    next(ui, asks, at + 1, answers, done);
                }
                None => done(None),
            });
        }
        Ask::Pick { id, title, options, active } => {
            let items = options
                .iter()
                .map(|(label, value, detail)| quick::item(value, label).description(detail))
                .collect::<Vec<_>>();
            quick::pick(&title).placeholder(&title).items(items).active(&active).open(ui, move |picked| {
                match picked {
                    Some(item) => {
                        answers.insert(id, json!(item.id));
                        next(ui, asks, at + 1, answers, done);
                    }
                    None => done(None),
                }
            });
        }
    }
}

/// The list's tasks a pick offers: their label, source, command and what they say of themselves.
fn rows(m: &Value, only_running: bool) -> Vec<quick::Item> {
    let mut configured = Vec::new();
    let mut detected = Vec::new();
    for t in m["items"].as_array().into_iter().flatten() {
        let running = t["running"] == true;
        if t["hide"] == true || (only_running && !running) {
            continue;
        }
        let label = t["label"].as_str().unwrap_or("");
        let mut words = t["detail"].as_str().unwrap_or("").to_string();
        if words.is_empty() {
            words = t["command"].as_str().unwrap_or("").to_string();
        }
        let workspace = t["source"] == "Workspace";
        let row = quick::item(label, label).description(&words).icon(if running { "loading" } else { "tools" });
        if workspace {
            configured.push(row.group("configured tasks"));
        } else {
            detected.push(row.group("detected tasks"));
        }
    }
    configured.extend(detected);
    configured
}

/// The session's list, taken for what it was asked for.
fn listed(ui: Ui, send: &Send, m: &Value) {
    let me = ui.state::<Tasks>();
    if let Some(b) = m["build"].as_str().filter(|b| me.build.with_untracked(|was| was != b)) {
        me.build.set(b.to_string());
    }
    let errors: Vec<&str> = m["errors"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    match m["asked"].as_str().unwrap_or("") {
        "run" => {
            if !errors.is_empty() {
                toast::warning(ui, &errors.iter().take(3).copied().collect::<Vec<_>>().join(" "));
            }
            let items = rows(m, false);
            if items.is_empty() {
                toast::info(ui, "No task to run found. Add one to .vscode/tasks.json.");
                return;
            }
            let send = send.clone();
            quick::pick("Run Task").placeholder("Select the task to run").items(items).open(ui, move |picked| {
                if let Some(item) = picked {
                    run(ui, &send, json!({"label": item.id}));
                }
            });
        }
        "terminate" | "running" => {
            let terminate = m["asked"] == "terminate";
            let mut items = rows(m, true);
            if items.is_empty() {
                toast::info(ui, "There are no running tasks.");
                return;
            }
            let labels: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
            let stop = |send: &Send, label: &str| send(json!({"type": "task_stop", "label": label}));
            if terminate && let [one] = labels.as_slice() {
                return stop(send, one);
            }
            if terminate {
                items.push(quick::item("", "All Running Tasks").icon("debug-stop"));
            }
            let (title, words) = if terminate {
                ("Terminate Task", "Select a task to terminate")
            } else {
                ("Show Running Tasks", "Select the task to show its output")
            };
            let (send, terms) = (send.clone(), m["items"].clone());
            quick::pick(title).placeholder(words).items(items).open(ui, move |picked| {
                let Some(item) = picked else { return };
                if !terminate {
                    let terminal = terms
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|t| t["label"] == item.id.as_str())
                        .and_then(|t| t["terminal"].as_u64());
                    if let Some(id) = terminal {
                        crate::terminal::show_tab(ui, id);
                    }
                } else if item.id.is_empty() {
                    labels.iter().for_each(|l| stop(&send, l));
                } else {
                    stop(&send, &item.id);
                }
            });
        }
        "build" => {
            let send = send.clone();
            let now = me.build.get_untracked();
            let items = ["Debug", "Release"]
                .iter()
                .map(|b| quick::item(b, b).description(if *b == now { "current" } else { "" }))
                .collect();
            quick::pick("Select Build Configuration")
                .placeholder("Select the build configuration of the folder's tasks")
                .items(items)
                .active(&now)
                .open(ui, move |picked| {
                    if let Some(item) = picked {
                        send(json!({"type": "build_configuration", "value": item.id}));
                    }
                });
        }
        _ => {}
    }
}

/// Restricted Mode: the commands the run would start are shown, and the person says whether they may run.
fn untrusted(ui: Ui, send: &Send, m: &Value) {
    let label = m["label"].as_str().unwrap_or("").to_string();
    let commands: Vec<&str> = m["commands"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let detail = format!(
        "This folder is in Restricted Mode: its tasks run only once you trust it. Running '{label}' runs:\n\n{}",
        commands.iter().map(|c| format!("  {c}")).collect::<Vec<_>>().join("\n")
    );
    let send = send.clone();
    dialog::ask(&format!("Run the task '{label}' of a folder you do not trust?"))
        .kind(Kind::Warning)
        .detail(&detail)
        .button("Trust Folder and Run")
        .button("Run Once")
        .open(ui, move |a| match a.button {
            Some(0) => {
                send(json!({"type": "trust_set", "trusted": true}));
                again(ui, &send, &label, json!({"allow": true}));
            }
            Some(1) => again(ui, &send, &label, json!({"allow": true})),
            _ => {}
        });
}

/// The session's messages that are this part's, taken in; none is taken away from the rest.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let label = m["label"].as_str().unwrap_or("");
    match m["type"].as_str().unwrap_or("") {
        "task_list" => listed(ui, send, m),
        "task_needs" => {
            let (send, label) = (send.clone(), label.to_string());
            let inputs = m["inputs"].as_array().cloned().unwrap_or_default();
            let had = ui.state::<Tasks>().asked.get_untracked()["inputs"].clone();
            ask_inputs(ui, &inputs, move |answers| {
                let Some(mut answers) = answers else { return };
                for (k, v) in had.as_object().into_iter().flatten() {
                    answers.entry(k.clone()).or_insert(v.clone());
                }
                again(ui, &send, &label, json!({"inputs": answers}));
            });
        }
        "task_untrusted" => untrusted(ui, send, m),
        "task_pick" => {
            let group = m["group"].as_str().unwrap_or("build").to_string();
            let items = m["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|l| quick::item(l, l).icon("tools"))
                .collect();
            let send = send.clone();
            let words = format!("Select the {group} task to run");
            quick::pick(&words).placeholder(&words).items(items).open(ui, move |picked| {
                if let Some(item) = picked {
                    again(ui, &send, &item.id, json!({}));
                }
            });
        }
        "task_failed" | "task_stop" => {
            if let Some(error) = m["error"].as_str() {
                toast::error_of(error).source("Tasks").show_in(ui);
            }
        }
        "task_started" => {
            if let (Some(old), Some(new)) = (m["replaces"].as_u64(), m["terminal"].as_u64()) {
                crate::terminal::reuse(ui, send, old, new);
            }
        }
        "build_configuration" => {
            if let Some(b) = m["value"].as_str() {
                ui.state::<Tasks>().build.set(b.to_string());
            }
            if let Some(error) = m["error"].as_str() {
                toast::error(ui, error);
            }
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    fn sent() -> (Send, Rc<RefCell<Vec<Value>>>) {
        let all = Rc::new(RefCell::new(Vec::new()));
        let seen = all.clone();
        (Rc::new(move |v| seen.borrow_mut().push(v)), all)
    }

    #[test]
    fn a_prompt_string_is_asked_in_an_input_box() {
        let a = ask_of(&json!({"id": "name", "type": "promptString", "description": "Name?", "default": "web"}));
        assert_eq!(a, Some(Ask::Text { id: "name".into(), prompt: "Name?".into(), value: "web".into(), password: false }));
    }

    #[test]
    fn a_password_is_asked_as_a_secret() {
        let a = ask_of(&json!({"id": "pw", "type": "promptString", "description": "Token", "password": true}));
        assert_eq!(a, Some(Ask::Text { id: "pw".into(), prompt: "Token".into(), value: String::new(), password: true }));
    }

    #[test]
    fn a_pick_string_offers_its_options_with_the_default_lit() {
        let a = ask_of(&json!({"id": "c", "type": "pickString", "description": "Which?", "default": "b",
            "options": ["a", {"label": "Bee", "value": "b"}, 7]}));
        let Some(Ask::Pick { title, options, active, .. }) = a else { panic!("a pick") };
        assert_eq!(title, "Which?");
        assert_eq!(active, "b");
        assert_eq!(
            options,
            vec![("a".to_string(), "a".to_string(), String::new()), ("Bee".into(), "b".into(), String::new())]
        );
    }

    #[test]
    fn a_pick_process_offers_the_processes_the_session_listed() {
        let a = ask_of(&json!({"id": "pid", "type": "pickProcess",
            "options": [{"label": "node server.js", "value": "4242", "description": "4242"}]}));
        let Some(Ask::Pick { title, options, .. }) = a else { panic!("a pick") };
        assert_eq!(title, "Select the process to attach to");
        assert_eq!(options, vec![("node server.js".to_string(), "4242".to_string(), "4242".to_string())]);
        assert_eq!(ask_of(&json!({"id": "x", "type": "command", "command": "x"})), None);
    }

    #[test]
    fn no_inputs_answers_at_once_and_one_opens_the_quick_input() {
        let ui = Ui::new();
        let got = Rc::new(RefCell::new(None));
        let g = got.clone();
        ask_inputs(ui, &[], move |a| *g.borrow_mut() = Some(a));
        assert_eq!(*got.borrow(), Some(Some(Map::new())));
        ask_inputs(ui, &[json!({"id": "n", "type": "promptString"})], |_| {});
        assert!(quick::is_open(ui));
    }

    #[test]
    fn run_build_task_sends_the_group_and_the_editor() {
        let ui = Ui::new();
        let (send, all) = sent();
        run(ui, &send, json!({"group": "build"}));
        assert_eq!(all.borrow()[0], json!({"type": "task_run", "group": "build", "context": {}}));
        again(ui, &send, "cargo: build", json!({"allow": true}));
        assert_eq!(all.borrow()[1], json!({"type": "task_run", "label": "cargo: build", "context": {}, "allow": true}));
    }

    #[test]
    fn the_run_task_pick_lists_configured_tasks_before_detected_ones() {
        let m = json!({"items": [
            {"label": "npm: build", "source": "npm", "command": "npm run build", "detail": ""},
            {"label": "mine", "source": "Workspace", "command": "make", "detail": "Mine", "running": true},
            {"label": "secret", "source": "Workspace", "command": "x", "hide": true},
        ]});
        let r = rows(&m, false);
        let labels: Vec<(&str, &str, &str)> =
            r.iter().map(|i| (i.label.as_str(), i.description.as_str(), i.group.as_str())).collect();
        assert_eq!(labels, vec![("mine", "Mine", "configured tasks"), ("npm: build", "npm run build", "detected tasks")]);
        assert_eq!(rows(&m, true).len(), 1);
    }

    #[test]
    fn terminate_with_one_running_task_stops_it_at_once() {
        let ui = Ui::new();
        let (send, all) = sent();
        let m = json!({"type": "task_list", "asked": "terminate", "items": [
            {"label": "watch", "source": "Workspace", "running": true, "terminal": 1000001},
            {"label": "idle", "source": "Workspace", "running": false}]});
        take(ui, &m, &send);
        assert_eq!(*all.borrow(), vec![json!({"type": "task_stop", "label": "watch"})]);
        assert!(!quick::is_open(ui));
    }

    #[test]
    fn a_task_run_again_takes_the_place_of_its_last_terminal() {
        let ui = Ui::new();
        let (send, all) = sent();
        let started = |id: u64, title: &str| json!({"type": "term_started", "id": id, "title": title, "kind": "task"});
        crate::terminal::take(ui, &started(1_000_001, "Task - a"), &send);
        crate::terminal::take(ui, &started(1_000_002, "Task - b"), &send);
        crate::terminal::ended(ui, 1_000_001, Some(0));
        crate::terminal::take(ui, &started(1_000_003, "Task - a"), &send);
        take(ui, &json!({"type": "task_started", "label": "a", "terminal": 1_000_003, "replaces": 1_000_001}), &send);
        let ids: Vec<u64> = ui.terms.with_untracked(|l| l.iter().map(|s| s.id).collect());
        assert_eq!(ids, vec![1_000_003, 1_000_002]);
        assert!(all.borrow().contains(&json!({"type": "term_close", "id": 1_000_001})));
        assert_eq!(
            crate::terminal::task_ended_text(Some(3)),
            "The terminal process terminated with exit code: 3. Terminal will be reused by tasks, press any key to \
             close it."
        );
        // One still running is not taken away.
        take(ui, &json!({"type": "task_started", "label": "b", "terminal": 1_000_004, "replaces": 1_000_002}), &send);
        assert_eq!(ui.terms.with_untracked(Vec::len), 2);
    }

    #[test]
    fn a_failure_is_said_in_a_toast() {
        let ui = Ui::new();
        let (send, _) = sent();
        take(ui, &json!({"type": "task_failed", "label": "a", "error": "The task 'a' ended with exit code 2."}), &send);
        assert_eq!(toast::count(ui), 1);
    }
}
