//! Read account limits through a short-lived Codex stdio app-server.

use std::{fmt, process::Stdio, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{ChildStdin, ChildStdout, Command},
    time::{Instant, timeout_at},
};

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// Static messages keep upstream output, credentials and command paths private.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodexError(pub &'static str);

impl fmt::Display for CodexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for CodexError {}

#[derive(Clone)]
pub struct CodexClient {
    timeout: Duration,
    command: Vec<String>,
}

impl CodexClient {
    pub fn new(timeout: Duration) -> Self {
        Self::with_command(
            timeout,
            vec![
                "codex".into(),
                "app-server".into(),
                "--stdio".into(),
                "-c".into(),
                "cli_auth_credentials_store=\"file\"".into(),
            ],
        )
    }

    pub fn with_command(timeout: Duration, command: Vec<String>) -> Self {
        Self { timeout, command }
    }

    pub async fn read_limits(&self) -> Result<Value, CodexError> {
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .ok_or(CodexError("Codex timeout is too large"))?;
        let (program, arguments) = self
            .command
            .split_first()
            .ok_or(CodexError("Codex command is empty"))?;
        let mut child = Command::new(program)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| CodexError("Codex process could not start"))?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = child.stdout.take().expect("piped stdout");
        let result = timeout_at(deadline, async {
            let mut buffered = Vec::new();
            send(
                &mut stdin,
                json!({
                    "id": 1,
                    "method": "initialize",
                    "params": {"clientInfo": {
                        "name": "ai_usage_exporter",
                        "version": env!("CARGO_PKG_VERSION")
                    }}
                }),
            )
            .await?;
            receive(&mut stdout, 1, &mut buffered).await?;
            send(&mut stdin, json!({"method": "initialized"})).await?;
            send(
                &mut stdin,
                json!({"id": 2, "method": "account/rateLimits/read"}),
            )
            .await?;
            receive(&mut stdout, 2, &mut buffered).await
        })
        .await
        .unwrap_or(Err(CodexError("Codex request timed out")));

        // Reap on every protocol outcome, including timeout and failed writes.
        // kill_on_drop additionally terminates a child if this future is cancelled.
        let cleanup = child.kill().await;
        if cleanup.is_err() && child.wait().await.is_err() {
            return Err(CodexError("Codex process could not be reaped"));
        }
        result
    }
}

async fn send(stream: &mut ChildStdin, message: Value) -> Result<(), CodexError> {
    let mut bytes = serde_json::to_vec(&message)
        .map_err(|_| CodexError("Codex request could not be encoded"))?;
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .await
        .map_err(|_| CodexError("Codex request could not be written"))?;
    stream
        .flush()
        .await
        .map_err(|_| CodexError("Codex request could not be written"))
}

async fn receive(
    stream: &mut ChildStdout,
    request_id: u64,
    buffered: &mut Vec<u8>,
) -> Result<Value, CodexError> {
    loop {
        if let Some(end) = buffered.iter().position(|byte| *byte == b'\n') {
            let message: Value = serde_json::from_slice(&buffered[..end])
                .map_err(|_| CodexError("Codex response is invalid"))?;
            buffered.drain(..=end);
            let object = message
                .as_object()
                .ok_or(CodexError("Codex response is not an object"))?;
            if object.get("id").and_then(Value::as_u64) == Some(request_id) {
                if object.contains_key("error") {
                    return Err(CodexError("Codex rejected the request"));
                }
                return object
                    .get("result")
                    .filter(|value| value.is_object())
                    .cloned()
                    .ok_or(CodexError("Codex response has no result object"));
            }
            continue;
        }
        let mut chunk = [0; 65536];
        let count = stream
            .read(&mut chunk)
            .await
            .map_err(|_| CodexError("Codex response could not be read"))?;
        if count == 0 {
            return Err(CodexError("Codex closed its response stream"));
        }
        if buffered.len() + count > MAX_RESPONSE_BYTES {
            return Err(CodexError("Codex response is too large"));
        }
        buffered.extend_from_slice(&chunk[..count]);
    }
}
