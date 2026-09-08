from dataclasses import dataclass

import pytest
from prometheus_client import CollectorRegistry, generate_latest

from ai_usage_exporter.codex import CodexError
from ai_usage_exporter.metrics import UsageCollector


@dataclass
class Clock:
    wall: float = 1000
    mono: float = 0

    def time(self) -> float:
        return self.wall

    def monotonic(self) -> float:
        return self.mono

    def advance(self, seconds: float) -> None:
        self.wall += seconds
        self.mono += seconds


def limits() -> dict[str, object]:
    return {
        "rateLimitsByLimitId": {
            "codex": {
                "primary": {
                    "usedPercent": 25,
                    "windowDurationMins": 300,
                    "resetsAt": 2000,
                },
                "secondary": {
                    "usedPercent": 80,
                    "windowDurationMins": 10080,
                    "resetsAt": 10000,
                },
            }
        }
    }


def scrape(collector: UsageCollector) -> str:
    registry = CollectorRegistry()
    registry.register(collector)
    return generate_latest(registry).decode()


def test_exports_both_quotas_and_reset_times() -> None:
    output = scrape(UsageCollector(limits, wall_time=lambda: 1000))
    assert 'codex_rate_limit_remaining_ratio{window="5h"} 0.75' in output
    assert 'codex_rate_limit_remaining_ratio{window="weekly"} 0.2' in output
    assert 'codex_rate_limit_reset_seconds{window="5h"} 1000.0' in output
    assert 'codex_rate_limit_reset_seconds{window="weekly"} 9000.0' in output
    assert 'codex_rate_limit_reset_timestamp_seconds{window="5h"} 2000.0' in output
    assert "ai_usage_exporter_scrape_success 1.0" in output
    assert "ai_usage_exporter_last_success_timestamp_seconds 1000.0" in output


def test_cache_reuses_quotas_but_countdown_keeps_advancing() -> None:
    clock = Clock()
    requests = 0

    def fetch() -> dict[str, object]:
        nonlocal requests
        requests += 1
        return limits()

    collector = UsageCollector(
        fetch,
        wall_time=clock.time,
        monotonic=clock.monotonic,
    )
    registry = CollectorRegistry(auto_describe=True)
    registry.register(collector)
    assert requests == 0
    scrape(collector)
    clock.advance(299)
    output = scrape(collector)
    assert requests == 1
    assert 'codex_rate_limit_reset_seconds{window="5h"} 701.0' in output
    assert "ai_usage_exporter_last_success_timestamp_seconds 1000.0" in output
    clock.advance(1)
    scrape(collector)
    assert requests == 2


def test_failed_refresh_removes_old_values_and_caches_failure() -> None:
    clock = Clock()
    requests = 0

    def fetch() -> dict[str, object]:
        nonlocal requests
        requests += 1
        if requests == 2:
            raise CodexError("Private upstream error")
        return limits()

    collector = UsageCollector(
        fetch,
        cache_ttl=60,
        wall_time=clock.time,
        monotonic=clock.monotonic,
    )
    scrape(collector)
    clock.advance(60)
    output = scrape(collector)
    assert "codex_rate_limit_" not in output
    assert "ai_usage_exporter_scrape_success 0.0" in output
    assert "ai_usage_exporter_last_success_timestamp_seconds 1000.0" in output
    assert "Private" not in output
    clock.advance(59)
    assert scrape(collector) == output
    assert requests == 2
    clock.advance(1)
    assert "ai_usage_exporter_scrape_success 1.0" in scrape(collector)
    assert requests == 3


@pytest.mark.parametrize("legacy", [True, False])
def test_periods_are_identified_by_duration_including_legacy(legacy: bool) -> None:
    bucket = {
        "limitId": "codex",
        "primary": {
            "usedPercent": 100,
            "windowDurationMins": 10080,
            "resetsAt": 10000,
        },
        "secondary": {
            "usedPercent": 0,
            "windowDurationMins": 300,
            "resetsAt": 2000,
        },
    }
    result: dict[str, object] = (
        {"rateLimits": bucket} if legacy else {"rateLimitsByLimitId": {"codex": bucket}}
    )
    output = scrape(UsageCollector(lambda: result, wall_time=lambda: 1000))
    assert 'codex_rate_limit_remaining_ratio{window="5h"} 1.0' in output
    assert 'codex_rate_limit_remaining_ratio{window="weekly"} 0.0' in output


def legacy_bucket() -> dict[str, object]:
    return {
        "limitId": "codex",
        "primary": {
            "usedPercent": 0,
            "windowDurationMins": 300,
            "resetsAt": 2000,
        },
        "secondary": {
            "usedPercent": 100,
            "windowDurationMins": 10080,
            "resetsAt": 10000,
        },
    }


@pytest.mark.parametrize("buckets", [{}, {"other": {}}, {"codex": None}])
def test_uses_legacy_limits_when_codex_bucket_is_absent(
    buckets: dict[str, object],
) -> None:
    result: dict[str, object] = {
        "rateLimitsByLimitId": buckets,
        "rateLimits": legacy_bucket(),
    }

    output = scrape(UsageCollector(lambda: result, wall_time=lambda: 1000))

    assert 'codex_rate_limit_remaining_ratio{window="5h"} 1.0' in output
    assert 'codex_rate_limit_remaining_ratio{window="weekly"} 0.0' in output
    assert "ai_usage_exporter_scrape_success 1.0" in output


def test_populated_codex_bucket_takes_precedence_over_legacy_limits() -> None:
    result = limits()
    result["rateLimits"] = legacy_bucket()

    output = scrape(UsageCollector(lambda: result, wall_time=lambda: 1000))

    assert 'codex_rate_limit_remaining_ratio{window="5h"} 0.75' in output
    assert 'codex_rate_limit_remaining_ratio{window="weekly"} 0.2' in output


@pytest.mark.parametrize("bucket", [{}, {"primary": None}, "invalid"])
def test_invalid_selected_codex_bucket_does_not_fall_back_to_legacy(
    bucket: object,
) -> None:
    result: dict[str, object] = {
        "rateLimitsByLimitId": {"codex": bucket},
        "rateLimits": legacy_bucket(),
    }

    output = scrape(UsageCollector(lambda: result, wall_time=lambda: 1000))

    assert "codex_rate_limit_" not in output
    assert "ai_usage_exporter_scrape_success 0.0" in output


def test_legacy_fallback_rejects_a_different_limit_id() -> None:
    bucket = legacy_bucket()
    bucket["limitId"] = "other"
    result: dict[str, object] = {"rateLimitsByLimitId": {}, "rateLimits": bucket}

    output = scrape(UsageCollector(lambda: result, wall_time=lambda: 1000))

    assert "codex_rate_limit_" not in output
    assert "ai_usage_exporter_scrape_success 0.0" in output


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("usedPercent", None),
        ("usedPercent", True),
        ("usedPercent", float("nan")),
        ("usedPercent", float("inf")),
        ("usedPercent", -1),
        ("usedPercent", "25"),
        ("windowDurationMins", 15),
        ("windowDurationMins", 10080),
        ("resetsAt", None),
        ("resetsAt", 999),
        ("resetsAt", 1000),
    ],
)
def test_invalid_or_expired_window_omits_all_quota_values(
    field: str,
    value: object,
) -> None:
    primary: dict[str, object] = {
        "usedPercent": 25,
        "windowDurationMins": 300,
        "resetsAt": 2000,
    }
    primary[field] = value
    result: dict[str, object] = {
        "rateLimits": {
            "primary": primary,
            "secondary": {
                "usedPercent": 50,
                "windowDurationMins": 10080,
                "resetsAt": 10000,
            },
        }
    }
    output = scrape(UsageCollector(lambda: result, wall_time=lambda: 1000))
    assert "codex_rate_limit_" not in output
    assert "ai_usage_exporter_scrape_success 0.0" in output
    assert "ai_usage_exporter_last_success_timestamp_seconds 0.0" in output


@pytest.mark.parametrize(
    "result",
    [
        {},
        {"rateLimits": None},
        {"rateLimits": {}},
        {"rateLimitsByLimitId": {"other": {}}},
        {"rateLimitsByLimitId": []},
        {"rateLimits": {"primary": None, "secondary": None}},
    ],
)
def test_missing_limits_are_not_reported_as_zero(result: dict[str, object]) -> None:
    output = scrape(UsageCollector(lambda: result, wall_time=lambda: 1000))
    assert "codex_rate_limit_" not in output
    assert "ai_usage_exporter_scrape_success 0.0" in output


def test_reset_expires_cache_early_without_refetch_loop() -> None:
    clock = Clock(wall=1990)
    requests = 0

    def fetch() -> dict[str, object]:
        nonlocal requests
        requests += 1
        return limits()

    collector = UsageCollector(
        fetch,
        wall_time=clock.time,
        monotonic=clock.monotonic,
    )
    assert 'codex_rate_limit_reset_seconds{window="5h"} 10.0' in scrape(collector)
    clock.advance(10)
    output = scrape(collector)
    assert requests == 2
    assert "codex_rate_limit_" not in output
    assert "ai_usage_exporter_scrape_success 0.0" in output
    clock.advance(299)
    scrape(collector)
    assert requests == 2
    clock.advance(1)
    scrape(collector)
    assert requests == 3
