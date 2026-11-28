//! Signing in to a guest: its user and password from the keyring (Secret Service's `secret-tool`, macOS's
//! `security`, Windows' Credential Manager through PowerShell), else asked of the person through [`Login`] (the IDE's
//! "Sign in to VM" dialog). What the person typed is kept in the keyring only when they asked to keep it, and only
//! once the guest took it. None of it reaches the model.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::Hypervisor;
use super::run::{Call, exec};
use crate::Output;

/// A guest's user and password.
#[derive(Clone, PartialEq, Eq)]
pub struct Guest {
    pub user: String,
    pub password: String,
}

impl std::fmt::Debug for Guest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Guest").field("user", &self.user).finish_non_exhaustive()
    }
}

/// What the person is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub vm: String,
    pub hypervisor: Hypervisor,
    /// The user to put in the form first (the one tried before, or none).
    pub user: String,
    /// The guest refused the last sign-in.
    pub refused: bool,
    /// The keyring is here, so what is typed can be kept.
    pub can_save: bool,
    /// The keyring's name as the person knows it ("the keyring", "the Keychain", "Credential Manager").
    pub keyring: String,
}

/// What the person answered.
#[derive(Clone, PartialEq, Eq)]
pub struct Signed {
    pub guest: Guest,
    /// Keep it in the keyring once the guest takes it.
    pub save: bool,
}

/// Who asks the person for a guest's user and password.
#[async_trait::async_trait]
pub trait Login: Send + Sync {
    /// None: the person cancelled, or nobody could be asked.
    async fn ask(&self, a: &Ask) -> Option<Signed>;
}

/// Who asks, as the tools' context holds it.
#[derive(Clone)]
pub struct Asker(pub Arc<dyn Login>);

impl std::fmt::Debug for Asker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Asker")
    }
}

/// The keyring of this system, by the program that reaches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyring {
    /// Secret Service (GNOME Keyring, KWallet) through `secret-tool`.
    Secret,
    /// macOS's Keychain through `security`.
    Keychain,
    /// Windows' Credential Manager (its password vault) through PowerShell.
    Vault,
}

/// The service the guests' sign-ins are kept under.
const SERVICE: &str = "code-arena-vm";

impl Keyring {
    /// This system's, when its program is here.
    pub fn here() -> Option<Keyring> {
        let k = if cfg!(windows) {
            Keyring::Vault
        } else if cfg!(target_os = "macos") {
            Keyring::Keychain
        } else {
            Keyring::Secret
        };
        crate::shell::which(k.program()).then_some(k)
    }

    pub fn program(self) -> &'static str {
        match self {
            Keyring::Secret => "secret-tool",
            Keyring::Keychain => "security",
            Keyring::Vault => "powershell",
        }
    }

    /// Its name in "Keep it in KEYRING".
    pub fn name(self) -> &'static str {
        match self {
            Keyring::Secret => "the keyring",
            Keyring::Keychain => "the Keychain",
            Keyring::Vault => "Credential Manager",
        }
    }

    /// The call that reads a machine's sign-in: what it says is [`secret`]'s text.
    pub fn lookup(self, key: &str) -> Call {
        match self {
            Keyring::Secret => Call::new("secret-tool", &["lookup", "service", SERVICE, "vm", key]),
            Keyring::Keychain => Call::new("security", &["find-generic-password", "-s", SERVICE, "-a", key, "-w"]),
            Keyring::Vault => Call::new("powershell", &["-NoProfile", "-NonInteractive", "-Command", "-"]).input(
                format!(
                    "{VAULT}$c = $v.Retrieve({}, {}); $c.RetrievePassword(); [Console]::Out.Write($c.Password)\n",
                    ps_quote(SERVICE),
                    ps_quote(key)
                ),
            ),
        }
    }

    /// The call that keeps a machine's sign-in (the secret on standard input where the program takes it there).
    pub fn store(self, key: &str, guest: &Guest) -> Call {
        let secret = secret(guest);
        match self {
            Keyring::Secret => Call::new(
                "secret-tool",
                &["store", "--label", &format!("Code Arena: sign-in to {key}"), "service", SERVICE, "vm", key],
            )
            .input(secret),
            // `security` takes the password only as an argument.
            Keyring::Keychain => {
                Call::new("security", &["add-generic-password", "-U", "-s", SERVICE, "-a", key, "-w", &secret])
            }
            Keyring::Vault => Call::new("powershell", &["-NoProfile", "-NonInteractive", "-Command", "-"]).input(
                format!(
                    "{VAULT}$v.Add((New-Object Windows.Security.Credentials.PasswordCredential({}, {}, {})))\n",
                    ps_quote(SERVICE),
                    ps_quote(key),
                    ps_quote(&secret)
                ),
            ),
        }
    }
}

const VAULT: &str = "[void][Windows.Security.Credentials.PasswordVault,Windows.Security.Credentials,\
ContentType=WindowsRuntime]; $v = New-Object Windows.Security.Credentials.PasswordVault; ";

/// A PowerShell string of a text, as it is.
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// What a sign-in is kept as in the keyring.
pub fn secret(g: &Guest) -> String {
    json!({"user": g.user, "password": g.password}).to_string()
}

/// A sign-in read back from the keyring.
pub fn guest_of(secret: &str) -> Option<Guest> {
    let v: Value = serde_json::from_str(secret.trim()).ok()?;
    Some(Guest { user: v["user"].as_str()?.to_string(), password: v["password"].as_str()?.to_string() })
}

/// The name a machine's sign-in is kept under.
pub fn key(h: Hypervisor, vm: &str) -> String {
    format!("{}:{vm}", h.id())
}

/// The most times the person is asked again after the guest refused.
const MOST_ASKS: usize = 3;

/// How long the keyring may take.
const KEYRING_TIME: Duration = Duration::from_secs(20);

/// Where the next sign-in to try comes from, call after call of a tool: the keyring first, then the person, again
/// when the guest refused.
pub struct SignIn {
    hypervisor: Hypervisor,
    vm: String,
    keyring: Option<Keyring>,
    login: Option<Arc<dyn Login>>,
    tried_keyring: bool,
    asks: usize,
    refused: bool,
    last_user: String,
    /// The last one tried came from the person, who asked to keep it.
    save: bool,
}

impl SignIn {
    pub fn new(hypervisor: Hypervisor, vm: &str, login: Option<Arc<dyn Login>>) -> SignIn {
        SignIn::with(hypervisor, vm, login, Keyring::here())
    }

    pub fn with(hypervisor: Hypervisor, vm: &str, login: Option<Arc<dyn Login>>, keyring: Option<Keyring>) -> SignIn {
        SignIn {
            hypervisor,
            vm: vm.to_string(),
            keyring,
            login,
            tried_keyring: false,
            asks: 0,
            refused: false,
            last_user: String::new(),
            save: false,
        }
    }

    /// The next user and password to try, or what the model is told when there is none.
    pub async fn next(&mut self) -> Result<Guest, Output> {
        if !self.tried_keyring {
            self.tried_keyring = true;
            if let Some(k) = self.keyring {
                let r = exec(&k.lookup(&key(self.hypervisor, &self.vm)), KEYRING_TIME).await;
                if let Some(g) = r.ok().then(|| guest_of(&r.out)).flatten() {
                    self.last_user = g.user.clone();
                    self.save = false;
                    return Ok(g);
                }
            }
        }
        let Some(login) = self.login.clone() else {
            return Err(Output::error(format!(
                "{} needs its guest's user and password, and nobody can be asked here: sign in from the IDE, or \
                 keep them in the keyring first.",
                self.vm
            )));
        };
        if self.asks > MOST_ASKS {
            return Err(Output::error(format!("The guest of {} refused every sign-in it was given.", self.vm)));
        }
        self.asks += 1;
        let ask = self.ask();
        match login.ask(&ask).await {
            Some(s) => {
                self.last_user = s.guest.user.clone();
                self.save = s.save && self.keyring.is_some();
                Ok(s.guest)
            }
            None => Err(Output::error(format!("The person did not sign in to {}.", self.vm))),
        }
    }

    /// The question the person is asked next.
    pub fn ask(&self) -> Ask {
        Ask {
            vm: self.vm.clone(),
            hypervisor: self.hypervisor,
            user: self.last_user.clone(),
            refused: self.refused,
            can_save: self.keyring.is_some(),
            keyring: self.keyring.map(|k| k.name()).unwrap_or("the keyring").to_string(),
        }
    }

    /// The guest refused the last one: the next is asked of the person, saying so.
    pub fn was_refused(&mut self) {
        self.refused = true;
        self.save = false;
    }

    /// The guest took it: kept in the keyring when the person asked. Says why not when it could not be.
    pub async fn took(&mut self, g: &Guest) -> Option<String> {
        if !std::mem::take(&mut self.save) {
            return None;
        }
        let k = self.keyring?;
        let r = exec(&k.store(&key(self.hypervisor, &self.vm), g), KEYRING_TIME).await.scrubbed(&g.password);
        (!r.ok()).then(|| format!("The sign-in was not kept in {}: {}", k.name(), r.why()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct Person(Mutex<Vec<Ask>>, Option<Signed>);

    #[async_trait::async_trait]
    impl Login for Person {
        async fn ask(&self, a: &Ask) -> Option<Signed> {
            self.0.lock().map(|mut l| l.push(a.clone())).ok();
            self.1.clone()
        }
    }

    fn fake() -> Guest {
        Guest { user: "tester".into(), password: "fake-pass".into() }
    }

    #[test]
    fn a_sign_in_is_kept_as_json_and_read_back() {
        let g = Guest { user: "a\"b".into(), password: "p'q\n".into() };
        assert_eq!(guest_of(&secret(&g)), Some(g));
        assert_eq!(guest_of("nonsense"), None);
        assert!(!format!("{:?}", fake()).contains("fake-pass"));
    }

    #[test]
    fn the_keyrings_calls_keep_the_secret_off_the_arguments_where_they_can() {
        let s = Keyring::Secret.store("vmware:a", &fake());
        assert_eq!(s.program, "secret-tool");
        assert!(s.args.iter().all(|a| !a.contains("fake-pass")));
        assert!(s.stdin.as_deref().is_some_and(|i| i.contains("fake-pass")));
        assert_eq!(Keyring::Secret.lookup("k").args, ["lookup", "service", SERVICE, "vm", "k"]);
        let v = Keyring::Vault.store("k", &Guest { user: "o'n".into(), password: "x".into() });
        assert!(v.args.iter().all(|a| !a.contains("o'n")));
        assert!(v.stdin.as_deref().is_some_and(|i| i.contains("o''n")));
        assert_eq!(ps_quote("it's"), "'it''s'");
    }

    #[tokio::test]
    async fn the_person_is_asked_and_asked_again_when_refused() {
        let person = Arc::new(Person(Mutex::new(vec![]), Some(Signed { guest: fake(), save: true })));
        let mut s = SignIn::with(Hypervisor::VirtualBox, "test-vm", Some(person.clone()), None);
        assert_eq!(s.next().await.map(|g| g.user), Ok("tester".into()));
        s.was_refused();
        assert!(s.next().await.is_ok());
        let asked = person.0.lock().map(|l| l.clone()).unwrap_or_default();
        assert_eq!(asked.len(), 2);
        assert!(!asked[0].refused && asked[1].refused);
        assert_eq!(asked[1].user, "tester");
        assert!(!asked[0].can_save, "no keyring here, so nothing is offered to keep");
        // No keyring: nothing to keep, nothing said.
        assert_eq!(s.took(&fake()).await, None);
    }

    #[tokio::test]
    async fn nobody_to_ask_or_a_cancel_is_said_to_the_model() {
        let mut s = SignIn::with(Hypervisor::VMware, "a.vmx", None, None);
        assert!(s.next().await.is_err_and(|o| o.text.contains("nobody can be asked")));
        let person = Arc::new(Person(Mutex::new(vec![]), None));
        let mut s = SignIn::with(Hypervisor::VMware, "a.vmx", Some(person), None);
        assert!(s.next().await.is_err_and(|o| o.text.contains("did not sign in")));
    }
}
