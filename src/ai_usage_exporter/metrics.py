"""Collect account quotas without storing Prometheus samples between scrapes."""

import math
import time
from collections.abc import Callable, Iterable
from dataclasses import dataclass
from threading import Lock
from typing import cast

from prometheus_client.core import GaugeMetricFamily, Metric

from ai_usage_exporter.codex import CodexError


@dataclass(frozen=True)
class QuotaWindow:
    label: str
    remaining: float
    resets_at: float


def _object(value: object) -> dict[str, object]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise ValueError("Expected quota object")
    return cast(dict[str, object], value)


def _number(value: object) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError("Expected quota number")
    try:
        number = float(value)
    except OverflowError:
        raise ValueError("Invalid quota number") from None
    if not math.isfinite(number) or number < 0:
        raise ValueError("Invalid quota number")
    return number


def parse_limits(result: dict[str, object], now: float) -> tuple[QuotaWindow, ...]:
    buckets = result.get("rateLimitsByLimitId")
    bucket = _object(
        (
            _object(buckets).get("codex")
            if buckets is not None
            else result.get("rateLimits")
        )
    )
    if bucket.get("limitId") not in (None, "codex"):
        raise ValueError("Unexpected quota bucket")
    windows = []
    for key in ("primary", "secondary"):
        window = _object(bucket.get(key))
        duration = _number(window.get("windowDurationMins"))
        label = {300.0: "5h", 10080.0: "weekly"}.get(duration)
        resets_at = _number(window.get("resetsAt"))
        if label is None or resets_at <= now:
            raise ValueError("Unsupported or expired quota window")
        windows.append(
            QuotaWindow(
                label,
                max(0, (100 - _number(window.get("usedPercent"))) / 100),
                resets_at,
            )
        )
    if {window.label for window in windows} != {"5h", "weekly"}:
        raise ValueError("Both quota windows are required")
    return tuple(windows)


class UsageCollector:
    def __init__(
        self,
        fetch: Callable[[], dict[str, object]],
        *,
        cache_ttl: float = 300,
        wall_time: Callable[[], float] = time.time,
        monotonic: Callable[[], float] = time.monotonic,
    ) -> None:
        self._fetch = fetch
        self._wall_time = wall_time
        self._monotonic = monotonic
        self._cache_ttl = cache_ttl
        self._lock = Lock()
        self._windows: tuple[QuotaWindow, ...] = ()
        self._expires = float("-inf")
        self._last_success = 0.0

    def describe(self) -> Iterable[Metric]:
        return ()

    def collect(self) -> Iterable[Metric]:
        with self._lock:
            if self._monotonic() >= self._expires or any(
                window.resets_at <= self._wall_time() for window in self._windows
            ):
                try:
                    result = self._fetch()
                    self._windows = parse_limits(result, self._wall_time())
                    self._last_success = self._wall_time()
                except CodexError, ValueError:
                    self._windows = ()
                self._expires = self._monotonic() + self._cache_ttl
                if self._windows:
                    until_reset = (
                        min(w.resets_at for w in self._windows) - self._wall_time()
                    )
                    self._expires = min(self._expires, self._monotonic() + until_reset)
            windows = self._windows
            last_success = self._last_success
        now = self._wall_time()
        yield GaugeMetricFamily(
            "ai_usage_exporter_scrape_success",
            "Latest quota fetch succeeded.",
            value=int(bool(windows)),
        )
        yield GaugeMetricFamily(
            "ai_usage_exporter_last_success_timestamp_seconds",
            "Unix time of the last successful quota fetch, or zero.",
            value=last_success,
        )
        if not windows:
            return
        remaining = GaugeMetricFamily(
            "codex_rate_limit_remaining_ratio",
            "Quota remaining (0 to 1).",
            labels=["window"],
        )
        reset = GaugeMetricFamily(
            "codex_rate_limit_reset_timestamp_seconds",
            "Next quota reset Unix time.",
            labels=["window"],
        )
        countdown = GaugeMetricFamily(
            "codex_rate_limit_reset_seconds",
            "Seconds until the next quota reset.",
            labels=["window"],
        )
        for window in windows:
            remaining.add_metric([window.label], window.remaining)
            reset.add_metric([window.label], window.resets_at)
            countdown.add_metric([window.label], max(0, window.resets_at - now))
        yield remaining
        yield reset
        yield countdown
