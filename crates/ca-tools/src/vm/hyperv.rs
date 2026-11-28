//! Hyper-V through PowerShell's Hyper-V module: a machine is its name, the guest is reached through PowerShell
//! Direct (`-VMName` with a credential). Every script goes in on standard input, so the password is never an
//! argument. Written to Hyper-V's documented commands, and tried only on Windows.

use std::path::Path;

use super::login::{Guest, ps_quote};
use super::run::Call;
use super::{Machine, Power, Snap};

/// PowerShell reading its script from standard input.
fn script(text: String) -> Call {
    Call::new("powershell", &["-NoProfile", "-NonInteractive", "-Command", "-"]).input(text)
}

/// The script's credential, as `$c`.
fn credential(g: &Guest) -> String {
    format!(
        "$c = New-Object System.Management.Automation.PSCredential({}, (ConvertTo-SecureString {} -AsPlainText \
         -Force)); ",
        ps_quote(&g.user),
        ps_quote(&g.password)
    )
}

pub fn list() -> Call {
    script("Get-VM | ForEach-Object { \"$($_.Name)`t$($_.State)\" }\n".into())
}

/// The machines, a "name<TAB>state" line each.
pub fn machines(said: &str) -> Vec<Machine> {
    said.lines()
        .filter_map(|l| {
            let (name, state) = l.trim_end().split_once('\t')?;
            Some(Machine { name: name.to_string(), state: state.to_ascii_lowercase() })
        })
        .collect()
}

pub fn power(vm: &str, p: Power) -> Call {
    let n = ps_quote(vm);
    script(match p {
        Power::Start => format!("Start-VM -Name {n}\n"),
        Power::Shutdown => format!("Stop-VM -Name {n} -Force\n"),
        Power::Off => format!("Stop-VM -Name {n} -TurnOff -Force\n"),
        Power::Reset => format!("Restart-VM -Name {n} -Force\n"),
        Power::Pause => format!("Suspend-VM -Name {n}\n"),
        Power::Resume => format!("Resume-VM -Name {n}\n"),
    })
}

pub fn snapshot(vm: &str, s: &Snap) -> Call {
    let n = ps_quote(vm);
    script(match s {
        Snap::List => format!("Get-VMSnapshot -VMName {n} | ForEach-Object {{ $_.Name }}\n"),
        Snap::Take(s) => format!("Checkpoint-VM -Name {n} -SnapshotName {}\n", ps_quote(s)),
        Snap::Restore(s) => format!("Restore-VMSnapshot -VMName {n} -Name {} -Confirm:$false\n", ps_quote(s)),
        Snap::Delete(s) => format!("Remove-VMSnapshot -VMName {n} -Name {}\n", ps_quote(s)),
    })
}

/// A command run in the guest's PowerShell; its exit code is the script's.
pub fn run(vm: &str, g: &Guest, command: &str) -> Call {
    script(format!(
        "{}$r = Invoke-Command -VMName {} -Credential $c -ScriptBlock {{ param($x) Invoke-Expression $x; \
         $LASTEXITCODE }} -ArgumentList {}; $r | Select-Object -SkipLast 1; exit ($r | Select-Object -Last 1)\n",
        credential(g),
        ps_quote(vm),
        ps_quote(command)
    ))
}

/// A file copied into the guest (`Copy-VMFile`) or out of it (through a session to the guest).
pub fn copy(vm: &str, g: &Guest, to_guest: bool, from: &str, to: &str) -> Call {
    if to_guest {
        script(format!(
            "Copy-VMFile -Name {} -SourcePath {} -DestinationPath {} -FileSource Host -CreateFullPath -Force\n",
            ps_quote(vm),
            ps_quote(from),
            ps_quote(to)
        ))
    } else {
        script(format!(
            "{}$s = New-PSSession -VMName {} -Credential $c; Copy-Item -FromSession $s -Path {} -Destination {} \
             -Recurse -Force; Remove-PSSession $s\n",
            credential(g),
            ps_quote(vm),
            ps_quote(from),
            ps_quote(to)
        ))
    }
}

/// Hyper-V has no command that saves a machine's screen.
pub fn screenshot(_vm: &str, _file: &Path) -> Option<Call> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_password_goes_in_the_script_never_the_arguments() {
        let g = Guest { user: "tester".into(), password: "fake'pass".into() };
        let c = run("Win 11", &g, "dir");
        assert!(c.args.iter().all(|a| !a.contains("fake")));
        assert!(c.stdin.as_deref().is_some_and(|s| s.contains("'fake''pass'") && s.contains("-VMName 'Win 11'")));
        assert_eq!(machines("a\tRunning\nb\tOff\n")[1], Machine { name: "b".into(), state: "off".into() });
        assert!(power("a", Power::Off).stdin.as_deref().is_some_and(|s| s.contains("-TurnOff")));
    }
}
