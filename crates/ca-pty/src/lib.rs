//! A program on a pseudo-terminal: its output read as it comes, what is typed written to it, its size changed, and it
//! stopped when the terminal is closed. Unix only for now (Windows' ConPTY comes later): elsewhere `spawn` says so.
//!
//! The only `unsafe` in Code Arena is here, each with why: the child's new session and controlling terminal (between
//! fork and exec, where only such calls are safe), and the terminal's size.

use std::io::{Read, Write};
use std::path::Path;

// Windows' own calls, for its pseudo-terminal: named here until that is written.
#[cfg(windows)]
use windows_sys as _;

/// A program running on a pseudo-terminal.
pub struct Pty {
    #[cfg(unix)]
    master: std::fs::File,
    child: std::process::Child,
}

impl Pty {
    /// Runs `program` (with `args`) in `cwd` on a terminal of `cols` × `rows`, `env` added to its environment.
    pub fn spawn(
        program: &Path,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
        cols: u16,
        rows: u16,
    ) -> std::io::Result<Pty> {
        Pty::spawn_without(program, args, cwd, env, &[], cols, rows)
    }

    /// As [`Pty::spawn`], the variables named in `without` taken out of what it inherits (a key the program is not
    /// to see).
    #[cfg(unix)]
    pub fn spawn_without(
        program: &Path,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
        without: &[&str],
        cols: u16,
        rows: u16,
    ) -> std::io::Result<Pty> {
        use std::os::unix::process::CommandExt;
        let size = nix::pty::Winsize { ws_row: rows.max(1), ws_col: cols.max(1), ws_xpixel: 0, ws_ypixel: 0 };
        let pair = nix::pty::openpty(Some(&size), None).map_err(std::io::Error::from)?;
        let slave = std::fs::File::from(pair.slave);
        let mut cmd = std::process::Command::new(program);
        for name in without {
            cmd.env_remove(name);
        }
        cmd.args(args)
            .current_dir(cwd)
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(slave.try_clone()?)
            .stdout(slave.try_clone()?)
            .stderr(slave);
        // SAFETY: between fork and exec only async-signal-safe calls may run; setsid and ioctl are. The child leaves
        // this process's session and takes the terminal on its stdin (fd 0) as its controlling one, so Ctrl+C and job
        // control reach the programs run in it.
        #[allow(unsafe_code)]
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        Ok(Pty { master: std::fs::File::from(pair.master), child })
    }

    #[cfg(not(unix))]
    pub fn spawn_without(
        _program: &Path,
        _args: &[String],
        _cwd: &Path,
        _env: &[(String, String)],
        _without: &[&str],
        _cols: u16,
        _rows: u16,
    ) -> std::io::Result<Pty> {
        Err(std::io::Error::other("the IDE's terminals are not on Windows yet"))
    }

    /// Where its output is read from (blocking reads; the end of it when the program ends).
    pub fn reader(&self) -> std::io::Result<Box<dyn Read + Send>> {
        #[cfg(unix)]
        return Ok(Box::new(self.master.try_clone()?));
        #[cfg(not(unix))]
        Err(std::io::Error::other("no terminal"))
    }

    /// Where what is typed is written.
    pub fn writer(&self) -> std::io::Result<Box<dyn Write + Send>> {
        #[cfg(unix)]
        return Ok(Box::new(self.master.try_clone()?));
        #[cfg(not(unix))]
        Err(std::io::Error::other("no terminal"))
    }

    /// The terminal's new size; the program is told (SIGWINCH).
    pub fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let size = libc::winsize { ws_row: rows.max(1), ws_col: cols.max(1), ws_xpixel: 0, ws_ypixel: 0 };
            // SAFETY: TIOCSWINSZ reads one winsize from the pointer given, which lives for the call.
            #[allow(unsafe_code)]
            let r = unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &size) };
            if r == -1 {
                return Err(std::io::Error::last_os_error());
            }
        }
        #[cfg(not(unix))]
        let _ = (cols, rows);
        Ok(())
    }

    /// Its exit code, once it has ended (`None` while it runs, or ended by a signal).
    pub fn ended(&mut self) -> Option<Option<i32>> {
        self.child.try_wait().ok().flatten().map(|s| s.code())
    }

    /// Stops it (its whole session: what it started too).
    pub fn stop(&mut self) {
        #[cfg(unix)]
        {
            let pid = nix::unistd::Pid::from_raw(-(self.child.id() as i32));
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGHUP);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if self.ended().is_none() {
            self.stop();
        }
    }
}

/// Bytes as text without cutting a character in two: what does not end a character yet is kept for the next read.
pub fn utf8_text(carry: &mut Vec<u8>, bytes: &[u8]) -> String {
    carry.extend_from_slice(bytes);
    let mut out = String::new();
    loop {
        match std::str::from_utf8(carry) {
            Ok(s) => {
                out.push_str(s);
                carry.clear();
                return out;
            }
            Err(e) => {
                let good = e.valid_up_to();
                out.push_str(std::str::from_utf8(&carry[..good]).unwrap_or_default());
                match e.error_len() {
                    // The end is the start of a character: kept.
                    None => {
                        carry.drain(..good);
                        return out;
                    }
                    // Not UTF-8: shown as the replacement character.
                    Some(n) => {
                        out.push('\u{fffd}');
                        carry.drain(..good + n);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_cut_between_reads_is_kept_whole() {
        let mut carry = Vec::new();
        let bytes = "héllo ✓".as_bytes();
        let (a, b) = bytes.split_at(2);
        assert_eq!(utf8_text(&mut carry, a), "h");
        assert_eq!(utf8_text(&mut carry, b), "éllo ✓");
        assert!(carry.is_empty());
        assert_eq!(utf8_text(&mut carry, &[0xff, b'x']), "\u{fffd}x");
    }

    #[cfg(unix)]
    #[test]
    fn a_shell_runs_on_its_terminal_at_its_size() {
        let d = tempfile::tempdir().expect("temp");
        let mut p =
            Pty::spawn(Path::new("/bin/sh"), &[], d.path(), &[("PS1".into(), "$ ".into())], 100, 30).expect("spawn");
        let mut w = p.writer().expect("writer");
        let mut r = p.reader().expect("reader");
        w.write_all(b"tty >/dev/null && echo is-a-tty; stty size; exit 3\n").expect("write");
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = r.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("is-a-tty"), "{text}");
        assert!(text.contains("30 100"), "{text}");
        p.stop();
    }

    #[cfg(unix)]
    #[test]
    fn a_variable_taken_out_is_not_inherited() {
        let d = tempfile::tempdir().expect("temp");
        let mut p = Pty::spawn_without(Path::new("/bin/sh"), &[], d.path(), &[], &["HOME"], 80, 24).expect("spawn");
        let mut w = p.writer().expect("writer");
        let mut r = p.reader().expect("reader");
        w.write_all(b"echo \"[${HOME-taken}]\"; exit\n").expect("write");
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = r.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("[taken]"), "{text}");
        p.stop();
    }
}
