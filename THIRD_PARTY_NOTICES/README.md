# 第三者のソフトウェアについて（LLMClient）

GUI の `Qwen27.exe`（`gui/` のビルド結果）は、Rust のクレートを静的に取り込んでいます。リリースの zip にもこのフォルダーの内容を入れています。

- `rust-crates.md` : 取り込まれるクレートの一覧（名前・版・ライセンス）
- `rust-crates-licenses.txt` : 上記クレートのライセンス文・NOTICE の全文（同じ文面はまとめて、対象クレートを併記）

## ライセンスの種類と義務

| ライセンス | 主なクレート | 義務 | 対応 |
|---|---|---|---|
| MIT / BSD-2 / BSD-3 / ISC / Zlib / 0BSD / Unlicense | 多数（iced、tokio、serde、windows、rustls 系など） | 著作権表示と許諾文をバイナリ配布にも付ける | `rust-crates-licenses.txt` |
| Apache-2.0（単独または OR の選択肢） | winit、wgpu 系、ring（ISC と併用）など | ライセンス文の同梱、NOTICE があればそれも同梱、変更したら明記 | 全文と NOTICE を `rust-crates-licenses.txt` に収録。クレートは無改変 |
| Unicode-3.0 | unicode-ident、ICU 系（icu_*、zerovec、yoke、tinystr など） | 著作権表示と許諾文を付ける | `rust-crates-licenses.txt` |
| BSL-1.0 | clipboard-win、error-code | バイナリ配布ではライセンス文の同梱は不要 | 全文を念のため収録 |
| CC0-1.0 | hexf-parse（imgref は Apache-2.0 との OR） | なし（パブリックドメイン相当） | 一覧のみ |
| `Apache-2.0 OR GPL-2.0-only` | self_cell | OR のため Apache-2.0 を選択（GPL は適用しない） | 一覧に記載 |
| ring の独自の併記（ISC / OpenSSL 由来） | ring | ring の LICENSE にある表示を付ける | `rust-crates-licenses.txt` に収録 |

GPL / LGPL / MPL など、配布条件を強く縛るライセンスだけのクレートは、Windows 向けの `Qwen27.exe` には含まれません（`cargo metadata` で依存を確認）。

## 同梱していない物

- フォントは同梱していません（Windows に入っている書体を名前で指定）。アイコンはコードで描画しています。
- Windows の DLL・Vulkan ローダーは実行する PC のものを使い、再配布していません。
- `client.mjs` / `src/` の Node.js CLI は自作のコードのみで、npm の依存はありません。