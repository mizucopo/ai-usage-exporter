//! Environment settings and HTTP routing.

use std::{collections::HashMap, fmt, sync::Arc, time::Duration};

use axum::{Router, extract::State, http::header, routing::get};

use crate::{codex::CodexClient, metrics::UsageCollector};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub cache_ttl: Duration,
    pub fetch_timeout: Duration,
}

#[derive(Debug, PartialEq)]
pub struct SettingsError(&'static str);

impl fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for SettingsError {}

impl Settings {
    pub fn from_env(env: &HashMap<String, String>) -> Result<Self, SettingsError> {
        let host = env
            .get("AI_USAGE_EXPORTER_HOST")
            .cloned()
            .unwrap_or_else(|| "0.0.0.0".into());
        if host.trim().is_empty() {
            return Err(SettingsError("AI_USAGE_EXPORTER_HOST must not be empty"));
        }
        let port = env
            .get("AI_USAGE_EXPORTER_PORT")
            .map_or("9173", String::as_str)
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or(SettingsError(
                "AI_USAGE_EXPORTER_PORT must be between 1 and 65535",
            ))?;
        Ok(Self {
            host,
            port,
            cache_ttl: duration_setting(env, "AI_USAGE_EXPORTER_CACHE_TTL_SECONDS", "300")?,
            fetch_timeout: duration_setting(env, "AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS", "10")?,
        })
    }
}

fn duration_setting(
    env: &HashMap<String, String>,
    name: &'static str,
    default: &str,
) -> Result<Duration, SettingsError> {
    let seconds = env
        .get(name)
        .map_or(default, String::as_str)
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0);
    seconds
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        .filter(|duration| !duration.is_zero())
        .ok_or(SettingsError(match name {
            "AI_USAGE_EXPORTER_CACHE_TTL_SECONDS" => {
                "AI_USAGE_EXPORTER_CACHE_TTL_SECONDS must be positive, finite and representable"
            }
            _ => {
                "AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS must be positive, finite and representable"
            }
        }))
}

#[derive(Clone)]
struct AppState {
    collector: Arc<UsageCollector>,
    client: CodexClient,
}

pub fn router(collector: Arc<UsageCollector>, client: CodexClient) -> Router {
    Router::new()
        .route("/metrics", get(metrics))
        .with_state(AppState { collector, client })
}

async fn metrics(
    State(state): State<AppState>,
) -> ([(header::HeaderName, &'static str); 1], String) {
    let body = state
        .collector
        .collect_with(|| state.client.read_limits())
        .await;
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_configured_settings() {
        let defaults = Settings::from_env(&HashMap::new()).unwrap();
        assert_eq!(defaults.host, "0.0.0.0");
        assert_eq!(defaults.port, 9173);
        assert_eq!(defaults.cache_ttl, Duration::from_secs(300));
        assert_eq!(defaults.fetch_timeout, Duration::from_secs(10));
        let configured = Settings::from_env(&HashMap::from([
            ("AI_USAGE_EXPORTER_HOST".into(), "127.0.0.1".into()),
            ("AI_USAGE_EXPORTER_PORT".into(), "12345".into()),
            ("AI_USAGE_EXPORTER_CACHE_TTL_SECONDS".into(), "60".into()),
            (
                "AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS".into(),
                "5.5".into(),
            ),
        ]))
        .unwrap();
        assert_eq!(configured.host, "127.0.0.1");
        assert_eq!(configured.port, 12345);
        assert_eq!(configured.cache_ttl, Duration::from_secs(60));
        assert_eq!(configured.fetch_timeout, Duration::from_secs_f64(5.5));
    }

    #[test]
    fn rejects_invalid_settings_without_panicking() {
        for (name, values) in [
            ("HOST", vec!["", " \t"]),
            ("PORT", vec!["0", "65536", "1.5", "-1", "secret-invalid"]),
            (
                "CACHE_TTL_SECONDS",
                vec!["0", "-1", "inf", "nan", "1e300", "1e-300"],
            ),
            (
                "FETCH_TIMEOUT_SECONDS",
                vec!["0", "-1", "inf", "nan", "1e300", "1e-300"],
            ),
        ] {
            for value in values {
                let env = HashMap::from([(format!("AI_USAGE_EXPORTER_{name}"), value.into())]);
                let error = Settings::from_env(&env).unwrap_err().to_string();
                assert!(!error.contains("secret"));
            }
        }
    }
}
