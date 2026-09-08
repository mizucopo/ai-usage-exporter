"""Environment configuration and HTTP serving."""

import math
from collections.abc import Mapping
from dataclasses import dataclass
from threading import Thread
from wsgiref.simple_server import WSGIServer

from prometheus_client import CollectorRegistry, start_http_server
from prometheus_client.exposition import ThreadingWSGIServer

from ai_usage_exporter.metrics import UsageCollector


@dataclass(frozen=True)
class Settings:
    host: str
    port: int
    cache_ttl: float
    fetch_timeout: float

    def __post_init__(self) -> None:
        if not self.host.strip():
            raise ValueError("AI_USAGE_EXPORTER_HOST must not be empty")
        if not 1 <= self.port <= 65535:
            raise ValueError("AI_USAGE_EXPORTER_PORT must be between 1 and 65535")
        for name, value in (
            ("CACHE_TTL_SECONDS", self.cache_ttl),
            ("FETCH_TIMEOUT_SECONDS", self.fetch_timeout),
        ):
            if not math.isfinite(value) or value <= 0:
                raise ValueError(
                    f"AI_USAGE_EXPORTER_{name} must be positive and finite"
                )

    @classmethod
    def from_env(cls, env: Mapping[str, str]) -> Settings:
        return cls(
            host=env.get("AI_USAGE_EXPORTER_HOST", "0.0.0.0"),
            port=int(env.get("AI_USAGE_EXPORTER_PORT", "9173")),
            cache_ttl=float(env.get("AI_USAGE_EXPORTER_CACHE_TTL_SECONDS", "300")),
            fetch_timeout=float(
                env.get("AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS", "10")
            ),
        )


def start_server(
    collector: UsageCollector,
    host: str,
    port: int,
) -> tuple[WSGIServer, Thread]:
    registry = CollectorRegistry()
    registry.register(collector)
    server, thread = start_http_server(port, addr=host, registry=registry)
    assert isinstance(server, ThreadingWSGIServer)
    # Shutdown waits for bounded Codex requests so children are reaped before exit.
    server.daemon_threads = False
    return server, thread
