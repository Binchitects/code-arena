//! The tools for virtual machines: a command run in a guest, files copied to and from it, its power, its snapshots
//! and its screen, through the hypervisor's own programs (VirtualBox's `VBoxManage`, VMware's `vmrun`, libvirt's
//! `virsh` and Hyper-V's PowerShell). Each hypervisor is a file of its own that makes the programs' arguments and
//! reads what they say; `tools.rs` is what the model calls. A guest's user and password come from the keyring or
//! from the person (`login.rs`), and never from the model nor into what it reads.
//!
//! The hypervisors' programs run as the person, outside the commands' sandbox: they talk to the hypervisor's own
//! service on the host, and reach nothing but the machines. Every call that changes a machine asks as a command does.

pub mod hyperv;
pub mod libvirt;
pub mod login;
pub mod run;
mod tools;
pub mod vbox;
pub mod vmware;

use std::path::PathBuf;
use std::sync::Arc;

pub use login::{Ask, Asker, Guest, Login, Signed};
pub use tools::{VmCopy, VmList, VmPower, VmRun, VmScreenshot, VmSnapshot, VmTest};

/// A hypervisor the tools drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hypervisor {
    VirtualBox,
    VMware,
    Libvirt,
    HyperV,
}

impl Hypervisor {
    pub const ALL: [Hypervisor; 4] =
        [Hypervisor::VirtualBox, Hypervisor::VMware, Hypervisor::Libvirt, Hypervisor::HyperV];

    /// Its name as the tools' `hypervisor` argument and the driver's messages say it.
    pub fn id(self) -> &'static str {
        match self {
            Hypervisor::VirtualBox => "virtualbox",
            Hypervisor::VMware => "vmware",
            Hypervisor::Libvirt => "libvirt",
            Hypervisor::HyperV => "hyperv",
        }
    }

    /// Its name as a person reads it.
    pub fn name(self) -> &'static str {
        match self {
            Hypervisor::VirtualBox => "VirtualBox",
            Hypervisor::VMware => "VMware",
            Hypervisor::Libvirt => "libvirt",
            Hypervisor::HyperV => "Hyper-V",
        }
    }

    pub fn of(id: &str) -> Option<Hypervisor> {
        let id = id.trim().to_ascii_lowercase().replace(['-', ' ', '_'], "");
        Hypervisor::ALL.into_iter().find(|h| h.id() == id || (id == "kvm" || id == "qemu") && *h == Hypervisor::Libvirt)
    }

    /// The program it is driven through.
    pub fn program(self) -> &'static str {
        match self {
            Hypervisor::VirtualBox => "VBoxManage",
            Hypervisor::VMware => "vmrun",
            Hypervisor::Libvirt => "virsh",
            Hypervisor::HyperV => "powershell",
        }
    }

    /// Whether a run in the guest signs in to it. libvirt's guest agent runs as the guest's own service.
    pub fn signs_in(self) -> bool {
        !matches!(self, Hypervisor::Libvirt)
    }

    /// Whether it is here: its program on PATH (Hyper-V only on Windows).
    pub fn here(self) -> bool {
        if self == Hypervisor::HyperV && !cfg!(windows) {
            return false;
        }
        crate::shell::which(self.program())
    }
}

/// The hypervisors on this machine, in the order a machine's name is looked for in them.
pub fn found() -> Vec<Hypervisor> {
    Hypervisor::ALL.into_iter().filter(|h| h.here()).collect()
}

/// Which hypervisor a call is for: the one it names, VMware for a `.vmx` file, else the first one here.
pub fn pick(named: Option<&str>, vm: &str, here: &[Hypervisor]) -> Result<Hypervisor, String> {
    if let Some(n) = named.filter(|n| !n.trim().is_empty()) {
        let h = Hypervisor::of(n).ok_or_else(|| {
            format!("{n} is not a hypervisor the tools know: say virtualbox, vmware, libvirt or hyperv")
        })?;
        return if here.contains(&h) { Ok(h) } else { Err(format!("{} is not installed here.", h.name())) };
    }
    if vm.to_ascii_lowercase().ends_with(".vmx") && here.contains(&Hypervisor::VMware) {
        return Ok(Hypervisor::VMware);
    }
    here.first().copied().ok_or_else(|| {
        "No hypervisor is installed here: the VM tools drive VirtualBox (VBoxManage), VMware (vmrun), libvirt \
         (virsh) or Hyper-V."
            .to_string()
    })
}

/// What to do with a machine's power.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    /// Started (without a window).
    Start,
    /// Asked to shut down, as its power button.
    Shutdown,
    /// Turned off at once.
    Off,
    Reset,
    Pause,
    Resume,
}

impl Power {
    pub fn of(s: &str) -> Option<Power> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "start" | "on" => Power::Start,
            "shutdown" | "stop" => Power::Shutdown,
            "off" | "poweroff" => Power::Off,
            "reset" | "restart" | "reboot" => Power::Reset,
            "pause" | "suspend" => Power::Pause,
            "resume" | "unpause" => Power::Resume,
            _ => return None,
        })
    }
}

/// What to do with a machine's snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Snap {
    List,
    Take(String),
    Restore(String),
    Delete(String),
}

/// A guest's operating system, for how a command is run in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Windows,
}

impl Os {
    pub fn of(s: Option<&str>) -> Os {
        match s.map(|s| s.trim().to_ascii_lowercase()) {
            Some(s) if s.starts_with("win") => Os::Windows,
            _ => Os::Linux,
        }
    }

    /// The guest's shell and how it is given a command.
    pub fn shell(self) -> (&'static str, &'static str) {
        match self {
            Os::Linux => ("/bin/sh", "-c"),
            Os::Windows => ("C:\\Windows\\System32\\cmd.exe", "/c"),
        }
    }
}

/// A machine as the tools list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    pub name: String,
    /// "running", "off", "paused", ... as its hypervisor says it; empty when it does not say.
    pub state: String,
}

/// Whether a text says the guest refused the user and password it was given.
pub fn refused(said: &str) -> bool {
    let s = said.to_ascii_lowercase();
    [
        "verr_authentication_failure",
        "the specified user was not able to logon",
        "invalid user name or password",
        "user name or password is incorrect",
        "the credential is invalid",
    ]
    .iter()
    .any(|w| s.contains(w))
}

/// Whether a failed call of a hypervisor's program failed because the guest refused the sign-in. Only the program's
/// own words count, never the guest command's output: VBoxManage's are its `VBoxManage: error:` lines (the guest's
/// errors come on the same stream), vmrun says its errors on its output, PowerShell on its errors.
pub fn refused_by(hv: Hypervisor, r: &run::Ran) -> bool {
    if r.ok() {
        return false;
    }
    match hv {
        Hypervisor::VirtualBox => r.err.lines().any(|l| l.starts_with("VBoxManage: error:") && refused(l)),
        Hypervisor::VMware => refused(&r.said()),
        Hypervisor::HyperV => refused(&r.err),
        Hypervisor::Libvirt => false,
    }
}

/// Where the tools put what they bring back (screens, files, logs): `.code-arena/vm` in the workspace.
pub fn kept_dir(ws: &ca_workspace::Workspace) -> PathBuf {
    ws.cwd().join(".code-arena").join("vm")
}

/// A name safe as part of a file name.
pub fn file_part(s: &str) -> String {
    let s: String = s
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(s)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .collect();
    let s = s.trim_end_matches(".vmx").trim_matches('-').to_string();
    if s.is_empty() { "vm".into() } else { s }
}

/// The VM tools, when a hypervisor is here (none at all otherwise, so the model is not offered them).
pub fn tools() -> Vec<Arc<dyn crate::Tool>> {
    if found().is_empty() {
        return vec![];
    }
    vec![
        Arc::new(VmList),
        Arc::new(VmPower),
        Arc::new(VmSnapshot),
        Arc::new(VmScreenshot),
        Arc::new(VmRun),
        Arc::new(VmCopy),
        Arc::new(VmTest),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hypervisor_is_picked_by_name_by_its_file_or_the_first_here() {
        let here = [Hypervisor::VirtualBox, Hypervisor::VMware];
        assert_eq!(pick(Some("VMware"), "x", &here), Ok(Hypervisor::VMware));
        assert_eq!(pick(None, "/vms/a/a.vmx", &here), Ok(Hypervisor::VMware));
        assert_eq!(pick(None, "test-vm", &here), Ok(Hypervisor::VirtualBox));
        assert!(pick(Some("libvirt"), "x", &here).is_err_and(|e| e.contains("not installed")));
        assert!(pick(Some("parallels"), "x", &here).is_err());
        assert!(pick(None, "x", &[]).is_err_and(|e| e.contains("No hypervisor")));
        assert_eq!(Hypervisor::of("kvm"), Some(Hypervisor::Libvirt));
        assert_eq!(Hypervisor::of("Hyper-V"), Some(Hypervisor::HyperV));
    }

    #[test]
    fn the_words_of_power_and_os() {
        assert_eq!(Power::of("on"), Some(Power::Start));
        assert_eq!(Power::of("reboot"), Some(Power::Reset));
        assert_eq!(Power::of("fly"), None);
        assert_eq!(Os::of(Some("Windows 11")), Os::Windows);
        assert_eq!(Os::of(None), Os::Linux);
    }

    #[test]
    fn a_refused_sign_in_is_told_apart() {
        assert!(refused("VBoxManage: error: VERR_AUTHENTICATION_FAILURE"));
        assert!(refused("Error: Invalid user name or password for the guest OS"));
        assert!(!refused("VBoxManage: error: Could not find a registered machine named 'x'"));
        assert!(!refused("Access is denied."), "a guest command's own failure is not a refused sign-in");
    }

    #[test]
    fn only_the_programs_own_words_say_the_guest_refused() {
        let ran = |code, out: &str, err: &str| run::Ran { code: Some(code), out: out.into(), err: err.into() };
        let vbox = ran(1, "", "VBoxManage: error: VERR_AUTHENTICATION_FAILURE\n");
        assert!(refused_by(Hypervisor::VirtualBox, &vbox));
        // The guest's command printed the words itself: its failure is its own.
        let guest = ran(1, "", "Invalid user name or password\n");
        assert!(!refused_by(Hypervisor::VirtualBox, &guest));
        // vmrun says its errors on its output.
        let vmrun = ran(255, "Error: Invalid user name or password for the guest OS\n", "");
        assert!(refused_by(Hypervisor::VMware, &vmrun));
        assert!(!refused_by(Hypervisor::VMware, &ran(0, "Error: Invalid user name or password", "")));
        assert!(!refused_by(Hypervisor::Libvirt, &vbox));
    }

    #[test]
    fn a_file_part_is_safe() {
        assert_eq!(file_part("/vms/Win 11/Win 11.vmx"), "Win-11");
        assert_eq!(file_part("test-vm"), "test-vm");
        assert_eq!(file_part("///"), "vm");
    }
}
