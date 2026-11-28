//! The agent's commands in a sandbox: bubblewrap on Linux, `sandbox-exec` on macOS. They read everything but what
//! holds secrets (which they cannot see), write only in the folders the tools may use, the build tools' caches and a
//! `/tmp` of their own, and cannot reach Docker's or Podman's sockets. Windows has none Code Arena can use.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Bubblewrap(PathBuf),
    SandboxExec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sandbox {
    pub kind: Kind,
    pub network: bool,
    pub writable: Vec<PathBuf>,
    pub hidden: Vec<PathBuf>,
}

/// What holds secrets, under the home folder: hidden from commands.
const SECRETS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".kube",
    ".docker",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    ".config/gh",
    ".mozilla",
    ".config/google-chrome",
    ".config/chromium",
    ".config/BraveSoftware",
    ".config/microsoft-edge",
    "Library/Keychains",
    "Library/Application Support/Google/Chrome",
];

/// The JSON files right in the home folder whose name starts with a dot (`~/.tool.json`).
fn home_json(home: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with('.') && n.ends_with(".json") && e.path().is_file()
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// The build tools' caches, under the home folder: writable.
const CACHES: &[&str] = &[
    ".cache",
    ".npm",
    ".cargo/registry",
    ".cargo/git",
    ".nuget/packages",
    "go/pkg/mod",
    ".gradle",
    ".m2",
    ".local/share/pnpm",
    ".yarn",
    "Library/Caches",
];

/// Container engines' sockets: out of reach.
const SOCKETS: &[&str] = &["/var/run/docker.sock", "/run/docker.sock", "/run/podman/podman.sock"];

impl Sandbox {
    /// The sandbox for this machine, made of the folders the tools may use, the person's home and Code Arena's own
    /// folders (hidden too).
    pub fn new(
        kind: Kind,
        network: bool,
        allowed: &[PathBuf],
        extra_writable: &[PathBuf],
        home: Option<&Path>,
        own: &[PathBuf],
        runtime_dir: Option<&Path>,
    ) -> Self {
        let mut writable: Vec<PathBuf> = allowed.to_vec();
        writable.extend(extra_writable.iter().cloned());
        let mut hidden: Vec<PathBuf> = own.to_vec();
        if let Some(h) = home {
            writable.extend(CACHES.iter().map(|c| h.join(c)));
            hidden.extend(SECRETS.iter().map(|s| h.join(s)));
            // The home's own `.NAME.json` files: other tools keep their state and keys there.
            hidden.extend(home_json(h));
        }
        hidden.extend(SOCKETS.iter().map(PathBuf::from));
        if let Some(r) = runtime_dir {
            hidden.push(r.join("docker.sock"));
            hidden.push(r.join("podman/podman.sock"));
        }
        // Each where it really is (`/var/run` is a link to `/run`: bubblewrap cannot bind through a link), once.
        let real = |v: Vec<PathBuf>| {
            let mut out: Vec<PathBuf> = Vec::new();
            for p in v {
                let r = std::fs::canonicalize(&p).unwrap_or(p);
                if !out.contains(&r) {
                    out.push(r);
                }
            }
            out
        };
        let writable = real(writable);
        let mut hidden = real(hidden);
        // A folder the tools may use is never hidden, even one inside a hidden one.
        hidden
            .retain(|h| !allowed.iter().any(|a| std::fs::canonicalize(a).unwrap_or_else(|_| a.clone()).starts_with(h)));
        Sandbox { kind, network, writable, hidden }
    }

    /// `program args…` as the sandbox runs it, in `cwd`.
    pub fn wrap(&self, program: &str, args: &[String], cwd: &Path) -> (String, Vec<String>) {
        self.wrap_reading(program, args, cwd, &[])
    }

    /// As [`Sandbox::wrap`], with folders under `/tmp` the command must still read (a folder opened from there, the
    /// program in it): the sandbox's own `/tmp` would hide them. Reading anywhere else needs nothing.
    pub fn wrap_reading(
        &self,
        program: &str,
        args: &[String],
        cwd: &Path,
        readable: &[PathBuf],
    ) -> (String, Vec<String>) {
        match &self.kind {
            Kind::Bubblewrap(bwrap) => {
                let mut a: Vec<String> = ["--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                for r in readable {
                    let r = std::fs::canonicalize(r).unwrap_or_else(|_| r.clone());
                    if r.starts_with("/tmp") && r.exists() && !self.writable.iter().any(|w| r.starts_with(w)) {
                        let p = r.display().to_string();
                        a.extend(["--ro-bind".into(), p.clone(), p]);
                    }
                }
                for w in &self.writable {
                    if w.exists() {
                        let p = w.display().to_string();
                        a.extend(["--bind".into(), p.clone(), p]);
                    }
                }
                for h in &self.hidden {
                    let p = h.display().to_string();
                    if h.is_dir() {
                        a.extend(["--tmpfs".into(), p]);
                    } else if h.exists() || std::fs::symlink_metadata(h).is_ok() {
                        a.extend(["--ro-bind".into(), "/dev/null".into(), p]);
                    }
                }
                if !self.network {
                    a.push("--unshare-net".into());
                }
                a.extend([
                    "--die-with-parent".into(),
                    "--new-session".into(),
                    "--chdir".into(),
                    cwd.display().to_string(),
                    "--".into(),
                    program.into(),
                ]);
                a.extend(args.iter().cloned());
                (bwrap.display().to_string(), a)
            }
            Kind::SandboxExec => {
                let mut a = vec!["-p".to_string(), self.profile(), program.to_string()];
                a.extend(args.iter().cloned());
                ("/usr/bin/sandbox-exec".into(), a)
            }
        }
    }

    /// macOS's sandbox profile: the last rule that matches decides.
    fn profile(&self) -> String {
        let q = |p: &Path| format!("\"{}\"", p.display().to_string().replace('\\', "\\\\").replace('"', "\\\""));
        let mut s = String::from("(version 1)\n(allow default)\n(deny file-write*)\n");
        s.push_str("(allow file-write* (subpath \"/private/tmp\") (subpath \"/private/var/folders\") (literal \"/dev/null\") (literal \"/dev/tty\") (regex #\"^/dev/fd/\"))\n");
        for w in &self.writable {
            s.push_str(&format!("(allow file-write* (subpath {}))\n", q(w)));
        }
        for h in &self.hidden {
            s.push_str(&format!("(deny file-read* file-write* (subpath {}))\n", q(h)));
        }
        if !self.network {
            s.push_str("(deny network*)\n(allow network* (local unix))\n");
        }
        s
    }

    pub fn describe(&self) -> Vec<String> {
        let mut out = vec![match &self.kind {
            Kind::Bubblewrap(p) => format!("Commands run in bubblewrap ({}).", p.display()),
            Kind::SandboxExec => "Commands run in macOS's sandbox (sandbox-exec).".into(),
        }];
        out.push(if self.network {
            "They may reach the network.".into()
        } else {
            "They cannot reach the network.".into()
        });
        out.push("They may write in:".into());
        out.extend(self.writable.iter().filter(|w| w.exists()).map(|w| format!("  {}", w.display())));
        out.push("  a /tmp of their own".into());
        out.push("They cannot see:".into());
        out.extend(self.hidden.iter().filter(|h| h.exists()).map(|h| format!("  {}", h.display())));
        out
    }
}

/// This machine's sandbox program, if it can make a sandbox here (bubblewrap needs user namespaces, which some
/// containers do not allow): found once a run. `Err` says why not.
pub fn find() -> Result<Kind, String> {
    static FOUND: OnceLock<Result<Kind, String>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            if cfg!(target_os = "macos") {
                return if Path::new("/usr/bin/sandbox-exec").exists() {
                    Ok(Kind::SandboxExec)
                } else {
                    Err("sandbox-exec is not on this Mac".into())
                };
            }
            if cfg!(windows) {
                return Err("Windows has no sandbox Code Arena can use".into());
            }
            let Some(bwrap) = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .filter(|d| d.is_absolute())
                .map(|d| d.join("bwrap"))
                .find(|p| p.is_file())
            else {
                return Err("bubblewrap (bwrap) is not installed: apt install bubblewrap".into());
            };
            let ok = std::process::Command::new(&bwrap)
                .args(["--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--", "true"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                Ok(Kind::Bubblewrap(bwrap))
            } else {
                Err("bubblewrap cannot make a sandbox here (user namespaces are not allowed, as in some containers)"
                    .into())
            }
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn bubblewraps_arguments() {
        let d = tempfile::tempdir().expect("temp");
        let work = d.path().join("work");
        let home = d.path().join("home");
        std::fs::create_dir_all(&work).expect("mkdir");
        std::fs::create_dir_all(home.join(".ssh")).expect("mkdir");
        std::fs::create_dir_all(home.join(".cache")).expect("mkdir");
        std::fs::write(home.join(".netrc"), "x").expect("write");
        // Where they really are (on macOS the temporary folder is under /private).
        let (work, home) = (std::fs::canonicalize(&work).expect("canon"), std::fs::canonicalize(&home).expect("canon"));
        let s = Sandbox::new(
            Kind::Bubblewrap("/usr/bin/bwrap".into()),
            false,
            std::slice::from_ref(&work),
            &[],
            Some(&home),
            &[],
            None,
        );
        let (prog, args) = s.wrap("bash", &["-c".into(), "make".into()], &work);
        assert_eq!(prog, "/usr/bin/bwrap");
        let joined = args.join(" ");
        assert!(joined.starts_with("--ro-bind / / --dev /dev --proc /proc --tmpfs /tmp"));
        assert!(joined.contains(&format!("--bind {} {}", work.display(), work.display())));
        assert!(joined.contains(&format!("--bind {0} {0}", home.join(".cache").display())));
        assert!(joined.contains(&format!("--tmpfs {}", home.join(".ssh").display())));
        assert!(joined.contains(&format!("--ro-bind /dev/null {}", home.join(".netrc").display())));
        assert!(joined.contains("--unshare-net"));
        assert!(joined.ends_with(&format!("--chdir {} -- bash -c make", work.display())));
    }

    #[cfg(unix)]
    #[test]
    fn paths_through_links_are_bound_where_they_really_are() {
        let d = tempfile::tempdir().expect("temp");
        let real = d.path().join("run");
        std::fs::create_dir_all(&real).expect("mkdir");
        std::fs::write(real.join("docker.sock"), "").expect("write");
        std::os::unix::fs::symlink(&real, d.path().join("varrun")).expect("link");
        let s = Sandbox::new(
            Kind::SandboxExec,
            true,
            &[],
            &[],
            None,
            &[d.path().join("varrun/docker.sock"), real.join("docker.sock")],
            None,
        );
        let canon = std::fs::canonicalize(real.join("docker.sock")).expect("canon");
        assert_eq!(s.hidden.iter().filter(|h| **h == canon).count(), 1);
        assert!(!s.hidden.iter().any(|h| h.starts_with(d.path().join("varrun"))));
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_the_tools_may_use_is_never_hidden() {
        let home = PathBuf::from("/home/me");
        let s = Sandbox::new(Kind::SandboxExec, true, &[home.join(".ssh/project")], &[], Some(&home), &[], None);
        assert!(!s.hidden.contains(&home.join(".ssh")));
        assert!(s.profile().contains("(allow file-write* (subpath \"/home/me/.ssh/project\"))"));
        assert!(!s.profile().contains("deny network"));
    }

    /// Where this machine can make one: a command cannot write outside, nor see a hidden folder.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_real_sandbox_keeps_writes_inside() {
        let Ok(kind) = find() else { return };
        // Not in a temporary folder: commands may write the system's (on macOS, $TMPDIR under /var/folders).
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        std::fs::create_dir_all(&target).expect("target");
        let d = tempfile::tempdir_in(&target).expect("temp");
        let work = d.path().join("work");
        let outside = d.path().join("outside");
        let secret = d.path().join("secret");
        std::fs::create_dir_all(&work).expect("mkdir");
        std::fs::create_dir_all(&outside).expect("mkdir");
        std::fs::create_dir_all(&secret).expect("mkdir");
        std::fs::write(secret.join("key"), "sk").expect("write");
        let work = std::fs::canonicalize(&work).expect("canon");
        let s = Sandbox {
            kind,
            network: true,
            writable: vec![work.clone()],
            hidden: vec![std::fs::canonicalize(&secret).expect("canon")],
        };
        let script = format!(
            "touch {w}/in && (touch {o}/out 2>/dev/null; echo out=$?) && (cat {s}/key 2>/dev/null; echo key=$?)",
            w = work.display(),
            o = std::fs::canonicalize(&outside).expect("canon").display(),
            s = std::fs::canonicalize(&secret).expect("canon").display()
        );
        let (prog, args) = s.wrap("sh", &["-c".into(), script], &work);
        let out = std::process::Command::new(prog).args(args).output().expect("runs");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(work.join("in").exists(), "{text}");
        assert!(!outside.join("out").exists(), "{text}");
        assert!(!text.contains("sk"), "{text}");
    }
}
