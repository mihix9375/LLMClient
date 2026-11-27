pub fn cli_help_text(version: &str) -> String {
    format!(
        "Qwen27 {v} — 統合 CLI / GUI クライアント\n\n\
使い方:\n\
  Qwen27 [オプション]                       GUI 起動 (Linux ヘッドレス TTY では自動で対話 CLI)\n\
  Qwen27 --cli | --chat | -i | chat         対話ターミナルモード (REPL)\n\
  Qwen27 -p <プロンプト> [オプション]       非対話ワンショット実行 (stdout に回答本文)\n\
  Qwen27 run <プロンプト> [オプション]      非対話ワンショット実行\n\
  echo \"質問\" | Qwen27 [オプション]         標準入力パイプから実行\n\
  Qwen27 <サブコマンド> [引数] [オプション] API サブコマンド実行\n\
  Qwen27 --api <サブコマンド> ...           互換 NDJSON モード (qwen.ps1 互換)\n\n\
主なサブコマンド:\n\
  health, connect [url], identity, commands, profiles, adapters, adapter-profiles,\n\
  conversations, conversation <id>, status [id], fetch|save <id>, cancel <id>,\n\
  answer <id> <qid> <text>, interrupt <id> <text>, compact <cid> [focus],\n\
  memory [list|add|delete], schedules [list|add|delete], share <cid> [list|add|remove],\n\
  files <cid> [path], upload <cid> <file...>, classify <text>,\n\
  settings, configure, preset <code|balanced|creative>, reset-sampling, gui-settings,\n\
  models [status|start|stop|restart|perf], perf, autotune, tune-status, tune-apply,\n\
  windows, standby, help, version\n\n\
主なオプション:\n\
  -p, --prompt <text>         実行プロンプト          --file <path>         ファイルから読み込み\n\
  --work-mode <mode>          standard|chat|coding|pc_control\n\
  --effort <level>            none|low|medium|xhigh   --budget <N>          推論予算トークン\n\
  --context <N> / --tokens <N> ウィンドウ/最大出力    --preset <name>       サンプリングプリセット\n\
  --url <url>                 API 接続先 URL          --auto-connect / --no-connect\n\
  --conv <id> / --parent <id> 会話ID / 親ジョブID     --full                ジョブ完全詳細取得\n\
  --filter <all|active|done>  履歴絞り込み            --query <text>        履歴検索語\n\
  --attach <path>             添付ファイル (最大5件)  --attach-clipboard    クリップボード添付\n\
  --workdir <dir|clear>       作業フォルダー          --read-dir <dir>      読み取り専用資料\n\
  --remove-read-dir <dir>     資料フォルダー解除      --clear-read-dirs     資料フォルダー全解除\n\
  --pc / --no-pc              PC画面操作許可          --window <title|hwnd> 対象ウィンドウ\n\
  --standby / --shared-desktop 連携待機 / 共有PC      --dark-theme / --light-theme\n\
  --json                      構造化 JSON 出力        --stream / --no-stream\n\
  -q, --quiet                 進捗表示を抑制          --save / --output <dir|path>\n\
  --no-sync-gui / --sync-gui  gui-settings.json 同期  --no-color            ANSI 装飾を無効化\n\
  -h, --help / -V, --version  ヘルプ / バージョン表示",
        v = version
    )
}

pub const SLASH_HELP: &str = "\
依頼を入力すると実行します（同一セッションで会話履歴を引き継ぎ）。
[接続・基本]
  /connect [url]  /url [url]        サーバーへ接続・切替（初回証明書自動取得）
  /auto-connect [on|off]            起動時自動接続の表示・切替
  /status [id] [full]               接続状態またはジョブ詳細表示 (?full=1 対応)
  /identity  /commands  /profiles   アクセス権限・コマンド・モデル情報
  /adapters  /adapter-profiles      LoRA アダプター情報
  /theme [dark|light|toggle]        テーマ切替 (/exit /quit で終了)

[会話・履歴・実行]
  /new  /clear                      新しい会話を開始
  /history [N|all|active|done|語句] 最近の会話一覧・絞り込み表示
  /open [id|#番号]  /show [id|#番号] 会話を読み込み・切り替え
  /conv [id|#番号|clear]            会話IDを表示/切替/解除
  /resume <jobId>                   完了ジョブから再開
  /continue  /retry                 「つづけて」送信 / 直前依頼を再送
  /copy                             直前の回答をクリップボードへコピー
  /compact [focus]                  現在の会話を要約・圧縮
  /fetch [id] [dir]  /save ...      成果物・ワークスペース一括保存
  /cancel [id]  /interrupt <id> <文> ジョブ中断 / 実行中への追加指示
  /multi (/end で送信, /cancel 中止) 複数行入力

[agy スラッシュコマンド (Direct 27B)]
  /plan <task>  /goal <task>        計画作成 / 完遂自律ループ
  /grill-me <topic>  /boost <task>  設計インタビュー / 推論強化
  /teamwork <task>  /browser <task> 工程分離完遂 / Web・画面調査優先

[添付・ワークスペース・共有]
  /attach [list|path|paste|remove|preview|clear]  /paste
  /files [list|path|save <path> [dest]]   サーバー側ワークスペース閲覧・保存
  /upload <file...>                       現在の会話へファイルアップロード
  /share [list|add <ip> [mode]|remove <ip|#番号>]  IP別アクセス共有
  /workdir [path|clear]  /reset  /pwd  /open-folder
  /read-dir [list|path|remove <N|path>|clear]

[モデル・設定・PC操作]
  /mode [standard|chat|coding|pc_control]  /effort [none|low|medium|xhigh]
  /role [planner|worker]  /plan [on|off]   /preset <code|balanced|creative>
  /reset-sampling  /budget <N>  /context <N>  /tokens <N>
  /temp /top-p /top-k /min-p /repeat-penalty /presence-penalty /frequency-penalty <値>
  /models [status|start|stop|restart|perf]  /perf
  /autotune [quick|thorough|coding|chat|apply]  /tune-status  /tune-apply
  /settings  /gui-settings  /learn [list|delete <id>|<note>]
  /schedule [list|cancel <id>|in 10m <task>|every 1h <task>]
  /workspace /local-cli /run-on-pc /pc [on|off]  /shared-desktop [on|off]
  /windows [q]  /window [name|#N|hwnd|clear]  /standby [on|off|status|win]  /stop-pc
  /classify <text>                  テキスト分類を実行";
