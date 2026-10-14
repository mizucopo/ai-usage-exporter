use std::{process::ExitCode, sync::Arc};

use ai_usage_exporter::{codex::CodexClient, metrics::UsageCollector, server};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--help" | "-h") => {
            println!(
                "ai-usage-exporter {}\n\nUsage: ai-usage-exporter [--help | --version]\n\nServes Prometheus metrics at /metrics. Configure with:\n  AI_USAGE_EXPORTER_HOST (default: 0.0.0.0)\n  AI_USAGE_EXPORTER_PORT (default: 9173)\n  AI_USAGE_EXPORTER_CACHE_TTL_SECONDS (default: 300)\n  AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS (default: 10)\n  CODEX_HOME (Codex CLI authentication directory)\n\nRequires a separately installed Codex CLI on PATH. Stop with Ctrl+C.",
                env!("CARGO_PKG_VERSION")
            );
            return ExitCode::SUCCESS;
        }
        Some("--version" | "-V") => {
            println!("ai-usage-exporter {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Cannot start exporter: {error}");
            ExitCode::from(2)
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let settings = server::Settings::from_process_env()?;
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(windows)]
    let mut terminate = tokio::signal::windows::ctrl_break()?;
    let listener = TcpListener::bind((settings.host.as_str(), settings.port)).await?;
    let router = server::router(
        Arc::new(UsageCollector::with_cache_ttl_seconds(settings.cache_ttl)),
        CodexClient::with_timeout_seconds(settings.fetch_timeout),
    );
    let shutdown = async move {
        #[cfg(unix)]
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
        #[cfg(windows)]
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
        #[cfg(not(any(unix, windows)))]
        let _ = tokio::signal::ctrl_c().await;
    };
    // Drain bounded scrapes before exiting so their Codex children are reaped.
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}
