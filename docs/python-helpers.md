# Python 補助処理

Rust 移行後も、Python を開発・CI の補助処理に使用する。exporter 本体、配布バイナリ、最終 Docker image の runtime に Python/uv を導入しない。通常の Rust 品質ゲートと配布時の Python 検証は別の役割を持つ。

## 残存の棚卸しと判断

| 対象 | 用途・呼び出し元 | 判断 |
| --- | --- | --- |
| `.github/scripts/release.py` | `release-classification.yml` の分類検証、`docker-release.yml` の採番・Rust matrix/build・image 照会・Release 配布・Latest 判定 | 現役の共通 controller として保持。共通 release workflow と連携し、採番、同一 commit/tag、既存 image 所有権、再実行、draft 完成後の公開、prerelease/Latest を保護する。image 所有権の検証・予約は workflow が `manage-docker-image-owner.sh` を呼ぶ |
| `tests/release_smoke.py` | `rust-quality-checks.yml` の native 配布検証と `docker-release.yml` の公開前検証 | 製品固有の現役テストとして保持。共通の archive/checksum 処理を使い、展開した exporter の HTTP・cache・mock Codex 通信・終了と子回収を検証する |
| `rust-quality-checks.yml` の inline Python / Python setup | `release.json` の選択 target と controller の runner 定義から native matrix を生成し、配布 smoke を実行する | 現役の CI 依存として保持。対象一覧・命名・packaging の別実装を作らない |
| `.gitignore` の Python / venv / pytest / coverage 除外 | Python 補助処理の bytecode/cache やローカル補助環境・生成物の Git 混入防止 | Copier 共通 policy として保持。除外の存在は pytest・venv を使うアプリや CI があることを意味しない |
| `.dockerignore` の Python cache / venv 除外 | source 内に置かれたローカル生成物も Docker build 入力から除外する | deny-all と Rust 入力の allowlist とともに保持。Python helper と tests を runtime image にコピーしない |
| `docs/docker-build-context.md` の Python 構成説明 | Copier 共通文書が説明する選択肢 | 保持。この製品の回答は `use_rust: true` / `use_python: false` で、現在の Docker build 入力は Rust 構成だけ |
| `docs/release.md` の Python hook 例 | 製品が追加同梱物を用意する場合の共通拡張例 | 保持。現在の `publication.rust.stage_command` は `[]` で、この例だけで製品に追加の Python hook があるとは扱わない |
| `src/python_number.rs` と関連 Rust tests | 移行前の設定値が受け付けた Python 数値構文の互換性を維持する Rust 実装 | 保持。Python アプリ source や Python runtime 依存ではない |
| `.github/scripts/docker-smoke.sh` の Python/uv 不在検査と README の runtime 説明 | 最終 image の runtime 契約を検証・説明する | 保持。mock scrape、Rust exporter、Codex CLI と runtime 依存の確認を継続する |

Issue #27 の調査開始時点の main `f3c31d6e499ba432c99bc090b383762599b4082e` には、`pyproject.toml`、`uv.lock`、`.python-version`、requirements/pytest 設定、`.pyi`、旧 Python アプリ source はない。旧 Rust 移行 Issue #14 の履歴と、上の現役補助処理・互換性の説明を区別する。削除可能な旧アプリ資産は見つからず、ファイル削除による整理は行わない。

## 開発・CI 依存

- 通常の品質ゲートは README の `cargo fmt`、`cargo clippy --locked`、`cargo test --locked` を使う。ローカル mock を使うため、実アカウントの認証や課金 API でテストしない。
- Python helper は標準ライブラリだけを使う。Python package の install、uv、pytest、venv は必須ではないため、Python アプリ用 manifest や lockfile を再導入しない。
- native 配布 job は `actions/setup-python` で Python 3.14 を用意する。共通 controller を呼ぶ Ubuntu job は runner の `python3` を使う。ローカルでは Python 3.12 以降を使用する。`tomllib`、SHA-256、tar/ZIP の展開を含む処理を、Python 3.12 と CI の 3.14 で検証する。
- 配布 smoke は Python に加えて Cargo/rustc を必要とする。Rust fixture を native mock CLI としてコンパイルし、一時 `CODEX_HOME` と mock だけの PATH を exporter に渡す。Codex CLI の実ユーザー認証をテストに使わない。

Python を用意した開発環境での配布検証例（Windows は `python3` を `python` に置換し、target を `x86_64-pc-windows-msvc` / `aarch64-pc-windows-msvc` にする。macOS ARM64 は `aarch64-apple-darwin`）:

```bash
python3 -I tests/release_smoke.py --target x86_64-unknown-linux-gnu --output target/release-smoke
```

選択した target と実行ホストの OS/CPU は一致させる。この検証はローカル build、archive、checksum、展開済み exporter の mock 動作を確認する。採番・image push・GitHub Release 公開・Latest 昇格は行わない。公開 workflow では、共通 build の成果物を `--verify-existing` で検証してから保持・添付する。

## 保持・移行と上流の境界

現時点では、controller、配布 smoke、matrix 読み出しを Python で維持する。これらは runtime の Rust 移行を妨げず、共通機構と既存の native/mock 検証を再利用できる。Python ファイルがあることだけを理由に削除・Rust への移植をしない。

controller と共通 policy・文書は repo-template 所有で、製品独自に撤廃・書換えない。将来、CI を Python 不要にする場合は、まず上流で controller の設計と代替の回帰検証を決め、Copier update で取り込む。製品側では配布 smoke と matrix の呼び出し元・依存・文書を同時に移行し、Rust 品質、全対象の archive/checksum/mock 起動、同一 commit/tag、再実行と公開/Latest の保護を維持する。

[repo-template #204](https://github.com/mizucopo/repo-template/issues/204) の公開直後 image 確認の再試行は共通 controller の別課題である。この整理では controller・release workflow・Copier revision を変更せず、上流の修正を置き換えない。最終配布物の Python/uv 不要契約と Docker の不在検査も維持する。
