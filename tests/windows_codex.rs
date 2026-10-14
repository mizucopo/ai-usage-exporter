#![cfg(windows)]

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    time::Duration,
};

use ai_usage_exporter::codex::CodexClient;
use serde_json::json;
use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0},
    System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
};

fn mock() -> &'static Path {
    static MOCK: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &MOCK
        .get_or_init(|| {
            let directory = tempfile::Builder::new()
                .prefix("mock codex & ")
                .tempdir()
                .unwrap();
            let executable = directory.path().join("codex.exe");
            let source = directory.path().join("mock.rs");
            std::fs::write(&source, include_str!("support/mock_codex.rs")).unwrap();
            let mut compiler =
                Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
            compiler
                .arg("--edition=2024")
                .arg(source)
                .arg("-o")
                .arg(&executable);
            // Also support an isolated toolchain without a globally installed SDK.
            if let Ok(flags) = std::env::var("CARGO_ENCODED_RUSTFLAGS") {
                compiler.args(flags.split('\u{1f}').filter(|flag| !flag.is_empty()));
            }
            let target = match std::env::consts::ARCH {
                "aarch64" => "AARCH64_PC_WINDOWS_MSVC",
                _ => "X86_64_PC_WINDOWS_MSVC",
            };
            if let Some(linker) = std::env::var_os(format!("CARGO_TARGET_{target}_LINKER")) {
                let mut option = OsString::from("linker=");
                option.push(linker);
                compiler.arg("-C").arg(option);
            }
            let output = compiler
                .output()
                .expect("rustc must be available to compile the local mock");
            assert!(
                output.status.success(),
                "mock compilation failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            (directory, executable)
        })
        .1
}

fn client(mode: &str, pid_file: &Path, timeout: f64) -> CodexClient {
    CodexClient::with_command_seconds(
        timeout,
        vec![
            mock().to_string_lossy().into_owned(),
            mode.into(),
            pid_file.to_string_lossy().into_owned(),
        ],
    )
}

fn pid(path: &Path) -> u32 {
    std::fs::read_to_string(path).unwrap().parse().unwrap()
}

fn terminated(pid: u32) -> bool {
    // SAFETY: this read-only handle is opened only for our local mock PID and
    // closed before returning. An already reaped child cannot be opened.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return true;
        }
        let result = WaitForSingleObject(handle, 0);
        assert_ne!(CloseHandle(handle), 0);
        result == WAIT_OBJECT_0
    }
}

#[tokio::test]
async fn native_mock_initializes_reads_limits_and_is_reaped() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("child.pid");
    let result = client("success", &pid_file, 5.0)
        .read_limits()
        .await
        .unwrap();
    assert_eq!(
        result,
        json!({"rateLimits":{"primary":{"usedPercent":25,"windowDurationMins":300,"resetsAt":4102444800_u64}}})
    );
    assert!(terminated(pid(&pid_file)));
}

#[tokio::test]
async fn failures_and_timeout_are_sanitized_and_reap_the_native_mock() {
    for (mode, timeout, message) in [
        ("error", 5.0, "Codex rejected the request"),
        ("invalid", 5.0, "Codex response is invalid"),
        ("oversize", 5.0, "Codex response is too large"),
        ("timeout", 0.5, "Codex request timed out"),
        ("slow", 0.25, "Codex request timed out"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("child.pid");
        let error = client(mode, &pid_file, timeout)
            .read_limits()
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), message);
        assert!(
            terminated(pid(&pid_file)),
            "mock remained alive after {mode}"
        );
    }
}

#[tokio::test]
async fn cancellation_terminates_the_native_mock() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("child.pid");
    let client = client("timeout", &pid_file, 30.0);
    let fetch = tokio::spawn(async move { client.read_limits().await });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let pid = loop {
        if let Ok(value) = std::fs::read_to_string(&pid_file)
            && let Ok(pid) = value.parse()
        {
            break pid;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(!terminated(pid));
    fetch.abort();
    assert!(fetch.await.unwrap_err().is_cancelled());
    while !terminated(pid) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "cancelled native mock remained alive"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn native_start_and_eof_failures_are_sanitized() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("child.pid");
    assert!(client("eof", &pid_file, 5.0).read_limits().await.is_err());
    let missing = CodexClient::with_command(
        Duration::from_secs(1),
        vec![
            directory
                .path()
                .join("secret-nonexistent.exe")
                .to_string_lossy()
                .into_owned(),
        ],
    );
    let error = missing.read_limits().await.unwrap_err();
    assert_eq!(error.to_string(), "Codex process could not start");
}
