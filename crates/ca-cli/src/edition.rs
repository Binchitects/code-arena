//! The two editions, one product: **managed** (signed in to Farzan Arena: everything) and **standalone** (no Arena:
//! the person's own providers, and everything that needs no Arena). What needs Arena is marked here once; in the
//! standalone edition it is hidden from `/help`, its tools are not offered, and asked for, it says which edition has it.

use ca_settings::Edition;

/// The commands that need Farzan Arena.
pub const NEEDS_ARENA: &[&str] = &["/web", "/sync", "/checklist", "/project", "/projects", "/assistant"];

/// What a command needing Arena says in the standalone edition; `None` when it may run.
pub fn locked(edition: Edition, command: &str) -> Option<String> {
    if edition == Edition::Managed || !NEEDS_ARENA.contains(&command) {
        return None;
    }
    Some(format!(
        "{command} needs Farzan Arena (the managed edition): this Code Arena is {}. code-arena login --url https://YOUR-ARENA turns it on; your providers stay.",
        edition.name()
    ))
}

/// `/help` as this edition has it: the lines of what needs Arena left out of the standalone edition.
pub fn help(edition: Edition, full: &str) -> String {
    if edition == Edition::Managed {
        return full.to_string();
    }
    let mut out: Vec<&str> = Vec::new();
    let mut skipping = false;
    for line in full.lines() {
        let first = line.split_whitespace().next().unwrap_or("");
        if NEEDS_ARENA.contains(&first) {
            skipping = true;
            continue;
        }
        // A command's continued lines (indented) go with it.
        if skipping && line.starts_with("    ") {
            continue;
        }
        skipping = false;
        out.push(line);
    }
    let mut t = out.join("\n");
    t.push_str("\n\nStandalone: chats kept on the web, Arena's memory and skills, checklists, projects and assistants need Farzan Arena (code-arena login --url).");
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_needs_arena_is_locked_out_of_the_standalone_edition_only() {
        assert!(locked(Edition::Managed, "/web").is_none());
        assert!(locked(Edition::Standalone, "/plan").is_none());
        assert!(locked(Edition::Standalone, "/web").expect("locked").contains("managed edition"));
        let full = "/help                 these commands\n/web [N]              your chats in Arena\n/checklist [N]  checklists\n                      more of it\n/plan                 the plan";
        let h = help(Edition::Standalone, full);
        assert!(h.contains("/help") && h.contains("/plan"));
        assert!(!h.contains("/web") && !h.contains("more of it") && !h.contains("/checklist ["));
        assert_eq!(help(Edition::Managed, full), full);
    }
}
