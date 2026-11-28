//! VirtualBox through `VBoxManage`: its machines by name, the guest reached through the Guest Additions, the
//! password given in a file only its owner reads.

use std::path::Path;

use super::run::Call;
use super::{Machine, Os, Power, Snap};

const P: &str = "VBoxManage";

pub fn list_all() -> Call {
    Call::new(P, &["list", "vms"])
}

pub fn list_running() -> Call {
    Call::new(P, &["list", "runningvms"])
}

/// The names in what `list vms` says (`"name" {uuid}` a line).
pub fn names(said: &str) -> Vec<String> {
    said.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix('"')?;
            let end = rest.rfind('"')?;
            Some(rest[..end].to_string())
        })
        .collect()
}

/// The machines, from `list vms` and `list runningvms`.
pub fn machines(all: &str, running: &str) -> Vec<Machine> {
    let on = names(running);
    names(all)
        .into_iter()
        .map(|name| {
            let state = if on.contains(&name) { "running" } else { "off" }.to_string();
            Machine { name, state }
        })
        .collect()
}

pub fn power(vm: &str, p: Power) -> Call {
    match p {
        Power::Start => Call::new(P, &["startvm", vm, "--type", "headless"]),
        Power::Shutdown => Call::new(P, &["controlvm", vm, "acpipowerbutton"]),
        Power::Off => Call::new(P, &["controlvm", vm, "poweroff"]),
        Power::Reset => Call::new(P, &["controlvm", vm, "reset"]),
        Power::Pause => Call::new(P, &["controlvm", vm, "pause"]),
        Power::Resume => Call::new(P, &["controlvm", vm, "resume"]),
    }
}

pub fn snapshot(vm: &str, s: &Snap) -> Call {
    match s {
        Snap::List => Call::new(P, &["snapshot", vm, "list", "--machinereadable"]),
        Snap::Take(n) => Call::new(P, &["snapshot", vm, "take", n]),
        Snap::Restore(n) => Call::new(P, &["snapshot", vm, "restore", n]),
        Snap::Delete(n) => Call::new(P, &["snapshot", vm, "delete", n]),
    }
}

/// The snapshots' names in `snapshot list --machinereadable` (`SnapshotName…="name"` lines).
pub fn snapshot_names(said: &str) -> Vec<String> {
    said.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            k.starts_with("SnapshotName").then(|| v.trim().trim_matches('"').to_string())
        })
        .collect()
}

pub fn screenshot(vm: &str, file: &Path) -> Call {
    Call::new(P, &["controlvm", vm]).arg("screenshotpng").arg(file.to_string_lossy())
}

/// A command run in the guest through its shell, its output waited for.
pub fn run(vm: &str, user: &str, password_file: &Path, os: Os, command: &str, limit_ms: u128) -> Call {
    let (shell, flag) = os.shell();
    Call::new(P, &["guestcontrol", vm, "run", "--exe", shell, "--username", user])
        .arg("--passwordfile")
        .arg(password_file.to_string_lossy())
        .args(["--wait-stdout", "--wait-stderr", "--timeout"])
        .arg(limit_ms.to_string())
        .args(["--", shell, flag, command])
}

/// A file copied into the guest (`to_guest`) or out of it, into the directory `to`.
pub fn copy(vm: &str, user: &str, password_file: &Path, to_guest: bool, from: &str, to: &str) -> Call {
    Call::new(P, &["guestcontrol", vm, if to_guest { "copyto" } else { "copyfrom" }, "--username", user])
        .arg("--passwordfile")
        .arg(password_file.to_string_lossy())
        .args(["--recursive", "--target-directory", to, from])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machines_are_read_from_what_it_lists() {
        let all = "\"test-vm\" {1111}\n\"Win 11 \"dev\"\" {2222}\n<inaccessible> {3}\n";
        let running = "\"test-vm\" {1111}\n";
        let m = machines(all, running);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].name.as_str(), m[0].state.as_str()), ("test-vm", "running"));
        assert_eq!((m[1].name.as_str(), m[1].state.as_str()), ("Win 11 \"dev\"", "off"));
        let snaps = "SnapshotName=\"clean\"\nSnapshotUUID=\"x\"\nSnapshotName-1=\"after\"\nCurrentSnapshotName=\"after\"\n";
        assert_eq!(snapshot_names(snaps), ["clean", "after"]);
    }

    #[test]
    fn a_run_takes_the_password_from_a_file_never_an_argument() {
        let c = run("test-vm", "tester", Path::new("/tmp/pw"), Os::Linux, "make test", 60_000);
        assert_eq!(c.program, "VBoxManage");
        assert!(c.args.windows(2).any(|w| w == ["--passwordfile", "/tmp/pw"]));
        assert_eq!(c.args[c.args.len() - 4..], ["--", "/bin/sh", "-c", "make test"]);
        assert!(!c.args.iter().any(|a| a == "--password"));
        let w = copy("test-vm", "tester", Path::new("/tmp/pw"), false, "C:\\logs", "/w/.code-arena/vm");
        assert_eq!(w.args[2], "copyfrom");
        assert_eq!(power("a", Power::Start).args, ["startvm", "a", "--type", "headless"]);
        assert_eq!(snapshot("a", &Snap::Restore("clean".into())).args, ["snapshot", "a", "restore", "clean"]);
    }
}
