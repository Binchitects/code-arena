//! Schedules: a prompt Code Arena runs on its own, in a folder, as a session of its own (`code-arena -p`, kept with a
//! chat in Arena as any session is): every so often, or every day (or days) at a time. Kept in the config folder's
//! `schedules.json`; run by `code-arena schedule due`, which a timer of the system's calls (`code-arena schedule
//! install`: a systemd user timer, a launchd agent, or a Task Scheduler task).

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub name: String,
    pub prompt: String,
    pub folder: PathBuf,
    /// How often, in seconds (at least a minute).
    pub every: u64,
    /// For a day or more: the time of day it runs at (local, `09:00`).
    #[serde(default)]
    pub at: Option<String>,
    /// plan (reads only), ask (each change refused: nobody is there to allow it), auto-edit or yolo.
    pub mode: String,
    #[serde(default)]
    pub model: Option<String>,
    /// Seconds since 1970.
    pub added: i64,
    #[serde(default)]
    pub last_run: Option<i64>,
    /// "ok", "failed (exit code N)", "running".
    #[serde(default)]
    pub last_status: Option<String>,
}

/// A run marked running this long ago ended without saying so (its process was killed): it may run again.
const STALE_SECS: i64 = 6 * 3600;

/// "90s", "30m", "2h", "1d": an interval in seconds.
pub fn interval(text: &str) -> Option<u64> {
    let t = text.trim();
    let (n, unit) = t.split_at(t.find(|c: char| !c.is_ascii_digit())?);
    let n: u64 = n.parse().ok()?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86_400,
        _ => return None,
    };
    (secs > 0).then_some(secs)
}

pub fn interval_words(secs: u64) -> String {
    match secs {
        s if s % 86_400 == 0 => format!("{}d", s / 86_400),
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

fn hhmm(at: &str) -> Option<(u32, u32)> {
    let (h, m) = at.split_once(':')?;
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}

impl Schedule {
    /// When it runs next (seconds since 1970): from its last run (or when it was added) every so often; for a day or
    /// more at its time of day.
    pub fn next(&self) -> i64 {
        use chrono::{Local, TimeZone, Timelike};
        let from = self.last_run.unwrap_or(self.added);
        let mut next = from + self.every as i64;
        if let Some((h, m)) = self.at.as_deref().and_then(hhmm)
            && self.every >= 86_400
            && let Some(t) = Local.timestamp_opt(next, 0).single()
            && let Some(at) = t.with_hour(h).and_then(|t| t.with_minute(m)).and_then(|t| t.with_second(0))
        {
            next = at.timestamp();
            // Its time that day is before the interval ends: the day after.
            if next < from + self.every as i64 - 3600 {
                next += 86_400;
            }
        }
        next
    }

    pub fn due(&self, now: i64) -> bool {
        if self.last_status.as_deref() == Some("running") && self.last_run.is_some_and(|r| now - r < STALE_SECS) {
            return false;
        }
        self.next() <= now
    }

    pub fn line(&self) -> String {
        let when = chrono::DateTime::from_timestamp(self.next(), 0)
            .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        format!(
            "{}  every {}{}  {}  in {}  ({}; next {when}): {}",
            self.name,
            interval_words(self.every),
            self.at.as_ref().map(|a| format!(" at {a}")).unwrap_or_default(),
            self.mode,
            self.folder.display(),
            self.last_status.clone().unwrap_or_else(|| "not run yet".into()),
            self.prompt.chars().take(80).collect::<String>()
        )
    }
}

pub fn file(config: &Path) -> PathBuf {
    config.join("schedules.json")
}

pub fn load(config: &Path) -> Vec<Schedule> {
    std::fs::read_to_string(file(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(config: &Path, all: &[Schedule]) -> anyhow::Result<()> {
    std::fs::create_dir_all(config)?;
    let tmp = file(config).with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(all)?)?;
    std::fs::rename(tmp, file(config))?;
    Ok(())
}

/// One schedule changed in the file (by its name), whatever else changed it meanwhile.
fn change(config: &Path, name: &str, f: impl FnOnce(&mut Schedule)) -> anyhow::Result<()> {
    let mut all = load(config);
    if let Some(s) = all.iter_mut().find(|s| s.name == name) {
        f(s);
    }
    save(config, &all)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Runs it now, in its folder, as its own session; its output kept in `schedules/NAME.log`. The exit code.
pub fn run(config: &Path, exe: &Path, s: &Schedule) -> anyhow::Result<i32> {
    change(config, &s.name, |x| {
        x.last_run = Some(now());
        x.last_status = Some("running".into());
    })?;
    let logs = config.join("schedules");
    std::fs::create_dir_all(&logs)?;
    let log = std::fs::File::create(logs.join(format!("{}.log", s.name)))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("-C").arg(&s.folder).arg("-p").arg(&s.prompt).arg("--mode").arg(&s.mode);
    if let Some(m) = &s.model {
        cmd.arg("--model").arg(m);
    }
    let status = cmd.stdin(std::process::Stdio::null()).stdout(log.try_clone()?).stderr(log).status();
    let code = status.as_ref().ok().and_then(|s| s.code()).unwrap_or(-1);
    change(config, &s.name, |x| {
        x.last_status = Some(if code == 0 { "ok".into() } else { format!("failed (exit code {code})") });
    })?;
    Ok(code)
}

/// `code-arena schedule …`.
pub fn command(config: &Path, home: Option<&Path>, cwd: &Path, args: &[String]) -> anyhow::Result<i32> {
    let exe = std::env::current_exe().context("this program's own path")?;
    let usage = "code-arena schedule add NAME --every 2h [--at 09:00] [--mode plan|ask|auto-edit|yolo] [--model M] [-C FOLDER] PROMPT…; list; run NAME; remove NAME; due; install; uninstall";
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["list"] => {
            let all = load(config);
            if all.is_empty() {
                println!("No schedules yet. {usage}");
            }
            for s in all {
                println!("{}", s.line());
            }
            Ok(0)
        }
        ["add", name, rest @ ..] => {
            let mut every = None;
            let mut at = None;
            let mut mode = "plan".to_string();
            let mut model = None;
            let mut folder = cwd.to_path_buf();
            let mut prompt = Vec::new();
            let mut i = 0;
            while i < rest.len() {
                let next = |i: usize| {
                    rest.get(i + 1).map(|s| s.to_string()).ok_or_else(|| anyhow::anyhow!("{} needs a value", rest[i]))
                };
                match rest[i] {
                    "--every" => {
                        every =
                            Some(interval(&next(i)?).ok_or_else(|| anyhow::anyhow!("--every takes 90s, 30m, 2h, 1d"))?);
                        i += 1;
                    }
                    "--at" => {
                        let a = next(i)?;
                        if hhmm(&a).is_none() {
                            bail!("--at takes a time of day: 09:00");
                        }
                        at = Some(a);
                        i += 1;
                    }
                    "--mode" => {
                        mode = next(i)?;
                        if ca_agent::Mode::parse(&mode).is_none() {
                            bail!("--mode is plan, ask, auto-edit or yolo");
                        }
                        i += 1;
                    }
                    "--model" => {
                        model = Some(next(i)?);
                        i += 1;
                    }
                    "-C" | "--cd" => {
                        folder = PathBuf::from(next(i)?);
                        i += 1;
                    }
                    w => prompt.push(w),
                }
                i += 1;
            }
            let Some(every) = every else { bail!("how often: --every 2h") };
            if every < 60 {
                bail!("a schedule runs at most every minute");
            }
            if prompt.is_empty() {
                bail!("what it does: the prompt, after the options");
            }
            let folder = std::fs::canonicalize(&folder).with_context(|| format!("the folder {}", folder.display()))?;
            let mut all = load(config);
            if all.iter().any(|s| s.name == *name) {
                bail!("there is a schedule {name} already: remove it first");
            }
            let s = Schedule {
                name: name.to_string(),
                prompt: prompt.join(" "),
                folder,
                every,
                at,
                mode,
                model,
                added: now(),
                last_run: None,
                last_status: None,
            };
            println!("Added: {}", s.line());
            all.push(s);
            save(config, &all)?;
            println!(
                "It runs when `code-arena schedule due` runs: `code-arena schedule install` sets a timer of the system's for it."
            );
            Ok(0)
        }
        ["remove", name] => {
            let mut all = load(config);
            let before = all.len();
            all.retain(|s| s.name != *name);
            if all.len() == before {
                bail!("no schedule {name}");
            }
            save(config, &all)?;
            println!("Removed {name}.");
            Ok(0)
        }
        ["run", name] => {
            let s = load(config)
                .into_iter()
                .find(|s| s.name == *name)
                .ok_or_else(|| anyhow::anyhow!("no schedule {name}"))?;
            let code = run(config, &exe, &s)?;
            println!(
                "{name}: {} (its output: {})",
                if code == 0 { "ok".to_string() } else { format!("exit code {code}") },
                config.join("schedules").join(format!("{name}.log")).display()
            );
            Ok(code)
        }
        ["due"] => {
            let t = now();
            let mut ran = 0;
            for s in load(config).into_iter().filter(|s| s.due(t)) {
                ran += 1;
                let _ = run(config, &exe, &s);
            }
            if ran > 0 {
                println!("{ran} schedule(s) ran.");
            }
            Ok(0)
        }
        ["install"] => install(home, &exe),
        ["uninstall"] => uninstall(home),
        _ => {
            eprintln!("{usage}");
            Ok(2)
        }
    }
}

/// A timer of the system's that runs `code-arena schedule due` every 5 minutes. On Linux and macOS its files are
/// written and the command that starts it said; on Windows Task Scheduler makes the task.
fn install(home: Option<&Path>, exe: &Path) -> anyhow::Result<i32> {
    let exe = exe.display().to_string();
    if cfg!(windows) {
        let ok = std::process::Command::new("schtasks")
            .args([
                "/Create",
                "/F",
                "/SC",
                "MINUTE",
                "/MO",
                "5",
                "/TN",
                "Code Arena schedules",
                "/TR",
                &format!("\"{exe}\" schedule due"),
            ])
            .status()
            .is_ok_and(|s| s.success());
        println!(
            "{}",
            if ok {
                "Task Scheduler runs \"code-arena schedule due\" every 5 minutes (task \"Code Arena schedules\")."
            } else {
                "schtasks did not make the task."
            }
        );
        return Ok(if ok { 0 } else { 1 });
    }
    let home = home.ok_or_else(|| anyhow::anyhow!("no home folder to write the timer into"))?;
    if cfg!(target_os = "macos") {
        let plist = home.join("Library/LaunchAgents/dev.code-arena.schedules.plist");
        std::fs::create_dir_all(plist.parent().unwrap_or(home))?;
        let esc = exe.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        std::fs::write(
            &plist,
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n  <key>Label</key><string>dev.code-arena.schedules</string>\n  <key>ProgramArguments</key><array><string>{esc}</string><string>schedule</string><string>due</string></array>\n  <key>StartInterval</key><integer>300</integer>\n</dict></plist>\n"
            ),
        )?;
        println!("Written {}. Load it: launchctl load {}", plist.display(), plist.display());
        return Ok(0);
    }
    let units = home.join(".config/systemd/user");
    std::fs::create_dir_all(&units)?;
    std::fs::write(
        units.join("code-arena-schedules.service"),
        format!(
            "[Unit]\nDescription=Code Arena's schedules: those due now\n\n[Service]\nType=oneshot\nExecStart=\"{exe}\" schedule due\n"
        ),
    )?;
    std::fs::write(
        units.join("code-arena-schedules.timer"),
        "[Unit]\nDescription=Code Arena's schedules, every 5 minutes\n\n[Timer]\nOnBootSec=2min\nOnUnitActiveSec=5min\n\n[Install]\nWantedBy=timers.target\n",
    )?;
    println!(
        "Written {0}/code-arena-schedules.timer and .service. Start it: systemctl --user daemon-reload && systemctl --user enable --now code-arena-schedules.timer",
        units.display()
    );
    Ok(0)
}

fn uninstall(home: Option<&Path>) -> anyhow::Result<i32> {
    if cfg!(windows) {
        let ok = std::process::Command::new("schtasks")
            .args(["/Delete", "/F", "/TN", "Code Arena schedules"])
            .status()
            .is_ok_and(|s| s.success());
        return Ok(if ok { 0 } else { 1 });
    }
    let home = home.ok_or_else(|| anyhow::anyhow!("no home folder"))?;
    for f in [
        home.join("Library/LaunchAgents/dev.code-arena.schedules.plist"),
        home.join(".config/systemd/user/code-arena-schedules.timer"),
        home.join(".config/systemd/user/code-arena-schedules.service"),
    ] {
        if f.exists() {
            std::fs::remove_file(&f)?;
            println!("Removed {}.", f.display());
        }
    }
    println!("On Linux, stop it too: systemctl --user disable --now code-arena-schedules.timer");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(every: u64, at: Option<&str>, added: i64) -> Schedule {
        Schedule {
            name: "n".into(),
            prompt: "p".into(),
            folder: "/r".into(),
            every,
            at: at.map(str::to_string),
            mode: "plan".into(),
            model: None,
            added,
            last_run: None,
            last_status: None,
        }
    }

    #[test]
    fn intervals_and_when_it_is_due() {
        assert_eq!(interval("90s"), Some(90));
        assert_eq!(interval("30m"), Some(1800));
        assert_eq!(interval("2h"), Some(7200));
        assert_eq!(interval("1d"), Some(86_400));
        assert_eq!(interval("2x"), None);
        assert_eq!(interval_words(7200), "2h");
        let x = s(3600, None, 1_000_000);
        assert!(!x.due(1_000_000 + 3599) && x.due(1_000_000 + 3600));
        // Running now: not again, unless it went quiet long ago.
        let mut r = x.clone();
        r.last_run = Some(1_000_000);
        r.last_status = Some("running".into());
        assert!(!r.due(1_000_000 + 7200));
        assert!(r.due(1_000_000 + STALE_SECS + 1));
        // Daily at a time: the next run falls at that time.
        let d = s(86_400, Some("09:00"), 1_000_000);
        let next = chrono::DateTime::from_timestamp(d.next(), 0).expect("t").with_timezone(&chrono::Local);
        assert_eq!(next.format("%H:%M").to_string(), "09:00");
    }
}
