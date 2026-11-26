//! Commits the model groups: `/commit` (all of the working tree's changes), and with `git.autoCommit` the turn's own
//! changes after each turn (the files it wrote, minus those that held the person's own changes when it began). The
//! groups are shown first (`git.autoCommit.review`, on): `/commit yes` makes them, `/commit no` drops them.

use ca_tools::commits::{self, Hunk};

use crate::session::Prepared;

/// Commits proposed, waiting for the person's word.
pub struct Pending {
    pub top: std::path::PathBuf,
    pub hunks: Vec<Hunk>,
    pub groups: Vec<ca_agent::commits::Group>,
}

/// The working tree's changes now, relative to the repository's top (for a turn's own changes: what was there first).
pub async fn dirty(p: &Prepared) -> Vec<String> {
    let Some(top) = commits::top(p.agent.ctx.workspace.root()).await else { return vec![] };
    commits::changed(&top).await.unwrap_or_default()
}

/// The model's commits for `files` (all changes when none are named).
pub async fn propose(p: &Prepared, files: Option<Vec<String>>, what: &str) -> Result<Pending, String> {
    let top = commits::top(p.agent.ctx.workspace.root()).await.ok_or("This folder is in no git repository.")?;
    if let Some(op) = commits::in_progress(&top).await {
        return Err(format!("git is in the middle of {op}: Code Arena commits nothing until it is done."));
    }
    if p.agent.ctx.jobs.as_ref().is_some_and(|j| j.list().iter().any(|i| i.running)) {
        return Err("Commands of the session still run (/jobs): Code Arena commits once they end.".into());
    }
    let hunks = commits::hunks(&top, files.as_deref()).await?;
    if hunks.is_empty() {
        return Err("Nothing to commit.".into());
    }
    let subjects = commits::subjects(&top, 20).await;
    let groups = ca_agent::commits::propose(&p.agent.gateway, &p.agent.model, &hunks, &subjects, what).await;
    Ok(Pending { top, hunks, groups })
}

pub fn show(pending: &Pending, ui: crate::ui::Ui) -> String {
    let mut out = Vec::new();
    for (i, g) in pending.groups.iter().enumerate() {
        let files: Vec<&str> = {
            let mut f: Vec<&str> =
                pending.hunks.iter().filter(|h| g.hunks.contains(&h.id)).map(|h| h.file.as_str()).collect();
            f.dedup();
            f
        };
        out.push(format!("{}. {}", i + 1, ui.bold(g.message.lines().next().unwrap_or(""))));
        for l in g.message.lines().skip(1).filter(|l| !l.trim().is_empty()) {
            out.push(ui.dim(&format!("   {l}")));
        }
        out.push(ui.dim(&format!("   {} ({} hunks)", files.join(", "), g.hunks.len())));
    }
    out.join("\n")
}

/// The groups made, in order; a PreCommit hook (or git's own hook) that refuses stops the series there.
pub async fn make(p: &Prepared, pending: &Pending) -> String {
    let mut said = Vec::new();
    for (i, g) in pending.groups.iter().enumerate() {
        let hunks: Vec<&Hunk> = pending.hunks.iter().filter(|h| g.hunks.contains(&h.id)).collect();
        if p.agent.hooks.has("PreCommit") {
            let files: Vec<&str> = hunks.iter().map(|h| h.file.as_str()).collect();
            let h = p.agent.hooks.run("PreCommit", "", serde_json::json!({"message": g.message, "files": files})).await;
            if let Some(why) = h.block {
                said.push(format!(
                    "A PreCommit hook stopped commit {} ({}): {why}",
                    i + 1,
                    g.message.lines().next().unwrap_or("")
                ));
                break;
            }
        }
        match commits::commit(&pending.top, &hunks, &g.message, &p.session).await {
            Ok(sha) => said.push(format!("{sha} {}", g.message.lines().next().unwrap_or(""))),
            Err(e) => {
                said.push(format!("Commit {} was not made: {e}", i + 1));
                break;
            }
        }
    }
    said.join("\n")
}

/// `/commit`, `/commit yes`, `/commit no`.
pub async fn command(rest: &str, p: &mut Prepared, ui: crate::ui::Ui) -> String {
    match rest {
        "yes" => match p.pending_commits.take() {
            Some(pending) => make(p, &pending).await,
            None => "No commits wait: /commit proposes them.".into(),
        },
        "no" => match p.pending_commits.take() {
            Some(_) => "Dropped: nothing was committed.".into(),
            None => "No commits wait.".into(),
        },
        _ => match propose(p, None, "Group all of the working tree's changes into commits.").await {
            Ok(pending) => {
                let shown = show(&pending, ui);
                p.pending_commits = Some(pending);
                format!("{shown}\n{}", ui.dim("/commit yes makes them; /commit no drops them."))
            }
            Err(e) => e,
        },
    }
}

/// After a turn, with `git.autoCommit` on: its own changes proposed (shown first when `git.autoCommit.review` is on).
pub async fn after_turn(
    p: &mut Prepared,
    wrote: &[std::path::PathBuf],
    dirty_before: &[String],
    review: bool,
    ui: crate::ui::Ui,
) -> Option<String> {
    let top = commits::top(p.agent.ctx.workspace.root()).await?;
    let now = commits::changed(&top).await.ok()?;
    let own: Vec<String> = wrote
        .iter()
        .filter_map(|w| w.strip_prefix(&top).ok().map(|r| r.to_string_lossy().replace('\\', "/")))
        .filter(|r| now.contains(r) && !dirty_before.contains(r))
        .collect();
    if own.is_empty() {
        return None;
    }
    match propose(p, Some(own), "Group the changes the last turn made into commits.").await {
        Ok(pending) if review => {
            let shown = show(&pending, ui);
            p.pending_commits = Some(pending);
            Some(format!(
                "The turn's changes, as commits:\n{shown}\n{}",
                ui.dim("/commit yes makes them; /commit no drops them.")
            ))
        }
        Ok(pending) => Some(make(p, &pending).await),
        Err(e) => Some(e),
    }
}
