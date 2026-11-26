//! The model proposes commits: given the changes by hunk (with ids) and the repository's last subjects (its style), it
//! answers with one `propose_commits` call: groups, each a message (a subject under 72 characters and a body saying
//! why) and its hunks. Every hunk must be in exactly one group; a proposal that is not is asked once more with why,
//! and failing again, one commit is made of all of it.

use ca_llm::{Gateway, Message, Request, ToolDef};
use ca_tools::commits::Hunk;
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub message: String,
    pub hunks: Vec<String>,
}

fn tool() -> ToolDef {
    ToolDef {
        name: "propose_commits".into(),
        description: "The commits these changes make, in order: each a message and the hunks (by id) it takes.".into(),
        parameters: json!({"type": "object", "properties": {"groups": {"type": "array", "items": {"type": "object", "properties": {
            "message": {"type": "string", "description": "The subject (under 72 characters, in the repository's style), a blank line, and a body saying why."},
            "hunks": {"type": "array", "items": {"type": "string"}, "description": "The ids of the hunks this commit takes."}
        }, "required": ["message", "hunks"]}}}, "required": ["groups"]}),
    }
}

/// Every hunk in exactly one group, only hunks there are, each message with a subject under 72 characters.
pub fn check(groups: &[Group], hunks: &[Hunk]) -> Result<(), String> {
    if groups.is_empty() {
        return Err("no group was given".into());
    }
    let mut seen: Vec<&str> = Vec::new();
    for g in groups {
        let subject = g.message.lines().next().unwrap_or("").trim();
        if subject.is_empty() {
            return Err("a group has no message".into());
        }
        if subject.chars().count() > 72 {
            return Err(format!("the subject \"{subject}\" is longer than 72 characters"));
        }
        if g.hunks.is_empty() {
            return Err(format!("\"{subject}\" takes no hunk"));
        }
        for id in &g.hunks {
            if !hunks.iter().any(|h| h.id == *id) {
                return Err(format!("there is no hunk {id}"));
            }
            if seen.contains(&id.as_str()) {
                return Err(format!("{id} is in two groups"));
            }
            seen.push(id);
        }
    }
    let left: Vec<&str> = hunks.iter().map(|h| h.id.as_str()).filter(|id| !seen.contains(id)).collect();
    if !left.is_empty() {
        return Err(format!("{} not in any group", left.join(", ")));
    }
    Ok(())
}

fn groups_of(v: &Value) -> Vec<Group> {
    v.get("groups")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|g| Group {
                    message: g.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
                    hunks: g
                        .get("hunks")
                        .and_then(Value::as_array)
                        .map(|h| h.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The model's commits for `hunks`, checked; else (asked twice) one commit of them all.
pub async fn propose(gateway: &Gateway, model: &str, hunks: &[Hunk], subjects: &[String], what: &str) -> Vec<Group> {
    let mut messages = vec![
        Message::system(
            "You group a change into commits, as a careful engineer would: one commit for each thing done (a fix, a feature, a refactoring, its tests with it), in an order that builds. Write each message in the repository's own style (its last subjects show it: Conventional Commits or not): a subject under 72 characters, a blank line, and a body that says why, not what the diff shows. Answer with one propose_commits call; every hunk goes in exactly one group.",
        ),
        Message::user(format!(
            "{what}\n\nThe repository's last subjects:\n{}\n\nThe hunks:\n\n{}",
            if subjects.is_empty() {
                "(none yet)".to_string()
            } else {
                subjects.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n")
            },
            ca_tools::commits::shown(hunks, 60_000)
        )),
    ];
    let mut best: Option<String> = None;
    for _ in 0..2 {
        let ask = Request {
            model: model.to_string(),
            messages: messages.clone(),
            tools: vec![tool()],
            max_tokens: Some(4000),
            ..Default::default()
        };
        let Ok(c) = gateway.complete(&ask, &mut ca_llm::Quiet).await else { break };
        let args = c
            .message
            .tool_calls
            .iter()
            .find(|t| t.name == "propose_commits")
            .and_then(|t| crate::mend::parse_args(&t.arguments, &mut vec![]).ok())
            .or_else(|| serde_json::from_str::<Value>(c.message.content.trim().trim_matches('`')).ok());
        let groups = args.as_ref().map(groups_of).unwrap_or_default();
        if let Some(g) = groups.first() {
            best.get_or_insert_with(|| g.message.clone());
        }
        match check(&groups, hunks) {
            Ok(()) => return groups,
            Err(why) => {
                messages.push(c.message.clone());
                for t in &c.message.tool_calls {
                    messages.push(Message::tool(t.id.clone(), format!("Not taken: {why}.")));
                }
                messages.push(Message::user(format!(
                    "That proposal does not hold: {why}. Propose again, every hunk in exactly one group."
                )));
            }
        }
    }
    let message = best
        .filter(|m| m.lines().next().is_some_and(|s| !s.trim().is_empty() && s.chars().count() <= 72))
        .unwrap_or_else(|| "chore: the changes of a Code Arena session".into());
    vec![Group { message, hunks: hunks.iter().map(|h| h.id.clone()).collect() }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(id: &str) -> Hunk {
        Hunk { id: id.into(), file: "a".into(), header: String::new(), body: String::new(), whole_file: false }
    }

    #[test]
    fn a_proposal_holds_only_with_every_hunk_once() {
        let hs = vec![hunk("h1"), hunk("h2")];
        let g = |m: &str, h: &[&str]| Group { message: m.into(), hunks: h.iter().map(|s| s.to_string()).collect() };
        assert!(check(&[g("fix: a", &["h1"]), g("feat: b", &["h2"])], &hs).is_ok());
        assert!(check(&[g("fix: a", &["h1"])], &hs).expect_err("left").contains("h2 not in any group"));
        assert!(check(&[g("fix: a", &["h1", "h2"]), g("x", &["h2"])], &hs).expect_err("twice").contains("two groups"));
        assert!(check(&[g("fix: a", &["h9"])], &hs).is_err());
        assert!(check(&[g(&"x".repeat(80), &["h1", "h2"])], &hs).expect_err("long").contains("72"));
    }
}
