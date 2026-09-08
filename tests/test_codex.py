import os
import resource
import sys
import textwrap
from contextlib import ExitStack
from pathlib import Path

import pytest

from ai_usage_exporter.codex import CodexClient, CodexError


def test_reads_limits_after_initialization() -> None:
    server = """
        import json
        import sys
        first = json.loads(sys.stdin.readline())
        assert first['method'] == 'initialize'
        assert first['params']['clientInfo']['name'] == 'ai_usage_exporter'
        print(json.dumps({'id': first['id'], 'result': {}}), flush=True)
        assert json.loads(sys.stdin.readline())['method'] == 'initialized'
        request = json.loads(sys.stdin.readline())
        assert request['method'] == 'account/rateLimits/read'
        print(json.dumps({'method': 'notice', 'params': {}}), flush=True)
        print(json.dumps({'id': 999, 'result': {}}), flush=True)
        print(json.dumps({'id': request['id'], 'result': {
            'rateLimits': {'primary': {'usedPercent': 25}}
        }}), flush=True)
    """
    client = CodexClient(command=[sys.executable, "-c", textwrap.dedent(server)])

    assert client.read_limits() == {"rateLimits": {"primary": {"usedPercent": 25}}}


def test_times_out_an_unresponsive_child() -> None:
    server = """
        import json
        import sys
        import time
        time.sleep(0.3)
        request = json.loads(sys.stdin.readline())
        print(json.dumps({'id': request['id'], 'result': {}}), flush=True)
        sys.stdin.readline()
        request = json.loads(sys.stdin.readline())
        print(json.dumps({'id': request['id'], 'result': {}}), flush=True)
    """
    client = CodexClient(
        timeout=0.05, command=[sys.executable, "-c", textwrap.dedent(server)]
    )

    with pytest.raises(CodexError, match="timed out"):
        client.read_limits()


@pytest.mark.parametrize(
    "response",
    [
        "secret-not-json",
        '{"id": 1, "error": {"message": "secret-token"}}',
        '{"id": 1, "result": "secret-token"}',
        '["secret-token"]',
        '{"id": 1}',
    ],
)
def test_rejects_invalid_responses_without_exposing_server_data(response: str) -> None:
    server = f"print({response!r}, flush=True)"
    client = CodexClient(command=[sys.executable, "-c", server])

    with pytest.raises(CodexError) as error:
        client.read_limits()

    assert "secret" not in str(error.value)
    assert error.value.__suppress_context__ or error.value.__context__ is None


def test_bounds_response_memory() -> None:
    server = """
        import sys
        sys.stdout.write('x' * (2 * 1024 * 1024))
        sys.stdout.flush()
    """
    client = CodexClient(command=[sys.executable, "-c", textwrap.dedent(server)])

    with pytest.raises(CodexError, match="too large"):
        client.read_limits()


@pytest.mark.parametrize("outcome", ["success", "error", "timeout"])
def test_always_reaps_child_process(tmp_path: Path, outcome: str) -> None:
    pid_file = tmp_path / "child.pid"
    server = f"""
        import json
        import os
        import pathlib
        import sys
        import time
        pathlib.Path({str(pid_file)!r}).write_text(str(os.getpid()))
        request = json.loads(sys.stdin.readline())
        print(json.dumps({{'id': request['id'], 'result': {{}}}}), flush=True)
        sys.stdin.readline()
        request = json.loads(sys.stdin.readline())
        if {outcome!r} == 'success':
            print(json.dumps({{'id': request['id'], 'result': {{}}}}), flush=True)
        elif {outcome!r} == 'error':
            print(json.dumps({{'id': request['id'], 'error': {{}}}}), flush=True)
        time.sleep(30)
    """
    client = CodexClient(
        timeout=0.5, command=[sys.executable, "-c", textwrap.dedent(server)]
    )

    if outcome == "success":
        assert client.read_limits() == {}
    else:
        with pytest.raises(CodexError):
            client.read_limits()

    with pytest.raises(ProcessLookupError):
        os.kill(int(pid_file.read_text()), 0)


def test_startup_failure_is_sanitized(tmp_path: Path) -> None:
    client = CodexClient(command=[str(tmp_path / "secret-nonexistent-codex")])

    with pytest.raises(CodexError) as error:
        client.read_limits()

    assert "secret" not in str(error.value)


def test_eof_is_a_failed_request() -> None:
    client = CodexClient(command=[sys.executable, "-c", "pass"])

    with pytest.raises(CodexError):
        client.read_limits()


def test_deadline_is_shared_by_initialization_and_limits_request() -> None:
    server = """
        import json
        import sys
        import time
        request = json.loads(sys.stdin.readline())
        time.sleep(0.2)
        print(json.dumps({'id': request['id'], 'result': {}}), flush=True)
        sys.stdin.readline()
        request = json.loads(sys.stdin.readline())
        time.sleep(0.2)
        print(json.dumps({'id': request['id'], 'result': {}}), flush=True)
    """
    client = CodexClient(
        timeout=0.3, command=[sys.executable, "-c", textwrap.dedent(server)]
    )

    with pytest.raises(CodexError, match="timed out"):
        client.read_limits()


def test_reads_limits_with_high_numbered_pipe_descriptors() -> None:
    original_limit = resource.getrlimit(resource.RLIMIT_NOFILE)
    if original_limit[1] != resource.RLIM_INFINITY and original_limit[1] < 1200:
        pytest.skip("Hard descriptor limit does not allow high-numbered pipes")
    server = """
        import json
        import sys
        request = json.loads(sys.stdin.readline())
        print(json.dumps({'id': request['id'], 'result': {}}), flush=True)
        sys.stdin.readline()
        request = json.loads(sys.stdin.readline())
        response = {'id': request['id'], 'result': {'rateLimits': {}}}
        print(json.dumps(response), flush=True)
    """
    client = CodexClient(command=[sys.executable, "-c", textwrap.dedent(server)])
    try:
        if original_limit[0] != resource.RLIM_INFINITY and original_limit[0] < 1200:
            resource.setrlimit(resource.RLIMIT_NOFILE, (1200, original_limit[1]))
        with ExitStack() as files:
            while files.enter_context(open(os.devnull, "rb")).fileno() < 1024:
                pass
            assert client.read_limits() == {"rateLimits": {}}
    finally:
        resource.setrlimit(resource.RLIMIT_NOFILE, original_limit)
