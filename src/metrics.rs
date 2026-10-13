//! Validate account quotas and render fresh Prometheus samples for every scrape.

use std::fmt::{self, Write};
use std::future::Future;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use tokio::sync::Mutex;

#[derive(Clone, Debug, PartialEq)]
pub struct QuotaWindow {
    pub label: &'static str,
    pub remaining: f64,
    pub resets_at: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaError {
    ExpectedObject,
    ExpectedNumber,
    InvalidNumber,
    UnexpectedBucket,
    UnsupportedOrExpiredWindow,
    ExpectedDistinctWindows,
}

impl fmt::Display for QuotaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ExpectedObject => "Expected quota object",
            Self::ExpectedNumber => "Expected quota number",
            Self::InvalidNumber => "Invalid quota number",
            Self::UnexpectedBucket => "Unexpected quota bucket",
            Self::UnsupportedOrExpiredWindow => "Unsupported or expired quota window",
            Self::ExpectedDistinctWindows => "Expected distinct quota windows",
        })
    }
}

impl std::error::Error for QuotaError {}

fn object(value: &Value) -> Result<&Map<String, Value>, QuotaError> {
    value.as_object().ok_or(QuotaError::ExpectedObject)
}

fn number(value: Option<&Value>) -> Result<f64, QuotaError> {
    let value = value
        .and_then(Value::as_f64)
        .ok_or(QuotaError::ExpectedNumber)?;
    if !value.is_finite() || value < 0.0 {
        return Err(QuotaError::InvalidNumber);
    }
    Ok(value)
}

/// Prefer the Codex bucket and identify periods by duration, including legacy data.
///
/// A malformed selected bucket or window invalidates the whole response; missing
/// and null windows are omitted rather than reported as zero quota.
pub fn parse_limits(result: &Value, now: f64) -> Result<Vec<QuotaWindow>, QuotaError> {
    let result = object(result)?;
    let selected = match result.get("rateLimitsByLimitId") {
        None | Some(Value::Null) => None,
        Some(buckets) => object(buckets)?.get("codex").filter(|v| !v.is_null()),
    }
    .or_else(|| result.get("rateLimits"))
    .ok_or(QuotaError::ExpectedObject)?;
    let bucket = object(selected)?;
    match bucket.get("limitId") {
        None | Some(Value::Null) => {}
        Some(Value::String(id)) if id == "codex" => {}
        Some(_) => return Err(QuotaError::UnexpectedBucket),
    }

    let mut windows = Vec::with_capacity(2);
    for key in ["primary", "secondary"] {
        let window = match bucket.get(key) {
            None | Some(Value::Null) => continue,
            Some(window) => object(window)?,
        };
        let duration = number(window.get("windowDurationMins"))?;
        let label = match duration {
            300.0 => "5h",
            10080.0 => "weekly",
            _ => return Err(QuotaError::UnsupportedOrExpiredWindow),
        };
        let resets_at = number(window.get("resetsAt"))?;
        if resets_at <= now {
            return Err(QuotaError::UnsupportedOrExpiredWindow);
        }
        windows.push(QuotaWindow {
            label,
            remaining: ((100.0 - number(window.get("usedPercent"))?) / 100.0).max(0.0),
            resets_at,
        });
    }
    if windows.is_empty() || (windows.len() == 2 && windows[0].label == windows[1].label) {
        return Err(QuotaError::ExpectedDistinctWindows);
    }
    Ok(windows)
}

struct CachedUsage {
    windows: Vec<QuotaWindow>,
    expires: f64,
    last_success: f64,
}

/// Cache upstream results, including failures, while keeping countdowns current.
///
/// The async lock covers the upstream fetch so simultaneous scrapes share a
/// refresh. Fetching starts only when `collect_with` is called. Fetch errors are
/// deliberately discarded so private upstream details cannot reach metrics.
pub struct UsageCollector {
    cache_ttl: f64,
    wall_time: Box<dyn Fn() -> f64 + Send + Sync>,
    monotonic: Box<dyn Fn() -> f64 + Send + Sync>,
    cache: Mutex<CachedUsage>,
}

impl Default for UsageCollector {
    fn default() -> Self {
        Self::new(Duration::from_secs(300))
    }
}

impl UsageCollector {
    pub fn new(cache_ttl: Duration) -> Self {
        Self::with_cache_ttl_seconds(cache_ttl.as_secs_f64())
    }

    /// Preserve the configured seconds without Duration range or precision limits.
    pub fn with_cache_ttl_seconds(cache_ttl: f64) -> Self {
        let started = Instant::now();
        Self::with_clocks_seconds(
            cache_ttl,
            || {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0.0, |duration| duration.as_secs_f64())
            },
            move || started.elapsed().as_secs_f64(),
        )
    }

    /// Inject clocks for deterministic compatibility tests and clock-boundary cases.
    pub fn with_clocks(
        cache_ttl: Duration,
        wall_time: impl Fn() -> f64 + Send + Sync + 'static,
        monotonic: impl Fn() -> f64 + Send + Sync + 'static,
    ) -> Self {
        Self::with_clocks_seconds(cache_ttl.as_secs_f64(), wall_time, monotonic)
    }

    /// Inject clocks while retaining the legacy positive finite seconds contract.
    pub fn with_clocks_seconds(
        cache_ttl: f64,
        wall_time: impl Fn() -> f64 + Send + Sync + 'static,
        monotonic: impl Fn() -> f64 + Send + Sync + 'static,
    ) -> Self {
        Self {
            cache_ttl,
            wall_time: Box::new(wall_time),
            monotonic: Box::new(monotonic),
            cache: Mutex::new(CachedUsage {
                windows: Vec::new(),
                expires: f64::NEG_INFINITY,
                last_success: 0.0,
            }),
        }
    }

    pub async fn collect_with<F, Fut, E>(&self, fetch: F) -> String
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Value, E>>,
    {
        let (windows, last_success) = {
            let mut cache = self.cache.lock().await;
            if (self.monotonic)() >= cache.expires
                || cache
                    .windows
                    .iter()
                    .any(|window| window.resets_at <= (self.wall_time)())
            {
                cache.windows = match fetch().await {
                    Ok(result) => match parse_limits(&result, (self.wall_time)()) {
                        Ok(windows) => {
                            cache.last_success = (self.wall_time)();
                            windows
                        }
                        Err(_) => Vec::new(),
                    },
                    Err(_) => Vec::new(),
                };
                cache.expires = (self.monotonic)() + self.cache_ttl;
                if let Some(next_reset) = cache
                    .windows
                    .iter()
                    .map(|window| window.resets_at)
                    .reduce(f64::min)
                {
                    let until_reset = next_reset - (self.wall_time)();
                    cache.expires = cache.expires.min((self.monotonic)() + until_reset);
                }
            }
            (cache.windows.clone(), cache.last_success)
        };
        render_metrics(&windows, last_success, (self.wall_time)())
    }
}

fn gauge_header(output: &mut String, name: &str, help: &str) {
    // String writes are infallible.
    writeln!(output, "# HELP {name} {help}\n# TYPE {name} gauge").unwrap();
}

fn metric_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

fn render_metrics(windows: &[QuotaWindow], last_success: f64, now: f64) -> String {
    let mut output = String::new();
    gauge_header(
        &mut output,
        "ai_usage_exporter_scrape_success",
        "Latest quota fetch succeeded.",
    );
    writeln!(
        output,
        "ai_usage_exporter_scrape_success {}",
        if windows.is_empty() { "0.0" } else { "1.0" }
    )
    .unwrap();
    gauge_header(
        &mut output,
        "ai_usage_exporter_last_success_timestamp_seconds",
        "Unix time of the last successful quota fetch, or zero.",
    );
    writeln!(
        output,
        "ai_usage_exporter_last_success_timestamp_seconds {}",
        metric_number(last_success)
    )
    .unwrap();
    if windows.is_empty() {
        return output;
    }
    for (name, help) in [
        (
            "codex_rate_limit_remaining_ratio",
            "Quota remaining (0 to 1).",
        ),
        (
            "codex_rate_limit_reset_timestamp_seconds",
            "Next quota reset Unix time.",
        ),
        (
            "codex_rate_limit_reset_seconds",
            "Seconds until the next quota reset.",
        ),
    ] {
        gauge_header(&mut output, name, help);
        for window in windows {
            let value = match name {
                "codex_rate_limit_remaining_ratio" => window.remaining,
                "codex_rate_limit_reset_timestamp_seconds" => window.resets_at,
                _ => (window.resets_at - now).max(0.0),
            };
            writeln!(
                output,
                "{name}{{window=\"{}\"}} {}",
                window.label,
                metric_number(value)
            )
            .unwrap();
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    };

    use serde_json::json;
    use tokio::sync::Notify;

    use super::*;

    #[derive(Clone)]
    struct Clock(Arc<StdMutex<(f64, f64)>>);

    impl Clock {
        fn new(wall: f64) -> Self {
            Self(Arc::new(StdMutex::new((wall, 0.0))))
        }

        fn advance(&self, seconds: f64) {
            let mut clock = self.0.lock().unwrap();
            clock.0 += seconds;
            clock.1 += seconds;
        }

        fn collector(&self, ttl: u64) -> UsageCollector {
            let wall = self.clone();
            let mono = self.clone();
            UsageCollector::with_clocks(
                Duration::from_secs(ttl),
                move || wall.0.lock().unwrap().0,
                move || mono.0.lock().unwrap().1,
            )
        }
    }

    fn limits() -> Value {
        json!({"rateLimitsByLimitId": {"codex": {
            "primary": {"usedPercent": 25, "windowDurationMins": 300, "resetsAt": 2000},
            "secondary": {"usedPercent": 80, "windowDurationMins": 10080, "resetsAt": 10000}
        }}})
    }

    fn legacy_bucket() -> Value {
        json!({
            "limitId": "codex",
            "primary": {"usedPercent": 0, "windowDurationMins": 300, "resetsAt": 2000},
            "secondary": {"usedPercent": 100, "windowDurationMins": 10080, "resetsAt": 10000}
        })
    }

    async fn scrape(collector: &UsageCollector, result: Value) -> String {
        collector
            .collect_with(|| async { Ok::<_, &str>(result) })
            .await
    }

    fn assert_failed(output: &str) {
        assert!(!output.contains("codex_rate_limit_"), "{output}");
        assert!(output.contains("ai_usage_exporter_scrape_success 0.0"));
    }

    #[tokio::test]
    async fn exports_both_quotas_with_exact_help_types_and_labels() {
        let collector = Clock::new(1000.0).collector(300);
        let output = scrape(&collector, limits()).await;
        assert_eq!(
            output,
            concat!(
                "# HELP ai_usage_exporter_scrape_success Latest quota fetch succeeded.\n",
                "# TYPE ai_usage_exporter_scrape_success gauge\n",
                "ai_usage_exporter_scrape_success 1.0\n",
                "# HELP ai_usage_exporter_last_success_timestamp_seconds Unix time of the last successful quota fetch, or zero.\n",
                "# TYPE ai_usage_exporter_last_success_timestamp_seconds gauge\n",
                "ai_usage_exporter_last_success_timestamp_seconds 1000.0\n",
                "# HELP codex_rate_limit_remaining_ratio Quota remaining (0 to 1).\n",
                "# TYPE codex_rate_limit_remaining_ratio gauge\n",
                "codex_rate_limit_remaining_ratio{window=\"5h\"} 0.75\n",
                "codex_rate_limit_remaining_ratio{window=\"weekly\"} 0.2\n",
                "# HELP codex_rate_limit_reset_timestamp_seconds Next quota reset Unix time.\n",
                "# TYPE codex_rate_limit_reset_timestamp_seconds gauge\n",
                "codex_rate_limit_reset_timestamp_seconds{window=\"5h\"} 2000.0\n",
                "codex_rate_limit_reset_timestamp_seconds{window=\"weekly\"} 10000.0\n",
                "# HELP codex_rate_limit_reset_seconds Seconds until the next quota reset.\n",
                "# TYPE codex_rate_limit_reset_seconds gauge\n",
                "codex_rate_limit_reset_seconds{window=\"5h\"} 1000.0\n",
                "codex_rate_limit_reset_seconds{window=\"weekly\"} 9000.0\n",
            )
        );
    }

    #[tokio::test]
    async fn exports_only_returned_quota_for_either_slot_and_response_schema() {
        for legacy in [false, true] {
            for slot in ["primary", "secondary"] {
                for missing in [false, true] {
                    for (duration, label, absent) in
                        [(300, "5h", "weekly"), (10080, "weekly", "5h")]
                    {
                        let mut bucket = if missing {
                            json!({})
                        } else {
                            json!({"primary": null, "secondary": null})
                        };
                        bucket[slot] = json!({"usedPercent": 69, "windowDurationMins": duration, "resetsAt": 2000});
                        let result = if legacy {
                            json!({"rateLimits": bucket})
                        } else {
                            json!({"rateLimitsByLimitId": {"codex": bucket}})
                        };
                        let output = scrape(&Clock::new(1000.0).collector(300), result).await;
                        assert!(output.contains(&format!(
                            "codex_rate_limit_remaining_ratio{{window=\"{label}\"}} 0.31"
                        )));
                        assert!(output.contains(&format!(
                            "codex_rate_limit_reset_timestamp_seconds{{window=\"{label}\"}} 2000.0"
                        )));
                        assert!(output.contains(&format!(
                            "codex_rate_limit_reset_seconds{{window=\"{label}\"}} 1000.0"
                        )));
                        assert!(!output.contains(&format!("window=\"{absent}\"")));
                        assert!(output.contains("ai_usage_exporter_scrape_success 1.0"));
                        assert!(
                            output.contains(
                                "ai_usage_exporter_last_success_timestamp_seconds 1000.0"
                            )
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn periods_are_identified_by_duration_and_overuse_is_clamped() {
        let bucket = json!({
            "limitId": "codex",
            "primary": {"usedPercent": 150, "windowDurationMins": 10080, "resetsAt": 10000},
            "secondary": {"usedPercent": 0, "windowDurationMins": 300, "resetsAt": 2000}
        });
        for result in [
            json!({"rateLimits": bucket}),
            json!({"rateLimitsByLimitId": {"codex": bucket}}),
        ] {
            let windows = parse_limits(&result, 1000.0).unwrap();
            assert_eq!(
                windows[0],
                QuotaWindow {
                    label: "weekly",
                    remaining: 0.0,
                    resets_at: 10000.0
                }
            );
            assert_eq!(
                windows[1],
                QuotaWindow {
                    label: "5h",
                    remaining: 1.0,
                    resets_at: 2000.0
                }
            );
        }
    }

    #[tokio::test]
    async fn cache_reuses_quotas_but_countdown_advances_until_ttl_boundary() {
        let clock = Clock::new(1000.0);
        let collector = clock.collector(300);
        let requests = AtomicUsize::new(0);
        let fetch = || async {
            requests.fetch_add(1, Ordering::SeqCst);
            Ok::<_, &str>(limits())
        };
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        collector.collect_with(fetch).await;
        clock.advance(299.0);
        let output = collector.collect_with(fetch).await;
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert!(output.contains("codex_rate_limit_reset_seconds{window=\"5h\"} 701.0"));
        assert!(output.contains("ai_usage_exporter_last_success_timestamp_seconds 1000.0"));
        clock.advance(1.0);
        collector.collect_with(fetch).await;
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn failed_refresh_removes_old_samples_preserves_success_and_caches_failure() {
        let clock = Clock::new(1000.0);
        let collector = clock.collector(60);
        let requests = AtomicUsize::new(0);
        let fetch = || async {
            if requests.fetch_add(1, Ordering::SeqCst) == 1 {
                Err("Private upstream error")
            } else {
                Ok(limits())
            }
        };
        collector.collect_with(fetch).await;
        clock.advance(60.0);
        let output = collector.collect_with(fetch).await;
        assert_failed(&output);
        assert!(output.contains("ai_usage_exporter_last_success_timestamp_seconds 1000.0"));
        assert!(!output.contains("Private"));
        clock.advance(59.0);
        assert_eq!(collector.collect_with(fetch).await, output);
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        clock.advance(1.0);
        assert!(
            collector
                .collect_with(fetch)
                .await
                .contains("ai_usage_exporter_scrape_success 1.0")
        );
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn refresh_to_one_quota_removes_absent_samples_and_preserves_cache() {
        let clock = Clock::new(1000.0);
        let collector = clock.collector(6000);
        let bucket = StdMutex::new(legacy_bucket());
        let requests = AtomicUsize::new(0);
        let fetch = || async {
            if requests.fetch_add(1, Ordering::SeqCst) == 2 {
                Err("Authentication failed")
            } else {
                Ok(json!({"rateLimits": *bucket.lock().unwrap()}))
            }
        };
        assert!(
            collector
                .collect_with(fetch)
                .await
                .contains("window=\"5h\"")
        );
        clock.advance(1000.0);
        bucket.lock().unwrap()["primary"] = Value::Null;
        let output = collector.collect_with(fetch).await;
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert!(!output.contains("window=\"5h\""));
        assert!(output.contains("codex_rate_limit_reset_seconds{window=\"weekly\"} 8000.0"));
        assert!(output.contains("ai_usage_exporter_scrape_success 1.0"));
        clock.advance(1000.0);
        let output = collector.collect_with(fetch).await;
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert!(output.contains("codex_rate_limit_reset_seconds{window=\"weekly\"} 7000.0"));
        assert!(output.contains("ai_usage_exporter_last_success_timestamp_seconds 2000.0"));
        clock.advance(5000.0);
        let output = collector.collect_with(fetch).await;
        assert_eq!(requests.load(Ordering::SeqCst), 3);
        assert_failed(&output);
        assert!(output.contains("ai_usage_exporter_last_success_timestamp_seconds 2000.0"));
        clock.advance(1.0);
        assert_eq!(collector.collect_with(fetch).await, output);
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn malformed_window_is_not_treated_as_absent() {
        for invalid in [
            json!({}),
            json!([]),
            json!("invalid"),
            json!(false),
            json!(0),
        ] {
            let mut bucket = legacy_bucket();
            bucket["primary"] = invalid;
            let output = scrape(
                &Clock::new(1000.0).collector(300),
                json!({"rateLimits": bucket}),
            )
            .await;
            assert_failed(&output);
        }
    }

    #[test]
    fn legacy_fallback_only_applies_when_codex_bucket_is_missing_or_null() {
        for buckets in [
            Value::Null,
            json!({}),
            json!({"other": {}}),
            json!({"codex": null}),
        ] {
            let result = json!({"rateLimitsByLimitId": buckets, "rateLimits": legacy_bucket()});
            let windows = parse_limits(&result, 1000.0).unwrap();
            assert_eq!(windows[0].remaining, 1.0);
            assert_eq!(windows[1].remaining, 0.0);
        }
        let mut preferred = limits();
        preferred["rateLimits"] = legacy_bucket();
        assert_eq!(parse_limits(&preferred, 1000.0).unwrap()[0].remaining, 0.75);
        for bucket in [json!({}), json!({"primary": null}), json!("invalid")] {
            let result =
                json!({"rateLimitsByLimitId": {"codex": bucket}, "rateLimits": legacy_bucket()});
            assert!(parse_limits(&result, 1000.0).is_err());
        }
    }

    #[test]
    fn rejects_different_limit_id_including_legacy_fallback() {
        for id in [json!("other"), json!(false), json!(0), json!({})] {
            let mut bucket = legacy_bucket();
            bucket["limitId"] = id;
            let result = json!({"rateLimitsByLimitId": {}, "rateLimits": bucket});
            assert_eq!(
                parse_limits(&result, 1000.0),
                Err(QuotaError::UnexpectedBucket)
            );
        }
    }

    #[tokio::test]
    async fn invalid_or_expired_windows_omit_every_quota_value() {
        for (field, value) in [
            ("usedPercent", Value::Null),
            ("usedPercent", json!(true)),
            ("usedPercent", json!(-1)),
            ("usedPercent", json!("25")),
            ("windowDurationMins", json!(15)),
            ("windowDurationMins", json!(10080)),
            ("windowDurationMins", json!(false)),
            ("windowDurationMins", json!(-300)),
            ("resetsAt", Value::Null),
            ("resetsAt", json!(999)),
            ("resetsAt", json!(1000)),
            ("resetsAt", json!(true)),
            ("resetsAt", json!(-1)),
        ] {
            let mut result = limits();
            result["rateLimitsByLimitId"]["codex"]["primary"][field] = value;
            let output = scrape(&Clock::new(1000.0).collector(300), result).await;
            assert_failed(&output);
            assert!(output.contains("ai_usage_exporter_last_success_timestamp_seconds 0.0"));
        }
    }

    #[tokio::test]
    async fn missing_limits_are_not_reported_as_zero_quota() {
        for result in [
            json!({}),
            json!({"rateLimits": null}),
            json!({"rateLimits": {}}),
            json!({"rateLimitsByLimitId": {"other": {}}}),
            json!({"rateLimitsByLimitId": []}),
            json!({"rateLimits": {"primary": null, "secondary": null}}),
            json!([]),
            json!(false),
        ] {
            assert_failed(&scrape(&Clock::new(1000.0).collector(300), result).await);
        }
    }

    #[tokio::test]
    async fn reset_expires_cache_early_without_refetch_loop() {
        let clock = Clock::new(1990.0);
        let collector = clock.collector(300);
        let requests = AtomicUsize::new(0);
        let fetch = || async {
            requests.fetch_add(1, Ordering::SeqCst);
            Ok::<_, &str>(limits())
        };
        assert!(
            collector
                .collect_with(fetch)
                .await
                .contains("codex_rate_limit_reset_seconds{window=\"5h\"} 10.0")
        );
        clock.advance(10.0);
        assert_failed(&collector.collect_with(fetch).await);
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        clock.advance(299.0);
        collector.collect_with(fetch).await;
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        clock.advance(1.0);
        collector.collect_with(fetch).await;
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn wall_clock_reset_invalidates_cache_even_before_monotonic_ttl() {
        let clock = Clock::new(1000.0);
        let collector = clock.collector(300);
        scrape(&collector, limits()).await;
        clock.0.lock().unwrap().0 = 2000.0;
        assert_failed(&scrape(&collector, limits()).await);
    }

    #[tokio::test]
    async fn validates_quotas_after_upstream_fetch_completes() {
        let clock = Clock::new(1990.0);
        let collector = clock.collector(300);
        let output = collector
            .collect_with(|| async {
                clock.advance(10.0);
                Ok::<_, &str>(limits())
            })
            .await;
        assert_failed(&output);
        assert!(output.contains("ai_usage_exporter_last_success_timestamp_seconds 0.0"));
    }

    #[tokio::test]
    async fn maximum_duration_does_not_overflow_cache_deadline() {
        let collector = UsageCollector::with_clocks(Duration::MAX, || 1000.0, || 0.0);
        assert_failed(
            &collector
                .collect_with(|| async { Err::<Value, _>("Upstream failed") })
                .await,
        );
        assert!(collector.cache.lock().await.expires.is_finite());
        // Even an enormous TTL still caches failures without invoking fetch again.
        let requests = AtomicUsize::new(0);
        assert_failed(
            &collector
                .collect_with(|| async {
                    requests.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, &str>(limits())
                })
                .await,
        );
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn huge_finite_seconds_cache_failures_beyond_duration_range() {
        let clock = Clock::new(1000.0);
        let mono = clock.clone();
        let collector =
            UsageCollector::with_clocks_seconds(1e300, || 1000.0, move || mono.0.lock().unwrap().1);
        let requests = AtomicUsize::new(0);
        let fetch = || async {
            requests.fetch_add(1, Ordering::SeqCst);
            Err::<Value, _>("Upstream failed")
        };
        let first = collector.collect_with(fetch).await;
        assert_failed(&first);
        clock.advance(1.0);
        assert_eq!(collector.collect_with(fetch).await, first);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn tiny_positive_seconds_keep_precision_and_expire_after_clock_advances() {
        let clock = Clock::new(1000.0);
        let mono = clock.clone();
        let collector = UsageCollector::with_clocks_seconds(
            1e-300,
            || 1000.0,
            move || mono.0.lock().unwrap().1,
        );
        let requests = AtomicUsize::new(0);
        let fetch = || async {
            requests.fetch_add(1, Ordering::SeqCst);
            Ok::<_, &str>(limits())
        };
        collector.collect_with(fetch).await;
        // A sub-nanosecond TTL remains positive rather than being rounded to zero.
        collector.collect_with(fetch).await;
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        clock.advance(2e-300);
        assert!(
            collector
                .collect_with(fetch)
                .await
                .contains("ai_usage_exporter_scrape_success 1.0")
        );
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn simultaneous_scrapes_share_one_upstream_refresh() {
        let collector = Arc::new(Clock::new(1000.0).collector(300));
        let requests = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(Notify::new());
        let second_entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let first = {
            let collector = collector.clone();
            let requests = requests.clone();
            let entered = entered.clone();
            let release = release.clone();
            tokio::spawn(async move {
                collector
                    .collect_with(|| async {
                        requests.fetch_add(1, Ordering::SeqCst);
                        entered.notify_one();
                        release.notified().await;
                        Ok::<_, &str>(limits())
                    })
                    .await
            })
        };
        entered.notified().await;
        let second = {
            let collector = collector.clone();
            let requests = requests.clone();
            let second_entered = second_entered.clone();
            tokio::spawn(async move {
                second_entered.notify_one();
                collector
                    .collect_with(|| async {
                        requests.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, &str>(limits())
                    })
                    .await
            })
        };
        // On this current-thread runtime, the second task reaches the locked
        // collector before the waiting test task can release the first fetch.
        second_entered.notified().await;
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        release.notify_one();
        assert_eq!(first.await.unwrap(), second.await.unwrap());
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }
}
