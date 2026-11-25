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

    pub fn ca(&self) -> Option<&std::path::Path> {
        self.connection.ca.as_deref()
    }

    pub fn arena(&self) -> anyhow::Result<ca_arena::ArenaClient> {
        let Some(url) = &self.connection.url else {
            bail!("not signed in: run code-arena login --url https://YOUR-ARENA")
        };
        Ok(ca_arena::ArenaClient::new(url, self.connection.api_key.clone(), self.ca())?)
    }

    pub fn gateway(&self) -> anyhow::Result<ca_llm::Gateway> {
        if !self.connection.signed_in() {
            bail!("not signed in: run code-arena login --url https://YOUR-ARENA (or set ARENA_URL and ARENA_API_KEY)");
        }
        let Some(gw) = self.connection.gateway_url() else {
            bail!(
                "this Arena is reached by an address, so its gateway cannot be guessed: give it with code-arena login --gateway URL (or ARENA_GATEWAY_URL)"
            );
        };
        let key = self.connection.api_key.clone().unwrap_or_default();
        Ok(ca_llm::Gateway::new(&gw, key, self.ca())?)
    }
}
