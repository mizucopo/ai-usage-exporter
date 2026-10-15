# Docker build context

`.dockerignore` は `**` で全入力を除外し、`Dockerfile` と選択した言語の通常 build 入力だけを許可します。Rust は `Cargo.toml`・`Cargo.lock`・`rust-toolchain.toml`・`src/`、Python は `pyproject.toml`・`uv.lock`・`.python-version`・`src/` を許可し、併用時は両方を含みます。`.git`・`.env`・Python cache・仮想環境は source 内でも除外します。root の Cargo `target/` は deny-all で除外し、`src/target/` 等の正当な source は保持します。Dockerfile は project が用意してください。

Python の `application` は src 直下の module を直接実行し、依存だけを install します。`package/library` は src 内の import package と build system を使い、project 自身も install します。独自 metadata が参照する README・LICENSE、package data、追加 lockfile 等が必要なら、その実際の path だけを追加してください。

## Policy の配置

Docker は選択した build context の直下にある `.dockerignore` を読みます。root の `.dockerignore` は root context 用です。Docker quality の `docker_build_context` が `docker/` 等の subdirectory の場合、その直下にも同じ既定 policy を生成します。Docker release は引き続き root context を使います。

`docker_build_context` は repository root からの相対 path で、ローカル directory を指定します。URL・Git remote（`github.com/...` の省略形、ユーザー名を省略した `host:path` も含む）・stdin の `-` は対応しません。remote 側や stdin の入力には、この repository に生成した policy が適用されないため入力時に拒否します。colon を含むローカル directory は `./docker:local` のように `./` を付けるか、`docker/app:local` のように slash の後に colon を置きます。

quality context の policy は template の除外 directory や project 所有の Tauri icon directory 内でも Copier 管理します。他の除外ファイルや icon の所有は維持します。`.git` 管理 directory は policy を commit・checkout できないため、context に指定できません。

各 policy の path はその context からの相対 path です。例えば `docker/` context の `!src/**` は `docker/src/` を許可し、root の `src/` は参照しません。必要な入力を context 内に配置し、project 固有の追記は実際に使う `.dockerignore` へ追加してください。root への追記は他 context へ自動転記しません。`dockerfile_path` は context 内外の独自 Dockerfile を指定でき、Docker は build 定義を別途読み込みます。

Dockerfile の隣に `<Dockerfile名>.dockerignore` がある場合は、context 直下の policy より優先されます。project が用意するこのファイルや Docker project hook の追加 context にも、deny-all と必要な入力の明示許可を適用して検証してください。選択した quality context 以外の policy は自動生成しません。配置と優先順位は [Docker の仕様](https://docs.docker.com/build/concepts/context/#dockerignore-files) を参照してください。

## Project 固有の入力

Dockerfile の `COPY`・`ADD` が必要とする入力を、使用する context の `.dockerignore` の末尾へ明示追加します。ディレクトリは親も許可してください。独自 `build.rs`・workspace member・設定も同じ方法で追加できます。例えば Rust の test stage 用に以下を追加します。

```dockerignore
!tests/
tests/**
!tests/*.rs
!tests/fixtures/
tests/fixtures/**
!tests/fixtures/public-test-ca.pem
```

Docker の `!directory/` は子も許可するため、上の例は親を許可した直後に `directory/**` で子を再度除外し、必要なファイルだけを許可しています。fixture は確認した公開テスト用ファイルだけを指定します。全 project で `tests/**`・`tests/fixtures/**` を許可する既定値はありません。switchbot-exporter のようにディレクトリ全体が公開テスト用と確認できた project は、個別に `!tests/fixtures/**` を追加できます。不要な入力や秘密を含むディレクトリ全体を許可しないでください。

build context に含めても runtime image への包含は別です。Dockerfile の test stage だけに `COPY tests ./tests` し、runtime stage には必要な成果物だけをコピーしてください。

## Copier 更新

root と選択した quality context の `.dockerignore` は Copier 管理のまま、project の追加を commit して clean な専用 branch で `copier update --trust --defaults --vcs-ref HEAD` を実行します。言語別入力や context policy を追加する更新と project の追記が競合する場合、Copier は両方を conflict marker 内に残します。既存の context 固有 policy がある場合も、template の言語別入力と必要な project 入力を残して marker を除去し、完全な差分を確認してください。`copier recopy` による上書きは使いません。

Copier 9.17.1 は Unicode path の同一ファイルを project と template が両方追加する処理に制約があります。日本語・アクセント付き文字などの context に既存の未管理 `.dockerignore` がある場合、初回の採用前にそのファイルを repository 内の未使用の ASCII 名（例えば `docker-context-policy.backup`）へ `git mv` し、commit して clean な状態にします。更新後、新 policy と backup の必要な project 入力を手動で統合し、確認してから backup を削除します。

Unicode context の更新では、Git の path quoting をこの command だけ有効にします。global・project の Git 設定は変更しません。既存の未管理 policy がない場合や、採用後の通常更新では事前の退避は不要です。

```sh
GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.quotePath GIT_CONFIG_VALUE_0=true \
  copier update --trust --defaults --vcs-ref HEAD
```

更新後は実際の build と、利用する test stage を検証します。allowlist 外の `.git`・`.env`・ローカル成果物・cache 等が context に入らず、必要な manifest・source・テスト入力だけが含まれることを確認してください。
