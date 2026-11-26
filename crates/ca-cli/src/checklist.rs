//! Checklists in a session: `/checklist` turns one of the person's (or the repository's) on in the session's chat in
//! Arena, the model goes through it and ticks each item with `checklist_check`, and the web chat shows the same ticks.

use std::sync::Arc;

use ca_arena::checklists::{self, Checklist, INSTRUCTIONS, Run};
use ca_llm::ToolDef;
use ca_sync::ChatSync;
use ca_tools::{Context, Kind, Output, Tool};
use serde_json::{Value, json};

use crate::session::Prepared;

/// `checklist_check`: the model ticks an item of the checklist on in this session.
pub struct CheckTool {
    pub sync: Arc<ChatSync>,
}

#[async_trait::async_trait]
impl Tool for CheckTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "checklist_check".into(),
            description: "Ticks an item of the checklist the person turned on in this session (or takes the tick back), with how it was done or checked.".into(),
            parameters: json!({"type": "object", "properties": {
                "item": {"type": "integer", "description": "The item's number (1 is the first)."},
                "done": {"type": "boolean", "description": "false takes the tick back (default true)."},
                "note": {"type": "string", "description": "How it was done or checked, or why it does not apply: short, concrete."}
            }, "required": ["item", "note"]}),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, args: &Value) -> String {
        format!("item {}", args.get("item").and_then(Value::as_u64).unwrap_or(0))
    }
    async fn run(&self, args: Value, _ctx: &Context) -> Output {
        let Some(chat) = self.sync.chat_id() else {
            return Output::error("No checklist is on in this session (the person turns one on with /checklist).");
        };
        let item = args.get("item").and_then(Value::as_u64).unwrap_or(0) as u32;
        let done = args.get("done").and_then(Value::as_bool) != Some(false);
        let note = args.get("note").and_then(Value::as_str);
        match self.sync.arena().tick(&chat, item, done, note, "model").await {
            Ok(Some(run)) => Output::ok(run.text()),
            Ok(None) => Output::error("No checklist is on in this session (the person turns one on with /checklist)."),
            Err(e) => Output::error(e.to_string()),
        }
    }
}

fn pick<'a>(lists: &'a [Checklist], which: &str) -> Option<&'a Checklist> {
    match which.parse::<usize>() {
        Ok(n) if (1..=lists.len()).contains(&n) => lists.get(n - 1),
        _ => lists.iter().find(|c| c.name.eq_ignore_ascii_case(which)),
    }
}

/// `/checklist`: none, the one on and the person's checklists; `NAME` or `N` turns one on; `off`; `tick N`, `untick N`;
/// `new NAME: item; item`, `edit N: item; item`, `delete N` (on an Arena that has them). The text to show.
pub async fn command(rest: &str, p: &mut Prepared) -> String {
    let Some(sync) = p.sync.clone() else {
        return "Checklists are kept in Arena: sign in to it first (code-arena login), so this session is kept in a chat there.".into();
    };
    let arena = sync.arena().clone();
    let root = p.agent.ctx.workspace.root().to_path_buf();
    let (word, more) = rest.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((rest, ""));
    let result: Result<String, String> = async {
        let e = |e: ca_arena::ArenaError| e.to_string();
        match word.to_ascii_lowercase().as_str() {
            "" => {
                let lists = arena.checklists().await.map_err(e)?;
                let on = match sync.chat_id() {
                    Some(c) => arena.checklist_run(&c).await.map_err(e)?,
                    None => None,
                };
                let repo = checklists::repo_checklists(&root);
                let mut t = match on {
                    Some(r) => r.text(),
                    None => "No checklist is on in this session.".into(),
                };
                if lists.is_empty() {
                    t.push_str("\nYou have no checklists yet: /checklist new NAME: an item; another (or Arena's Workspace → Checklists).");
                } else {
                    let names: Vec<String> = lists.iter().enumerate().map(|(i, c)| format!("{}. {}", i + 1, c.name)).collect();
                    t.push_str(&format!("\nYours: {} (/checklist N or NAME turns one on; edit N: items; delete N).", names.join(", ")));
                }
                if !repo.is_empty() {
                    let names: Vec<&str> = repo.iter().map(|c| c.name.as_str()).collect();
                    t.push_str(&format!("\nThe repository's (.code-arena/checklists): {} (/checklist NAME turns one on).", names.join(", ")));
                }
                Ok(t)
            }
            "off" => {
                if let Some(c) = sync.chat_id() {
                    arena.stop_checklist(&c).await.map_err(e)?;
                }
                Ok("The checklist is off.".into())
            }
            w @ ("tick" | "untick") => {
                let n: u32 = more.parse().map_err(|_| format!("/checklist {w} N: the item's number."))?;
                let chat = sync.chat_id().ok_or("No checklist is on in this session.")?;
                match arena.tick(&chat, n, w == "tick", None, "person").await.map_err(e)? {
                    Some(r) => Ok(r.text()),
                    None => Err("No checklist is on in this session.".into()),
                }
            }
            "new" => {
                let (name, items) = checklists::split_list(more).ok_or("/checklist new NAME: an item; another; a third")?;
                arena.save_checklist(None, &name, &items).await.map_err(e)?;
                Ok(format!("Your checklist {name} is kept in Arena ({} items): /checklist {name} turns it on.", items.len()))
            }
            "edit" => {
                let (which, items) = checklists::split_list(more).ok_or("/checklist edit N: an item; another")?;
                let lists = arena.checklists().await.map_err(e)?;
                let c = pick(&lists, &which).ok_or(format!("You have no checklist {which}: /checklist lists yours."))?;
                arena.save_checklist(Some(&c.id), &c.name, &items).await.map_err(e)?;
                Ok(format!("{} has {} items now.", c.name, items.len()))
            }
            "delete" => {
                let lists = arena.checklists().await.map_err(e)?;
                let c = pick(&lists, more).ok_or(format!("You have no checklist {more}: /checklist lists yours."))?;
                arena.delete_checklist(&c.id).await.map_err(e)?;
                Ok(format!("{} is deleted.", c.name))
            }
            _ => {
                let which = rest.trim();
                let lists = arena.checklists().await.map_err(e)?;
                let chosen: Checklist = match pick(&lists, which) {
                    Some(c) => c.clone(),
                    None => {
                        // The repository's: kept as one of the person's of that name, its items as the repository has them.
                        let repo = checklists::repo_checklists(&root);
                        let r = repo
                            .iter()
                            .find(|c| c.name.eq_ignore_ascii_case(which))
                            .ok_or(format!("You have no checklist named {which}: /checklist lists yours."))?;
                        let id = lists.iter().find(|c| c.name.eq_ignore_ascii_case(&r.name)).map(|c| c.id.clone());
                        arena.save_checklist(id.as_deref(), &r.name, &r.items).await.map_err(e)?
                    }
                };
                let chat = sync
                    .ensure_chat()
                    .await
                    .ok_or("This session has no chat in Arena yet: say something first, then turn the checklist on.")?;
                let run: Run = arena
                    .start_checklist(&chat, &chosen.id)
                    .await
                    .map_err(e)?
                    .ok_or("Arena did not turn it on.")?;
                p.agent.note(format!("(Code Arena: {INSTRUCTIONS})\n\n{}", run.text()));
                Ok(format!("{}\nThe model goes through it, and ticks each item as it does it.", run.text()))
            }
        }
    }
    .await;
    match result {
        Ok(t) | Err(t) => t,
    }
}

/// A resumed session whose chat has a checklist on: the model is told again.
pub async fn remind(p: &mut Prepared) {
    let Some(sync) = p.sync.clone() else { return };
    let Some(chat) = sync.chat_id() else { return };
    if let Ok(Some(run)) = sync.arena().checklist_run(&chat).await {
        p.agent.note(format!("(Code Arena: {INSTRUCTIONS})\n\n{}", run.text()));
    }
}
