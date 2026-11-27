# LLMClient

**LLMServer**（別リポジトリ）の API に接続するクライアントです。

```
LLMClient (GUI Qwen27.exe / CLI client.mjs) ──HTTPS/HTTP──> LLMServer ──> llama-server
```

## 構成

| 場所 | 内容 |
|---|---|
| `gui/` | Windows 用 GUI `Qwen27.exe`（Rust、iced）のソースとビルドスクリプト。Linux 向けの CLI としてもビルドできます |
| `client.mjs`, `src/` | Node.js 22 以上の CLI クライアント（`node client.mjs`、使い方は下の「使い方（CLI）」） |
| `qwen.cmd`, `qwen.ps1` | `Qwen27.exe` を呼ぶ PowerShell のラッパー（同じフォルダーに `Qwen27.exe` が必要） |
| `THIRD_PARTY_NOTICES/` | 取り込んでいる第三者のクレートのライセンス表示 |

ビルド済みの `Qwen27.exe` は GitHub の Releases にあります（zip に `LICENSE` と `THIRD_PARTY_NOTICES` を入れています）。

## できること

- サーバーのジョブの作成・進捗の表示・結果の取得・質問への回答、会話の履歴
- 添付ファイルの送信、作業フォルダー・PC の画面操作のサーバーへの中継（許可した範囲のみ）
- サーバーのモデルの起動・停止、サンプリングなどの設定

## 必要な物

- GUI のビルド: Rust（stable、MSVC ツールチェーン）、Visual Studio 2022 の C++ ビルドツール、Windows SDK
- CLI: Node.js 22 以上（npm の依存なし）
- 接続先の LLMServer（HTTPS で公開されたもの）

## ビルド（GUI）

```powershell
# Visual Studio の MSVC が既定の場所以外にある場合
$env:QWEN_MSVC_BASE = 'D:\VS2022\VC\Tools\MSVC'
.\gui\build.ps1 -OutputPath .\out\Qwen27.exe
```

- `-OutputPath` を省略するとリポジトリ直下の `Qwen27.exe` を上書きします。
- `build.ps1` は `cargo build --release --offline` を使います。初回など依存クレートがキャッシュに無いときは、先に `cargo fetch --manifest-path gui/Cargo.toml` を実行してください。
- `-Test` を付けると `cargo test` なども実行します。
- Windows と Linux の同時ビルドは `gui/build-all.ps1`（zig と cargo-zigbuild が必要）。

## 起動と設定

- 接続先は、GUI の設定画面、CLI の `--url`（または `--api-url`）、環境変数 `QWEN_API_URL` のどれかで指定します。未設定のときは `https://localhost:8443` を試します。
- サーバーの証明書を検証します。未登録の `https://` 接続先へ初回接続すると、`Qwen27.exe` が証明書の有効期限・SAN と `/health` 応答を検証したうえで `https-server.json`（SHA-256 ピン）と `certs/server.crt` を実行フォルダーへ自動生成・保存します（既存ピンと異なる証明書へ差し替わった場合は自動上書きせず拒否します）。明示的に証明書ファイルを指定する場合は `QWEN_CA_FILE` にパスを設定できます。
- 設定は実行フォルダーの `gui-settings.json` に保存されます（コミットしない）。設定例のファイルはありません。

## 使い方（CLI）

CLI は統合 CLI（`Qwen27.exe`）・互換 NDJSON（`Qwen27.exe --api`）・Node.js 版の 3 通りがあり、GUI で行える操作（接続・会話履歴の検索/閲覧・継続/再送・添付/クリップボード貼付・共有・ワークスペース閲覧/保存・PC連携待機/緊急停止・自動最適化・テーマ設定など）をすべて CLI からも実行できます。

| CLI | 起動 | 向いている使い方 |
|---|---|---|
| 統合 CLI（`Qwen27.exe`） | `Qwen27.exe --cli` / `-p "..."` / `<コマンド>` | 対話 REPL・パイプ・単発コマンドを `Qwen27.exe` 単体で使う時 |
| 互換 NDJSON CLI | `Qwen27.exe --api <コマンド>` | JSON / NDJSON で自動化スクリプトから呼ぶ時 |
| PowerShell のラッパー | `qwen.cmd <コマンド>` / `qwen.ps1` | Windows で `Qwen27.exe` を人が対話・実行する時 |
| Node.js 版 | `node client.mjs <コマンド>` | Node.js がある環境 |

### よく使うコマンド

```powershell
Qwen27.exe --cli                                 # 統合 対話 CLI (未接続でも起動し /connect で接続可)
Qwen27.exe connect --url https://<server>:8443   # 初回接続と https-server.json 自動生成
Qwen27.exe -p "やりたいこと"                     # 非対話ワンショット実行（stdout へ出力）
Qwen27.exe health                                # サーバーの状態
Qwen27.exe conversations --filter done           # 会話履歴一覧・絞り込み
Qwen27.exe conversation <会話ID>                 # 会話の詳細表示
Qwen27.exe fetch <ジョブID> --output .\dl        # 成果物＋ワークスペース一括保存
```

### コマンドの一覧

| 分類 | コマンド |
|---|---|
| 接続・情報 | `help`・`version`・`connect [URL]`・`health`・`identity`・`commands`・`windows`・`profiles`・`adapters`・`adapter-profiles` |
| ジョブ | `run [目的]`・`-p <目的>`・`status [ID] [--full]`・`fetch <ID>`・`save <ID>`・`cancel <ID>`・`answer <ID> <質問ID> <文>`・`interrupt <ID> <文>` |
| 会話 | `chat` / `--cli`・`conversations [--filter active|done] [--query 語]`・`conversation <ID>`・`compact <会話ID> [焦点]` |
| 覚え書き・予定 | `memory [list\|add <文>\|delete <ID>]`・`schedules [list\|add <指定> <目的>\|delete <ID>]` |
| ファイル・共有 | `files <会話ID> [path]`・`upload <会話ID> <file...>`・`share <会話ID> [list\|add <IP>\|remove <IP>]` |
| モデル・調整 | `models [status\|start\|stop\|restart\|perf]`・`perf`・`autotune`・`tune-status`・`tune-apply`・`standby` |
| 設定・分類 | `settings`・`configure`・`preset <code\|balanced\|creative>`・`reset-sampling`・`gui-settings`・`classify <文>` |

### 主なオプション

オプションは既定で `gui-settings.json` に保存され、GUI と共有されます（対話 REPL / ワンショットは既定でその回限り、`--sync-gui` で保存）。

| 分類 | オプション |
|---|---|
| 接続・出力 | `--url <URL>`・`--auto-connect` / `--no-connect`・`--json`・`--stream` / `--no-stream`・`-q` / `--quiet`・`--no-color` |
| 動き方 | `--work-mode <standard\|chat\|coding\|pc_control>`・`--role <planner\|worker>`・`--plan` / `--no-plan` / `--direct` |
| 量 | `--context <N>`・`--tokens <N>`・`--effort <none\|low\|medium\|xhigh>`・`--budget <N>` |
| サンプリング | `--preset <code\|balanced\|creative>`・`--temp`・`--top-p`・`--top-k`・`--min-p`・`--repeat-penalty`・`--presence-penalty`・`--frequency-penalty` |
| 作業の場所 | `--workspace` / `--no-workspace`・`--local-cli` / `--no-local-cli`・`--run-on-pc` / `--server-windows`・`--workdir <フォルダー\|clear>`・`--read-dir <フォルダー>`・`--remove-read-dir <フォルダー>`・`--clear-read-dirs` |
| PC の操作 | `--pc` / `--no-pc`・`--window <タイトル\|hwnd>`・`--standby` / `--no-standby`・`--shared-desktop`・`--dark-theme` / `--light-theme` |
| 入出力・履歴 | `--attach <ファイル>`・`--attach-clipboard`・`--conv <会話ID>`・`--parent <ID>`・`--file <goal.txt>`・`--output <先>`・`--full`・`--filter <all\|active\|done>`・`--query <語>`・`--local-only` |

### 対話モード (`--cli`) 内の主なスラッシュコマンド

- `/connect [url]`・`/open <#番号|ID>`・`/history [N|active|done|語句]`・`/continue`・`/retry`・`/copy`
- `/attach [list|path|paste|remove|preview|clear]`・`/paste`・`/files [list|path|save <path>]`・`/upload <file>`・`/share [list|add|remove]`
- `/plan <task>`・`/goal <task>`・`/grill-me <topic>`・`/boost <task>`・`/teamwork <task>`・`/browser <task>`
- `/mode`・`/effort`・`/preset`・`/reset-sampling`・`/autotune`・`/tune-status`・`/tune-apply`・`/models`・`/perf`・`/theme`
- `/workdir`・`/reset`・`/open-folder`・`/read-dir`・`/windows`・`/window`・`/pc`・`/shared-desktop`・`/standby [on|off|status]`・`/stop-pc`

`Qwen27.exe --api` と `qwen.cmd` もコマンド群に対応しています（`Qwen27.exe --help` または対話内の `/help` で全一覧）。

### GUI

`Qwen27.exe` を引数なしで起動し、設定画面で接続先を入れて使います。

## ライセンス

自作部分は MIT ライセンスです（`LICENSE`）。取り込んでいる Rust クレートのライセンス表示と義務は `THIRD_PARTY_NOTICES/` にまとめています。