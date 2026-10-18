//! Environment settings and HTTP routing.

use std::{collections::HashMap, fmt, io::Write, sync::Arc};

use axum::{Router, extract::State, http::header, routing::get};

use crate::{codex::CodexClient, metrics::UsageCollector};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub cache_ttl: f64,
    pub fetch_timeout: f64,
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
    pub fn from_process_env() -> Result<Self, SettingsError> {
        let mut env = HashMap::new();
        // Leave unrelated environment bytes (including CODEX_HOME) untouched.
        for name in [
            "AI_USAGE_EXPORTER_HOST",
            "AI_USAGE_EXPORTER_PORT",
            "AI_USAGE_EXPORTER_CACHE_TTL_SECONDS",
            "AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS",
        ] {
            match std::env::var(name) {
                Ok(value) => {
                    env.insert(name.into(), value);
                }
                Err(std::env::VarError::NotPresent) => {}
                Err(std::env::VarError::NotUnicode(_)) => {
                    return Err(SettingsError(
                        "Exporter settings must contain valid UTF-8 text",
                    ));
                }
            }
        }
        Self::from_env(&env)
    }

    pub fn from_env(env: &HashMap<String, String>) -> Result<Self, SettingsError> {
        let host = env
            .get("AI_USAGE_EXPORTER_HOST")
            .cloned()
            .unwrap_or_else(|| "0.0.0.0".into());
        if host
            .chars()
            .all(|value| value.is_whitespace() || matches!(value, '\u{1c}'..='\u{1f}'))
        {
            return Err(SettingsError("AI_USAGE_EXPORTER_HOST must not be empty"));
        }
        let port = crate::python_number::port(
            env.get("AI_USAGE_EXPORTER_PORT")
                .map_or("9173", String::as_str),
        )
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
) -> Result<f64, SettingsError> {
    let seconds =
        crate::python_number::positive_float(env.get(name).map_or(default, String::as_str));
    seconds.ok_or(SettingsError(match name {
        "AI_USAGE_EXPORTER_CACHE_TTL_SECONDS" => {
            "AI_USAGE_EXPORTER_CACHE_TTL_SECONDS must be positive and finite"
        }
        _ => "AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS must be positive and finite",
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
        .collect_with(|| async {
            state.client.read_limits().await.inspect_err(|error| {
                // CodexError is publicly constructible; never print its payload.
                let reason = match error.0 {
                    "Codex command is empty" | "Codex process could not start" => "process_start",
                    "Codex request timed out" => "timeout",
                    "Codex response is invalid"
                    | "Codex response is not an object"
                    | "Codex response has no result object"
                    | "Codex response is too large" => "invalid_response",
                    "Codex rejected the request" => "request_rejected",
                    _ => "request_failed",
                };
                // A failed log sink must not interrupt the scrape or cache update.
                let _ = writeln!(std::io::stderr(), "Codex quota fetch failed: {reason}");
            })
        })
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
        assert_eq!(defaults.cache_ttl, 300.0);
        assert_eq!(defaults.fetch_timeout, 10.0);
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
        assert_eq!(configured.cache_ttl, 60.0);
        assert_eq!(configured.fetch_timeout, 5.5);
    }

    #[test]
    fn accepts_every_positive_finite_duration_like_the_python_settings() {
        for value in ["1e-300", "1e300"] {
            let settings = Settings::from_env(&HashMap::from([
                ("AI_USAGE_EXPORTER_CACHE_TTL_SECONDS".into(), value.into()),
                (
                    "AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS".into(),
                    value.into(),
                ),
            ]))
            .unwrap();
            assert_eq!(settings.cache_ttl, value.parse::<f64>().unwrap());
            assert_eq!(settings.fetch_timeout, value.parse::<f64>().unwrap());
        }
    }

    #[test]
    fn legacy_numeric_syntax_is_preserved_for_every_numeric_setting() {
        let settings = Settings::from_env(&HashMap::from([
            ("AI_USAGE_EXPORTER_PORT".into(), " +٩_١٧٣ ".into()),
            (
                "AI_USAGE_EXPORTER_CACHE_TTL_SECONDS".into(),
                "１_０００".into(),
            ),
            (
                "AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS".into(),
                "\u{a0}+١_٠.٥e-١\u{85}".into(),
            ),
        ]))
        .unwrap();
        assert_eq!(settings.port, 9173);
        assert_eq!(settings.cache_ttl, 1000.0);
        assert_eq!(settings.fetch_timeout, 1.05);
    }

    #[test]
    fn rejects_invalid_settings_without_panicking() {
        for (name, values) in [
            ("HOST", vec!["", " \t", "\u{1c}\u{1f}"]),
            ("PORT", vec!["0", "65536", "1.5", "-1", "secret-invalid"]),
            ("CACHE_TTL_SECONDS", vec!["0", "-1", "inf", "nan"]),
            ("FETCH_TIMEOUT_SECONDS", vec!["0", "-1", "inf", "nan"]),
        ] {
            for value in values {
                let env = HashMap::from([(format!("AI_USAGE_EXPORTER_{name}"), value.into())]);
                let error = Settings::from_env(&env).unwrap_err().to_string();
                assert!(!error.contains("secret"));
            }
        }
    }
}
