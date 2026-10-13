# Codex Usage Limits

ChatGPT アカウントに適用される Codex の利用制限と、その残量・リセット情報を扱うコンテキストです。

## Language

**Rate limit（利用制限）**:
Codex の利用に適用される、一定期間の使用量の上限です。
_Avoid_: 残高、クレジット

**Window（制限期間）**:
利用制限が適用される期間です。このコンテキストでは5時間と週間の2種類を扱います。
_Avoid_: セッション、集計間隔

**Remaining ratio（残量比率）**:
制限期間の利用上限に対する未使用分の割合です。0は使い切った状態、1は全量が残っている状態を表します。
_Avoid_: 使用率、残りトークン数

**Reset（リセット）**:
制限期間の使用量が更新され、その期間の利用枠が再び使用可能になることです。
_Avoid_: キャッシュ更新、認証更新

## Architecture

エクスポーターは Rust の単一プロセスで HTTP、Prometheus 形式への変換、
TTL キャッシュを実装します。Codex CLI は各キャッシュ更新で起動する短命の
stdio app-server とし、`initialize` → `initialized` → `account/rateLimits/read`
で利用制限を取得します。直接 API 呼び出しや永続 app-server は使用しません。

SIGINT / SIGTERM では新しい HTTP 接続の受付を停止し、実行中の取得処理を
タイムアウトまで待って child process を終了・回収してから終了します。
Codex の応答バッファは 1 MiB までで、認証保存は引き続き Codex CLI の
file credential store と `CODEX_HOME` に任せます。

この移植では性能計測・ベンチマークを行わず、設定とメトリクスの互換性を
機能テストで確認します。
