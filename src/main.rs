use std::{process::ExitCode, sync::Arc};

use ai_usage_exporter::{codex::CodexClient, metrics::UsageCollector, server};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Cannot start exporter: {error}");
            ExitCode::from(2)
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let settings = server::Settings::from_env(&std::env::vars().collect())?;
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
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
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
    };
    // Drain bounded scrapes before exiting so their Codex children are reaped.
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}
