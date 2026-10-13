#![cfg(unix)]

use std::{
    ffi::OsString,
    io::{Read, Write},
    net::TcpStream,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
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

#[test]
fn sigint_and_sigterm_drain_inflight_scrapes_and_reap_codex() {
    for signal in ["-INT", "-TERM"] {
        let directory = tempfile::tempdir().unwrap();
        let fake = directory.path().join("codex");
        let pid_file = directory.path().join("codex.pid");
        let codex_home = directory
            .path()
            .join(OsString::from_vec(b"fake-home-\xff".to_vec()));
        // read blocks on the quota request without spawning grandchildren.
        let script = format!(
            "#!/bin/sh\n[ \"$CODEX_HOME\" = \"$EXPECTED_CODEX_HOME\" ] || exit 1\nprintf '%s' $$ > '{}'\nread -r request\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\nread -r note\nread -r request\nread -r never\n",
            pid_file.display()
        );
        std::fs::write(&fake, script).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let child = Command::new(env!("CARGO_BIN_EXE_ai-usage-exporter"))
            .env_clear()
            .env("PATH", directory.path())
            .env("CODEX_HOME", &codex_home)
            .env("EXPECTED_CODEX_HOME", &codex_home)
            .env(
                OsString::from_vec(b"UNRELATED-\xff".to_vec()),
                OsString::from_vec(b"unused-\xff".to_vec()),
            )
            .env("AI_USAGE_EXPORTER_HOST", "127.0.0.1")
            .env("AI_USAGE_EXPORTER_PORT", address.port().to_string())
            .env("AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS", "0.5")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut exporter = Exporter(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            if let Ok(stream) = TcpStream::connect(address) {
                break stream;
            }
            assert!(Instant::now() < deadline, "exporter failed to listen");
            std::thread::sleep(Duration::from_millis(10));
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        while !pid_file.exists() {
            assert!(Instant::now() < deadline, "Codex did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            Command::new("kill")
                .args([signal, &exporter.0.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("ai_usage_exporter_scrape_success 0.0"));
        loop {
            if let Some(status) = exporter.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "exporter did not shut down");
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid = std::fs::read_to_string(pid_file).unwrap();
        assert!(
            !Command::new("kill")
                .args(["-0", &pid])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
    }
}

#[test]
fn invalid_configuration_exits_with_status_two() {
    for value in [OsString::from("0"), OsString::from_vec(vec![0xff])] {
        let child = Command::new(env!("CARGO_BIN_EXE_ai-usage-exporter"))
            .env_clear()
            .env("AI_USAGE_EXPORTER_PORT", value)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut exporter = Exporter(child);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = exporter.0.try_wait().unwrap() {
                assert_eq!(status.code(), Some(2));
                break;
            }
            assert!(Instant::now() < deadline, "invalid settings were accepted");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
