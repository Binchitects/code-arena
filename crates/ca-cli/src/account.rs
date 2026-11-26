//! `login`, `logout`, `models` and `doctor`.

use std::path::PathBuf;

use anyhow::{Context as _, bail};
use ca_arena::signin::{PasswordSignIn, Step};
use ca_arena::{ArenaError, Capabilities, Feature, State};
use ca_core::Secret;
use ca_settings::Connection;
use ca_settings::connection::normalize_url;

use crate::app::App;
use crate::ui::{self, Ui};

pub struct LoginArgs {
    /// A provider of one's own instead of an Arena: anthropic, openai, deepseek, openrouter, mistral, ollama, or a
    /// name of one's own with `url` (any OpenAI-compatible API).
    pub provider: Option<String>,
    /// `openai` or `anthropic`, for a provider of one's own.
    pub kind: Option<String>,
    pub url: Option<String>,
    pub user: Option<String>,
    pub gateway: Option<String>,
    pub ca: Option<PathBuf>,
}

/// This machine's name, for the key a password sign-in makes ("Code Arena on NAME").
fn machine_name() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this machine".into())
}

/// `login --provider NAME`: a provider kept with the person's key (asked without showing it, or its variable's), its
/// models listed to check it. The standalone edition when there is no Arena; beside Arena's models when there is.
pub async fn login_provider(app: &App, name: &str, args: &LoginArgs) -> anyhow::Result<()> {
    let ui = app.ui;
    let known = ca_llm::providers::known(name);
    let mut entry = app.connection.providers.get(name).cloned().unwrap_or_default();
    if args.url.is_some() {
        entry.url = args.url.clone();
    }
    if args.kind.is_some() {
        entry.kind = args.kind.clone();
    }
    // Its settings must hold before a key is asked for.
    let provider = ca_llm::providers::Provider::parse(name, &entry.settings())
        .map_err(|e| anyhow::anyhow!("{e} (known: anthropic, openai, deepseek, openrouter, mistral, ollama)"))?;
    let env_var = known.map(|k| k.key_env);
    let from_env = env_var.and_then(|v| app.settings.env().get(v).map(|k| (v, k.to_string())));
    let key = match &from_env {
        Some((var, k)) => {
            ui.say(&ui.dim(&format!("Using {var} from the environment: it is read at each run, never written down.")));
            Secret::new(k)
        }
        None => {
            let ask = match known {
                Some(k) => format!("{}'s API key (from {}; Enter for none): ", name, k.about),
                None => format!("{name}'s API key (Enter for none): "),
            };
            let typed = rpassword::prompt_password(ask)?;
            Secret::new(typed.trim())
        }
    };
    if key.is_empty() && name != "ollama" && !provider.url.starts_with("http://") {
        bail!("{name} needs a key: give it when asked, or set {}", env_var.unwrap_or("its key"));
    }
    let g = ca_llm::Gateway::standalone(vec![(provider.clone(), key.clone())], app.ca())?;
    let models = g.models().await.with_context(|| format!("{name} did not list its models with that key"))?;
    let mut conn = Connection::load(&app.paths.credentials_file(), &ca_settings::Env::default())?;
    entry.api_key = if from_env.is_some() || key.is_empty() { None } else { Some(key) };
    entry.key_from_env = false;
    entry.from_env = false;
    conn.providers.insert(name.to_string(), entry);
    conn.save(&app.paths.credentials_file())?;
    let shown: Vec<&str> = models.iter().take(8).map(|m| m.id.as_str()).collect();
    ui.say(&format!(
        "{} {name}: {} models ({}{}).",
        ui.green("Kept"),
        models.len(),
        shown.join(", "),
        if models.len() > shown.len() { ", …" } else { "" }
    ));
    let edition = if conn.signed_in() {
        "beside Arena's models (where Arena's admins allow it)"
    } else {
        "the standalone edition: no Arena"
    };
    ui.say(&ui.dim(&format!("Its models are named {name}:MODEL, {edition}. Choose one with --model or /model.")));
    Ok(())
}

/// `logout --provider NAME`: the provider and its key forgotten here.
pub fn logout_provider(app: &App, name: &str) -> anyhow::Result<()> {
    let mut conn = Connection::load(&app.paths.credentials_file(), &ca_settings::Env::default())?;
    if conn.providers.remove(name).is_none() {
        app.ui.say(&format!("No provider {name} is kept here."));
        return Ok(());
    }
    conn.save(&app.paths.credentials_file())?;
    app.ui.say(&format!("{name} and its key are forgotten here."));
    Ok(())
}

/// The company's CA, copied into the config folder (`arena-ca.pem`): the copy is what Code Arena reads from then on.
fn keep_ca(app: &App, given: &std::path::Path) -> anyhow::Result<PathBuf> {
    let pem = std::fs::read(given).with_context(|| format!("the CA file {}", given.display()))?;
    let kept = app.paths.config.join("arena-ca.pem");
    std::fs::create_dir_all(&app.paths.config)?;
    std::fs::write(&kept, &pem)?;
    // It must be one: read as every request will read it.
    if let Err(e) = ca_net::client(Some(&kept)) {
        let _ = std::fs::remove_file(&kept);
        anyhow::bail!("the CA file {}: {e}", given.display());
    }
    Ok(kept)
}

pub async fn login(app: &App, args: LoginArgs) -> anyhow::Result<()> {
    let ui = app.ui;
    if let Some(name) = args.provider.clone() {
        return login_provider(app, &name, &args).await;
    }
    let url = match args.url.or_else(|| app.connection.url.clone()) {
        Some(u) => u,
        None => ui::ask_line("The Arena's address (https://…): ")?,
    };
    let url = normalize_url(&url)?;
    // A CA given now is kept as Code Arena's own copy, so moving or deleting the file given does not break it.
    let ca = match args.ca {
        Some(p) => Some(keep_ca(app, &p)?),
        None => app.connection.ca.clone(),
    };
    let mut conn = Connection {
        url: Some(url.clone()),
        gateway: args.gateway.or_else(|| app.connection.gateway.clone()),
        ca,
        // The person's own providers stay (those of the environment are not written).
        providers: app.connection.providers.clone(),
        ..Default::default()
    };

    let key = match &args.user {
        Some(user) => password_sign_in(&ui, &url, user, conn.ca.as_deref()).await?,
        None => {
            let typed = rpassword::prompt_password(
                "Your API key (Your account → API key; press Enter to sign in with a user name and password instead): ",
            )?;
            if typed.trim().is_empty() {
                let user = ui::ask_line("User name: ")?;
                if user.is_empty() {
                    bail!("no key and no user name: nothing to sign in with");
                }
                password_sign_in(&ui, &url, &user, conn.ca.as_deref()).await?
            } else {
                Secret::new(typed.trim())
            }
        }
    };
    conn.api_key = Some(key.clone());

    // The key is checked where it is used: the gateway first, then Arena itself.
    let Some(gw) = conn.gateway_url() else {
        bail!("{url} is an address, so its gateway cannot be guessed: give it with --gateway URL");
    };
    let gateway = ca_llm::Gateway::new(&gw, key.clone(), conn.ca.as_deref())?;
    let models = gateway.models().await.with_context(|| format!("checking the key with the gateway ({gw})"))?;
    let arena = ca_arena::ArenaClient::new(&url, Some(key.clone()), conn.ca.as_deref())?;
    let me = arena.me().await;
    if let Ok(me) = &me {
        conn.account = Some(me.display_name.clone().unwrap_or_else(|| me.user_name.clone()));
    }
    let tools = arena_tools(&conn, &key).await;
    conn.save(&app.paths.credentials_file())?;

    let who = conn.account.as_deref().map(|a| format!(" as {a}")).unwrap_or_default();
    ui.say(&format!("{} Signed in to {url}{who}.", ui.green("✓")));
    ui.say(&format!(
        "  {} models your key may use; {}",
        models.len(),
        match tools {
            Ok(n) => format!("{n} of Arena's tools."),
            Err(e) => format!("Arena's tools: {e}"),
        }
    ));
    if let Err(e) = me {
        ui.say(&ui.yellow(&format!("  Arena did not say whose the key is: {e}")));
    }
    ui.say(&ui.dim(&format!("  Kept in {} (readable by you only).", app.paths.credentials_file().display())));
    Ok(())
}

async fn password_sign_in(ui: &Ui, url: &str, user: &str, ca: Option<&std::path::Path>) -> anyhow::Result<Secret> {
    let s = PasswordSignIn::new(url, ca)?;
    let password = Secret::new(rpassword::prompt_password(format!("{user}'s password: "))?);
    if s.password(user, &password).await? == Step::SecondFactor {
        let code =
            Secret::new(rpassword::prompt_password("The code from your authenticator app (or a recovery code): ")?);
        s.code(&code).await?;
    }
    let machine = machine_name();
    let key = s.make_key(&machine).await?;
    ui.say(&ui.dim(&format!(
        "  Made the key \"{}\" for this machine (Your account → API keys lists it).",
        PasswordSignIn::key_name(&machine)
    )));
    Ok(key)
}

/// How many tools Arena's MCP endpoint lists.
async fn arena_tools(conn: &Connection, key: &Secret) -> Result<usize, String> {
    let url = conn.mcp_url().ok_or("no address")?;
    let mut m = ca_mcp::Client::new(&url, Some(key.clone()), conn.ca.as_deref()).map_err(|e| e.to_string())?;
    m.initialize().await.map_err(|e| e.to_string())?;
    m.list_tools().await.map(|t| t.len()).map_err(|e| e.to_string())
}

pub async fn logout(app: &App) -> anyhow::Result<()> {
    let ui = app.ui;
    if !app.connection.signed_in() {
        ui.say("Not signed in.");
        return Ok(());
    }
    // Arena revokes the key when it is one Code Arena made; the person's own main key stays theirs to replace.
    match app.arena()?.sign_out().await {
        Ok(()) => {}
        Err(ArenaError::Key(_)) => {}
        Err(e) => ui.say(
            &ui.yellow(&format!("Arena did not take the sign-out ({e}); the key is forgotten here all the same.")),
        ),
    }
    let mut conn = Connection::load(&app.paths.credentials_file(), &ca_settings::Env::default())?;
    conn.api_key = None;
    conn.account = None;
    conn.save(&app.paths.credentials_file())?;
    ui.say("Signed out: the key is forgotten here.");
    Ok(())
}

pub async fn models(app: &App) -> anyhow::Result<()> {
    let models = app.gateway()?.models().await?;
    let width = models.iter().map(|m| m.id.len()).max().unwrap_or(0);
    for m in &models {
        let window = match (m.context, m.max_output) {
            (Some(c), Some(o)) => format!("{} tokens, answers up to {}", ui::thousands(c), ui::thousands(o)),
            (Some(c), None) => format!("{} tokens", ui::thousands(c)),
            _ => "its window not said".into(),
        };
        let mut can = vec![];
        if m.vision {
            can.push("sees pictures");
        }
        if !m.tools {
            can.push("no tools");
        }
        let can = if can.is_empty() { String::new() } else { format!(" ({})", can.join(", ")) };
        println!("{:<width$}  {}{}", m.id, app.ui.dim(&window), can);
    }
    if models.is_empty() {
        app.ui.say("The gateway lists no model for your key.");
    }
    Ok(())
}

fn state_words(ui: &Ui, s: &State) -> String {
    match s {
        State::On => ui.green("yes"),
        State::Denied(w) => ui.yellow(&format!("not yours to use: {w}")),
        State::Off(w) => ui.yellow(&format!("off here: {w}")),
        State::Absent => ui.dim("this Arena does not have it"),
        State::Unknown(w) => ui.red(&format!("unknown: {w}")),
    }
}

/// Keeps what a server answered, for the next start.
fn keep_capabilities(app: &App, arena: &ca_arena::ArenaClient, stamp: Option<String>, caps: &Capabilities) {
    let probed_at = u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0);
    ca_arena::Kept { url: arena.base().to_string(), probed_at, stamp, capabilities: caps.clone() }
        .write(&ca_arena::Kept::file(&app.paths.servers_dir(), arena.base()));
}

pub async fn doctor(app: &App) -> anyhow::Result<()> {
    let ui = app.ui;
    println!("{}", ui.bold(&format!("{} {}", ca_core::PRODUCT, ca_core::VERSION)));
    println!("  settings  {}", app.paths.config.display());
    println!("  data      {}", app.paths.data.display());
    println!("  edition   {}", app.edition().name());
    let c = &app.connection;
    let providers = app.providers();
    if !providers.is_empty() {
        println!("\n{}", ui.bold("Your providers"));
        for (p, _) in &providers {
            let key = providers.iter().find(|x| x.0.name == p.name).map(|x| x.1.clone()).unwrap_or_default();
            let one = ca_llm::Gateway::standalone(vec![(p.clone(), key)], app.ca())?;
            match one.models().await {
                Ok(m) => println!("  {:<10}{} ({} models)", p.name, p.url, m.len()),
                Err(e) => println!("  {:<10}{}", p.name, ui.red(&e.to_string())),
            }
        }
    }
    let Some(url) = &c.url else {
        if providers.is_empty() {
            println!(
                "\nNothing to ask a model of yet: code-arena login --url https://YOUR-ARENA (the managed edition), or code-arena login --provider NAME (standalone: your own key)."
            );
        } else {
            println!(
                "\n{}",
                ui.dim("Standalone: what needs Farzan Arena (chats on the web, its memory, skills, checklists, projects, its tools) is off. code-arena login --url https://YOUR-ARENA turns it on.")
            );
        }
        return Ok(());
    };
    println!("\n{}", ui.bold("Arena"));
    println!("  address   {url}");
    println!("  gateway   {}", c.gateway_url().unwrap_or_else(|| "(none: give one with login --gateway)".into()));
    println!("  tools     {}", c.mcp_url().unwrap_or_default());
    if let Some(ca) = &c.ca {
        println!("  CA        {}", ca.display());
    }
    let arena = match app.arena() {
        Ok(a) => a,
        Err(e) => {
            println!("\n{}", ui.red(&format_chain(&e)));
            return Ok(());
        }
    };
    match arena.info().await {
        Ok(info) => println!(
            "  server    {} {}",
            info.name.unwrap_or_else(|| "Arena".into()),
            info.version
                .map(|v| format!("(says it is {v}; features are read from its answers, not this)"))
                .unwrap_or_default()
        ),
        Err(e) => println!("  server    {}", ui.red(&e.to_string())),
    }
    if !c.signed_in() {
        println!("\nNo API key: code-arena login");
        return Ok(());
    }
    match arena.me().await {
        Ok(me) => {
            println!(
                "  account   {}{}",
                me.display_name.unwrap_or(me.user_name),
                if me.denied.is_empty() { String::new() } else { format!(" (denied: {})", me.denied.join(", ")) }
            );
        }
        Err(e) => println!("  account   {}", ui.red(&e.to_string())),
    }
    match app.gateway()?.models().await {
        Ok(m) => println!("  models    {}", m.len()),
        Err(e) => println!("  models    {}", ui.red(&e.to_string())),
    }
    match arena_tools(c, c.api_key.as_ref().unwrap_or(&Secret::default())).await {
        Ok(n) => println!("  MCP       {n} of Arena's tools"),
        Err(e) => println!("  MCP       {}", ui.yellow(&e)),
    }
    println!("\n{}", ui.bold("What this Arena has"));
    let stamp = arena.settings_stamp().await;
    match arena.capabilities().await {
        Ok(caps) => {
            for f in Feature::ALL {
                println!("  {:<22}{}", f.name(), state_words(&ui, &caps.state(*f)));
            }
            keep_capabilities(app, &arena, stamp, &caps);
        }
        Err(e) => println!("  {}", ui.red(&e.to_string())),
    }
    Ok(())
}

fn format_chain(e: &anyhow::Error) -> String {
    e.chain().map(|c| c.to_string()).collect::<Vec<_>>().join(": ")
}
