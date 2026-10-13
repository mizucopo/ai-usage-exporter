#!/bin/sh
# Exercise the runtime with a local fake Codex; no account or external service.
set -eu

[ "$(id -u)" = 10001 ]
[ "$CODEX_HOME" = /var/lib/codex ]
[ -w "$CODEX_HOME" ]
for program in python python3 uv; do
    if command -v "$program" >/dev/null 2>&1; then
        echo "Unexpected runtime: $program" >&2
        exit 1
    fi
done
[ ! -d /app/.venv ]
codex --version | grep -F '0.153.4'

mkdir /tmp/fake-codex
cat > /tmp/fake-codex/codex <<'MOCK'
#!/bin/sh
set -eu
[ "$1" = app-server ]
[ "$2" = --stdio ]
[ "$3" = -c ]
[ "$4" = 'cli_auth_credentials_store="file"' ]
[ "$CODEX_HOME" = /var/lib/codex ]
printf x >> /tmp/fetches
read -r request
printf '%s\n' '{"id":1,"result":{}}'
read -r notification
[ "$notification" = '{"method":"initialized"}' ]
read -r request
printf '%s\n' '{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":25,"windowDurationMins":300,"resetsAt":4102444800},"secondary":{"usedPercent":80,"windowDurationMins":10080,"resetsAt":4102444800}}}}'
MOCK
chmod 755 /tmp/fake-codex/codex
PATH="/tmp/fake-codex:$PATH" ai-usage-exporter &
exporter_pid=$!
trap 'kill "$exporter_pid" 2>/dev/null || true; wait "$exporter_pid" 2>/dev/null || true' EXIT
attempt=0
until wget -qO /tmp/metrics http://127.0.0.1:9173/metrics; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 50 ]
    sleep 0.1
done
grep -F 'codex_rate_limit_remaining_ratio{window="5h"} 0.75' /tmp/metrics
grep -F 'codex_rate_limit_remaining_ratio{window="weekly"} 0.2' /tmp/metrics
grep -F 'ai_usage_exporter_scrape_success 1.0' /tmp/metrics
wget -qO /tmp/metrics http://127.0.0.1:9173/metrics
[ "$(cat /tmp/fetches)" = x ]
kill -TERM "$exporter_pid"
wait "$exporter_pid"
trap - EXIT
printf '%s\n' 'Docker runtime smoke passed.'
