//! Other agents' files, read when the person names those agents (`compat.agents`, empty by default). An agent named
//! `NAME` has its folder `.NAME/` (in the project and the home: commands, skills, sub-agents, output styles, its
//! `settings.json`'s hooks and status line), its instructions `NAME.md` (upper case), its plugins' manifest folder
//! `.NAME-plugin/` and its variables `${NAME_PLUGIN_ROOT}` and `NAME_PROJECT_DIR`.

/// The agents named, as given (lower case, letters, digits, `-` and `_`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Agents(pub Vec<String>);

impl Agents {
    pub fn new(names: &[String]) -> Self {
        Agents(
            names
                .iter()
                .map(|n| n.trim().trim_start_matches('.').to_ascii_lowercase())
                .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
                .collect(),
        )
    }

    /// Their folders: `.NAME`.
    pub fn folders(&self) -> Vec<String> {
        self.0.iter().map(|n| format!(".{n}")).collect()
    }

    /// Their instruction files: `NAME.md`, upper case.
    pub fn instruction_files(&self) -> Vec<String> {
        self.0.iter().map(|n| format!("{}.md", n.to_ascii_uppercase())).collect()
    }

    /// Their plugins' manifest folders: `.NAME-plugin`.
    pub fn plugin_folders(&self) -> Vec<String> {
        self.0.iter().map(|n| format!(".{n}-plugin")).collect()
    }

    /// A variable of theirs: `NAME_SUFFIX`, upper case, `-` as `_`.
    pub fn variables(&self, suffix: &str) -> Vec<String> {
        self.0.iter().map(|n| format!("{}_{suffix}", n.to_ascii_uppercase().replace('-', "_"))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agents_names_follow_its_name() {
        let a = Agents::new(&["Other".into(), ".my-agent".into(), "bad name".into(), "".into()]);
        assert_eq!(a.folders(), [".other", ".my-agent"]);
        assert_eq!(a.instruction_files(), ["OTHER.md", "MY-AGENT.md"]);
        assert_eq!(a.plugin_folders(), [".other-plugin", ".my-agent-plugin"]);
        assert_eq!(a.variables("PLUGIN_ROOT"), ["OTHER_PLUGIN_ROOT", "MY_AGENT_PLUGIN_ROOT"]);
        assert!(Agents::default().folders().is_empty());
    }
}
