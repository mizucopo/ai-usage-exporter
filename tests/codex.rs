#![cfg(unix)]

use std::{process::Command, time::Duration};

use ai_usage_exporter::codex::CodexClient;
use serde_json::json;

fn client(script: &str, timeout: f64) -> CodexClient {
    CodexClient::with_command(
        Duration::from_secs_f64(timeout),
        vec!["sh".into(), "-c".into(), script.into()],
    )
}

const INITIALIZE: &str = r#"
read -r request
case "$request" in *'"method":"initialize"'*) ;; *) exit 1 ;; esac
case "$request" in *'"name":"ai_usage_exporter"'*) ;; *) exit 1 ;; esac
printf '%s\n' '{"id":1,"result":{}}'
read -r notification
[ "$notification" = '{"method":"initialized"}' ] || exit 1
read -r request
case "$request" in *'"method":"account/rateLimits/read"'*) ;; *) exit 1 ;; esac
"#;

#[tokio::test]
async fn initializes_then_reads_limits_ignoring_notifications_and_other_ids() {
    let script = format!(
        "{INITIALIZE}\nprintf '%s\\n' '{{\"method\":\"notice\"}}' '{{\"id\":999,\"result\":{{}}}}' '{{\"id\":2,\"result\":{{\"rateLimits\":{{\"primary\":{{\"usedPercent\":25}}}}}}}}'"
    );
    let result = client(&script, 5.0).read_limits().await.unwrap();
    assert_eq!(result, json!({"rateLimits":{"primary":{"usedPercent":25}}}));
}

#[tokio::test]
async fn invalid_responses_are_sanitized() {
    for response in [
        "secret-not-json",
        r#"{"id":1,"error":{"message":"secret-token"}}"#,
        r#"{"id":1,"result":"secret-token"}"#,
        r#"["secret-token"]"#,
        r#"{"id":1}"#,
    ] {
        let script = format!("read -r request; printf '%s\\n' '{response}'");
        let error = client(&script, 5.0).read_limits().await.unwrap_err();
        assert!(!error.to_string().contains("secret"));
    }
}

#[tokio::test]
async fn bounds_response_buffer_without_newline() {
    let script =
        "read -r request; i=0; while [ $i -lt 17000 ]; do printf '%064d' 0; i=$((i+1)); done";
    let error = client(script, 10.0).read_limits().await.unwrap_err();
    assert_eq!(error.to_string(), "Codex response is too large");
}

#[tokio::test]
async fn handles_eof_nonzero_exit_and_start_failure() {
    for script in ["exit 0", "exit 7", "read -r request; exit 7"] {
        assert!(client(script, 5.0).read_limits().await.is_err());
    }
    let missing = CodexClient::with_command(
        Duration::from_secs(1),
        vec!["/secret-nonexistent-codex".into()],
    );
    assert!(
        !missing
            .read_limits()
            .await
            .unwrap_err()
            .to_string()
            .contains("secret")
    );
}

#[tokio::test]
async fn deadline_covers_initialization_and_limits_request_together() {
    let script = "read -r request; sleep 0.15; printf '%s\\n' '{\"id\":1,\"result\":{}}'; read -r note; read -r request; sleep 0.15; printf '%s\\n' '{\"id\":2,\"result\":{}}'";
    let error = client(script, 0.25).read_limits().await.unwrap_err();
    assert_eq!(error.to_string(), "Codex request timed out");
}

#[tokio::test]
async fn reaps_child_after_success_error_and_timeout() {
    for outcome in ["success", "error", "timeout"] {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("child.pid");
        let response = match outcome {
            "success" => "printf '%s\\n' '{\"id\":2,\"result\":{}}'",
            "error" => "printf '%s\\n' '{\"id\":2,\"error\":{}}'",
            _ => "",
        };
        let script = format!(
            "printf '%s' $$ > '{}'; {INITIALIZE}\n{response}\nwhile :; do :; done",
            pid_file.display()
        );
        let result = client(&script, 0.5).read_limits().await;
        assert_eq!(result.is_ok(), outcome == "success");
        let pid = std::fs::read_to_string(pid_file).unwrap();
        assert!(
            !Command::new("kill")
                .args(["-0", &pid])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        );
    }
}

#[tokio::test]
async fn cancelling_a_fetch_terminates_and_reaps_its_child() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("child.pid");
    let script = format!(
        "printf '%s' $$ > '{}'; read -r request; while :; do :; done",
        pid_file.display()
    );
    let client = client(&script, 30.0);
    let fetch = tokio::spawn(async move { client.read_limits().await });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !pid_file.exists() {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let pid = std::fs::read_to_string(pid_file).unwrap();
    fetch.abort();
    assert!(fetch.await.unwrap_err().is_cancelled());
    loop {
        let alive = Command::new("kill")
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        if !alive {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "cancelled child was not reaped"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
