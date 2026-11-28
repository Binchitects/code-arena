//! Running a hypervisor's program: its words out, its words in, a time limit, and what must never be shown (a
//! guest's password) taken out of what it said.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

/// One program to run: what it is, its arguments, and what goes in on its standard input (a secret, never an
/// argument when the program can take it so).
#[derive(Clone, PartialEq, Eq)]
pub struct Call {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<String>,
}

impl std::fmt::Debug for Call {
    // What goes in may be a secret: it is never printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Call").field("program", &self.program).field("args", &self.args).finish_non_exhaustive()
    }
}

impl Call {
    pub fn new(program: &str, args: &[&str]) -> Call {
        Call { program: program.into(), args: args.iter().map(|a| a.to_string()).collect(), stdin: None }
    }

    pub fn arg(mut self, a: impl Into<String>) -> Call {
        self.args.push(a.into());
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: Into<String>>(mut self, all: I) -> Call {
        self.args.extend(all.into_iter().map(Into::into));
        self
    }

    pub fn input(mut self, text: impl Into<String>) -> Call {
        self.stdin = Some(text.into());
        self
    }
}

/// What a program did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ran {
    /// Its exit code; none when it could not start, ran out of time or was ended by a signal.
    pub code: Option<i32>,
    pub out: String,
    pub err: String,
}

impl Ran {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// What it said, out then err, each trimmed, as one text.
    pub fn said(&self) -> String {
        let (o, e) = (self.out.trim_end(), self.err.trim_end());
        match (o.is_empty(), e.is_empty()) {
            (true, true) => String::new(),
            (false, true) => o.to_string(),
            (true, false) => e.to_string(),
            (false, false) => format!("{o}\n{e}"),
        }
    }

    /// Why it failed, as the model is told: what it said, or its exit code.
    pub fn why(&self) -> String {
        let said = self.said();
        if !said.is_empty() {
            return said;
        }
        match self.code {
            Some(c) => format!("It exited with code {c}."),
            None => "It did not finish.".into(),
        }
    }

    /// The same, a secret taken out of everything it said.
    pub fn scrubbed(mut self, secret: &str) -> Ran {
        self.out = scrub(&self.out, secret);
        self.err = scrub(&self.err, secret);
        self
    }
}

/// A text with a secret in it shown as dots (an empty secret is nothing to take out).
pub fn scrub(text: &str, secret: &str) -> String {
    if secret.is_empty() { text.to_string() } else { text.replace(secret, "••••••") }
}

/// Runs a call, ended when it takes longer than `limit` (and when the call that waits for it is dropped).
pub async fn exec(call: &Call, limit: Duration) -> Ran {
    let mut cmd = tokio::process::Command::new(&call.program);
    cmd.args(&call.args)
        .stdin(if call.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Ran { code: None, out: String::new(), err: format!("{} could not start: {e}", call.program) },
    };
    if let (Some(text), Some(mut pipe)) = (&call.stdin, child.stdin.take()) {
        let _ = pipe.write_all(text.as_bytes()).await;
        drop(pipe);
    }
    match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(Ok(o)) => Ran {
            code: o.status.code(),
            out: String::from_utf8_lossy(&o.stdout).into_owned(),
            err: String::from_utf8_lossy(&o.stderr).into_owned(),
        },
        Ok(Err(e)) => Ran { code: None, out: String::new(), err: format!("{} failed: {e}", call.program) },
        Err(_) => Ran {
            code: None,
            out: String::new(),
            err: format!("{} took longer than {} s and was stopped.", call.program, limit.as_secs()),
        },
    }
}

/// A file only its owner may read, holding a secret for a program that reads it from a file (VirtualBox's
/// `--passwordfile`); it is removed when dropped.
pub struct SecretFile(PathBuf);

impl SecretFile {
    pub fn new(secret: &str) -> std::io::Result<SecretFile> {
        use std::io::Write;
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let path = std::env::temp_dir().join(format!("code-arena-vm-{}-{n}", std::process::id()));
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&path)?;
        f.write_all(secret.as_bytes())?;
        Ok(SecretFile(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for SecretFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Bytes as base64 (the guest agent's file writes take it).
pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Base64 as bytes (what the guest agent reads and runs give back); none when it is not base64.
pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_goes_there_and_back() {
        for text in ["", "f", "fo", "foo", "foob", "fooba", "foobar", "hello\nworld\u{0}"] {
            let e = b64_encode(text.as_bytes());
            assert_eq!(b64_decode(&e).as_deref(), Some(text.as_bytes()), "{text:?} as {e}");
        }
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_decode("!!"), None);
    }

    #[test]
    fn a_secret_is_never_said_or_printed() {
        let r = Ran { code: Some(1), out: "user fake-pass".into(), err: "bad fake-pass".into() }.scrubbed("fake-pass");
        assert!(!r.said().contains("fake-pass"));
        assert_eq!(scrub("abc", ""), "abc");
        let c = Call::new("p", &["a"]).input("fake-pass");
        assert!(!format!("{c:?}").contains("fake-pass"));
    }

    #[test]
    fn why_says_what_it_said_or_its_code() {
        assert_eq!(Ran { code: Some(3), ..Ran::default() }.why(), "It exited with code 3.");
        assert_eq!(Ran { code: Some(3), out: "o\n".into(), err: "e".into() }.why(), "o\ne");
    }

    #[tokio::test]
    async fn a_program_that_is_not_there_says_so() {
        let r = exec(&Call::new("no-such-program-here", &[]), Duration::from_secs(5)).await;
        assert_eq!(r.code, None);
        assert!(r.err.contains("could not start"));
    }
}
