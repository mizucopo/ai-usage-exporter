# ai-usage-exporter

Python 3.14 の application 構成です。現在は開発・Docker 配布用の初期設定のみで、使用量の収集・出力処理は未実装です。

## 開発

```bash
uv sync --locked
uv run task check
```

実装は `src/`、テストは `tests/` に配置します。

## Docker

```bash
docker build --check .
docker build -t ai-usage-exporter:local .
docker run --rm ai-usage-exporter:local
```

現時点の起動コマンドは `python --version` です。アプリケーション実装時に Dockerfile の `CMD` を実行対象へ変更してください。イメージには開発用依存関係を含めません。

Dockerfile、`uv.lock`、`.dockerignore` のアプリケーション入力の許可項目は、このリポジトリで管理します。それ以外の生成ファイルは Copier の標準出力を採用しています。Docker の `COPY` 対象を追加するときは `.dockerignore` の許可項目も更新してください。

## Docker Hub への公開

公開先は `mizucopo/ai-usage-exporter`、ログイン名は `mizucopo` です。GitHub Actions の repository secret `DOCKERHUB_TOKEN` に、このイメージへの push 権限を持つトークンを設定してください。

`main` への push で、Docker イメージのバージョンタグと `latest`、Git tag、GitHub Release を作成します。バージョンは `pyproject.toml` の `project.version` を使います。PR では Python 品質チェックと Docker ビルドを実行します。
