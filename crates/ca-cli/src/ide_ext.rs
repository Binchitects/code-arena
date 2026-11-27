//! Where the IDE's areas join its driver. The driver (`ide.rs`) is one loop: a face (the native window, a browser's
//! page) sends a message, something answers it, and the answer goes to every face. An area (source control, the
//! language servers, the sessions, ...) is one file, `ide_<area>.rs`, that answers its own messages through what is
//! here, so that `ide.rs`, `ide_files.rs` and this file are not edited for it.
//!
//! # An area
//!
//! `ide_<area>.rs` holds `pub struct Here;` and `impl Area for Here`, and is named once in [`areas!`] below (every
//! planned area is, as a stub: fill your own file). An area overrides only what it needs of [`Area`]:
//!
//! - [`Area::MESSAGES`]: the messages it answers, each with its turn rule. Nothing reaches an area that is not
//!   listed there, and no two areas may list the same name (a test fails).
//! - [`Area::side`], [`Area::shared`], [`Area::idle`]: the three places a message is answered, by its rule (below).
//! - [`Area::state`], [`Area::status`], [`Area::tasks`]: fields it adds to the driver's own `state`, `status` and
//!   `tasks` messages (a new key each; an area never changes a key that is there).
//! - [`Area::shape`], [`Area::command`]: its part in a message the person sends (`send`, `again`): what the
//!   thread shows and the model reads, or the answer itself when the message is a command of the area's.
//! - [`Area::before_turn`], [`Area::after_turn`]: what it does around every turn.
//! - [`Area::start`]: its background work, begun when the driver starts and again when the session or the folder
//!   is another.
//!
//! # Turn rules
//!
//! While a turn runs the session itself ([`Prepared`]) is held by the turn. So every message says, in `MESSAGES`,
//! what becomes of it then:
//!
//! | Rule | Plan tag | Answered by | While a turn runs |
//! |---|---|---|---|
//! | [`Rule::Side`] | [T] | [`Area::side`] | at once: it needs only the project's files ([`Side`]) |
//! | [`Rule::Turn`] | [T] | [`Area::shared`] | at once, from the handles in [`Shared`] |
//! | [`Rule::Queue`] | [Q] | [`Area::idle`] | kept, and answered in the order asked once the turn has ended |
//! | [`Rule::Latest`] | [Q] | [`Area::idle`] | as `Queue`, but only the newest of its name is kept |
//! | [`Rule::Refuse`] | [R] | [`Area::idle`] | refused: a `notice` "Wait for the answer, or stop it, to X." and `refused` |
//!
//! `side` and `shared` are plain functions: they run inside the turn's loop, so they must come back at once.
//! Anything slow (a walk, git, a program, the network) goes to `tokio::task::spawn_blocking` or `tokio::spawn`
//! with clones of what it needs, and sends its reply from there. `idle` runs only between turns, with the session
//! in its hands ([`Ctx`]), and may await. A message kept for the turn's end is answered by the same `idle` as when
//! no turn ran, so its usual replies (and refusals) are how the faces hear of it; when it is kept the faces get
//! `deferred{of}`, and when it is refused `refused{of, reason}` (both with the key the message carried: `id`,
//! `q`, `path`, `key`, `name`, `query`).
//!
//! What a `shared` handler cannot reach in the session, it reads from [`Shared`]: the handles the session shares
//! (its store, plan, goals, jobs, memory, servers) and plain copies of the rest (the settings, the library's
//! sources, the models, and in [`Was`] the model, mode, thinking and window as they were when the session last
//! stood still). `Shared` is made again after every `idle` handler, so between turns it is never behind.
//!
//! # Messages
//!
//! - A name carries its area's prefix (`git_`, `lsp_`, `dap_`, `task_`, `nb_`, `ext_`, `session_`, `memory_`,
//!   `skill_`, ...). The driver's own replies are taken ([`TAKEN`]): no request may be named as one of them.
//! - Messages only gain fields. Replies go to every face, so each carries the key it answers.
//! - A name that begins with `_` is the driver's own: an area's background work posts it ([`Shared::post`]) to
//!   have its `idle` handler called with the session, between turns. From a face such a name is dropped.
//! - [`say`] is a `notice` in the thread; what a command written for the terminal says is collected with
//!   `crate::ui::Ui::capture()` and said with it.
//!
//! # Turns
//!
//! Only the driver's loop starts a turn. A handler that wants one sets `ctx.p.said_next`: after every message
//! answered between turns, and after every turn, the loop runs what `said_next` holds as the next turn (a plan's
//! review, a goal's next turn, a job that ended), with `user{carried: true}`. A turn the person stopped, or one
//! that failed, ends the chain (`said_next` is dropped after the areas' `after_turn`). After a turn the order is:
//! `turn_end`, each area's `after_turn`, `status`; then what waited for the turn ([Q]) in the order asked; then a
//! message the person sent too late for the turn to read (it runs as a turn of its own, begun with
//! `user{late: true}`: the faces showed it as `queued` when it was sent, and a face that keeps that bubble may put
//! the question in its place); then `said_next`.
//!
//! # Whose the shared messages are
//!
//! The messages several areas have a part in stay the driver's, and each area takes its part through a function
//! of its own: `send` and `again` through [`Area::shape`] and [`Area::command`] (the face's message, with
//! whatever fields the area gave it, is in [`Sent::msg`]; what the area adds to `user` and `queued` goes in
//! [`Sent::fields`]); `state`, `status` and `tasks` through the functions of those names. The terminals' and the
//! files' messages (`term_open`, `save`, `search`, ...) are `ide_files.rs`'s.
//!
//! # What an area keeps
//!
//! An area keeps no global: its own state is a type of its own in [`Kept`], `sh.kept.of::<Mine>()` for what lasts
//! as long as the driver and `side.kept.of::<Mine>()` for what belongs to the folder (it is new when another
//! folder is opened).

use std::any::{Any, TypeId};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Map, Value, json};
use tokio::sync::{broadcast, mpsc};

use crate::app::App;
use crate::ide_files::Side;
use crate::session::{Opts, Prepared};

/// What becomes of a message while a turn runs, and so which of an area's functions answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// [T] Answered at once by [`Area::side`]: it needs the project's files only.
    Side,
    /// [T] Answered at once by [`Area::shared`], from the handles kept in [`Shared`].
    Turn,
    /// [Q] Answered by [`Area::idle`]; while a turn runs it is kept and answered, in the order asked, when it ends.
    Queue,
    /// [Q] As `Queue`, but a newer one of the same name takes the place of one still waiting.
    Latest,
    /// [R] Answered by [`Area::idle`]; while a turn runs it is refused. The words say what for, after "Wait for
    /// the answer, or stop it, to": `Refuse("ask again")`.
    Refuse(&'static str),
}

/// The driver's own replies and events: a request named as one of them would be read by the faces as that reply.
#[allow(dead_code)] // Read by the test that keeps the names apart.
pub const TAKEN: &[&str] = &[
    "status",
    "tasks",
    "busy",
    "user",
    "turn_start",
    "turn_end",
    "text",
    "reasoning",
    "notice",
    "tool_start",
    "tool_end",
    "step_end",
    "ask",
    "answered",
    "queued",
    "context_detail",
    "setting_kept",
    "setting_refused",
    "theme",
    "refused",
    "deferred",
    "behind",
    "file",
    "saved",
    "created",
    "renamed",
    "deleted",
    "stats",
    "term_out",
    "term_exit",
];

/// The most messages kept for a turn's end; what comes after is refused.
pub const MOST_LATER: usize = 200;

/// Sends a message to every face.
pub fn send(out: &broadcast::Sender<String>, v: Value) {
    let _ = out.send(v.to_string());
}

/// A line in the thread, apart from the answer.
pub fn say(out: &broadcast::Sender<String>, text: &str) {
    send(out, json!({"type": "notice", "text": text}));
}

/// A message's name.
pub fn kind(m: &Value) -> &str {
    m["type"].as_str().unwrap_or("")
}

/// What a message is told apart by, copied into what answers it without the session (`deferred`, `refused`).
fn with_key(mut reply: Value, m: &Value) -> Value {
    for k in ["id", "q", "path", "key", "name", "query"] {
        if !m[k].is_null() && !m[k].is_object() && !m[k].is_array() {
            reply[k] = m[k].clone();
        }
    }
    reply
}

/// Tells the faces a message was not taken, and why: a line in the thread and `refused{of, reason}`.
pub fn refuse(out: &broadcast::Sender<String>, m: &Value, reason: &str) {
    say(out, reason);
    send(out, with_key(json!({"type": "refused", "of": kind(m), "reason": reason}), m));
}

/// What a message refused while a turn runs is told: `to` are its rule's words.
pub fn wait_to(to: &str) -> String {
    format!("Wait for the answer, or stop it, to {to}.")
}

/// The messages that wait for the turn to end, in the order they were asked.
#[derive(Default)]
pub struct Later(VecDeque<Value>);

impl Later {
    /// Keeps a message for the turn's end; `latest`: in place of one of its name still waiting. `false`: too many
    /// wait already, it was not kept.
    pub fn keep(&mut self, m: Value, latest: bool) -> bool {
        if latest {
            let name = kind(&m).to_string();
            self.0.retain(|w| kind(w) != name);
        }
        if self.0.len() >= MOST_LATER {
            return false;
        }
        self.0.push_back(m);
        true
    }

    /// The one that waited longest, taken out.
    pub fn next(&mut self) -> Option<Value> {
        self.0.pop_front()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Keeps a message for the turn's end and tells the faces (`deferred`), or refuses it when too many wait.
pub fn defer(m: &Value, later: &mut Later, out: &broadcast::Sender<String>, latest: bool) {
    if later.keep(m.clone(), latest) {
        send(out, with_key(json!({"type": "deferred", "of": kind(m)}), m));
    } else {
        refuse(out, m, "Too much waits for the answer to end already: wait for it, or stop it.");
    }
}

/// A message that came while a turn runs: `true` when it is answered now ([T], by the driver or [`shared`]).
/// Else it was kept for the turn's end and the faces told `deferred` ([Q]), or refused and the faces told why
/// ([R], or too many waiting), or it is nobody's and is left.
pub fn meanwhile(m: &Value, later: &mut Later, out: &broadcast::Sender<String>) -> bool {
    match rule(kind(m)) {
        Some(Rule::Turn) => true,
        Some(Rule::Side) | None => false,
        Some(r @ (Rule::Queue | Rule::Latest)) => {
            defer(m, later, out, r == Rule::Latest);
            false
        }
        Some(Rule::Refuse(to)) => {
            refuse(out, m, &wait_to(to));
            false
        }
    }
}

/// What an area keeps, one value per type, made the first time it is asked for. The value guards what changes in
/// it itself (a `Mutex` inside).
#[derive(Clone, Default)]
pub struct Kept(Arc<Mutex<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>>);

impl Kept {
    #[allow(dead_code)] // The areas' own state: none keeps any yet.
    pub fn of<T: Default + Send + Sync + 'static>(&self) -> Arc<T> {
        let held = || match self.0.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        let had = held().get(&TypeId::of::<T>()).cloned();
        // Made with nothing held, so that a value whose making asks for another one's does not wait on itself.
        let any = match had {
            Some(any) => any,
            None => {
                let made: Arc<dyn Any + Send + Sync> = Arc::new(T::default());
                held().entry(TypeId::of::<T>()).or_insert(made).clone()
            }
        };
        // Kept by its own type, so it is that type.
        any.downcast::<T>().unwrap_or_else(|_| Arc::new(T::default()))
    }
}

/// What the driver holds for as long as it runs, whatever session it is working in.
#[allow(dead_code)] // What taking up another session needs is read by the areas that do it.
pub struct Driver {
    /// To every face.
    pub out: broadcast::Sender<String>,
    /// To the driver itself: see [`Shared::post`].
    pub post: mpsc::UnboundedSender<Value>,
    /// Who asks the person (the cards in the thread): a session taken up in this one's place is prepared with it.
    pub approver: Arc<dyn ca_agent::Approver>,
    /// What the person sent while a turn ran, for the turn to read: a session taken up keeps it
    /// (`p.agent.inbox = Some(drv.inbox.clone())`).
    pub inbox: Arc<Mutex<Vec<String>>>,
    /// How the IDE was started (the folder, the mode, the model): what a session taken up is prepared from.
    pub opts: Opts,
    /// The models the pickers offer.
    pub models: Vec<String>,
    /// The Arena signed in to, when there is one (made once: whoever signs in or out puts the new one here).
    pub arena: Option<ca_arena::ArenaClient>,
    /// What waits for the turn to end.
    pub later: Later,
    /// What the areas keep for as long as the driver runs.
    pub kept: Kept,
}

/// The session as a handler may reach it while a turn holds the session itself: the handles the session shares,
/// and plain copies of the rest. A clone is cheap enough to move into a task.
#[derive(Clone)]
#[allow(dead_code)] // The areas read these; the driver itself needs few of them.
pub struct Shared {
    pub out: broadcast::Sender<String>,
    post: mpsc::UnboundedSender<Value>,
    /// The project's files, terminals and watching.
    pub side: Arc<Side>,
    /// What the areas keep for as long as the driver runs.
    pub kept: Kept,
    pub store: Arc<Mutex<ca_session::Store>>,
    /// The session's id in the store.
    pub session: String,
    pub folder: String,
    pub workspace: Arc<ca_workspace::Workspace>,
    pub checkpoints: Arc<ca_tools::checkpoints::Checkpoints>,
    pub plan: Arc<ca_tools::plan::PlanBook>,
    pub goals: Arc<ca_agent::goal::GoalBook>,
    pub jobs: Option<Arc<ca_tools::jobs::Jobs>>,
    pub memory: Arc<ca_memory::Memory>,
    pub servers: crate::servers::Servers,
    /// The session's chat in Arena, when it is kept there.
    pub sync: Option<Arc<ca_sync::ChatSync>>,
    pub notices: crate::session::Notices,
    pub loop_reports: crate::loops::Reports,
    /// What the person sent for the running turn to read.
    pub inbox: Arc<Mutex<Vec<String>>>,
    /// Asks the model to stop thinking and answer.
    pub skip: Arc<ca_agent::Skip>,
    pub gateway: ca_llm::Gateway,
    /// Where commands, skills and sub-agents are read from.
    pub sources: Arc<Vec<ca_agent::library::Source>>,
    pub settings: Arc<ca_settings::Settings>,
    pub paths: ca_core::Paths,
    pub home: Option<PathBuf>,
    pub edition: ca_settings::Edition,
    /// The Arena signed in to, when there is one.
    pub arena: Option<ca_arena::ArenaClient>,
    /// The Arena's address, when there is one.
    pub arena_url: Option<String>,
    pub models: Arc<Vec<String>>,
    pub was: Was,
}

/// What only the session itself knows, as it was when the session last stood still (before the turn, while one
/// runs).
#[derive(Clone, Debug, Default, PartialEq)]
#[allow(dead_code)] // For the areas' answers while a turn runs.
pub struct Was {
    pub model: String,
    pub mode: String,
    pub thinking: String,
    /// The window: `{used, size, compact_at}`.
    pub context: Value,
}

impl Shared {
    /// The session's handles and copies, as it stands now.
    pub fn of(p: &Prepared, app: &App, drv: &Driver, side: &Arc<Side>) -> Shared {
        let w = &p.agent.window;
        Shared {
            out: drv.out.clone(),
            post: drv.post.clone(),
            side: side.clone(),
            kept: drv.kept.clone(),
            store: p.store.clone(),
            session: p.session.clone(),
            folder: p.folder.clone(),
            workspace: p.agent.ctx.workspace.clone(),
            checkpoints: p.agent.ctx.checkpoints.clone(),
            plan: p.agent.ctx.plan.clone(),
            goals: p.goals.clone(),
            jobs: p.agent.ctx.jobs.clone(),
            memory: p.memory.clone(),
            servers: p.servers.clone(),
            sync: p.sync.clone(),
            notices: p.notices.clone(),
            loop_reports: p.loop_reports.clone(),
            inbox: drv.inbox.clone(),
            skip: p.agent.skip.clone(),
            gateway: p.agent.gateway.clone(),
            sources: Arc::new(p.sources.clone()),
            settings: Arc::new(app.settings.clone()),
            paths: app.paths.clone(),
            home: app.home.clone(),
            edition: app.edition(),
            arena: drv.arena.clone(),
            arena_url: app.connection.url.clone(),
            models: Arc::new(drv.models.clone()),
            was: Was {
                model: p.agent.model.clone(),
                mode: p.agent.policy.mode.name().to_string(),
                thinking: crate::ide::thinking_name(&p.agent.thinking),
                context: json!({"used": p.agent.context_used(), "size": w.size, "compact_at": w.compact_at}),
            },
        }
    }

    /// Sends a message to the driver itself, as if a face had: an area's background work has its `idle` handler
    /// called with the session this way, by its rule (at once between turns, kept or refused while one runs).
    /// Name it with a `_` first, so that no face can send it.
    #[allow(dead_code)] // For the areas' background work.
    pub fn post(&self, m: Value) {
        let _ = self.post.send(m);
    }

    /// A line in the thread.
    #[allow(dead_code)] // For the areas' answers.
    pub fn say(&self, text: &str) {
        say(&self.out, text);
    }
}

/// The session in a handler's hands, between turns.
#[allow(dead_code)] // The areas' handlers reach the session through it.
pub struct Ctx<'a> {
    pub app: &'a mut App,
    /// The session. Another may be put in its place (`*ctx.p = session::prepare(ctx.app, &ctx.drv.opts,
    /// ctx.drv.approver.clone(), resume).await?`, then `ctx.p.agent.inbox = Some(ctx.drv.inbox.clone())`): the
    /// driver sees it is another and sends `state` and `tasks` itself.
    pub p: &'a mut Prepared,
    /// The project's side. Another may be put in its place when the folder is another (`Side::new`, then
    /// `start_watching`): the one before ends with its last holder, its watching and its terminals with it.
    pub side: &'a mut Arc<Side>,
    pub drv: &'a mut Driver,
    /// The session as it was before this handler: made again after it.
    pub sh: &'a Shared,
}

/// A message the person sent, on its way to the model.
#[allow(dead_code)] // The areas shape it; the driver reads what they made of it.
pub struct Sent {
    /// The face's message as it came (its `context`, `quote`, `attachments`).
    pub msg: Value,
    /// What the thread shows as the question.
    pub shown: String,
    /// What the model reads.
    pub model: String,
    /// A turn runs: the message joins it at its next step (`queued`) instead of starting one.
    pub meanwhile: bool,
    /// Set by an area while a turn runs: keep the whole message until the turn has ended and send it then (a
    /// command needs the session).
    pub later: bool,
    /// Set by an area that answered it itself: nothing goes to the model.
    pub done: bool,
    /// More fields for the `user` (or `queued`) message the faces get.
    pub fields: Map<String, Value>,
}

impl Sent {
    pub fn new(msg: &Value, text: String, meanwhile: bool) -> Sent {
        Sent {
            msg: msg.clone(),
            shown: text.clone(),
            model: text,
            meanwhile,
            later: false,
            done: false,
            fields: Map::new(),
        }
    }

    /// A message for the faces with the areas' fields in it (none takes the place of one that is there).
    pub fn with_fields(&self, mut v: Value) -> Value {
        if let Some(o) = v.as_object_mut() {
            for (k, f) in &self.fields {
                o.entry(k.clone()).or_insert_with(|| f.clone());
            }
        }
        v
    }
}

/// How a turn ended.
#[allow(dead_code)] // For the areas' `after_turn`.
pub struct TurnEnd {
    /// What the model was sent.
    pub message: String,
    /// What the turn did, when it ended well.
    pub outcome: Option<ca_agent::TurnOutcome>,
    /// The person stopped it.
    pub stopped: bool,
    /// Why it failed, when it did.
    pub error: Option<String>,
    pub took: std::time::Duration,
    /// The session started it itself (`said_next`), not the person.
    pub carried: bool,
}

/// One area of the IDE in the driver. Every function has a default that does nothing: an area overrides what it
/// needs (see the module's text).
pub trait Area {
    /// The messages this area answers, each with its turn rule.
    const MESSAGES: &'static [(&'static str, Rule)] = &[];

    /// The area's background work begins: when the driver starts, and again when the session or the folder is
    /// another (what was begun for the one before should end by itself: hold `sh.session` and compare).
    fn start(sh: &Shared) {
        let _ = sh;
    }

    /// Answers a [`Rule::Side`] message. `true`: it was this area's.
    fn side(side: &Side, m: &Value) -> bool {
        let _ = (side, m);
        false
    }

    /// Answers a [`Rule::Turn`] message, a turn running or not. `true`: it was this area's.
    fn shared(sh: &Shared, m: &Value) -> bool {
        let _ = (sh, m);
        false
    }

    /// Answers a [`Rule::Queue`], [`Rule::Latest`] or [`Rule::Refuse`] message, between turns. `true`: it was
    /// this area's.
    async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
        let _ = (ctx, m);
        false
    }

    /// The person sent a message: the area's part of what the thread shows and the model reads (a quote put
    /// first, a file's lines attached). Called between turns and while one runs (`sent.meanwhile`).
    fn shape(sh: &Shared, sent: &mut Sent) {
        let _ = (sh, sent);
    }

    /// The person sent a message between turns: a command of this area's is answered here (`sent.done = true`).
    async fn command(ctx: &mut Ctx<'_>, sent: &mut Sent) {
        let _ = (ctx, sent);
    }

    /// A turn is about to begin with this message.
    async fn before_turn(ctx: &mut Ctx<'_>, message: &str) {
        let _ = (ctx, message);
    }

    /// A turn ended (`turn_end` is sent already). Setting `ctx.p.said_next` has the session carry on by itself.
    async fn after_turn(ctx: &mut Ctx<'_>, end: &TurnEnd) {
        let _ = (ctx, end);
    }

    /// Fields this area adds to `state` (`p`: the session, when no turn holds it).
    fn state(sh: &Shared, p: Option<&Prepared>, v: &mut Value) {
        let _ = (sh, p, v);
    }

    /// Fields this area adds to `status`.
    fn status(sh: &Shared, p: Option<&Prepared>, v: &mut Value) {
        let _ = (sh, p, v);
    }

    /// Fields this area adds to `tasks` (said while a turn runs too, so from the handles only).
    fn tasks(sh: &Shared, v: &mut Value) {
        let _ = (sh, v);
    }
}

/// Names every area once: its messages join the table, and each of the driver's calls reaches it.
macro_rules! areas {
    ($($area:ident),* $(,)?) => {
        /// Who claims which messages: the driver itself, the project's side, then the areas.
        fn claims() -> Vec<(&'static str, &'static [(&'static str, Rule)])> {
            vec![
                ("ide", crate::ide::MESSAGES),
                ("ide_files", crate::ide_files::MESSAGES),
                $((stringify!($area), <crate::$area::Here as Area>::MESSAGES),)*
            ]
        }

        /// A file-side message of an area's, answered ([`Rule::Side`]). `Side::handle` asks here first.
        pub fn side(side: &Side, m: &Value) -> bool {
            let Some((area, Rule::Side)) = owner(kind(m)) else { return false };
            $(if area == stringify!($area) {
                return <crate::$area::Here as Area>::side(side, m);
            })*
            false
        }

        /// A message of an area's that is answered whether a turn runs or not ([`Rule::Turn`]).
        pub fn shared(sh: &Shared, m: &Value) -> bool {
            let Some((area, Rule::Turn)) = owner(kind(m)) else { return false };
            $(if area == stringify!($area) {
                return <crate::$area::Here as Area>::shared(sh, m);
            })*
            false
        }

        /// A message of an area's that needs the session, answered between turns.
        pub async fn idle(ctx: &mut Ctx<'_>, m: &Value) -> bool {
            let Some((area, Rule::Queue | Rule::Latest | Rule::Refuse(_))) = owner(kind(m)) else { return false };
            $(if area == stringify!($area) {
                return <crate::$area::Here as Area>::idle(ctx, m).await;
            })*
            false
        }

        /// Every area's background work begins.
        pub fn start(sh: &Shared) {
            $(<crate::$area::Here as Area>::start(sh);)*
        }

        /// Every area's part in a message the person sent.
        pub fn shape(sh: &Shared, sent: &mut Sent) {
            $(<crate::$area::Here as Area>::shape(sh, sent);)*
        }

        /// A message the person sent between turns, offered to each area until one answers it itself.
        pub async fn command(ctx: &mut Ctx<'_>, sent: &mut Sent) {
            $(if !sent.done {
                <crate::$area::Here as Area>::command(ctx, sent).await;
            })*
        }

        pub async fn before_turn(ctx: &mut Ctx<'_>, message: &str) {
            $(<crate::$area::Here as Area>::before_turn(ctx, message).await;)*
        }

        pub async fn after_turn(ctx: &mut Ctx<'_>, end: &TurnEnd) {
            $(<crate::$area::Here as Area>::after_turn(ctx, end).await;)*
        }

        /// The areas' fields in `state`.
        pub fn state(sh: &Shared, p: Option<&Prepared>, v: &mut Value) {
            $(<crate::$area::Here as Area>::state(sh, p, v);)*
        }

        /// The areas' fields in `status`.
        pub fn status(sh: &Shared, p: Option<&Prepared>, v: &mut Value) {
            $(<crate::$area::Here as Area>::status(sh, p, v);)*
        }

        /// The areas' fields in `tasks`.
        pub fn tasks(sh: &Shared, v: &mut Value) {
            $(<crate::$area::Here as Area>::tasks(sh, v);)*
        }
    };
}

/// The areas, each named once. The tests add one of their own (`ide_probe`), which shows that a message reaches
/// its area.
macro_rules! listed {
    ($($probe:ident)?) => {
        areas!(
            ide_account,
            ide_agents,
            ide_arena,
            ide_attach,
            ide_automation,
            ide_banners,
            ide_chat,
            ide_complete,
            ide_dap,
            ide_extensions,
            ide_folders,
            ide_format,
            ide_git,
            ide_goal,
            ide_harness,
            ide_lsp,
            ide_media,
            ide_memory,
            ide_notebook,
            ide_plan,
            ide_problems,
            ide_review,
            ide_sessions,
            ide_settings,
            ide_skills,
            ide_snippets,
            ide_symbols,
            ide_tasks,
            ide_tools,
            ide_trust,
            ide_update,
            ide_vm,
            $($probe,)?
        );
    };
}

#[cfg(not(test))]
listed!();
#[cfg(test)]
listed!(ide_probe);

/// Every message by its name: whose it is, and its rule. The first to claim a name has it (a test fails when a
/// second does).
fn table() -> &'static HashMap<&'static str, (&'static str, Rule)> {
    static TABLE: OnceLock<HashMap<&'static str, (&'static str, Rule)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = HashMap::new();
        for (area, messages) in claims() {
            for (name, rule) in messages {
                t.entry(*name).or_insert((area, *rule));
            }
        }
        t
    })
}

fn owner(name: &str) -> Option<(&'static str, Rule)> {
    table().get(name).copied()
}

/// A message's turn rule; `None`: nobody answers it.
pub fn rule(name: &str) -> Option<Rule> {
    owner(name).map(|(_, r)| r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn got(rx: &mut broadcast::Receiver<String>) -> Vec<Value> {
        let mut all = Vec::new();
        while let Ok(t) = rx.try_recv() {
            all.push(serde_json::from_str(&t).expect("json"));
        }
        all
    }

    #[test]
    fn every_message_has_one_owner_and_none_is_named_as_a_reply_of_the_drivers() {
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for (area, messages) in claims() {
            for (name, rule) in messages {
                if let Some(first) = seen.insert(name, area) {
                    panic!("{name} is claimed by {first} and by {area}");
                }
                assert!(!name.is_empty() && !TAKEN.contains(name), "{area}: {name} is a reply of the driver's");
                if let Rule::Refuse(to) = rule {
                    assert!(!to.is_empty() && !to.ends_with('.'), "{area}: {name} says what it is refused for");
                }
                // The project's side answers its own without the session; the driver's own all need it.
                match area {
                    "ide_files" => assert_eq!(*rule, Rule::Side, "{name}"),
                    "ide" => assert_ne!(*rule, Rule::Side, "{name}"),
                    _ => {}
                }
            }
        }
        assert_eq!(owner("send"), Some(("ide", Rule::Turn)));
        assert_eq!(owner("open"), Some(("ide_files", Rule::Side)));
        assert_eq!(rule("no_such_message"), None);
    }

    #[test]
    fn what_comes_while_a_turn_runs_is_answered_kept_or_refused_by_its_rule() {
        let (out, mut rx) = broadcast::channel(64);
        let mut later = Later::default();
        // [T]: now, and nothing said about it.
        assert!(meanwhile(&json!({"type": "stop_job", "id": 2}), &mut later, &out));
        assert!(got(&mut rx).is_empty() && later.is_empty());
        // [Q]: kept, in the order asked, and the faces told.
        assert!(!meanwhile(&json!({"type": "model", "model": "qwen-large"}), &mut later, &out));
        assert!(!meanwhile(&json!({"type": "set_setting", "key": "a", "value": 1}), &mut later, &out));
        assert!(!meanwhile(&json!({"type": "thinking", "level": "high"}), &mut later, &out));
        assert!(!meanwhile(&json!({"type": "set_setting", "key": "b", "value": 2}), &mut later, &out));
        let said = got(&mut rx);
        assert_eq!(said.len(), 4, "{said:?}");
        assert_eq!(said[1], json!({"type": "deferred", "of": "set_setting", "key": "a"}));
        // [R]: refused with the reason, as a line in the thread and for the face that asked.
        assert!(!meanwhile(&json!({"type": "again", "q": 3, "text": "once more"}), &mut later, &out));
        let said = got(&mut rx);
        let reason = "Wait for the answer, or stop it, to ask again.";
        assert_eq!(said[0], json!({"type": "notice", "text": reason}));
        assert_eq!(said[1], json!({"type": "refused", "of": "again", "reason": reason, "q": 3}));
        // Nobody's, and the project's own (answered before this is asked): left.
        assert!(!meanwhile(&json!({"type": "bye"}), &mut later, &out));
        assert!(!meanwhile(&json!({"type": "ls", "path": ""}), &mut later, &out));
        assert!(got(&mut rx).is_empty());
        // When the turn has ended: each in the order it was asked.
        let order: Vec<String> = std::iter::from_fn(|| later.next())
            .map(|m| format!("{}{}", kind(&m), m["key"].as_str().unwrap_or("")))
            .collect();
        assert_eq!(order, ["model", "set_settinga", "thinking", "set_settingb"]);
    }

    #[test]
    fn only_the_newest_of_a_latest_message_waits_and_not_more_than_the_most() {
        let (out, mut rx) = broadcast::channel(1024);
        let mut later = Later::default();
        assert!(later.keep(json!({"type": "context", "n": 1}), true));
        assert!(later.keep(json!({"type": "model", "model": "a"}), false));
        assert!(later.keep(json!({"type": "context", "n": 2}), true));
        assert_eq!(later.len(), 2);
        assert_eq!(later.next().expect("first")["model"], "a");
        assert_eq!(later.next().expect("then")["n"], 2);
        for i in 0..MOST_LATER {
            assert!(!meanwhile(&json!({"type": "mode", "mode": "ask", "id": i}), &mut later, &out));
        }
        assert_eq!(later.len(), MOST_LATER);
        got(&mut rx);
        assert!(!meanwhile(&json!({"type": "mode", "mode": "plan"}), &mut later, &out));
        let said = got(&mut rx);
        assert_eq!(said[1]["type"], "refused", "{said:?}");
        assert_eq!(later.len(), MOST_LATER, "the one too many was not kept");
        // A newer one of a kind already waiting still takes its place.
        assert!(later.keep(json!({"type": "mode", "mode": "yolo"}), true));
        assert_eq!(later.len(), 1);
    }

    #[test]
    fn an_area_keeps_one_value_of_each_type() {
        #[derive(Default)]
        struct Mine(Mutex<u32>);
        #[derive(Default)]
        struct Other(Mutex<u32>);
        let kept = Kept::default();
        *kept.of::<Mine>().0.lock().expect("lock") += 2;
        *kept.clone().of::<Mine>().0.lock().expect("lock") += 3;
        assert_eq!(*kept.of::<Mine>().0.lock().expect("lock"), 5, "the same value each time");
        assert_eq!(*kept.of::<Other>().0.lock().expect("lock"), 0, "another type, another value");
        assert_eq!(*Kept::default().of::<Mine>().0.lock().expect("lock"), 0, "another keeper, another value");
        // A value that is made from another one's: asked for while the first is being made.
        struct Both(Kept, Arc<Mine>);
        impl Default for Both {
            fn default() -> Self {
                let kept = KEEPER.with(|k| k.borrow().clone());
                let mine = kept.of::<Mine>();
                Both(kept, mine)
            }
        }
        thread_local!(static KEEPER: std::cell::RefCell<Kept> = std::cell::RefCell::new(Kept::default()));
        KEEPER.with(|k| *k.borrow_mut() = kept.clone());
        let both = kept.of::<Both>();
        assert_eq!(*both.1.0.lock().expect("lock"), 5, "and it is the one kept");
        assert!(Arc::ptr_eq(&both, &both.0.of::<Both>()));
    }

    #[test]
    fn the_areas_fields_join_a_message_without_taking_a_place() {
        let mut s = Sent::new(&json!({"type": "send", "text": "hello"}), "hello".into(), false);
        s.fields.insert("quote".into(), json!("a line"));
        s.fields.insert("text".into(), json!("not this"));
        assert_eq!(
            s.with_fields(json!({"type": "user", "text": "hello", "q": 1})),
            json!({"type": "user", "text": "hello", "q": 1, "quote": "a line"})
        );
        assert_eq!((s.shown.as_str(), s.model.as_str(), s.later, s.done), ("hello", "hello", false, false));
    }
}
