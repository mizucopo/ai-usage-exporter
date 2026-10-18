#![cfg(unix)]

use std::{
    fs::{self, File},
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Exporter(Child);

impl Drop for Exporter {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn scrape(address: SocketAddr) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("content-type: text/plain; version=0.0.4; charset=utf-8"));
    response.split_once("\r\n\r\n").unwrap().1.to_owned()
}

#[test]
fn diagnostics_are_safe_and_only_emitted_for_failed_refreshes() {
    for (mode, diagnostic) in [
        ("start", "Codex quota fetch failed: process_start\n"),
        ("timeout", "Codex quota fetch failed: timeout\n"),
        ("invalid", "Codex quota fetch failed: invalid_response\n"),
        ("shape", "Codex quota fetch failed: invalid_response\n"),
        ("rejected", "Codex quota fetch failed: request_rejected\n"),
        ("eof", "Codex quota fetch failed: request_failed\n"),
        (
            "quota",
            "Codex quota parsing failed: Expected quota number\n",
        ),
        ("success", ""),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let command_directory = directory.path().join("secret-command-path");
        fs::create_dir(&command_directory).unwrap();
        let calls = directory.path().join("calls");
        let stderr = directory.path().join("stderr");
        let stdout = directory.path().join("stdout");
        if mode != "start" {
            let response = match mode {
                "timeout" => "read -r never".to_owned(),
                "invalid" => {
                    "printf '%s\\n' 'secret-token raw-response /private/secret-path'".into()
                }
                "shape" => "printf '%s\\n' '[\"secret-token\",\"/private/secret-path\"]'".into(),
                "eof" => "exit 7".into(),
                _ => {
                    let response = match mode {
                        "rejected" => {
                            serde_json::json!({"id": 2, "error": {"message": "secret-token /private/secret-path"}})
                        }
                        "quota" => {
                            serde_json::json!({"id": 2, "result": {"rateLimits": {"primary": {
                                "usedPercent": "secret-token /private/secret-path", "windowDurationMins": 300, "resetsAt": 4102444800_u64
                            }}}})
                        }
                        "success" => {
                            serde_json::json!({"id": 2, "result": {"rateLimits": {"primary": {
                                "usedPercent": 25, "windowDurationMins": 300, "resetsAt": 4102444800_u64
                            }}}})
                        }
                        _ => unreachable!(),
                    };
                    format!(
                        "printf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\nread -r note\nread -r request\nprintf '%s\\n' '{response}'"
                    )
                }
            };
            let fake = command_directory.join("codex");
            fs::write(
                &fake,
                format!(
                    "#!/bin/sh\nprintf x >> \"$MOCK_CALLS\"\nprintf '%s\\n' 'secret-stderr /private/secret-path' >&2\nread -r request\n{response}\nread -r never\n"
                ),
            )
            .unwrap();
            fs::set_permissions(fake, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let child = Command::new(env!("CARGO_BIN_EXE_ai-usage-exporter"))
            .env_clear()
            .env("PATH", &command_directory)
            .env("CODEX_HOME", directory.path().join("secret-auth-path"))
            .env("MOCK_CALLS", &calls)
            .env("AI_USAGE_EXPORTER_HOST", "127.0.0.1")
            .env("AI_USAGE_EXPORTER_PORT", address.port().to_string())
            .env("AI_USAGE_EXPORTER_CACHE_TTL_SECONDS", "3600")
            .env("AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS", "0.5")
            .stdout(Stdio::from(File::create(&stdout).unwrap()))
            .stderr(Stdio::from(File::create(&stderr).unwrap()))
            .spawn()
            .unwrap();
        let mut exporter = Exporter(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if TcpStream::connect(address).is_ok() {
                break;
            }
            assert!(exporter.0.try_wait().unwrap().is_none(), "exporter exited");
            assert!(Instant::now() < deadline, "exporter failed to listen");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(fs::read_to_string(&stderr).unwrap(), "");
        for _ in 0..3 {
            let metrics = scrape(address);
            if mode == "success" {
                assert!(metrics.contains("ai_usage_exporter_scrape_success 1.0"));
                assert!(metrics.contains("codex_rate_limit_remaining_ratio{window=\"5h\"} 0.75"));
            } else {
                assert!(metrics.contains("ai_usage_exporter_scrape_success 0.0"));
                assert!(metrics.contains("ai_usage_exporter_last_success_timestamp_seconds 0.0"));
                assert!(!metrics.contains("codex_rate_limit_"));
            }
            assert!(!metrics.contains("secret"));
            // Exact output also rules out raw responses, command paths and stderr.
            assert_eq!(fs::read_to_string(&stderr).unwrap(), diagnostic, "{mode}");
            assert_eq!(fs::read_to_string(&stdout).unwrap(), "");
        }
        if mode != "start" {
            assert_eq!(fs::read_to_string(calls).unwrap(), "x", "{mode}");
        }
    }
}
