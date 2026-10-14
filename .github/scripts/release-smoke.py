"""Validate the common native archive and the packaged exporter with a local mock.

No GitHub mutation, Codex authentication or account service is used.
"""

import argparse
import concurrent.futures
import ctypes
import importlib.util
import json
import os
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import tomllib
from pathlib import Path
from types import SimpleNamespace
from urllib import request

ROOT = Path(__file__).resolve().parents[2]


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def alive(pid):
    if os.name == "nt":
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.restype = ctypes.c_void_p
        kernel.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32]
        kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
        kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        handle = kernel.OpenProcess(0x00100000, False, pid)
        if not handle:
            return False
        try:
            return kernel.WaitForSingleObject(handle, 0) == 258
        finally:
            kernel.CloseHandle(handle)
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def stop(process, mock):
    if os.name == "nt":
        subprocess.run([str(mock), "signal", str(process.pid)], check=True, timeout=10)
    else:
        process.send_signal(signal.SIGTERM)


def wait_for(predicate, message):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def exporter_smoke(binary, mock, directory, mode, in_flight=False, cli_path=None):
    case = directory / (
        mode + ("-drain" if in_flight else "") + ("-npm" if cli_path else "")
    )
    case.mkdir()
    home = case / "codex-home"
    home.mkdir()
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    pid_file, ready_file, calls = case / "pid", case / "ready", case / "calls"
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("AI_USAGE_EXPORTER_", "CODEX_", "OPENAI_"))
    }
    env.update(
        PATH=str(cli_path or mock.parent),
        CODEX_HOME=str(home),
        AI_USAGE_EXPORTER_HOST="127.0.0.1",
        AI_USAGE_EXPORTER_PORT=str(port),
        AI_USAGE_EXPORTER_CACHE_TTL_SECONDS="300",
        AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS="1",
        AI_USAGE_EXPORTER_MOCK_MODE=mode,
        AI_USAGE_EXPORTER_MOCK_PID_FILE=str(pid_file),
        AI_USAGE_EXPORTER_MOCK_READY_FILE=str(ready_file),
        AI_USAGE_EXPORTER_MOCK_CODEX_HOME=str(home),
        AI_USAGE_EXPORTER_MOCK_CALLS_FILE=str(calls),
    )
    flags = subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0
    startup = None
    if os.name == "nt":
        # Give headless runners an isolated console for the control-event test.
        # Hide its window; the helper attaches only to this exporter console.
        flags |= subprocess.CREATE_NEW_CONSOLE
        startup = subprocess.STARTUPINFO()
        startup.dwFlags = subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = subprocess.SW_HIDE
    opener = request.build_opener(request.ProxyHandler({}))
    process = subprocess.Popen(
        [str(binary)],
        env=env,
        cwd=case,
        creationflags=flags,
        startupinfo=startup,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )

    def listening():
        require(process.poll() is None, "exporter exited before HTTP startup")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                return True
        except OSError:
            return False

    def metrics():
        with opener.open(f"http://127.0.0.1:{port}/metrics", timeout=10) as response:
            require(response.status == 200, "unexpected HTTP status")
            require("text/plain" in response.headers["Content-Type"], "content type")
            return response.read().decode()

    try:
        wait_for(listening, "HTTP startup timed out")
        if in_flight:
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                pending = pool.submit(metrics)
                wait_for(ready_file.exists, "mock did not receive quota request")
                stop(process, mock)
                body = pending.result(timeout=10)
        else:
            body = metrics()
        expected_success = "1.0" if mode == "success" else "0.0"
        require(f"ai_usage_exporter_scrape_success {expected_success}" in body, mode)
        if mode == "success":
            require(
                'codex_rate_limit_remaining_ratio{window="5h"} 0.75' in body, "quota"
            )
            require("4102444800" in body, "reset timestamp")
            require(
                'codex_rate_limit_remaining_ratio{window="5h"} 0.75' in metrics(),
                "cached quota",
            )
            require(calls.read_text() == "x", "cache unexpectedly spawned another CLI")
        else:
            require(
                "codex_rate_limit_remaining_ratio{" not in body, "failed quota leaked"
            )
        require(pid_file.exists(), "mock CLI did not start")
        pid = int(pid_file.read_text())
        wait_for(lambda: not alive(pid), "mock child remained after scrape")
        if not in_flight:
            stop(process, mock)
        process.communicate(timeout=10)
        require(process.returncode == 0, "exporter shutdown failed")
        require(not alive(pid), "mock child remained after exporter shutdown")
    finally:
        if process.poll() is None:
            process.kill()
        process.communicate(timeout=10)
        if pid_file.exists():
            pid = int(pid_file.read_text())
            if alive(pid):
                os.kill(pid, signal.SIGTERM)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--verify-existing", action="store_true")
    args = parser.parse_args()
    spec = importlib.util.spec_from_file_location(
        "release", ROOT / ".github/scripts/release.py"
    )
    release = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(release)
    policy = json.loads((ROOT / ".github/release.json").read_text(encoding="utf-8"))
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))[
        "package"
    ]["version"]
    sha = (
        subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT).decode().strip()
    )
    plan = {"rust": policy["publication"]["rust"], "release_tag": version}
    git = SimpleNamespace(root=str(ROOT))
    output = args.output.resolve()
    if not args.verify_existing:
        release.build_rust_distribution(git, plan, sha, args.target, output)
    artifact = output / args.target
    manifest = json.loads((artifact / "manifest.json").read_text())
    require(
        manifest["binaries"]
        == ["ai-usage-exporter" + (".exe" if os.name == "nt" else "")],
        "executables",
    )
    with tempfile.TemporaryDirectory() as temporary:
        directory = Path(temporary)
        retained = directory / ("rust-release-" + args.target)
        shutil.copytree(artifact, retained)
        selected_plan = {**plan, "rust": {**plan["rust"], "targets": [args.target]}}
        release.rust_deliverables(git, selected_plan, sha, directory)
        extracted = directory / "extracted"
        extracted.mkdir()
        release.rust_extract(artifact / manifest["archive"], extracted)
        binary = extracted / manifest["binaries"][0]
        require(
            subprocess.check_output([str(binary), "--version"], timeout=10)
            .decode()
            .strip()
            == f"ai-usage-exporter {version}",
            "version",
        )
        mock_dir = directory / "mock cli"
        mock_dir.mkdir()
        mock = mock_dir / ("codex.exe" if os.name == "nt" else "codex")
        compiler = [
            "rustc",
            "--edition=2024",
            str(ROOT / "tests/support/mock_codex.rs"),
            "-o",
            str(mock),
        ]
        compiler.extend(
            os.environ.get("CARGO_ENCODED_RUSTFLAGS", "").split("\x1f")
            if os.environ.get("CARGO_ENCODED_RUSTFLAGS")
            else []
        )
        linker = os.environ.get(
            "CARGO_TARGET_" + args.target.upper().replace("-", "_") + "_LINKER"
        )
        if linker:
            compiler.extend(["-C", "linker=" + linker])
        subprocess.run(compiler, check=True, timeout=120)
        for mode in ("success", "error", "timeout"):
            exporter_smoke(binary, mock, directory, mode)
        exporter_smoke(binary, mock, directory, "timeout", in_flight=True)
        if os.name == "nt":
            prefix = directory / "npm prefix & literal"
            prefix.mkdir()
            # This wrapper must never run; the exporter owns the native child.
            (prefix / "codex.cmd").write_text("@exit /b 99\n")
            architecture = "arm64" if args.target.startswith("aarch64") else "x64"
            native = (
                prefix
                / "node_modules"
                / "@openai"
                / ("codex-win32-" + architecture)
                / "vendor"
                / args.target
                / "bin"
                / "codex.exe"
            )
            native.parent.mkdir(parents=True)
            shutil.copy2(mock, native)
            exporter_smoke(binary, mock, directory, "success", cli_path=prefix)
    print(
        f"{args.target}: archive, SHA-256, startup, metrics, cache, mock and shutdown passed"
    )


if __name__ == "__main__":
    main()
