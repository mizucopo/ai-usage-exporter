# ai-usage-exporter

Codex の5時間・週間の利用制限について、残量とリセットまでの時間を Prometheus に公開する Python 3.14 のカスタムエクスポーターです。単一の ChatGPT アカウントを対象に、Codex CLI の `account/rateLimits/read` で取得した情報を `/metrics` から公開します。

## Docker で起動

イメージには Codex CLI `0.153.4` の公式 Linux musl バイナリを SHA-256 検証して同梱します。Dockerfile は `linux/amd64` と `linux/arm64` に対応し、開発用依存関係を含めません。

```bash
docker build --check .
docker build -t ai-usage-exporter:local .
docker volume create ai-usage-exporter-codex

# 初回のみ実行し、表示された案内に従って ChatGPT アカウントでログイン
docker run --rm -it \
  -v ai-usage-exporter-codex:/var/lib/codex \
  --entrypoint codex ai-usage-exporter:local \
  -c 'cli_auth_credentials_store="file"' login --device-auth

docker run -d --name ai-usage-exporter \
  -p 9173:9173 \
  -v ai-usage-exporter-codex:/var/lib/codex \
  ai-usage-exporter:local

curl http://localhost:9173/metrics
```

認証情報の保存先は書き込み可能な専用 `CODEX_HOME` ボリュームです。ログインと通常起動は同じユーザー（UID/GID `10001`）で動作し、認証情報の保存・更新は Codex CLI に任せます。エクスポーターもファイルへの認証保存を指定します。認証未設定でも HTTP は起動し、メトリックスで取得失敗を報告します。再ログインする場合も同じログインコマンドとボリュームを使用してください。

## 設定

環境変数で変更できます。

| 環境変数 | デフォルト | 内容 |
| --- | --- | --- |
| `AI_USAGE_EXPORTER_HOST` | `0.0.0.0` | HTTP 待受アドレス |
| `AI_USAGE_EXPORTER_PORT` | `9173` | HTTP ポート（1〜65535） |
| `AI_USAGE_EXPORTER_CACHE_TTL_SECONDS` | `300` | 取得結果のキャッシュ秒数（正の有限値） |
| `AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS` | `10` | Codex 起動・初期化・取得全体のタイムアウト秒数（正の有限値） |
| `CODEX_HOME` | Docker: `/var/lib/codex` | Codex CLI の認証情報などの保存先 |

例えばキャッシュを60秒に変更する場合、`docker run` に `-e AI_USAGE_EXPORTER_CACHE_TTL_SECONDS=60` を追加します。コンテナー内のポートを変更する場合は `-p` の転送先ポートも合わせてください。

初回 scrape で取得し、同時 scrape の取得処理は1回にまとめます。キャッシュは設定期間か次のリセット時刻の早い方で失効します。リセットまでの秒数は scrape ごとに現在時刻から再計算します。

認証・通信・応答解析の失敗時は、残量・リセットのメトリックスを公開せず、成功フラグを `0` にします。次の取得は設定したキャッシュ期間だけ待ちます。過去のリセット時刻を含む応答でも連続取得しません。

## メトリックス

| メトリックス | 内容 |
| --- | --- |
| `codex_rate_limit_remaining_ratio` | 残量（0〜1） |
| `codex_rate_limit_reset_timestamp_seconds` | リセットの Unix 時刻 |
| `codex_rate_limit_reset_seconds` | リセットまでの秒数（最小0） |
| `ai_usage_exporter_scrape_success` | 最新取得結果が有効なら1、失敗なら0 |
| `ai_usage_exporter_last_success_timestamp_seconds` | 最終取得成功の Unix 時刻。成功前は0 |

上の3メトリックスは `window="5h"` と `window="weekly"` の2系列を公開します。残量は使用率25%なら `0.75` です。`codex` の制限情報から期間の長さで5時間・週間を識別し、両方の残量とリセット情報が揃わない場合は取得失敗とします。取得成功フラグはキャッシュ内の最新取得結果を示し、毎回 Codex にアクセスしたことは意味しません。

Prometheus 設定例（エクスポーターと同じホストで実行する場合）:

```yaml
scrape_configs:
  - job_name: codex
    scrape_interval: 30s
    scrape_timeout: 15s
    static_configs:
      - targets: ["localhost:9173"]
```

`scrape_timeout` は取得タイムアウトに HTTP 処理の余裕を加えた値にしてください。Prometheus を別コンテナーで動かす場合は、接続可能なエクスポーターのホスト名に置き換えてください。

## 開発

```bash
uv sync --locked
uv run task check
PYTHONPATH=src uv run python -m ai_usage_exporter
```

ローカル実行には `PATH` 上の Codex CLI とログイン済みの `CODEX_HOME` が必要です。実装は `src/`、テストは `tests/` に配置します。

Dockerfile、`uv.lock`、`.dockerignore` のアプリケーション入力の許可項目は、このリポジトリで管理します。それ以外の生成ファイルは Copier の標準出力を採用しています。Docker の `COPY` 対象を追加するときは `.dockerignore` の許可項目も更新してください。

## Docker Hub への公開

公開先は `mizucopo/ai-usage-exporter`、ログイン名は `mizucopo` です。GitHub Actions の repository secret `DOCKERHUB_TOKEN` に、このイメージへの push 権限を持つトークンを設定してください。

`main` への push で、Docker イメージのバージョンタグと `latest`、Git tag、GitHub Release を作成します。バージョンは `pyproject.toml` の `project.version` を使います。PR では Python 品質チェックと Docker ビルドを実行します。
