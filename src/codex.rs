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
    timeout: f64,
    command: Vec<String>,
}

impl CodexClient {
    pub fn new(timeout: Duration) -> Self {
        Self::with_timeout_seconds(timeout.as_secs_f64())
    }

    pub fn with_timeout_seconds(timeout: f64) -> Self {
        Self::with_command_seconds(
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
        Self::with_command_seconds(timeout.as_secs_f64(), command)
    }

    pub fn with_command_seconds(timeout: f64, command: Vec<String>) -> Self {
        Self { timeout, command }
    }

    pub async fn read_limits(&self) -> Result<Value, CodexError> {
        // Preserve the legacy positive-finite setting range. Subnanosecond
        // timeouts expire immediately; timeouts beyond Instant's range are
        // effectively unlimited rather than preventing HTTP startup.
        let deadline = Duration::try_from_secs_f64(self.timeout)
            .ok()
            .and_then(|timeout| Instant::now().checked_add(timeout));
        let (program, arguments) = self
            .command
            .split_first()
            .ok_or(CodexError("Codex command is empty"))?;
        let mut child = Command::new(executable(program))
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| CodexError("Codex process could not start"))?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = child.stdout.take().expect("piped stdout");
        let communication = async {
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
        };
        let result = match deadline {
            Some(deadline) => timeout_at(deadline, communication)
                .await
                .unwrap_or(Err(CodexError("Codex request timed out"))),
            None => communication.await,
        };

        // Reap on every protocol outcome, including timeout and failed writes.
        // kill_on_drop additionally terminates a child if this future is cancelled.
        let cleanup = child.kill().await;
        if cleanup.is_err() && child.wait().await.is_err() {
            return Err(CodexError("Codex process could not be reaped"));
        }
        result
    }
}

fn executable(program: &str) -> std::path::PathBuf {
    #[cfg(windows)]
    if program.eq_ignore_ascii_case("codex")
        && let Some(path) = std::env::var_os("PATH")
            .and_then(|path| windows_codex_executable(std::env::split_paths(&path)))
    {
        return path;
    }
    program.into()
}

#[cfg(windows)]
fn windows_codex_executable(
    directories: impl IntoIterator<Item = std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    // npm's cmd/Node wrappers spawn a native grandchild. Launch that executable
    // directly so kill_on_drop and cleanup own the actual app-server process.
    let platform = match std::env::consts::ARCH {
        "x86_64" => Some(("codex-win32-x64", "x86_64-pc-windows-msvc")),
        "aarch64" => Some(("codex-win32-arm64", "aarch64-pc-windows-msvc")),
        _ => None,
    };
    for directory in directories {
        let native = directory.join("codex.exe");
        if native.is_file() {
            return Some(native);
        }
        let Some((package, target)) = platform else {
            continue;
        };
        if !directory.join("codex.cmd").is_file() && !directory.join("codex.ps1").is_file() {
            continue;
        }
        let scope = directory.join("node_modules").join("@openai");
        let codex = scope.join("codex");
        for vendor in [
            codex
                .join("node_modules")
                .join("@openai")
                .join(package)
                .join("vendor"),
            scope.join(package).join("vendor"),
            codex.join("vendor"),
        ] {
            let native = vendor.join(target).join("bin").join("codex.exe");
            if native.is_file() {
                return Some(native);
            }
        }
    }
    None
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

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    fn platform() -> (&'static str, &'static str) {
        match std::env::consts::ARCH {
            "aarch64" => ("codex-win32-arm64", "aarch64-pc-windows-msvc"),
            _ => ("codex-win32-x64", "x86_64-pc-windows-msvc"),
        }
    }

    #[test]
    fn resolves_native_executables_in_path_order_without_a_shell() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first path & literal");
        let second = directory.path().join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let native = first.join("codex.exe");
        std::fs::write(&native, b"native mock").unwrap();
        std::fs::write(second.join("codex.exe"), b"other mock").unwrap();
        assert_eq!(windows_codex_executable([first, second]), Some(native));
        assert_eq!(
            executable("explicit-native.exe"),
            std::path::PathBuf::from("explicit-native.exe")
        );
    }

    #[test]
    fn resolves_npm_nested_hoisted_and_legacy_bundled_binaries() {
        let (package, target) = platform();
        for layout in ["nested", "hoisted", "bundled"] {
            let directory = tempfile::tempdir().unwrap();
            let prefix = directory.path().join("npm prefix & literal");
            std::fs::create_dir_all(&prefix).unwrap();
            std::fs::write(prefix.join("codex.cmd"), b"must never execute this shim").unwrap();
            let scope = prefix.join("node_modules").join("@openai");
            let vendor = match layout {
                "nested" => scope
                    .join("codex")
                    .join("node_modules")
                    .join("@openai")
                    .join(package)
                    .join("vendor"),
                "hoisted" => scope.join(package).join("vendor"),
                _ => scope.join("codex").join("vendor"),
            };
            let native = vendor.join(target).join("bin").join("codex.exe");
            std::fs::create_dir_all(native.parent().unwrap()).unwrap();
            std::fs::write(&native, b"native mock").unwrap();
            assert_eq!(windows_codex_executable([prefix]), Some(native), "{layout}");
        }
    }

    #[test]
    fn absent_native_dependency_does_not_launch_an_npm_wrapper() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("codex.cmd"), b"unused shim").unwrap();
        assert_eq!(windows_codex_executable([directory.path().into()]), None);
    }
}
