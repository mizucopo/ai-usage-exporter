"""Read account limits using Codex's stdio app-server protocol."""

import json
import os
import select
import subprocess
import time
from collections.abc import Sequence
from typing import IO, cast

from ai_usage_exporter import __version__


class CodexError(Exception):
    """The account limits could not be obtained safely."""


class CodexClient:
    """Start a short-lived Codex process for each account limits request."""

    def __init__(
        self, timeout: float = 10.0, command: Sequence[str] | None = None
    ) -> None:
        self.timeout = timeout
        self.command = (
            list(command)
            if command is not None
            else [
                "codex",
                "app-server",
                "--stdio",
                "-c",
                'cli_auth_credentials_store="file"',
            ]
        )

    def read_limits(self) -> dict[str, object]:
        try:
            return self._read_limits()
        except OSError, ValueError, RecursionError:
            raise CodexError("Codex process or response is invalid") from None

    def _read_limits(self) -> dict[str, object]:
        deadline = time.monotonic() + self.timeout
        with subprocess.Popen(
            self.command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=False,
        ) as process:
            assert process.stdin is not None
            assert process.stdout is not None
            buffered = bytearray()
            try:
                self._send(
                    process.stdin,
                    {
                        "id": 1,
                        "method": "initialize",
                        "params": {
                            "clientInfo": {
                                "name": "ai_usage_exporter",
                                "version": __version__,
                            }
                        },
                    },
                )
                self._receive(process.stdout, 1, deadline, buffered)
                self._send(process.stdin, {"method": "initialized"})
                self._send(
                    process.stdin, {"id": 2, "method": "account/rateLimits/read"}
                )
                return self._receive(process.stdout, 2, deadline, buffered)
            finally:
                if process.poll() is None:
                    process.kill()
                process.wait()

    @staticmethod
    def _send(stream: IO[bytes], message: dict[str, object]) -> None:
        stream.write(json.dumps(message).encode() + b"\n")
        stream.flush()

    @staticmethod
    def _receive(
        stream: IO[bytes], request_id: int, deadline: float, buffered: bytearray
    ) -> dict[str, object]:
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise CodexError("Codex request timed out")
            if b"\n" not in buffered:
                if not select.select([stream], [], [], remaining)[0]:
                    raise CodexError("Codex request timed out")
                chunk = os.read(stream.fileno(), 65536)
                if not chunk:
                    raise CodexError("Codex closed its response stream")
                buffered.extend(chunk)
                if len(buffered) > 1024 * 1024:
                    raise CodexError("Codex response is too large")
                continue
            line, _, tail = buffered.partition(b"\n")
            buffered[:] = tail
            message = json.loads(line)
            if not isinstance(message, dict):
                raise CodexError("Codex response is not an object")
            if message.get("id") == request_id:
                if "error" in message:
                    raise CodexError("Codex rejected the request")
                result = message.get("result")
                if not isinstance(result, dict):
                    raise CodexError("Codex response has no result object")
                return cast(dict[str, object], result)
