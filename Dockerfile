FROM ghcr.io/astral-sh/uv:0.12.10 AS uv
FROM python:3.14-slim AS codex-download

ARG TARGETARCH
RUN TARGETARCH="$TARGETARCH" python - <<'PY'
import hashlib
import io
import os
from pathlib import Path
import tarfile
import urllib.request

artifacts = {
    "amd64": ("x86_64", "f479424eca092484dc40d87ae28c44f4cc40234a60045d6131e493800d814a30"),
    "arm64": ("aarch64", "5cda6182bd94c3a30f2eb63a495489ebf7f691fddb14d70f48c6c1a5071b6cde"),
}
arch, expected = artifacts[os.environ["TARGETARCH"]]
name = f"codex-{arch}-unknown-linux-musl"
url = f"https://github.com/openai/codex/releases/download/rust-v0.153.4/{name}.tar.gz"
with urllib.request.urlopen(url, timeout=120) as response:
    archive = response.read()
if hashlib.sha256(archive).hexdigest() != expected:
    raise SystemExit("Codex archive checksum mismatch")
with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as bundle:
    binary = bundle.extractfile(name)
    if binary is None:
        raise SystemExit("Codex binary missing from archive")
    destination = Path("/codex")
    destination.write_bytes(binary.read())
    destination.chmod(0o755)
PY

FROM python:3.14-slim

COPY --from=uv /uv /usr/local/bin/uv
COPY --from=codex-download /codex /usr/local/bin/codex

WORKDIR /app
ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PYTHONPATH=/app/src \
    PATH="/app/.venv/bin:$PATH" \
    CODEX_HOME=/var/lib/codex \
    AI_USAGE_EXPORTER_HOST=0.0.0.0 \
    AI_USAGE_EXPORTER_PORT=9173 \
    AI_USAGE_EXPORTER_CACHE_TTL_SECONDS=300 \
    AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS=10

COPY pyproject.toml uv.lock ./
RUN uv sync --locked --no-dev --no-cache

COPY src ./src

RUN groupadd --gid 10001 exporter \
    && useradd --uid 10001 --gid exporter --create-home exporter \
    && mkdir -p /var/lib/codex \
    && chown exporter:exporter /var/lib/codex

USER exporter
VOLUME ["/var/lib/codex"]
EXPOSE 9173

CMD ["python", "-m", "ai_usage_exporter"]
