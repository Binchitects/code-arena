//! Commands with no time limit (a full build, a long test suite) and in the background (a dev server, a
//! watcher): each a job with its output kept as it comes (the last 256 KB). A job with no time limit that ends
//! tells the session, which carries on from there in a turn of its own; one in the background runs until stopped.

use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

/// What is kept of a job's output.
const KEPT: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobInfo {
    pub id: u32,
    pub command: String,
    pub background: bool,
    pub running: bool,
    pub exit: Option<i32>,
    /// The turn that started it (its question's place in the session).
    pub turn: Option<i64>,
    /// It was stopped (by the person, or with its turn), not left to end.
    pub stopped: bool,
    /// When it began and when it ended, in milliseconds since the Unix epoch.
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
}

/// Now, in milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// A job with no time limit ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub id: u32,
    pub command: String,
    pub exit: Option<i32>,
    pub stopped: bool,
    /// The end of its output (8,000 characters at most).
    pub tail: String,
}

struct Job {
    info: Arc<Mutex<JobInfo>>,
    output: Arc<Mutex<VecDeque<u8>>>,
    pid: Option<u32>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Default)]
pub struct Jobs {
    jobs: Mutex<Vec<Job>>,
    ended: Mutex<Option<mpsc::UnboundedSender<Ended>>>,
    ends: Mutex<Option<mpsc::UnboundedSender<u32>>>,
    current_turn: Mutex<Option<i64>>,
}

impl std::fmt::Debug for Jobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Jobs").finish_non_exhaustive()
    }
}

impl Jobs {
    /// Where ended jobs are told: the session that carries on from them.
    pub fn listen(&self) -> mpsc::UnboundedReceiver<Ended> {
        let (tx, rx) = mpsc::unbounded_channel();
        if let Ok(mut e) = self.ended.lock() {
            *e = Some(tx);
        }
        rx
    }

    /// Where every job's end is told by its id, those in the background too: for what shows the jobs.
    pub fn ends(&self) -> mpsc::UnboundedReceiver<u32> {
        let (tx, rx) = mpsc::unbounded_channel();
        if let Ok(mut e) = self.ends.lock() {
            *e = Some(tx);
        }
        rx
    }

    pub fn current_turn(&self) -> Option<i64> {
        self.current_turn.lock().ok().and_then(|t| *t)
    }

    pub fn begin_turn(&self, seq: i64) {
        if let Ok(mut t) = self.current_turn.lock() {
            *t = Some(seq);
        }
    }

    /// Starts `program args…` in `cwd` as a job of its own.
    pub fn start(
        &self,
        command: &str,
        program: &str,
        args: &[String],
        cwd: &Path,
        hidden: &[&str],
        background: bool,
    ) -> std::io::Result<u32> {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for v in hidden {
            cmd.env_remove(v);
        }
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd.spawn()?;
        let id = self.jobs.lock().map(|j| j.len() as u32 + 1).unwrap_or(1);
        let turn = self.current_turn.lock().ok().and_then(|t| *t);
        let info = Arc::new(Mutex::new(JobInfo {
            id,
            command: command.to_string(),
            background,
            running: true,
            exit: None,
            turn,
            stopped: false,
            started_ms: now_ms(),
            ended_ms: None,
        }));
        let output = Arc::new(Mutex::new(VecDeque::new()));
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pid = child.id();
        let mut readers = Vec::new();
        for pipe in [
            child.stdout.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
            child.stderr.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let out = output.clone();
            readers.push(tokio::spawn(async move {
                let mut pipe = pipe;
                let mut buf = vec![0u8; 8192];
                while let Ok(n) = pipe.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut o) = out.lock() {
                        o.extend(&buf[..n]);
                        while o.len() > KEPT {
                            o.pop_front();
                        }
                    }
                }
            }));
        }
        let (i2, o2, s2) = (info.clone(), output.clone(), stopped.clone());
        let tell = self.ended.lock().ok().and_then(|e| e.clone());
        let ends = self.ends.lock().ok().and_then(|e| e.clone());
        tokio::spawn(async move {
            let status = child.wait().await.ok();
            // The readers end when its pipes close: the output is whole then. A program it left running (`cmd &`)
            // may hold them open: then what came within two seconds is what is told.
            let _ = tokio::time::timeout(Duration::from_secs(2), futures_util::future::join_all(readers)).await;
            let exit = status.and_then(|s| s.code());
            let stopped = s2.load(std::sync::atomic::Ordering::SeqCst);
            let (command, background) = match i2.lock() {
                Ok(mut i) => {
                    i.running = false;
                    i.exit = exit;
                    i.stopped = stopped;
                    i.ended_ms = Some(now_ms());
                    (i.command.clone(), i.background)
                }
                Err(_) => return,
            };
            if let Some(tx) = ends {
                let _ = tx.send(id);
            }
            // Its end starts no turn when it ran in the background.
            if !background && let Some(tx) = tell {
                let tail = tail_of(&o2, 8_000);
                let _ = tx.send(Ended { id, command, exit, stopped, tail });
            }
        });
        if let Ok(mut j) = self.jobs.lock() {
            j.push(Job { info, output, pid, stopped });
        }
        Ok(id)
    }

    pub fn info(&self, id: u32) -> Option<JobInfo> {
        let jobs = self.jobs.lock().ok()?;
        jobs.iter().find_map(|j| j.info.lock().ok().filter(|i| i.id == id).map(|i| i.clone()))
    }

    pub fn list(&self) -> Vec<JobInfo> {
        self.jobs
            .lock()
            .map(|j| j.iter().filter_map(|x| x.info.lock().ok().map(|i| i.clone())).collect())
            .unwrap_or_default()
    }

    /// The end of a job's output.
    pub fn output(&self, id: u32, chars: usize) -> Option<String> {
        let jobs = self.jobs.lock().ok()?;
        let job = jobs.iter().find(|j| j.info.lock().map(|i| i.id == id).unwrap_or(false))?;
        Some(tail_of(&job.output, chars))
    }

    /// Waits for a job to end (`limit` at most); whether it did.
    pub async fn wait(&self, id: u32, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            match self.info(id) {
                Some(i) if !i.running => return true,
                None => return true,
                _ => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Stops a job and everything it started.
    pub fn stop(&self, id: u32) -> bool {
        let Ok(jobs) = self.jobs.lock() else { return false };
        let Some(job) = jobs.iter().find(|j| j.info.lock().map(|i| i.id == id && i.running).unwrap_or(false)) else {
            return false;
        };
        job.stopped.store(true, std::sync::atomic::Ordering::SeqCst);
        kill_group(job.pid);
        true
    }

    /// Stops the jobs with no time limit that the turn at `seq` started (Ctrl+C or Stop on that turn); those in the
    /// background run on.
    pub fn stop_turn(&self, seq: i64) -> Vec<u32> {
        let ids: Vec<u32> = self
            .list()
            .into_iter()
            .filter(|i| i.running && !i.background && i.turn == Some(seq))
            .map(|i| i.id)
            .collect();
        for id in &ids {
            self.stop(*id);
        }
        ids
    }

    /// Stops every job (the session ends).
    pub fn stop_all(&self) {
        for i in self.list() {
            if i.running {
                self.stop(i.id);
            }
        }
    }
}

fn tail_of(out: &Mutex<VecDeque<u8>>, chars: usize) -> String {
    let bytes: Vec<u8> = out.lock().map(|o| o.iter().copied().collect()).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let n = text.chars().count();
    if n <= chars { text.into_owned() } else { text.chars().skip(n - chars).collect() }
}

pub(crate) fn kill_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
        let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGKILL);
    }
    #[cfg(windows)]
    if let Some(pid) = pid {
        // The job's tree, as Windows ends one.
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_job_runs_on_and_tells_when_it_ends() {
        let jobs = Jobs::default();
        let mut ended = jobs.listen();
        jobs.begin_turn(7);
        let d = tempfile::tempdir().expect("temp");
        let id = jobs
            .start("echo built", "sh", &["-c".into(), "echo building; echo built".into()], d.path(), &[], false)
            .expect("start");
        let e = tokio::time::timeout(Duration::from_secs(10), ended.recv()).await.expect("in time").expect("told");
        assert_eq!((e.id, e.exit, e.stopped), (id, Some(0), false));
        assert_eq!(e.tail, "building\nbuilt\n");
        let info = jobs.info(id).expect("info");
        assert_eq!(info.turn, Some(7));
        assert!(!info.stopped && info.ended_ms.is_some_and(|e| e >= info.started_ms));
    }

    #[tokio::test]
    async fn stopping_a_turns_jobs_leaves_the_background_ones() {
        let jobs = Jobs::default();
        let mut ended = jobs.listen();
        let mut ends = jobs.ends();
        jobs.begin_turn(3);
        let d = tempfile::tempdir().expect("temp");
        let long = jobs.start("sleep", "sh", &["-c".into(), "sleep 30".into()], d.path(), &[], false).expect("start");
        let server = jobs.start("server", "sh", &["-c".into(), "sleep 30".into()], d.path(), &[], true).expect("start");
        assert_eq!(jobs.stop_turn(3), vec![long]);
        let e = tokio::time::timeout(Duration::from_secs(10), ended.recv()).await.expect("in time").expect("told");
        assert!(e.stopped && jobs.info(long).expect("info").stopped);
        assert_eq!(ends.recv().await, Some(long));
        assert!(jobs.info(server).expect("info").running);
        jobs.stop_all();
        assert!(jobs.wait(server, Duration::from_secs(10)).await);
        // The one in the background tells its end there too, though no turn is started by it.
        assert_eq!(tokio::time::timeout(Duration::from_secs(10), ends.recv()).await.expect("in time"), Some(server));
    }
}
