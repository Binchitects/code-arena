//! VMware Workstation and Fusion through `vmrun`: a machine is its `.vmx` file, the guest is reached through VMware
//! Tools. `vmrun` takes the guest's password only as an argument, and a program run in the guest gives back only its
//! exit code, so a command's output goes to a file in the guest that is copied back.

use std::path::Path;

use super::run::Call;
use super::{Machine, Os, Power, Snap};

const P: &str = "vmrun";

/// The host type: Fusion on macOS, Workstation elsewhere.
fn host() -> &'static str {
    if cfg!(target_os = "macos") { "fusion" } else { "ws" }
}

fn base() -> Call {
    Call::new(P, &["-T", host()])
}

fn signed(user: &str, password: &str) -> Call {
    base().args(["-gu", user, "-gp", password])
}

/// Only the running machines: `vmrun` knows no others.
pub fn list() -> Call {
    base().arg("list")
}

/// The machines in what `vmrun list` says ("Total running VMs: N", then one `.vmx` a line).
pub fn machines(said: &str) -> Vec<Machine> {
    said.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("Total running"))
        .map(|l| Machine { name: l.to_string(), state: "running".into() })
        .collect()
}

pub fn power(vmx: &str, p: Power) -> Call {
    match p {
        Power::Start => base().args(["start", vmx, "nogui"]),
        Power::Shutdown => base().args(["stop", vmx, "soft"]),
        Power::Off => base().args(["stop", vmx, "hard"]),
        Power::Reset => base().args(["reset", vmx, "soft"]),
        Power::Pause => base().args(["pause", vmx]),
        Power::Resume => base().args(["unpause", vmx]),
    }
}

pub fn snapshot(vmx: &str, s: &Snap) -> Call {
    match s {
        Snap::List => base().args(["listSnapshots", vmx]),
        Snap::Take(n) => base().args(["snapshot", vmx, n]),
        Snap::Restore(n) => base().args(["revertToSnapshot", vmx, n]),
        Snap::Delete(n) => base().args(["deleteSnapshot", vmx, n]),
    }
}

/// The snapshots in what `listSnapshots` says ("Total snapshots: N", then a name a line).
pub fn snapshot_names(said: &str) -> Vec<String> {
    said.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("Total snapshots")).map(String::from).collect()
}

pub fn screenshot(vmx: &str, user: &str, password: &str, file: &Path) -> Call {
    signed(user, password).args(["captureScreen", vmx]).arg(file.to_string_lossy())
}

/// Where a command's output is written in the guest.
pub fn out_file(os: Os) -> &'static str {
    match os {
        Os::Linux => "/tmp/code-arena-vm-out.txt",
        Os::Windows => "C:\\Windows\\Temp\\code-arena-vm-out.txt",
    }
}

/// A command run in the guest through its shell, its output and errors into [`out_file`].
pub fn run(vmx: &str, user: &str, password: &str, os: Os, command: &str) -> Call {
    let (shell, flag) = os.shell();
    // Both shells read the redirection the same way.
    let line = format!("({command}) > {} 2>&1", out_file(os));
    signed(user, password).args(["runProgramInGuest", vmx, shell, flag]).arg(line)
}

/// The exit code `runProgramInGuest` tells when it is not 0 ("Guest program exited with non-zero exit code: N").
pub fn exit_code(said: &str) -> Option<i32> {
    let at = said.find("exit code:")?;
    said[at + "exit code:".len()..].split_whitespace().next()?.trim_end_matches('.').parse().ok()
}

pub fn copy(vmx: &str, user: &str, password: &str, to_guest: bool, from: &str, to: &str) -> Call {
    let what = if to_guest { "copyFileFromHostToGuest" } else { "copyFileFromGuestToHost" };
    signed(user, password).args([what, vmx, from, to])
}

pub fn delete_in_guest(vmx: &str, user: &str, password: &str, file: &str) -> Call {
    signed(user, password).args(["deleteFileInGuest", vmx, file])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_vmrun_says_is_read() {
        let m = machines("Total running VMs: 2\n/vms/a/a.vmx\n/vms/b/b.vmx\n");
        assert_eq!(m.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["/vms/a/a.vmx", "/vms/b/b.vmx"]);
        assert_eq!(snapshot_names("Total snapshots: 1\nclean\n"), ["clean"]);
        assert_eq!(exit_code("Error: Guest program exited with non-zero exit code: 2"), Some(2));
        assert_eq!(exit_code("all well"), None);
    }

    #[test]
    fn a_run_writes_its_output_to_a_file_in_the_guest() {
        let c = run("/a.vmx", "tester", "fake-pass", Os::Linux, "make test");
        assert!(c.args.windows(2).any(|w| w == ["-gu", "tester"]));
        assert_eq!(c.args.last().map(String::as_str), Some("(make test) > /tmp/code-arena-vm-out.txt 2>&1"));
        assert_eq!(power("/a.vmx", Power::Off).args[2..], ["stop", "/a.vmx", "hard"]);
        assert_eq!(copy("/a.vmx", "u", "p", true, "/h", "/g").args[6], "copyFileFromHostToGuest");
    }
}
