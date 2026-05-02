# CLAUDE.md: gv

## ドキュメント方針

本プロジェクトは人間による開発作業がほぼ発生しないため、
コーディング規約・設計判断・実装上の注意点はCLAUDE.mdに集約する。
`docs/development/`配下は外部開発者向けにコンセプト・必要環境・セットアップ・
リリース手順・ドキュメントサイト運用・アーキテクチャ概要のみを置く。
`docs/guide/`配下は利用者向け文書である。

## 開発手順

- rust, node, pnpmなどはmise経由で実行する
- 普段使うのは`mise run format`（フォーマット + 軽量lint）と`mise run test`（全チェック実行）
- 全taskの一覧とリリース手順は[docs/development/development.md](docs/development/development.md)を参照
- コミット前の検証方法: `uvx pyfltr run-for-agent`
  - ドキュメントなどのみの変更の場合は省略可（pre-commitで実行されるため）
  - 修正後の再実行時は、対象ファイルや対象ツールを必要に応じて絞って実行する（最終検証はCIに委ねる前提）
    - 例: `uvx pyfltr run-for-agent --commands=cargo-clippy,cargo-test path/to/file`
- 作業完了時、コミットと並行してバックグラウンドでリリースビルド（`mise run build`）も実行する。
  完了は待たなくてよい（ユーザーによる動作確認をスムーズにするため）

## コーディング規約

### ロックのpoison

`Mutex::lock()` / `RwLock::read()` / `RwLock::write()`のpoisonは
「他スレッドがロック保持中にパニックした」ことを示し、
これは不変条件違反とみなしてプロセスを止めるのが安全である。

- `expect("<lock 名> lock poisoned")`形式でpanicさせてよい（Rust標準ライブラリも同様の慣例）
- メッセージは`"<lock 名> lock poisoned"`形式で統一する。
  これによりログでの追跡が容易になる
- 上記方針はsusie系を含むすべてのモジュールに適用する。
  `map_err`で`anyhow::Error`化する旧パターンは禁止し、新規・既存ともに`expect`形式へ揃える

### 配布TOMLのSSOT

配布TOMLが正規ソースとなる種別（キーバインド・既定ソート種別など）は、
TOML側を唯一のSSOTとし、ソースコード内に同等のハードコードデフォルトを置かない。

- ビルド時は`include_str!`で取り込み、起動時にパースして反映する
- Rust側の`Default`実装と配布TOMLの既定値が一致することを単体テストで保証する
- 配布TOMLが正しくパースできることも単体テストで保証する

例として、デフォルトキーバインドは`ぐらびゅ.keys.default.toml`を唯一のSSOTとして管理する。
パース可否と既定値の一致は`KeyConfig::parse_toml`の単体テスト
（`default_toml_parses_and_resolves`）で検証する。

### ファイル名の自然順比較

ファイル名の自然順比較は`shlwapi.dll`の`StrCmpLogicalW`を使う。
Windowsエクスプローラーの並びと一致させるため、先頭ゼロ付き数値の扱いがエクスプローラーと
乖離するクロスプラットフォーム実装（`natord`等）は採用しない。

### エラーハンドリング

- ユーザーが明示的に実行した操作（ファイル移動・コピー・保存・クリップボード操作など）が失敗した場合は、
  必ず`show_error_title()`でタイトルバーにエラーを表示する。
  `eprintln!`のみでの出力は禁止（ユーザーに見えない）
- Susieプラグインのロード・設定ファイルのパースなど、バックグラウンド処理や初期化時のエラーは
  `eprintln!`でstderrに出力する（デバッグ用）。
  フォールバック動作がある場合はそのまま続行してよい
- 可能な限り`Result`で呼び出し元に返し、`app.rs`のアクションハンドラでエラー表示を行う。
  中間層でエラーを握り潰さない

### Windowsバッチファイル生成

`cmd.exe`はバッチファイルをシステムのANSIコードページ（日本語環境ではCP932）で読む。
UTF-8で書くとDBCSバイト列が改行やコマンド構文を破壊する。

- ファイル名に日本語を含むバッチファイルはUTF-8 BOM + `chcp 65001`で書き出す。
  CP932だとcopyコマンド等のパス引数で日本語ファイル名を正しく解釈できない場合がある
- Rustの`format!`はLFのみ出力するため`replace('\n', "\r\n")`でCRLFに変換が必要
- `if ( ... )`ブロック内に日本語テキストがあると、DBCSトレイルバイトが特殊文字と誤認される。
  日本語を含む場合は`goto`で制御フローを構成する
- CP932変換が必要な場合は`WideCharToMultiByte(CP_ACP, ...)`を使う
- バッチファイルのテストは実際に`cmd /c`で実行して結果を検証する。
  テスト不可能な副作用（`start`・`del "%~f0"`等）はヘルパーで無効化してコアロジックを検証可能にする

## unsafe-reviewerの必須呼び出し

`unsafe`ブロックを新規追加・変更した直後は、必ず`Task`ツールで
`subagent_type=unsafe-reviewer`を呼び出し、対象ファイルの絶対パスを与えてレビューを受けること。
既存の`unsafe`を含むファイルを編集しても、`unsafe`部分そのものに変更がなければ対象外。

SAFETYコメントの粒度判定基準は[.claude/agents/unsafe-reviewer.md](.claude/agents/unsafe-reviewer.md)を唯一のSSOTとする。
