# ai-usage-exporter

Codex の5時間・週間の利用制限について、残量とリセットまでの時間を Prometheus に公開する Rust のカスタムエクスポーターです。単一の ChatGPT アカウントを対象に、Codex CLI の `account/rateLimits/read` で取得した情報を `/metrics` から公開します。

## バイナリで起動

[GitHub Releases](https://github.com/mizucopo/ai-usage-exporter/releases) の Downloads から、自分の OS / CPU に合う archive と同名の `.sha256` を取得します。次の公開から、同じ version / tag に以下の4種類を添付します。過去の Release にない添付ファイルは追加しません。

| OS / CPU | Archive | 実行ファイル |
| --- | --- | --- |
| Linux x86_64 (GNU) | `ai-usage-exporter-<tag>-linux-x86_64.tar.gz` | `ai-usage-exporter` |
| Windows x86_64 / x64 | `ai-usage-exporter-<tag>-windows-x86_64.zip` | `ai-usage-exporter.exe` |
| Windows ARM64 | `ai-usage-exporter-<tag>-windows-arm64.zip` | `ai-usage-exporter.exe` |
| macOS Apple Silicon / ARM64 | `ai-usage-exporter-<tag>-macos-arm64.tar.gz` | `ai-usage-exporter` |

Windows ZIP は exporter の `.exe`、Linux / macOS の tar.gz は実行ファイルを archive 直下に含みます。Codex CLI は同梱しません。[公式の導入手順](https://github.com/openai/codex#installation)に従って同じ OS / CPU の Codex CLI を別途用意し、`codex --version` が通る PATH に置きます。Windows は native `codex.exe` または公式 npm package の `codex.cmd` と、その package に含まれる native executable に対応します。任意の `.cmd` / PowerShell wrapper は使えません。公式 native archive を使う場合は、実行ファイルを `codex.exe` にリネームして PATH に配置します。

Windows の MSVC バイナリは `VCRUNTIME140.dll` を使用します。起動前に [Microsoft Visual C++ v14 Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist) の最新の対応 package が導入されていることを確認してください。exporter が x64 なら X64、ARM64 なら ARM64 を選びます。runtime は archive に同梱しません。`VCRUNTIME140.dll` が見つからない場合は、この package を別途導入します。

exporter は scrape 時に Codex の `app-server` を子プロセスとして起動し、認証の保存方式に `file` を指定します。CLI と exporter を同じユーザー・同じ書き込み可能な `CODEX_HOME` で実行し、開始前に Codex CLI でログインしてください。既に file 方式でログイン済みなら、その `CODEX_HOME` を使えます。keyring だけに認証が保存されている場合は、運用者が CLI の file 方式でログインを用意する必要があります。exporter 自身はログインを行いません。

Windows PowerShell（`<tag>` を Downloads の tag に置換）:

```powershell
$archive = 'ai-usage-exporter-<tag>-windows-x86_64.zip' # ARM64 は windows-arm64.zip
$expected = (Get-Content "$archive.sha256").Split(' ')[0]
if ((Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) {
    throw 'SHA-256 mismatch'
}
Expand-Archive $archive -DestinationPath .\ai-usage-exporter
$env:CODEX_HOME = Join-Path $env:USERPROFILE '.codex-exporter'
# 初回のみ、表示された案内に従ってログイン
codex -c 'cli_auth_credentials_store="file"' login --device-auth
$env:AI_USAGE_EXPORTER_HOST = '127.0.0.1'
$env:AI_USAGE_EXPORTER_PORT = '9173'
.\ai-usage-exporter\ai-usage-exporter.exe --help
.\ai-usage-exporter\ai-usage-exporter.exe
# 別の PowerShell から: (Invoke-WebRequest http://127.0.0.1:9173/metrics).Content
```

macOS ARM64（Linux は archive 名と `sha256sum -c` に置換）:

```bash
archive='ai-usage-exporter-<tag>-macos-arm64.tar.gz'
shasum -a 256 -c "$archive.sha256"
mkdir -p ai-usage-exporter
tar -xzf "$archive" -C ai-usage-exporter
export CODEX_HOME="$HOME/.codex-exporter"
# 初回のみ、表示された案内に従ってログイン
codex -c 'cli_auth_credentials_store="file"' login --device-auth
export AI_USAGE_EXPORTER_HOST=127.0.0.1
export AI_USAGE_EXPORTER_PORT=9173
./ai-usage-exporter/ai-usage-exporter --help
./ai-usage-exporter/ai-usage-exporter
# 別の terminal から: curl http://127.0.0.1:9173/metrics
```

停止は起動した terminal で `Ctrl+C`（Windows は `Ctrl+Break` も対応）を使います。macOS / Linux は `SIGTERM` でも停止できます。実行中の scrape は取得タイムアウトまで待って子プロセスを回収し、終了します。設定の既定値・キャッシュ・Prometheus の仕様は Docker と共通で、下の環境変数表を参照してください。バイナリの署名・macOS notarization は提供していません。

PR CI は4対象の native runner で、mock CLI による通信・`/metrics`・キャッシュ・停止、および配布 archive の checksum・展開・起動を検証します。これは実アカウント・一般利用者の実機での確認とは別です。実認証・課金 API はテストで使用しません。

## Docker で起動

イメージには Codex CLI `0.153.4` の公式 Linux musl バイナリを SHA-256 検証して同梱します。Dockerfile は `linux/amd64` と `linux/arm64` に対応します。最終イメージは Alpine と Rust の実行バイナリ、Codex CLI、CA 証明書で構成し、Python runtime・uv・venv・開発用依存関係を含めません。

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

上の3メトリックスは、`codex` の制限情報から期間の長さで5時間・週間を識別し、返された枠だけを `window="5h"` または `window="weekly"` で公開します。週間枠のみ・5時間枠のみでも、有効な残量とリセット情報があれば取得成功とします。`null` または省略された枠は推測・補完せず、前回取得時に存在していても公開しません。残量は使用率25%なら `0.75` です。

両枠がない場合や、返された枠に不正データ・未対応の期間・期限切れ・同一期間の重複がある場合は取得失敗とし、残量・リセットのメトリックスを公開しません。取得成功フラグはキャッシュ内の最新取得結果を示し、毎回 Codex にアクセスしたことは意味しません。

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

アプリ本体と通常の品質ゲートは Rust です。Python は共通 release controller と配布 archive の mock 検証に使用する開発・CI 補助依存として残します。native 配布 CI は Python 3.14 を用意します。ローカルの配布検証では Python 3.12 以降と Rust toolchain を使用し、Python の外部 package、uv、pytest、venv は必須ではありません。最終 Docker image と配布バイナリは Python/uv を要求しません。残存ファイル・呼び出し元・保持理由と上流との境界は [Python 補助処理](docs/python-helpers.md) を参照してください。

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo run --locked --bin ai-usage-exporter
```

ローカル実行には `PATH` 上の Codex CLI とログイン済みの `CODEX_HOME` が必要です。実装は `src/`、テストは `tests/` に配置します。

Dockerfile、`Cargo.toml` の依存関係、`Cargo.lock`、`.dockerignore` のアプリケーション入力の許可項目は、このリポジトリで管理します。それ以外の生成ファイルは Copier の標準出力を採用しています。Docker の `COPY` 対象を追加するときは `.dockerignore` の許可項目も更新してください。

## Docker Hub への公開

公開先は `mizucopo/ai-usage-exporter`、ログイン名は `mizucopo` です。GitHub Actions の repository secret `DOCKERHUB_TOKEN` に、このイメージへの push 権限を持つトークンを設定してください。

PR 全体の差分を分類し、公開する PR には `release:patch`・`release:minor`・`release:major` のラベルを一つ付けます。採番元は `Cargo.toml` の `package.version` で、GitHub Actions が squash merge 後に採番し、Docker イメージ、Git tag、GitHub Release を公開します。PR では Rust 品質チェックと Docker ビルドを実行します。詳細は [CONTRIBUTING.md](CONTRIBUTING.md) と [docs/release.md](docs/release.md) を参照してください。

今回の Rust 移植では性能計測・ベンチマークを行っていません。メモリ、イメージサイズ、起動・応答時間の改善量は未確認です。
