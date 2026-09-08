"""Run the exporter until SIGINT or SIGTERM."""

import os
import signal
import sys
from threading import Event
from types import FrameType

from ai_usage_exporter.codex import CodexClient
from ai_usage_exporter.metrics import UsageCollector
from ai_usage_exporter.server import Settings, start_server


def main() -> int:
    try:
        settings = Settings.from_env(os.environ)
        collector = UsageCollector(
            CodexClient(timeout=settings.fetch_timeout).read_limits,
            cache_ttl=settings.cache_ttl,
        )
        server, thread = start_server(collector, settings.host, settings.port)
    except (ValueError, OSError) as error:
        print(f"Cannot start exporter: {error}", file=sys.stderr)
        return 2

    stop = Event()

    def request_stop(_signum: int, _frame: FrameType | None) -> None:
        stop.set()

    signal.signal(signal.SIGINT, request_stop)
    signal.signal(signal.SIGTERM, request_stop)
    try:
        stop.wait()
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
    return 0


if __name__ == "__main__":
    sys.exit(main())
