//! The VM tools the model calls: `vm_list`, `vm_power`, `vm_snapshot`, `vm_screenshot`, `vm_run`, `vm_copy` and
//! `vm_test` (a snapshot restored, the build copied in and run, its logs and screen brought back, the machine put
//! back). Each finds its hypervisor ([`super::pick`]), and the ones that reach into the guest sign in to it once per
//! call, asking again when the guest refuses.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ca_llm::ToolDef;
use serde_json::{Value, json};

use super::login::{Guest, SignIn};
use super::run::{Call, Ran, SecretFile, exec};
use super::{Hypervisor, Machine, Os, Power, Snap, file_part, hyperv, kept_dir, libvirt, refused_by, vbox, vmware};
use crate::{Context, Kind, Output, Tool, arg_str};

/// What reaches into the guest.
enum Op<'a> {
    Run {
        os: Os,
        command: &'a str,
    },
    /// `to` is the folder the file goes into, on the other side.
    Copy {
        to_guest: bool,
        from: &'a str,
        to: &'a str,
    },
    Screen {
        file: &'a Path,
    },
}

/// One machine as a call works on it: its hypervisor, its name, and the guest's sign-in once the guest took it.
struct Vm {
    hv: Hypervisor,
    name: String,
    limit: Duration,
    sign: SignIn,
    guest: Option<Guest>,
    /// What the model is told besides (a sign-in that could not be kept).
    notes: Vec<String>,
}

impl Vm {
    fn of(args: &Value, ctx: &Context) -> Result<Vm, Output> {
        let name = arg_str(args, "vm")?.trim().to_string();
        let hv = super::pick(args["hypervisor"].as_str(), &name, &super::found()).map_err(Output::error)?;
        Ok(Vm {
            hv,
            sign: SignIn::new(hv, &name, ctx.vm_login.clone().map(|a| a.0)),
            name,
            limit: ctx.shell_timeout,
            guest: None,
            notes: vec![],
        })
    }

    async fn call(&self, c: Call) -> Ran {
        exec(&c, self.limit).await
    }

    fn needs_guest(&self, op: &Op<'_>) -> bool {
        self.hv.signs_in() && !matches!(op, Op::Screen { .. } if self.hv != Hypervisor::VMware)
    }

    /// Does it, signed in when it needs to be: the guest's refusal asks the person again.
    async fn signed(&mut self, op: &Op<'_>) -> Result<Ran, Output> {
        if !self.needs_guest(op) {
            return Ok(self.once(op, None).await);
        }
        loop {
            let g = match &self.guest {
                Some(g) => g.clone(),
                None => self.sign.next().await?,
            };
            let r = self.once(op, Some(&g)).await.scrubbed(&g.password);
            if refused_by(self.hv, &r) {
                self.guest = None;
                self.sign.was_refused();
                continue;
            }
            // Kept in the keyring (when the person asked) only once the guest surely took it: a call that worked.
            if r.ok()
                && let Some(n) = self.sign.took(&g).await
            {
                self.notes.push(n);
            }
            self.guest = Some(g);
            return Ok(r);
        }
    }

    async fn once(&self, op: &Op<'_>, g: Option<&Guest>) -> Ran {
        let (vm, hv) = (self.name.as_str(), self.hv);
        let user = g.map(|g| g.user.as_str()).unwrap_or("");
        let password = g.map(|g| g.password.as_str()).unwrap_or("");
        let secret = || SecretFile::new(password).map_err(|e| failed(format!("The password file: {e}")));
        match (op, hv) {
            (Op::Run { os, command }, Hypervisor::VirtualBox) => match secret() {
                Ok(f) => self.call(vbox::run(vm, user, f.path(), *os, command, self.limit.as_millis())).await,
                Err(r) => r,
            },
            (Op::Run { os, command }, Hypervisor::VMware) => self.vmware_run(user, password, *os, command).await,
            (Op::Run { os, command }, Hypervisor::Libvirt) => libvirt::run(vm, *os, command, self.limit).await,
            (Op::Run { command, .. }, Hypervisor::HyperV) => {
                self.call(hyperv::run(
                    vm,
                    g.unwrap_or(&Guest { user: String::new(), password: String::new() }),
                    command,
                ))
                .await
            }
            (Op::Copy { to_guest, from, to }, Hypervisor::VirtualBox) => match secret() {
                Ok(f) => self.call(vbox::copy(vm, user, f.path(), *to_guest, from, to)).await,
                Err(r) => r,
            },
            (Op::Copy { to_guest, from, to }, _) => {
                let to = into_folder(from, to, *to_guest);
                match hv {
                    Hypervisor::VMware => self.call(vmware::copy(vm, user, password, *to_guest, from, &to)).await,
                    Hypervisor::Libvirt => match libvirt::copy(vm, *to_guest, from, &to).await {
                        Ok(n) => Ran { code: Some(0), out: format!("{n} bytes"), err: String::new() },
                        Err(e) => failed(e),
                    },
                    _ => {
                        let g = g.cloned().unwrap_or(Guest { user: String::new(), password: String::new() });
                        self.call(hyperv::copy(vm, &g, *to_guest, from, &to)).await
                    }
                }
            }
            (Op::Screen { file }, Hypervisor::VirtualBox) => self.call(vbox::screenshot(vm, file)).await,
            (Op::Screen { file }, Hypervisor::VMware) => self.call(vmware::screenshot(vm, user, password, file)).await,
            (Op::Screen { file }, Hypervisor::Libvirt) => self.call(libvirt::screenshot(vm, file)).await,
            (Op::Screen { file }, Hypervisor::HyperV) => match hyperv::screenshot(vm, file) {
                Some(c) => self.call(c).await,
                None => failed("Hyper-V has no command that saves a machine's screen.".into()),
            },
        }
    }

    /// `vmrun` gives back only the exit code: the output was written to a file in the guest, copied back here.
    async fn vmware_run(&self, user: &str, password: &str, os: Os, command: &str) -> Ran {
        let vm = self.name.as_str();
        let r = self.call(vmware::run(vm, user, password, os, command)).await;
        let code = if r.ok() { Some(0) } else { vmware::exit_code(&r.said()) };
        if code.is_none() {
            return r;
        }
        let here = std::env::temp_dir().join(format!("code-arena-vm-out-{}-{}.txt", std::process::id(), stamp()));
        let back =
            self.call(vmware::copy(vm, user, password, false, vmware::out_file(os), &here.to_string_lossy())).await;
        let out = std::fs::read(&here).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let _ = std::fs::remove_file(&here);
        let _ = self.call(vmware::delete_in_guest(vm, user, password, vmware::out_file(os))).await;
        Ran { code, out, err: if back.ok() { String::new() } else { back.why() } }
    }

    fn told(&self, text: String) -> Output {
        let mut text = text;
        for n in &self.notes {
            text.push_str(&format!("\n{n}"));
        }
        Output::ok(text)
    }
}

fn failed(err: String) -> Ran {
    Ran { code: None, out: String::new(), err }
}

/// A file's place in the folder `to` on the other side, by its name (the guest's paths read by their slashes).
fn into_folder(from: &str, to: &str, to_guest: bool) -> String {
    let name = from.rsplit(['/', '\\']).next().unwrap_or(from);
    if !to_guest {
        return Path::new(to).join(name).to_string_lossy().into_owned();
    }
    let sep = if to.contains('\\') { '\\' } else { '/' };
    format!("{}{sep}{name}", to.trim_end_matches(['/', '\\']))
}

/// Seconds since 1970, for the names of what is brought back.
fn stamp() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The machines of a hypervisor.
async fn machines(hv: Hypervisor, limit: Duration) -> Result<Vec<Machine>, String> {
    let read = |r: Ran| if r.ok() { Ok(r.out) } else { Err(r.why()) };
    Ok(match hv {
        Hypervisor::VirtualBox => {
            let all = read(exec(&vbox::list_all(), limit).await)?;
            let on = read(exec(&vbox::list_running(), limit).await)?;
            vbox::machines(&all, &on)
        }
        Hypervisor::VMware => vmware::machines(&read(exec(&vmware::list(), limit).await)?),
        Hypervisor::Libvirt => libvirt::machines(&read(exec(&libvirt::list(), limit).await)?),
        Hypervisor::HyperV => hyperv::machines(&read(exec(&hyperv::list(), limit).await)?),
    })
}

fn power_call(hv: Hypervisor, vm: &str, p: Power) -> Call {
    match hv {
        Hypervisor::VirtualBox => vbox::power(vm, p),
        Hypervisor::VMware => vmware::power(vm, p),
        Hypervisor::Libvirt => libvirt::power(vm, p),
        Hypervisor::HyperV => hyperv::power(vm, p),
    }
}

fn snapshot_call(hv: Hypervisor, vm: &str, s: &Snap) -> Call {
    match hv {
        Hypervisor::VirtualBox => vbox::snapshot(vm, s),
        Hypervisor::VMware => vmware::snapshot(vm, s),
        Hypervisor::Libvirt => libvirt::snapshot(vm, s),
        Hypervisor::HyperV => hyperv::snapshot(vm, s),
    }
}

fn snapshot_names(hv: Hypervisor, said: &str) -> Vec<String> {
    match hv {
        Hypervisor::VirtualBox => vbox::snapshot_names(said),
        Hypervisor::VMware => vmware::snapshot_names(said),
        _ => libvirt::snapshot_names(said),
    }
}

/// What a run in the guest tells the model: its output and how it ended.
fn run_said(r: &Ran) -> String {
    let said = r.said();
    let end = match r.code {
        Some(0) => "Exit code 0.".to_string(),
        Some(c) => format!("Exit code {c}."),
        None => "It did not finish.".into(),
    };
    if said.is_empty() { end } else { format!("{said}\n{end}") }
}

/// The `vm` and `hypervisor` arguments every tool has.
fn schema(more: Value, required: &[&str]) -> Value {
    let mut props = json!({
        "vm": {"type": "string", "description": "The machine: its name (VirtualBox, libvirt, Hyper-V) or its .vmx file (VMware)."},
        "hypervisor": {"type": "string", "enum": ["virtualbox", "vmware", "libvirt", "hyperv"], "description": "Only when more than one is installed: the first found is used otherwise."}
    });
    if let (Some(p), Some(m)) = (props.as_object_mut(), more.as_object()) {
        p.extend(m.clone());
    }
    let mut req = vec!["vm"];
    req.extend_from_slice(required);
    json!({"type": "object", "properties": props, "required": req})
}

fn vm_of(args: &Value) -> &str {
    args["vm"].as_str().unwrap_or_default()
}

fn s<'a>(args: &'a Value, k: &str) -> &'a str {
    args[k].as_str().unwrap_or_default()
}

pub struct VmList;

#[async_trait::async_trait]
impl Tool for VmList {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "vm_list".into(),
            description: "Lists the virtual machines of the hypervisors installed here, and whether each runs (VMware lists only the running ones).".into(),
            parameters: json!({"type": "object", "properties": {"hypervisor": {"type": "string", "enum": ["virtualbox", "vmware", "libvirt", "hyperv"]}}}),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, args: &Value) -> String {
        s(args, "hypervisor").to_string()
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let here = super::found();
        let which: Vec<Hypervisor> = match args["hypervisor"].as_str().filter(|h| !h.is_empty()) {
            Some(n) => match super::pick(Some(n), "", &here) {
                Ok(h) => vec![h],
                Err(e) => return Output::error(e),
            },
            None => here,
        };
        if which.is_empty() {
            return Output::error(super::pick(None, "", &[]).err().unwrap_or_default());
        }
        let mut text = String::new();
        for hv in which {
            text.push_str(&format!("{}:\n", hv.name()));
            match machines(hv, ctx.shell_timeout).await {
                Ok(m) if m.is_empty() => text.push_str("  (none)\n"),
                Ok(m) => m.iter().for_each(|m| text.push_str(&format!("  {}  {}\n", m.name, m.state))),
                Err(e) => text.push_str(&format!("  {e}\n")),
            }
        }
        Output::ok(text.trim_end().to_string())
    }
}

pub struct VmPower;

#[async_trait::async_trait]
impl Tool for VmPower {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "vm_power".into(),
            description: "Starts a virtual machine (without a window), shuts it down, turns it off at once, resets, pauses or resumes it.".into(),
            parameters: schema(
                json!({"action": {"type": "string", "enum": ["start", "shutdown", "off", "reset", "pause", "resume"]}}),
                &["action"],
            ),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Shell
    }
    fn subject(&self, args: &Value) -> String {
        format!("{}: {}", vm_of(args), s(args, "action"))
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let Some(p) = Power::of(s(&args, "action")) else {
            return Output::error("action is one of start, shutdown, off, reset, pause, resume");
        };
        let vm = match Vm::of(&args, ctx) {
            Ok(v) => v,
            Err(o) => return o,
        };
        let r = vm.call(power_call(vm.hv, &vm.name, p)).await;
        if r.ok() { Output::ok(format!("{}: {} done.", vm.name, s(&args, "action"))) } else { Output::error(r.why()) }
    }
}

pub struct VmSnapshot;

#[async_trait::async_trait]
impl Tool for VmSnapshot {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "vm_snapshot".into(),
            description: "Lists a virtual machine's snapshots, takes one, restores one (VirtualBox: the machine must be off) or deletes one.".into(),
            parameters: schema(
                json!({
                    "action": {"type": "string", "enum": ["list", "take", "restore", "delete"]},
                    "name": {"type": "string", "description": "The snapshot (not for list)."}
                }),
                &["action"],
            ),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Shell
    }
    fn subject(&self, args: &Value) -> String {
        format!("{}: {} {}", vm_of(args), s(args, "action"), s(args, "name")).trim_end().to_string()
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let name = s(&args, "name").trim().to_string();
        let snap = match (s(&args, "action"), name.is_empty()) {
            ("list", _) => Snap::List,
            (_, true) => return Output::error("name is missing: the snapshot to take, restore or delete"),
            ("take", _) => Snap::Take(name),
            ("restore", _) => Snap::Restore(name),
            ("delete", _) => Snap::Delete(name),
            _ => return Output::error("action is one of list, take, restore, delete"),
        };
        let vm = match Vm::of(&args, ctx) {
            Ok(v) => v,
            Err(o) => return o,
        };
        let r = vm.call(snapshot_call(vm.hv, &vm.name, &snap)).await;
        if !r.ok() {
            return Output::error(r.why());
        }
        match snap {
            Snap::List => {
                let names = snapshot_names(vm.hv, &r.out);
                if names.is_empty() {
                    Output::ok(format!("{} has no snapshots.", vm.name))
                } else {
                    Output::ok(names.join("\n"))
                }
            }
            Snap::Take(n) => Output::ok(format!("Snapshot {n} of {} taken.", vm.name)),
            Snap::Restore(n) => Output::ok(format!("{} is back at snapshot {n}.", vm.name)),
            Snap::Delete(n) => Output::ok(format!("Snapshot {n} of {} deleted.", vm.name)),
        }
    }
}

/// A screen saved into `.code-arena/vm`.
async fn save_screen(vm: &mut Vm, ctx: &Context) -> Result<PathBuf, Output> {
    let dir = kept_dir(&ctx.workspace);
    std::fs::create_dir_all(&dir).map_err(|e| Output::error(format!("{} could not be made: {e}", dir.display())))?;
    let ext = if vm.hv == Hypervisor::Libvirt { "ppm" } else { "png" };
    let file = dir.join(format!("{}-{}.{ext}", file_part(&vm.name), stamp()));
    let r = vm.signed(&Op::Screen { file: &file }).await?;
    if r.ok() { Ok(file) } else { Err(Output::error(r.why())) }
}

fn shown(path: &Path, ctx: &Context) -> String {
    path.strip_prefix(ctx.workspace.cwd()).unwrap_or(path).to_string_lossy().into_owned()
}

pub struct VmScreenshot;

#[async_trait::async_trait]
impl Tool for VmScreenshot {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "vm_screenshot".into(),
            description: "Saves a running virtual machine's screen as a picture in .code-arena/vm, and says where."
                .into(),
            parameters: schema(json!({}), &[]),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Read
    }
    fn subject(&self, args: &Value) -> String {
        vm_of(args).to_string()
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let mut vm = match Vm::of(&args, ctx) {
            Ok(v) => v,
            Err(o) => return o,
        };
        match save_screen(&mut vm, ctx).await {
            Ok(f) => vm.told(format!("The screen of {} is in {}.", vm.name, shown(&f, ctx))),
            Err(o) => o,
        }
    }
}

pub struct VmRun;

#[async_trait::async_trait]
impl Tool for VmRun {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "vm_run".into(),
            description: "Runs a command in a running virtual machine's guest, through its shell (/bin/sh, or cmd.exe on Windows; PowerShell on Hyper-V), and gives its output and exit code. The guest's user and password are asked of the person when needed: never ask for them or pass them.".into(),
            parameters: schema(
                json!({
                    "command": {"type": "string", "description": "The command line, as the guest's shell reads it."},
                    "os": {"type": "string", "enum": ["linux", "windows"], "description": "The guest's system (linux when not said)."}
                }),
                &["command"],
            ),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Shell
    }
    fn subject(&self, args: &Value) -> String {
        format!("{}: {}", vm_of(args), s(args, "command"))
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let command = match arg_str(&args, "command") {
            Ok(c) => c.to_string(),
            Err(o) => return o,
        };
        let mut vm = match Vm::of(&args, ctx) {
            Ok(v) => v,
            Err(o) => return o,
        };
        let os = Os::of(args["os"].as_str());
        match vm.signed(&Op::Run { os, command: &command }).await {
            Ok(r) if r.code.is_none() => Output::error(r.why()),
            Ok(r) => {
                let o = vm.told(run_said(&r));
                if r.ok() { o } else { Output { is_error: true, ..o } }
            }
            Err(o) => o,
        }
    }
}

/// A file copied, as the model is told; the host's side inside the workspace.
async fn copy_one(vm: &mut Vm, ctx: &Context, to_guest: bool, from: &str, to: &str) -> Result<String, Output> {
    let (from, to) = if to_guest {
        let host = ctx.workspace.resolve(from).map_err(|e| Output::error(e.to_string()))?;
        (host.to_string_lossy().into_owned(), to.to_string())
    } else {
        let host = if to.trim().is_empty() {
            kept_dir(&ctx.workspace).join(file_part(&vm.name))
        } else {
            ctx.workspace.resolve(to).map_err(|e| Output::error(e.to_string()))?
        };
        std::fs::create_dir_all(&host)
            .map_err(|e| Output::error(format!("{} could not be made: {e}", host.display())))?;
        (from.to_string(), host.to_string_lossy().into_owned())
    };
    let r = vm.signed(&Op::Copy { to_guest, from: &from, to: &to }).await?;
    if !r.ok() {
        return Err(Output::error(r.why()));
    }
    let to = if to_guest { to } else { shown(Path::new(&to), ctx) };
    Ok(format!("{from} copied {} {to}.", if to_guest { "into the guest's" } else { "to" }))
}

pub struct VmCopy;

#[async_trait::async_trait]
impl Tool for VmCopy {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "vm_copy".into(),
            description: "Copies a file into a running virtual machine's guest, or out of it into the workspace (into .code-arena/vm/VM when no folder is said). The file keeps its name in the folder it goes to.".into(),
            parameters: schema(
                json!({
                    "direction": {"type": "string", "enum": ["in", "out"], "description": "in: from the workspace into the guest; out: from the guest."},
                    "from": {"type": "string", "description": "The file: in the workspace (in), or in the guest (out)."},
                    "to": {"type": "string", "description": "The folder it goes into: in the guest (in), or in the workspace (out)."}
                }),
                &["direction", "from"],
            ),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Shell
    }
    fn subject(&self, args: &Value) -> String {
        let arrow = if s(args, "direction") == "out" { "<-" } else { "->" };
        format!("{}: {} {arrow} {}", vm_of(args), s(args, "from"), s(args, "to")).trim_end().to_string()
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let to_guest = match s(&args, "direction") {
            "in" => true,
            "out" => false,
            _ => return Output::error("direction is in or out"),
        };
        if to_guest && s(&args, "to").trim().is_empty() {
            return Output::error("to is missing: the guest's folder the file goes into");
        }
        let from = match arg_str(&args, "from") {
            Ok(f) => f.to_string(),
            Err(o) => return o,
        };
        let mut vm = match Vm::of(&args, ctx) {
            Ok(v) => v,
            Err(o) => return o,
        };
        match copy_one(&mut vm, ctx, to_guest, &from, s(&args, "to")).await {
            Ok(t) => vm.told(t),
            Err(o) => o,
        }
    }
}

/// How long a guest may take to answer after it starts.
const READY_TIME: Duration = Duration::from_secs(180);

pub struct VmTest;

#[async_trait::async_trait]
impl Tool for VmTest {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "vm_test".into(),
            description: "Tests in a virtual machine in one go: restores the snapshot (when given), starts the machine, waits for its guest, copies the files in, runs the command, brings back the files and the screen into .code-arena/vm, then puts the machine back at the snapshot.".into(),
            parameters: schema(
                json!({
                    "snapshot": {"type": "string", "description": "The clean snapshot to start from and go back to."},
                    "copy_in": {"type": "array", "description": "Files of the workspace to copy in first.", "items": {"type": "object", "properties": {"from": {"type": "string"}, "to": {"type": "string", "description": "The guest's folder."}}, "required": ["from", "to"]}},
                    "command": {"type": "string", "description": "What to run in the guest."},
                    "os": {"type": "string", "enum": ["linux", "windows"]},
                    "copy_out": {"type": "array", "description": "The guest's files (logs) to bring back.", "items": {"type": "string"}},
                    "screenshot": {"type": "boolean", "description": "Bring back the screen after the command (true when not said)."},
                    "keep": {"type": "boolean", "description": "Leave the machine as the test left it instead of going back to the snapshot."}
                }),
                &["command"],
            ),
        }
    }
    fn kind(&self) -> Kind {
        Kind::Shell
    }
    fn subject(&self, args: &Value) -> String {
        let snap = s(args, "snapshot");
        let from = if snap.is_empty() { String::new() } else { format!(" (from {snap})") };
        format!("{}{from}: {}", vm_of(args), s(args, "command"))
    }
    async fn run(&self, args: Value, ctx: &Context) -> Output {
        let command = match arg_str(&args, "command") {
            Ok(c) => c.to_string(),
            Err(o) => return o,
        };
        let mut vm = match Vm::of(&args, ctx) {
            Ok(v) => v,
            Err(o) => return o,
        };
        let os = Os::of(args["os"].as_str());
        let snap = s(&args, "snapshot").trim().to_string();
        let mut steps: Vec<String> = vec![];
        let result = test(&mut vm, ctx, &args, os, &snap, &command, &mut steps).await;
        // The machine goes back to the snapshot, whatever the test did.
        if !snap.is_empty() && args["keep"] != true {
            let _ = vm.call(power_call(vm.hv, &vm.name, Power::Off)).await;
            let r = vm.call(snapshot_call(vm.hv, &vm.name, &Snap::Restore(snap.clone()))).await;
            steps.push(if r.ok() {
                format!("Back at snapshot {snap}.")
            } else {
                format!("Not back at {snap}: {}", r.why())
            });
        }
        let ok = result.is_ok();
        if let Err(e) = result {
            steps.push(e);
        }
        let o = vm.told(steps.join("\n"));
        if ok { o } else { Output { is_error: true, ..o } }
    }
}

/// The test's steps, each said in `steps`; the first that fails ends it, said in the error.
async fn test(
    vm: &mut Vm,
    ctx: &Context,
    args: &Value,
    os: Os,
    snap: &str,
    command: &str,
    steps: &mut Vec<String>,
) -> Result<(), String> {
    let why = |o: Output| o.text;
    if !snap.is_empty() {
        let _ = vm.call(power_call(vm.hv, &vm.name, Power::Off)).await;
        let r = vm.call(snapshot_call(vm.hv, &vm.name, &Snap::Restore(snap.to_string()))).await;
        if !r.ok() {
            return Err(format!("The snapshot {snap} was not restored: {}", r.why()));
        }
        steps.push(format!("Restored snapshot {snap}."));
    }
    let r = vm.call(power_call(vm.hv, &vm.name, Power::Start)).await;
    steps.push(if r.ok() { "Started.".into() } else { "Started already (or could not start).".into() });
    // The guest answers once its tools run: a command that does nothing, tried until it does.
    let began = std::time::Instant::now();
    let nothing = if os == Os::Windows { "exit 0" } else { "true" };
    loop {
        let r = vm.signed(&Op::Run { os, command: nothing }).await.map_err(why)?;
        if r.ok() {
            break;
        }
        if began.elapsed() > READY_TIME {
            return Err(format!("The guest did not answer in {} s: {}", READY_TIME.as_secs(), r.why()));
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    steps.push("The guest answers.".into());
    for c in args["copy_in"].as_array().into_iter().flatten() {
        let t = copy_one(vm, ctx, true, s(c, "from"), s(c, "to")).await.map_err(why)?;
        steps.push(t);
    }
    let r = vm.signed(&Op::Run { os, command }).await.map_err(why)?;
    steps.push(format!("Ran {command}:\n{}", run_said(&r)));
    let dir = kept_dir(&ctx.workspace).join(format!("{}-{}", file_part(&vm.name), stamp()));
    let dir_shown = shown(&dir, ctx);
    for f in args["copy_out"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        match copy_one(vm, ctx, false, f, &dir_shown).await {
            Ok(t) => steps.push(t),
            Err(o) => steps.push(format!("{f} was not brought back: {}", o.text)),
        }
    }
    if args["screenshot"] != false {
        match save_screen(vm, ctx).await {
            Ok(f) => steps.push(format!("The screen is in {}.", shown(&f, ctx))),
            Err(o) => steps.push(format!("No screen: {}", o.text)),
        }
    }
    if r.ok() { Ok(()) } else { Err("The command failed.".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_goes_into_the_folder_on_the_other_side() {
        assert_eq!(into_folder("/w/build/app", "/opt/test/", true), "/opt/test/app");
        assert_eq!(into_folder("/w/app.exe", "C:\\test", true), "C:\\test\\app.exe");
        assert_eq!(into_folder("C:\\logs\\run.log", "/w/.code-arena/vm", false), "/w/.code-arena/vm/run.log");
    }

    #[test]
    fn what_a_run_says() {
        assert_eq!(run_said(&Ran { code: Some(0), out: "ok\n".into(), err: String::new() }), "ok\nExit code 0.");
        assert_eq!(run_said(&Ran { code: Some(2), ..Ran::default() }), "Exit code 2.");
    }

    #[test]
    fn every_tool_has_the_machine_and_a_subject_without_secrets() {
        for t in [&VmPower as &dyn Tool, &VmSnapshot, &VmScreenshot, &VmRun, &VmCopy, &VmTest] {
            let d = t.def();
            assert_eq!(d.parameters["required"][0], "vm", "{}", d.name);
            assert!(d.parameters["properties"].get("password").is_none(), "{}", d.name);
        }
        let a = json!({"vm": "test-vm", "command": "make test", "snapshot": "clean"});
        assert_eq!(VmTest.subject(&a), "test-vm (from clean): make test");
        assert_eq!(VmCopy.subject(&json!({"vm": "a", "direction": "out", "from": "/x"})), "a: /x <-");
    }

    #[tokio::test]
    async fn a_call_with_no_hypervisor_here_says_so() {
        if !super::super::found().is_empty() {
            return;
        }
        let (_d, ctx) = crate::testing::context();
        let o = VmRun.run(json!({"vm": "x", "command": "true"}), &ctx).await;
        assert!(o.is_error && o.text.contains("No hypervisor"));
    }
}
