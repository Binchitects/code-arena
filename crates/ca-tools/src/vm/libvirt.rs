//! libvirt (KVM, QEMU) through `virsh`: a machine is its domain's name, the guest is reached through
//! qemu-guest-agent, which runs as the guest's own service and so needs no sign-in. Files go through the agent's
//! file calls, a piece at a time.

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::run::{Call, Ran, b64_decode, b64_encode, exec};
use super::{Machine, Os, Power, Snap};

const P: &str = "virsh";

/// The most bytes read or written in one agent call.
const PIECE: usize = 48 * 1024;

pub fn list() -> Call {
    Call::new(P, &["list", "--all"])
}

/// The machines in what `virsh list --all` says: a head, a rule, then "Id Name State" a line (the id `-` while off).
pub fn machines(said: &str) -> Vec<Machine> {
    said.lines()
        .skip_while(|l| !l.trim_start().starts_with("---"))
        .skip(1)
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let _id = w.next()?;
            let name = w.next()?.to_string();
            Some(Machine { name, state: w.collect::<Vec<_>>().join(" ") })
        })
        .collect()
}

pub fn power(vm: &str, p: Power) -> Call {
    let what = match p {
        Power::Start => "start",
        Power::Shutdown => "shutdown",
        Power::Off => "destroy",
        Power::Reset => "reboot",
        Power::Pause => "suspend",
        Power::Resume => "resume",
    };
    Call::new(P, &[what, vm])
}

pub fn snapshot(vm: &str, s: &Snap) -> Call {
    match s {
        Snap::List => Call::new(P, &["snapshot-list", vm, "--name"]),
        Snap::Take(n) => Call::new(P, &["snapshot-create-as", vm, n]),
        Snap::Restore(n) => Call::new(P, &["snapshot-revert", vm, n]),
        Snap::Delete(n) => Call::new(P, &["snapshot-delete", vm, n]),
    }
}

pub fn snapshot_names(said: &str) -> Vec<String> {
    said.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()
}

/// The screen, as the hypervisor gives it (QEMU's is a PPM picture).
pub fn screenshot(vm: &str, file: &Path) -> Call {
    Call::new(P, &["screenshot", vm]).arg(file.to_string_lossy())
}

/// A call to the guest agent.
pub fn agent(vm: &str, execute: &str, arguments: Value) -> Call {
    Call::new(P, &["qemu-agent-command", vm]).arg(json!({"execute": execute, "arguments": arguments}).to_string())
}

/// What an agent call gave back (its `return`), or why not.
pub fn returned(r: &Ran) -> Result<Value, String> {
    if !r.ok() {
        return Err(r.why());
    }
    let v: Value = serde_json::from_str(r.out.trim()).map_err(|e| format!("The guest agent said what is not JSON: {e}"))?;
    Ok(v["return"].clone())
}

/// What a finished `guest-exec-status` says: the exit code and the output, none while it runs.
pub fn finished(status: &Value) -> Option<Ran> {
    if status["exited"] != true {
        return None;
    }
    let text = |k: &str| {
        status[k].as_str().and_then(b64_decode).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
    };
    Some(Ran {
        code: status["exitcode"].as_i64().map(|c| c as i32).or(Some(-1)),
        out: text("out-data"),
        err: text("err-data"),
    })
}

/// A command run in the guest through the agent, waited for until `limit`.
pub async fn run(vm: &str, os: Os, command: &str, limit: Duration) -> Ran {
    let (shell, flag) = os.shell();
    let started = exec(
        &agent(vm, "guest-exec", json!({"path": shell, "arg": [flag, command], "capture-output": true})),
        limit,
    )
    .await;
    let pid = match returned(&started) {
        Ok(v) => v["pid"].as_i64().unwrap_or(-1),
        Err(e) => return Ran { code: None, out: String::new(), err: e },
    };
    let began = Instant::now();
    let mut wait = Duration::from_millis(100);
    loop {
        let r = exec(&agent(vm, "guest-exec-status", json!({"pid": pid})), Duration::from_secs(30)).await;
        match returned(&r) {
            Ok(v) => {
                if let Some(done) = finished(&v) {
                    return done;
                }
            }
            Err(e) => return Ran { code: None, out: String::new(), err: e },
        }
        if began.elapsed() > limit {
            return Ran { code: None, out: String::new(), err: format!("It ran longer than {} s.", limit.as_secs()) };
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(Duration::from_secs(2));
    }
}

/// A file copied into the guest (`to_guest`: `from` on the host to the guest's path `to`) or out of it.
pub async fn copy(vm: &str, to_guest: bool, from: &str, to: &str) -> Result<u64, String> {
    let t = Duration::from_secs(60);
    let open = |path: &str, mode: &str| agent(vm, "guest-file-open", json!({"path": path, "mode": mode}));
    if to_guest {
        let bytes = std::fs::read(from).map_err(|e| format!("{from} could not be read: {e}"))?;
        let h = returned(&exec(&open(to, "w"), t).await)?["handle"].clone();
        let mut result = Ok(bytes.len() as u64);
        for piece in bytes.chunks(PIECE) {
            let w = agent(vm, "guest-file-write", json!({"handle": h, "buf-b64": b64_encode(piece)}));
            if let Err(e) = returned(&exec(&w, t).await) {
                result = Err(e);
                break;
            }
        }
        let _ = exec(&agent(vm, "guest-file-close", json!({"handle": h})), t).await;
        result
    } else {
        let h = returned(&exec(&open(from, "r"), t).await)?["handle"].clone();
        let mut bytes = Vec::new();
        let result = loop {
            let r = agent(vm, "guest-file-read", json!({"handle": h, "count": PIECE}));
            match returned(&exec(&r, t).await) {
                Ok(v) => {
                    bytes.extend(v["buf-b64"].as_str().and_then(b64_decode).unwrap_or_default());
                    if v["eof"] == true || v["count"].as_u64() == Some(0) {
                        break Ok(());
                    }
                }
                Err(e) => break Err(e),
            }
        };
        let _ = exec(&agent(vm, "guest-file-close", json!({"handle": h})), t).await;
        result?;
        std::fs::write(to, &bytes).map_err(|e| format!("{to} could not be written: {e}"))?;
        Ok(bytes.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virsh_list_is_read() {
        let said = " Id   Name      State\n------------------------------\n 1    web       running\n -    test-vm   shut off\n\n";
        let m = machines(said);
        assert_eq!(m.len(), 2);
        assert_eq!((m[1].name.as_str(), m[1].state.as_str()), ("test-vm", "shut off"));
        assert_eq!(power("a", Power::Off).args, ["destroy", "a"]);
    }

    #[test]
    fn the_agent_is_spoken_to_in_json_and_its_output_read() {
        let c = agent("a", "guest-exec", json!({"path": "/bin/sh"}));
        let v: Value = serde_json::from_str(&c.args[2]).unwrap_or_default();
        assert_eq!(v["execute"], "guest-exec");
        let r = Ran { code: Some(0), out: "{\"return\":{\"pid\":42}}".into(), err: String::new() };
        assert_eq!(returned(&r).map(|v| v["pid"].clone()), Ok(json!(42)));
        let status = json!({"exited": true, "exitcode": 1, "out-data": b64_encode(b"hi\n")});
        let done = finished(&status);
        assert_eq!(done.as_ref().map(|d| (d.code, d.out.as_str())), Some((Some(1), "hi\n")));
        assert!(finished(&json!({"exited": false})).is_none());
    }
}
