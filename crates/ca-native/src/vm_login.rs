//! Signing in to a virtual machine's guest, asked of the person when one of the agent's VM tools needs it: the form
//! "Sign in to VM" (what the guest is, and that it refused the last sign-in when it did), its user and password,
//! "Keep it in the keyring" when the keyring is here, Cancel and Sign in. Each question gets a fresh form; questions
//! asked while one is open wait their turn. A question answered from another face (`vm_login_done`) closes the form;
//! Escape, Cancel or a press beside the form answers it cancelled. While the person does not look at the chat, the
//! corner says it waits for them (`notify.rs`).
//!
//! Messages: `vm_login{id, vm, hypervisor, user, refused, can_save, keyring}` and `vm_login_done{id}` in, the waiting
//! ones in `state.vm_logins`; `vm_login_answer{id, user, password, save}` or `vm_login_answer{id, cancel: true}` out.
//! The password is cleared as soon as it is sent.

use floem::action::exec_after;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect};
use floem::style::CursorStyle;
use floem::views::{Decorators, container, h_stack, label};
use floem::{AnyView, IntoView};
use serde_json::{Value, json};

use crate::icons::icon;
use crate::layers::{self, Place};
use crate::state::{Send, Ui};
use crate::widgets::form::{self, Form};

/// How long after a sign-in the keys wait before they go back to the chat, for the guest to refuse it.
const BACK_TO_CHAT: std::time::Duration = std::time::Duration::from_millis(300);

/// A question, as the driver asked it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    pub id: u64,
    pub vm: String,
    pub hypervisor: String,
    pub user: String,
    pub refused: bool,
    pub can_save: bool,
    pub keyring: String,
}

impl Question {
    pub fn of(m: &Value) -> Option<Question> {
        let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
        Some(Question {
            id: m["id"].as_u64()?,
            vm: s("vm"),
            hypervisor: s("hypervisor"),
            user: s("user"),
            refused: m["refused"] == true,
            can_save: m["can_save"] == true,
            keyring: if s("keyring").is_empty() { "the keyring".into() } else { s("keyring") },
        })
    }

    /// The machine's name as the title says it: a VMware machine's `.vmx` file by its name only.
    fn short(&self) -> String {
        let name = self.vm.rsplit(['/', '\\']).next().unwrap_or(&self.vm);
        name.strip_suffix(".vmx").unwrap_or(name).to_string()
    }

    pub fn title(&self) -> String {
        format!("Sign in to {}", self.short())
    }

    pub fn detail(&self) -> String {
        let why = format!(
            "The agent's tools need the user and password of the guest to work in it ({}). They are never shown in \
             the chat.",
            if self.hypervisor.is_empty() { "the virtual machine" } else { self.hypervisor.as_str() }
        );
        if self.refused { format!("The guest refused the last sign-in. {why}") } else { why }
    }

    pub fn keep(&self) -> String {
        format!("Keep it in {}", self.keyring)
    }
}

/// A question arrived: it waits after the others, once (one asked again by `state` is already here).
pub fn arrived(waiting: &mut Vec<Question>, q: Question) {
    if !waiting.iter().any(|w| w.id == q.id) {
        waiting.push(q);
    }
}

/// The answer the driver is sent.
pub fn answer(id: u64, user: &str, password: &str, save: bool) -> Value {
    json!({"type": "vm_login_answer", "id": id, "user": user.trim(), "password": password, "save": save})
}

pub fn cancelled(id: u64) -> Value {
    json!({"type": "vm_login_answer", "id": id, "cancel": true})
}

/// The form and the questions.
#[derive(Clone, Copy)]
struct VmLogin {
    form: Form,
    /// The questions not answered yet; the first is the one the form asks while it is open.
    waiting: RwSignal<Vec<Question>>,
    /// The question the form asks now.
    asked: RwSignal<Option<Question>>,
    user: RwSignal<String>,
    password: RwSignal<String>,
    save: RwSignal<bool>,
}

impl crate::ext::State for VmLogin {
    fn new(ui: Ui) -> VmLogin {
        VmLogin {
            form: Form::new(ui),
            waiting: ui.scope.create_rw_signal(Vec::new()),
            asked: ui.scope.create_rw_signal(None),
            user: ui.scope.create_rw_signal(String::new()),
            password: ui.scope.create_rw_signal(String::new()),
            save: ui.scope.create_rw_signal(false),
        }
    }
}

impl VmLogin {
    /// The next question waiting, in a fresh form, when none is asked now.
    fn next(self) {
        if self.asked.with_untracked(Option::is_some) {
            return;
        }
        let Some(q) = self.waiting.with_untracked(|w| w.first().cloned()) else { return };
        self.user.set(q.user.clone());
        self.password.set(String::new());
        if self.save.get_untracked() {
            self.save.set(false);
        }
        self.asked.set(Some(q));
        self.form.show(None);
    }

    /// The question asked now is no longer (answered here or elsewhere): the form closes without telling anyone,
    /// and the next one is asked; when none waits a moment later, the keys go back to the chat. The moment lets a
    /// guest that refused the password ask again first: the chat taking the keys from the form asked again would
    /// close it, and so cancel the sign-in.
    fn finish(self, ui: Ui, id: u64) {
        self.waiting.update(|w| w.retain(|q| q.id != id));
        if self.asked.with_untracked(|a| a.as_ref().is_some_and(|q| q.id == id)) {
            self.asked.set(None);
            self.password.set(String::new());
            self.form.close();
            self.next();
            exec_after(BACK_TO_CHAT, move |_| {
                if self.asked.with_untracked(Option::is_none) {
                    crate::chat_bridge::focus(ui);
                }
            });
        }
    }
}

/// Sign in pressed (or Enter): sent when there is a user.
fn sign_in(ui: Ui, send: &Send) {
    let st = ui.state::<VmLogin>();
    let Some(q) = st.asked.get_untracked() else { return };
    let user = st.user.get_untracked();
    if user.trim().is_empty() {
        return;
    }
    let save = q.can_save && st.save.get_untracked();
    st.password.with_untracked(|p| send(answer(q.id, &user, p, save)));
    st.finish(ui, q.id);
}

fn host(ui: Ui, send: Send) -> AnyView {
    let (t, st) = (ui.colors, ui.state::<VmLogin>());
    let f = st.form;
    // The form closed by Escape, Cancel or a press beside it, with its question still asked: answered cancelled.
    {
        let send = send.clone();
        create_effect(move |_| {
            if !f.open.get()
                && let Some(q) = st.asked.get_untracked()
            {
                send(cancelled(q.id));
                // Not from inside the effect: the next question opens the form again.
                floem::action::exec_after(std::time::Duration::ZERO, move |_| st.finish(ui, q.id));
            }
        });
    }
    let go = {
        let send = send.clone();
        move || sign_in(ui, &send)
    };
    let user = f.named(ui, "User", f.input(ui, st.user, "User name", go.clone()));
    let password = f.named(ui, "Password", f.secret(ui, st.password, go.clone()));
    let keep = h_stack((
        container(icon("check", 14.0).style(move |s| s.apply_if(!st.save.get(), |s| s.hide()))).style(move |s| {
            let c = t.get();
            s.width(18.0)
                .height(18.0)
                .flex_shrink(0.0_f32)
                .items_center()
                .justify_center()
                .border(1.0)
                .border_radius(3.0)
                .border_color(c.checkbox_border)
                .background(c.checkbox_bg)
                .color(c.checkbox_fg)
        }),
        label(move || st.asked.with(|a| a.as_ref().map(Question::keep).unwrap_or_default()))
            .style(|s| s.selectable(false)),
    ))
    .on_click_stop(move |_| st.save.update(|on| *on = !*on))
    .style(move |s| {
        s.gap(8.0)
            .items_center()
            .cursor(CursorStyle::Pointer)
            .apply_if(!st.asked.with(|a| a.as_ref().is_some_and(|q| q.can_save)), |s| s.hide())
    })
    .into_any();
    let ready = move || st.user.with(|u| !u.trim().is_empty());
    let buttons = form::buttons(vec![
        form::button(ui, || "Cancel".into(), false, || true, move || f.close()),
        form::button(ui, || "Sign in".into(), true, ready, go),
    ]);
    f.host(
        ui,
        move || st.asked.with(|a| a.as_ref().map(Question::title).unwrap_or_default()),
        move || st.asked.with(|a| a.as_ref().map(Question::detail).unwrap_or_default()),
        vec![user, password, keep],
        buttons,
    )
}

/// Its layer over the window, once at start.
pub fn register(_ui: Ui, _send: &Send) {
    layers::add(Place::Over, "vm.login", 31, |ui, send| host(ui, send).into_any());
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let st = ui.state::<VmLogin>();
    match m["type"].as_str().unwrap_or("") {
        "vm_login" => {
            if let Some(q) = Question::of(m) {
                st.waiting.update(|w| arrived(w, q));
                st.next();
            }
            true
        }
        "vm_login_done" => {
            if let Some(id) = m["id"].as_u64() {
                st.finish(ui, id);
            }
            true
        }
        "state" => {
            let asked: Vec<Question> =
                m["vm_logins"].as_array().into_iter().flatten().filter_map(Question::of).collect();
            if !asked.is_empty() {
                st.waiting.update(|w| asked.into_iter().for_each(|q| arrived(w, q)));
                st.next();
            }
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(id: u64, refused: bool) -> Question {
        Question::of(&json!({"type": "vm_login", "id": id, "vm": "/vms/Win 11/Win 11.vmx", "hypervisor": "VMware",
            "user": "tester", "refused": refused, "can_save": true, "keyring": "the Keychain"}))
        .unwrap_or_else(|| Question {
            id,
            vm: String::new(),
            hypervisor: String::new(),
            user: String::new(),
            refused,
            can_save: false,
            keyring: String::new(),
        })
    }

    #[test]
    fn the_form_says_what_it_is_for_and_when_the_guest_refused() {
        let a = q(1, false);
        assert_eq!(a.title(), "Sign in to Win 11");
        assert!(a.detail().contains("(VMware)"));
        assert!(!a.detail().contains("refused"));
        assert!(q(2, true).detail().starts_with("The guest refused the last sign-in."));
        assert_eq!(a.keep(), "Keep it in the Keychain");
        let plain = Question::of(&json!({"id": 3, "vm": "test-vm"}));
        assert_eq!(plain.map(|p| p.keep()), Some("Keep it in the keyring".to_string()));
        assert!(Question::of(&json!({"vm": "x"})).is_none(), "a question without its id is not one");
    }

    #[test]
    fn the_corner_says_a_sign_in_waits_apart_from_the_calls() {
        let m = json!({"type": "vm_login", "id": 3, "vm": "test-vm", "hypervisor": "VirtualBox"});
        let said = crate::notify::said(&m);
        assert!(said.as_ref().is_some_and(|(_, title, _)| title == "Waiting for you: sign in to test-vm"));
        let ask = crate::notify::said(&json!({"type": "ask", "id": 3, "tool": "run_shell"}));
        assert_ne!(said.map(|s| s.0), ask.map(|s| s.0));
    }

    #[test]
    fn questions_wait_their_turn_once_each() {
        let mut w = vec![];
        arrived(&mut w, q(1, false));
        arrived(&mut w, q(2, false));
        arrived(&mut w, q(1, false));
        assert_eq!(w.iter().map(|q| q.id).collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn the_answer_carries_its_id() {
        let a = answer(4, " tester ", "fake-pass", true);
        assert_eq!(
            (a["type"].as_str(), a["id"].as_u64(), a["user"].as_str()),
            (Some("vm_login_answer"), Some(4), Some("tester"))
        );
        assert_eq!(cancelled(4), json!({"type": "vm_login_answer", "id": 4, "cancel": true}));
    }
}
