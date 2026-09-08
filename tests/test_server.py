from concurrent.futures import ThreadPoolExecutor
from threading import Event
from urllib.request import urlopen

import pytest

from ai_usage_exporter.metrics import UsageCollector
from ai_usage_exporter.server import Settings, start_server
from tests.test_metrics import limits


def test_default_port_and_configurable_cache() -> None:
    defaults = Settings.from_env({})
    assert defaults.port == 9173
    assert defaults.cache_ttl == 300
    assert defaults.host == "0.0.0.0"
    assert defaults.fetch_timeout == 10
    configured = Settings.from_env(
        {
            "AI_USAGE_EXPORTER_PORT": "12345",
            "AI_USAGE_EXPORTER_CACHE_TTL_SECONDS": "60",
            "AI_USAGE_EXPORTER_HOST": "127.0.0.1",
            "AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS": "5.5",
        }
    )
    assert configured.port == 12345
    assert configured.cache_ttl == 60
    assert configured.host == "127.0.0.1"
    assert configured.fetch_timeout == 5.5


@pytest.mark.parametrize(
    "key, value",
    [
        ("PORT", "0"),
        ("PORT", "65536"),
        ("PORT", "1.5"),
        ("CACHE_TTL_SECONDS", "0"),
        ("CACHE_TTL_SECONDS", "-1"),
        ("CACHE_TTL_SECONDS", "inf"),
        ("CACHE_TTL_SECONDS", "nan"),
        ("FETCH_TIMEOUT_SECONDS", "0"),
        ("FETCH_TIMEOUT_SECONDS", "-1"),
        ("FETCH_TIMEOUT_SECONDS", "inf"),
        ("FETCH_TIMEOUT_SECONDS", "nan"),
        ("HOST", ""),
    ],
)
def test_invalid_settings_fail_at_startup(key: str, value: str) -> None:
    with pytest.raises(ValueError):
        Settings.from_env({f"AI_USAGE_EXPORTER_{key}": value})


def test_concurrent_http_scrapes_share_one_fetch() -> None:
    started = Event()
    release = Event()
    requests = 0

    def fetch() -> dict[str, object]:
        nonlocal requests
        requests += 1
        started.set()
        assert release.wait(5)
        return limits()

    collector = UsageCollector(fetch, wall_time=lambda: 1000)
    server, thread = start_server(collector, "127.0.0.1", 0)

    def get() -> str:
        with urlopen(
            f"http://127.0.0.1:{server.server_port}/metrics", timeout=5
        ) as response:
            assert response.status == 200
            assert "text/plain" in response.headers["Content-Type"]
            return str(response.read().decode())

    try:
        with ThreadPoolExecutor(max_workers=6) as executor:
            pending = [executor.submit(get) for _ in range(6)]
            try:
                assert started.wait(5)
            finally:
                release.set()
            outputs = [future.result(timeout=5) for future in pending]
        assert requests == 1
        assert all(
            'codex_rate_limit_remaining_ratio{window="5h"} 0.75' in s for s in outputs
        )
    finally:
        release.set()
        server.shutdown()
        server.server_close()
        thread.join(5)
