//! What every command starts from: the folders, the environment, the settings and the Arena connection.

use std::path::PathBuf;

use anyhow::{Context as _, bail};
use ca_core::Paths;
use ca_settings::{Connection, Env, Settings};

use crate::ui::Ui;

pub struct App {
    pub paths: Paths,
    pub settings: Settings,
    pub connection: Connection,
    pub ui: Ui,
    /// The person's home, unless `CODE_ARENA_HOME` keeps everything in one folder.
    pub home: Option<PathBuf>,
}

impl App {
    /// `project_settings`: the project's `.code-arena/settings.json`, when there is a project.
    pub fn load(project_settings: Option<PathBuf>) -> anyhow::Result<Self> {
        let env = Env::from_process();
        let var = |name: &str| env.get(name).map(PathBuf::from);
        let paths = Paths::resolve(var("CODE_ARENA_HOME"), var("XDG_CONFIG_HOME"), var("XDG_DATA_HOME"));
        let settings = Settings::load(&paths.settings_file(), project_settings.as_deref(), env.clone());
        let connection = Connection::load(&paths.credentials_file(), &env).context("the Arena connection")?;
        let ui = Ui::new(settings.bool(&ca_settings::schema::COLOR), env.get("NO_COLOR").is_some());
        for p in &settings.problems {
            ui.say(&ui.yellow(&format!("{}: {}", p.file.display(), p.message)));
        }
        let home = if env.get("CODE_ARENA_HOME").is_some() { None } else { dirs::home_dir() };
        Ok(App { paths, settings, connection, ui, home })
    }

    /// The other agents whose files are read too (`compat.agents`).
    pub fn agents(&self) -> ca_core::compat::Agents {
        ca_core::compat::Agents::new(&self.settings.strings(&ca_settings::schema::COMPAT_AGENTS))
    }

    pub fn ca(&self) -> Option<&std::path::Path> {
        self.connection.ca.as_deref()
    }

    pub fn arena(&self) -> anyhow::Result<ca_arena::ArenaClient> {
        let Some(url) = &self.connection.url else {
            bail!("not signed in: run code-arena login --url https://YOUR-ARENA")
        };
        Ok(ca_arena::ArenaClient::new(url, self.connection.api_key.clone(), self.ca())?)
    }

    pub fn edition(&self) -> ca_settings::Edition {
        self.connection.edition()
    }

    /// The person's providers that can be asked, each with its key; one whose settings do not hold is said.
    pub fn providers(&self) -> Vec<(ca_llm::providers::Provider, ca_core::Secret)> {
        let mut out = Vec::new();
        for (name, entry) in self.connection.usable_providers() {
            match ca_llm::providers::Provider::parse(name, &entry.settings()) {
                Ok(p) => out.push((p, entry.api_key.clone().unwrap_or_default())),
                Err(e) => self.ui.say(&self.ui.yellow(&format!("The provider {e}: left out."))),
            }
        }
        out
    }

    /// The models: Arena's gateway (managed, with the person's providers beside it) or the providers alone
    /// (standalone).
    pub fn gateway(&self) -> anyhow::Result<ca_llm::Gateway> {
        match self.edition() {
            ca_settings::Edition::Standalone => return Ok(ca_llm::Gateway::standalone(self.providers(), self.ca())?),
            ca_settings::Edition::None => bail!(
                "nothing to ask a model of yet: code-arena login --url https://YOUR-ARENA (Farzan Arena, the managed edition), or code-arena login --provider NAME (your own key: anthropic, openai, deepseek, openrouter, mistral, ollama, or any OpenAI-compatible API with --url)"
            ),
            ca_settings::Edition::Managed => {}
        }
        let Some(gw) = self.connection.gateway_url() else {
            bail!(
                "this Arena is reached by an address, so its gateway cannot be guessed: give it with code-arena login --gateway URL (or ARENA_GATEWAY_URL)"
            );
        };
        let key = self.connection.api_key.clone().unwrap_or_default();
        let mut g = ca_llm::Gateway::new(&gw, key, self.ca())?;
        g.set_providers(self.providers());
        Ok(g)
    }
}
