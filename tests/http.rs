#![cfg(unix)]

use std::{sync::Arc, time::Duration};

use ai_usage_exporter::{codex::CodexClient, metrics::UsageCollector, server};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use tower::ServiceExt;

#[tokio::test]
async fn concurrent_http_scrapes_share_one_fetch_and_content_type() {
    let directory = tempfile::tempdir().unwrap();
    let counter = directory.path().join("fetches");
    let script = format!(
        "printf x >> '{}'; read -r request; printf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'; read -r note; read -r request; sleep 0.05; printf '%s\\n' '{{\"id\":2,\"result\":{{\"rateLimits\":{{\"primary\":{{\"usedPercent\":25,\"windowDurationMins\":300,\"resetsAt\":2000}}}}}}}}'",
        counter.display()
    );
    let router = server::router(
        Arc::new(UsageCollector::with_clocks(
            Duration::from_secs(300),
            || 1000.0,
            || 0.0,
        )),
        CodexClient::with_command(
            Duration::from_secs(5),
            vec!["sh".into(), "-c".into(), script],
        ),
    );
    let mut tasks = Vec::new();
    for _ in 0..6 {
        let router = router.clone();
        tasks.push(tokio::spawn(async move {
            let response = router
                .oneshot(
                    Request::builder()
                        .uri("/metrics")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(
                response.headers()["Content-Type"],
                "text/plain; version=0.0.4; charset=utf-8"
            );
            let output = to_bytes(response.into_body(), 65536).await.unwrap();
            String::from_utf8(output.to_vec()).unwrap()
        }));
    }
    for task in tasks {
        let output = task.await.unwrap();
        assert!(output.contains("codex_rate_limit_remaining_ratio{window=\"5h\"} 0.75"));
        assert!(output.contains("ai_usage_exporter_scrape_success 1.0"));
    }
    assert_eq!(std::fs::read_to_string(counter).unwrap(), "x");
}

#[tokio::test]
async fn authentication_or_process_failure_is_a_successful_http_scrape_with_failure_metrics() {
    let router = server::router(
        Arc::new(UsageCollector::new(Duration::from_secs(300))),
        CodexClient::with_command(Duration::from_secs(1), vec!["/nonexistent-codex".into()]),
    );
    let response = router
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    let output = String::from_utf8(body.to_vec()).unwrap();
    assert!(output.contains("ai_usage_exporter_scrape_success 0.0"));
    assert!(output.contains("ai_usage_exporter_last_success_timestamp_seconds 0.0"));
    assert!(!output.contains("codex_rate_limit_"));
}
