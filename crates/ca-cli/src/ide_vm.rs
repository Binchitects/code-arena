//! A virtual machine's sign-in, asked of the person when a VM tool needs it (`ca_tools::vm`).
//!
//! One of the IDE's areas: `ide_ext` says how an area joins the driver (its messages and their turn rules, its
//! fields in `state` and `tasks`, its part around a turn). Before every turn it gives the session's tools (and its
//! sub-agents') who asks: the faces, through these messages.
//!
//! Messages:
//! - Event `vm_login{id, vm, hypervisor, user, refused, can_save, keyring}`: a tool waits for the guest's user and
//!   password. `hypervisor` is its name as a person reads it, `user` the one to put first, `refused` that the guest
//!   refused the last sign-in, `can_save` that the keyring is here (`keyring` its name: "the keyring", ...).
//! - `vm_login_answer{id, user, password, save}` or `vm_login_answer{id, cancel: true}` [T]: the first face to
//!   answer decides; every face is told `vm_login_done{id}`, which a stopped turn also sends.
//! - `state` gains `vm_logins`: the questions still waiting, for a face that comes while one does.
//!
//! The password goes to the tool only: it is never said to the faces, kept, or written to the log.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ca_tools::vm::{Ask, Asker, Guest, Login, Signed};
use serde_json::{Value, json};
use tokio::sync::{broadcast, oneshot};

use crate::ide_ext::{Area, Ctx, Rule, Shared, send};
use crate::session::Prepared;

/// This area in the driver.
pub struct Here;

impl Area for Here {
    const MESSAGES: &'static [(&'static str, Rule)] = &[("vm_login_answer", Rule::Turn)];

    fn shared(sh: &Shared, m: &Value) -> bool {
        let waiting = sh.kept.of::<Waiting>();
        if let Some(id) = m["id"].as_u64() {
            answer(&waiting, &sh.out, id, m);
        }
        true
    }

    async fn before_turn(ctx: &mut Ctx<'_>, _message: &str) {
        let asker = Asker(Arc::new(Faces { waiting: ctx.sh.kept.of::<Waiting>(), out: ctx.sh.out.clone() }));
        ctx.p.agent.ctx.vm_login = Some(asker.clone());
        if let Ok(mut c) = ctx.p.crew.lock()
            && let Some(crew) = c.as_mut()
        {
            crew.ctx.vm_login = Some(asker);
        }
    }

    fn state(sh: &Shared, _p: Option<&Prepared>, v: &mut Value) {
        v["vm_logins"] = Value::Array(sh.kept.of::<Waiting>().asked());
    }
}

/// A question waiting: who is told the answer, and the question as the faces were sent it.
type Asked = HashMap<u64, (oneshot::Sender<Option<Signed>>, Value)>;

/// The questions waiting for an answer, by id: who is told it, and the question as the faces were sent it.
#[derive(Default)]
pub struct Waiting {
    next: AtomicU64,
    asked: Mutex<Asked>,
}

impl Waiting {
    fn with<R>(&self, f: impl FnOnce(&mut Asked) -> R) -> R {
        match self.asked.lock() {
            Ok(mut g) => f(&mut g),
            Err(e) => f(&mut e.into_inner()),
        }
    }

    /// The questions still waiting, oldest first.
    fn asked(&self) -> Vec<Value> {
        let mut all: Vec<(u64, Value)> = self.with(|a| a.iter().map(|(id, (_, q))| (*id, q.clone())).collect());
        all.sort_by_key(|(id, _)| *id);
        all.into_iter().map(|(_, q)| q).collect()
    }
}

/// The question as the faces are sent it.
pub fn question(id: u64, a: &Ask) -> Value {
    json!({
        "type": "vm_login",
        "id": id,
        "vm": a.vm,
        "hypervisor": a.hypervisor.name(),
        "user": a.user,
        "refused": a.refused,
        "can_save": a.can_save,
        "keyring": a.keyring,
    })
}

/// What a face answered: none when it cancelled (or sent no user).
pub fn answer_of(m: &Value) -> Option<Signed> {
    if m["cancel"] == true {
        return None;
    }
    let user = m["user"].as_str()?.trim();
    if user.is_empty() {
        return None;
    }
    let password = m["password"].as_str().unwrap_or("").to_string();
    Some(Signed { guest: Guest { user: user.to_string(), password }, save: m["save"] == true })
}

/// A face answered: the tool is told (once: a later answer to the same question is nothing), every face hears it is
/// answered.
fn answer(waiting: &Waiting, out: &broadcast::Sender<String>, id: u64, m: &Value) {
    if let Some((tell, _)) = waiting.with(|a| a.remove(&id)) {
        let _ = tell.send(answer_of(m));
        send(out, json!({"type": "vm_login_done", "id": id}));
    }
}

/// Who asks: every face, the first to answer decides.
struct Faces {
    waiting: Arc<Waiting>,
    out: broadcast::Sender<String>,
}

/// Takes a question away when its tool stops waiting for it (the turn was stopped), and tells the faces.
struct Gone<'a> {
    faces: &'a Faces,
    id: u64,
}

impl Drop for Gone<'_> {
    fn drop(&mut self) {
        if self.faces.waiting.with(|a| a.remove(&self.id)).is_some() {
            send(&self.faces.out, json!({"type": "vm_login_done", "id": self.id}));
        }
    }
}

#[async_trait::async_trait]
impl Login for Faces {
    async fn ask(&self, a: &Ask) -> Option<Signed> {
        let id = self.waiting.next.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        let q = question(id, a);
        self.waiting.with(|w| w.insert(id, (tx, q.clone())));
        let _gone = Gone { faces: self, id };
        send(&self.out, q);
        rx.await.ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use ca_tools::vm::Hypervisor;

    use super::*;

    fn ask() -> Ask {
        Ask {
            vm: "test-vm".into(),
            hypervisor: Hypervisor::VirtualBox,
            user: "tester".into(),
            refused: true,
            can_save: true,
            keyring: "the keyring".into(),
        }
    }

    #[test]
    fn the_question_carries_its_id_and_no_secret() {
        let q = question(7, &ask());
        assert_eq!(q["type"], "vm_login");
        assert_eq!(q["id"], 7);
        assert_eq!(q["hypervisor"], "VirtualBox");
        assert_eq!(q["refused"], true);
        assert!(q.get("password").is_none());
    }

    #[test]
    fn an_answer_is_read_and_a_cancel_is_none() {
        let a = answer_of(&json!({"id": 1, "user": " tester ", "password": "fake-pass", "save": true}));
        assert_eq!(
            a.as_ref().map(|s| (s.guest.user.as_str(), s.guest.password.as_str(), s.save)),
            Some(("tester", "fake-pass", true))
        );
        assert!(answer_of(&json!({"id": 1, "cancel": true, "user": "x"})).is_none());
        assert!(answer_of(&json!({"id": 1, "user": ""})).is_none());
    }

    #[tokio::test]
    async fn the_first_answer_decides_and_every_face_hears_it_is_done() {
        let (out, mut heard) = broadcast::channel(16);
        let waiting = Arc::new(Waiting::default());
        let faces = Faces { waiting: waiting.clone(), out: out.clone() };
        let a = ask();
        let asking = faces.ask(&a);
        tokio::pin!(asking);
        // The question goes out before anything is answered.
        tokio::select! {
            _ = &mut asking => panic!("answered with nobody answering"),
            _ = tokio::task::yield_now() => {}
        }
        let q: Value = serde_json::from_str(&heard.recv().await.unwrap_or_default()).unwrap_or_default();
        let id = q["id"].as_u64().unwrap_or(0);
        assert_eq!(waiting.asked().len(), 1);
        answer(&waiting, &out, id, &json!({"user": "tester", "password": "fake-pass"}));
        answer(&waiting, &out, id, &json!({"cancel": true}));
        let got = asking.await;
        assert_eq!(got.map(|s| s.guest.password), Some("fake-pass".to_string()));
        let done: Value = serde_json::from_str(&heard.recv().await.unwrap_or_default()).unwrap_or_default();
        assert_eq!((done["type"].as_str(), done["id"].as_u64()), (Some("vm_login_done"), Some(id)));
        assert!(heard.try_recv().is_err(), "the second answer is nothing");
        assert!(waiting.asked().is_empty());
    }

    #[tokio::test]
    async fn a_question_dropped_by_a_stopped_turn_is_done_for_the_faces() {
        let (out, mut heard) = broadcast::channel(16);
        let waiting = Arc::new(Waiting::default());
        let faces = Faces { waiting: waiting.clone(), out };
        let a = ask();
        {
            let asking = faces.ask(&a);
            tokio::pin!(asking);
            tokio::select! {
                _ = &mut asking => {}
                _ = tokio::task::yield_now() => {}
            }
        }
        let kinds: Vec<String> = std::iter::from_fn(|| heard.try_recv().ok())
            .filter_map(|m| serde_json::from_str::<Value>(&m).ok())
            .map(|v| v["type"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(kinds, ["vm_login", "vm_login_done"]);
        assert!(waiting.asked().is_empty());
    }
}
