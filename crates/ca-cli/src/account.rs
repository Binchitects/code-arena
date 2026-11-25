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

pub async fn login(app: &App, args: LoginArgs) -> anyhow::Result<()> {
    let ui = app.ui;
    let url = match args.url.or_else(|| app.connection.url.clone()) {
        Some(u) => u,
        None => ui::ask_line("The Arena's address (https://…): ")?,
    };
    let url = normalize_url(&url)?;
    let ca = match args.ca.or_else(|| app.connection.ca.clone()) {
        Some(p) => Some(std::fs::canonicalize(&p).with_context(|| format!("the CA file {}", p.display()))?),
        None => None,
    };
    let mut conn = Connection {
        url: Some(url.clone()),
        gateway: args.gateway.or_else(|| app.connection.gateway.clone()),
        ca,
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
    let mut m = ca_mcp::HttpMcp::new(&url, Some(key.clone()), conn.ca.as_deref()).map_err(|e| e.to_string())?;
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
fn keep_capabilities(app: &App, url: &str, caps: &Capabilities) {
    let host = url.split("://").nth(1).unwrap_or(url).replace([':', '/'], "_");
    let file = app.paths.servers_dir().join(format!("{host}.json"));
    let body = serde_json::json!({"url": url, "probedAt": chrono::Utc::now().to_rfc3339(), "capabilities": caps});
    if std::fs::create_dir_all(app.paths.servers_dir()).is_ok() {
        let _ = std::fs::write(file, serde_json::to_string_pretty(&body).unwrap_or_default());
    }
}

pub async fn doctor(app: &App) -> anyhow::Result<()> {
    let ui = app.ui;
    println!("{}", ui.bold(&format!("{} {}", ca_core::PRODUCT, ca_core::VERSION)));
    println!("  settings  {}", app.paths.config.display());
    println!("  data      {}", app.paths.data.display());
    let c = &app.connection;
    let Some(url) = &c.url else {
        println!("\nNot signed in: code-arena login --url https://YOUR-ARENA");
        return Ok(());
    };
    println!("\n{}", ui.bold("Arena"));
    println!("  address   {url}");
    println!("  gateway   {}", c.gateway_url().unwrap_or_else(|| "(none: give one with login --gateway)".into()));
    println!("  tools     {}", c.mcp_url().unwrap_or_default());
    if let Some(ca) = &c.ca {
        println!("  CA        {}", ca.display());
    }
    let arena = app.arena()?;
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
    match arena.capabilities().await {
        Ok(caps) => {
            for f in Feature::ALL {
                println!("  {:<22}{}", f.name(), state_words(&ui, &caps.state(*f)));
            }
            keep_capabilities(app, url, &caps);
        }
        Err(e) => println!("  {}", ui.red(&e.to_string())),
    }
    Ok(())
}
