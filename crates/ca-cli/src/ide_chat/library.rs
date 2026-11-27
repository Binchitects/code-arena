//! What the box's `/` offers, and a line of several `/names` sent.
//!
//! - `library` → `library{commands: [{name, description, hint, source}], skills: [{name, description, source,
//!   path}], builtins: [{name, description}]}`: the project's and the person's commands and the skills, as the
//!   session reads them (`ca_agent::library::load`), each command once and before a skill of its name. The chat's
//!   own commands (`builtins`) are none yet. A `key` the request carries is said again in the reply.
//! - `expand`: a line typed `/NAME words` made what goes to the model, and `/one /two words` each of them for the
//!   same words, one after the other (the menu lets several skills be chosen together).

use ca_agent::library::{Library, load};
use serde_json::{Value, json};

use crate::ide_ext::{Shared, send};

/// The reply to `library`, from what was read.
fn listed(library: &Library) -> Value {
    let commands: Vec<Value> = library
        .commands
        .iter()
        .map(|c| json!({"name": c.name, "description": c.description, "hint": c.argument_hint, "source": c.source}))
        .collect();
    let skills: Vec<Value> = library
        .skills
        .iter()
        .filter(|k| !library.commands.iter().any(|c| c.name == k.name))
        .map(|k| json!({"name": k.name, "description": k.description, "source": k.source, "path": k.path}))
        .collect();
    json!({"type": "library", "commands": commands, "skills": skills, "builtins": []})
}

/// `library`: read off the driver's loop when there is one to be off, the folders being on disk.
pub fn asked(sh: &Shared, m: &Value) -> bool {
    let (sources, out) = (sh.sources.clone(), sh.out.clone());
    let key = Some(&m["key"]).filter(|k| k.is_string() || k.is_number()).cloned();
    let work = move || {
        let mut reply = listed(&load(&sources));
        if let Some(key) = key {
            reply["key"] = key;
        }
        send(&out, reply);
    };
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => drop(rt.spawn_blocking(work)),
        Err(_) => work(),
    }
    true
}

/// A line beginning with `/` made what goes to the model: each `/NAME` at its start that is a command or a skill,
/// given the words after them all. None: its first name is neither.
pub fn expand(library: &Library, line: &str) -> Option<String> {
    let known =
        |name: &str| library.commands.iter().any(|c| c.name == name) || library.skills.iter().any(|k| k.name == name);
    let mut names = Vec::new();
    let mut rest = line.trim_start();
    while let Some(after) = rest.strip_prefix('/') {
        let name = after.split(char::is_whitespace).next().unwrap_or(after);
        if name.is_empty() || !known(name) {
            break;
        }
        names.push(name);
        rest = after[name.len()..].trim_start();
    }
    if names.len() < 2 {
        return library.slash(line.trim_start());
    }
    let each: Vec<String> = names.iter().filter_map(|name| library.slash(&format!("/{name} {rest}"))).collect();
    Some(each.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use ca_agent::library::Source;

    use super::*;

    fn library() -> (tempfile::TempDir, Library) {
        let d = tempfile::tempdir().expect("temp");
        let commands = d.path().join("commands");
        std::fs::create_dir_all(&commands).expect("commands");
        std::fs::write(
            commands.join("review.md"),
            "---\ndescription: Review the change\nargument-hint: FILE\n---\nReview $ARGUMENTS carefully.",
        )
        .expect("command");
        for (name, what) in [("review", "A skill the command hides"), ("tests", "Write the tests")] {
            let at = d.path().join("skills").join(name);
            std::fs::create_dir_all(&at).expect("skill");
            std::fs::write(at.join("SKILL.md"), format!("---\nname: {name}\ndescription: {what}\n---\nDo {name}."))
                .expect("skill");
        }
        let library = load(&[Source { label: "yours".into(), dir: d.path().to_path_buf() }]);
        (d, library)
    }

    #[test]
    fn the_menu_is_given_the_commands_with_their_hints_and_each_skill_a_command_does_not_hide() {
        let (_d, lib) = library();
        let got = listed(&lib);
        assert_eq!(got["type"], "library");
        assert_eq!(
            got["commands"],
            json!([{"name": "review", "description": "Review the change", "hint": "FILE", "source": "yours"}])
        );
        let skills: Vec<&str> =
            got["skills"].as_array().expect("skills").iter().filter_map(|k| k["name"].as_str()).collect();
        assert_eq!(skills, ["tests"]);
        assert_eq!(got["skills"][0]["description"], "Write the tests");
        assert!(got["skills"][0]["path"].as_str().is_some_and(|p| p.ends_with("SKILL.md")));
        assert_eq!(got["builtins"], json!([]));
        // No prompt goes to the menu.
        assert!(!got.to_string().contains("carefully"));
    }

    #[test]
    fn several_names_chosen_together_are_each_given_the_words_after_them() {
        let (_d, lib) = library();
        assert_eq!(expand(&lib, "/review cart.rs").as_deref(), Some("Review cart.rs carefully."));
        let both = expand(&lib, "/review  /tests cart.rs").expect("both");
        assert!(both.starts_with("Review cart.rs carefully.\n\nUse the skill tests:"), "{both}");
        assert!(both.ends_with("The request: cart.rs"), "{both}");
        // A name that is neither ends the names: the rest are words.
        assert_eq!(expand(&lib, "/review /nothing here").as_deref(), Some("Review /nothing here carefully."));
        assert_eq!(expand(&lib, "/nothing at all"), None);
    }
}
