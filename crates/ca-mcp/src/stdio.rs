//! A server run as a child process, speaking MCP as JSON lines on its stdin and stdout. Its stderr is kept (the
//! last lines) for when it fails.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use crate::{Incoming, McpError, Progress, Transport, classify};

/// How to start the server: as `mcpServers` gives it (`command`, `args`, `env`, `cwd`).
#[derive(Debug, Clone, Default)]
pub struct StdioSpec {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    /// Variables added (`None` takes one away).
    pub env: Vec<(String, Option<String>)>,
    pub cwd: Option<PathBuf>,
}

type Waiting = Arc<Mutex<HashMap<u64, (oneshot::Sender<Result<Value, String>>, mpsc::UnboundedSender<String>)>>>;

pub struct StdioTransport {
    name: String,
    stdin: tokio::process::ChildStdin,
    waiting: Waiting,
    stderr: Arc<Mutex<VecDeque<String>>>,
    _child: tokio::process::Child,
}

impl StdioTransport {
    pub async fn start(spec: StdioSpec) -> Result<Self, McpError> {
        let mut cmd = tokio::process::Command::new(&spec.command);
        cmd.args(&spec.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        // The person's key stays Code Arena's.
        cmd.env_remove("ARENA_API_KEY");
        for (k, v) in &spec.env {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            };
        }
        if let Some(d) = &spec.cwd {
            cmd.current_dir(d);
        }
        let mut child =
            cmd.spawn().map_err(|e| McpError::Spawn(spec.name.clone(), format!("{}: {e}", spec.command)))?;
        let stdin = child.stdin.take().ok_or_else(|| McpError::Spawn(spec.name.clone(), "no stdin".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| McpError::Spawn(spec.name.clone(), "no stdout".into()))?;
        let stderr_pipe = child.stderr.take();
        let waiting: Waiting = Arc::new(Mutex::new(HashMap::new()));
        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        let w = waiting.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                match classify(&msg) {
                    Incoming::Answer(id, answer) => {
                        if let Some((tx, _)) = w.lock().ok().and_then(|mut m| m.remove(&id)) {
                            let _ = tx.send(answer);
                        }
                    }
                    Incoming::Progress(text) => {
                        // A progress report starts again the wait of every call running.
                        if let Ok(m) = w.lock() {
                            for (_, p) in m.values() {
                                let _ = p.send(text.clone());
                            }
                        }
                    }
                    Incoming::Other => {}
                }
            }
            // The server ended: every call still waiting hears it.
            if let Ok(mut m) = w.lock() {
                m.clear();
            }
        });
        if let Some(pipe) = stderr_pipe {
            let kept = stderr.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(pipe).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(mut k) = kept.lock() {
                        if k.len() >= 50 {
                            k.pop_front();
                        }
                        k.push_back(line);
                    }
                }
            });
        }
        Ok(Self { name: spec.name, stdin, waiting, stderr, _child: child })
    }

    async fn write(&mut self, message: &Value) -> Result<(), McpError> {
        let mut line = serde_json::to_vec(message)
            .map_err(|e| McpError::Protocol { url: self.name.clone(), message: e.to_string() })?;
        line.push(b'\n');
        self.stdin.write_all(&line).await.map_err(|_| self.ended())?;
        self.stdin.flush().await.map_err(|_| self.ended())
    }

    fn ended(&self) -> McpError {
        let tail: Vec<String> =
            self.stderr.lock().map(|k| k.iter().rev().take(5).rev().cloned().collect()).unwrap_or_default();
        if tail.is_empty() {
            McpError::Ended(self.name.clone())
        } else {
            McpError::Ended(format!("{} ({})", self.name, tail.join(" / ")))
        }
    }
}

#[async_trait::async_trait]
impl Transport for StdioTransport {
    async fn request(
        &mut self,
        id: u64,
        message: &Value,
        idle: Duration,
        progress: Progress<'_>,
    ) -> Result<Value, McpError> {
        let (tx, mut rx) = oneshot::channel();
        let (ptx, mut prx) = mpsc::unbounded_channel();
        if let Ok(mut m) = self.waiting.lock() {
            m.insert(id, (tx, ptx));
        }
        self.write(message).await?;
        loop {
            // Progress first: a report sent before the answer is taken before it.
            tokio::select! {
                biased;
                Some(text) = prx.recv() => progress(&text),
                answer = &mut rx => {
                    return match answer {
                        Ok(a) => a.map_err(|m| McpError::Protocol { url: self.name.clone(), message: m }),
                        Err(_) => Err(self.ended()),
                    };
                }
                _ = tokio::time::sleep(idle) => {
                    if let Ok(mut m) = self.waiting.lock() {
                        m.remove(&id);
                    }
                    return Err(McpError::Timeout(idle.as_secs()));
                }
            }
        }
    }

    async fn notify(&mut self, message: &Value) -> Result<(), McpError> {
        self.write(message).await
    }

    fn describe(&self) -> String {
        self.name.clone()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::Client;
    use serde_json::json;

    /// A tiny MCP server in sh: answers initialize, lists one tool, echoes a call.
    const SERVER: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"serverInfo":{"name":"echo"}}}\n' "$id" ;;
    *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
    *'"tools/call"'*) printf '{"jsonrpc":"2.0","method":"notifications/progress","params":{"progress":1,"total":2}}\n'
                       printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echoed"}]}}\n' "$id" ;;
  esac
done
"#;

    #[tokio::test]
    async fn a_child_process_server() {
        let spec = StdioSpec {
            name: "echo".into(),
            command: "sh".into(),
            args: vec!["-c".into(), SERVER.into()],
            ..Default::default()
        };
        let mut c = Client::stdio(spec).await.expect("start");
        c.initialize().await.expect("init");
        assert_eq!(c.server_name.as_deref(), Some("echo"));
        assert_eq!(c.list_tools().await.expect("tools")[0].name, "echo");
        let mut said = vec![];
        let r = c
            .call_tool("echo", json!({}), Duration::from_secs(10), &mut |p| said.push(p.to_string()))
            .await
            .expect("call");
        assert_eq!(r.text, "echoed");
        assert_eq!(said, vec!["50%"]);
    }

    #[tokio::test]
    async fn a_server_that_ends_is_said() {
        let spec = StdioSpec {
            name: "gone".into(),
            command: "sh".into(),
            args: vec!["-c".into(), "echo bye >&2; exit 0".into()],
            ..Default::default()
        };
        let mut c = Client::stdio(spec).await.expect("start");
        let e = c.initialize().await.expect_err("ended");
        assert!(e.gone(), "{e}");
    }
}
