# CLAUDE.md: gv

## 開発手順

- rust, node, pnpmなどはmise経由で実行する
- リリース手順: [docs/development/development.md](docs/development/development.md) 参照
- コミット前の検証方法: `uvx pyfltr run-for-agent`
  - ドキュメントなどのみの変更の場合は省略可（pre-commitで実行されるため）
  - 修正後の再実行時は、対象ファイルや対象ツールを必要に応じて絞って実行する（最終検証はCIに委ねる前提）
    - 例: `uvx pyfltr run-for-agent --commands=cargo-clippy,cargo-test path/to/file`

### miseタスク一覧

| コマンド          | 内容                                                                |
| ----------------- | ------------------------------------------------------------------- |
| `mise run setup`  | 開発環境のセットアップ（rustfmt・clippy・pnpm install・pre-commit） |
| `mise run format` | フォーマット + 軽量lint（開発時の手動実行用。自動修正あり）         |
| `mise run test`   | 全チェック実行（これを通過すればコミット可能）                      |
| `mise run build`  | リリースビルド（`cargo build --release`）                           |
| `mise run clean`  | ビルド成果物の削除                                                  |
| `mise run update` | 依存パッケージの更新（cargo・pnpm・GitHub Actionsのピン更新）       |
| `mise run docs`   | ドキュメントのローカルプレビュー（VitePress dev server）            |

## 注意点

- コミットメッセージはConventional Commits形式に従う。
  ただし記述の方向性があまり変わらないような軽微な修正は`chore`などにしてよい。
- `Mutex::lock()` / `RwLock::read()` / `RwLock::write()` のpoisonは「他スレッドがロック保持中にパニックした」
  ことを示し、これは不変条件違反とみなしてプロセスを止めるのが安全。
- そのため `expect("<lock 名> lock poisoned")` 形式でpanicさせてよい（Rust標準ライブラリも同様の慣例）。
- メッセージは `"<lock 名> lock poisoned"` 形式で統一する。これによりログでの追跡が容易になる。
- 上記のlock poison方針はsusie系を含むすべてのモジュールに適用する。
  `map_err` で `anyhow::Error` 化する旧パターンは禁止し、新規・既存ともに `expect` 形式へ揃える。
- 配布TOMLが正規ソースとなる種別（キーバインド・既定ソート種別など）は、TOML側を唯一のSSOTとし、
  ソースコード内に同等のハードコードデフォルトを置かない。
  ビルド時は `include_str!` で取り込み、起動時にパースして反映する。
  起動時にTOMLの値をアプリ状態へ反映する経路を必ず持たせ、Rust側の `Default` 実装と
  配布TOMLの既定値が一致することを単体テストで保証する。
  パース失敗を防ぐため、配布TOMLが正しくパースできることも単体テストで保証する。
- ファイル名の自然順比較は `shlwapi.dll` の `StrCmpLogicalW` を使う。
  Windowsエクスプローラーの並びと一致させるため、先頭ゼロ付き数値の扱いがエクスプローラーと
  乖離するクロスプラットフォーム実装（`natord`等）は採用しない。

### unsafe-reviewer の必須呼び出し

`unsafe`ブロックを含む`.rs`ファイルを編集・新規作成した直後は、必ず`Task`ツールで
`subagent_type=unsafe-reviewer`を呼び出し、対象ファイルの絶対パスを与えてレビューを受けること。
これは`.claude/hooks/post-edit-rust.sh`のstderrリマインダとペアになっている恒久ルールである。
`unsafe`を1行も触っていない場合でも、編集したファイルに既存の`unsafe`が含まれていれば対象となる。

`SAFETY`コメント粒度: 自明な`unsafe`（OS仕様で安全確定するWin32 APIなど）は省略可。
生ポインタ操作・所有権遷移・メモリーマップ・COMオブジェクト・Win32構造受け渡し・
`unsafe impl Send`/`unsafe impl Sync`など、根拠が読み手に自明でない場合は
必ず`// SAFETY:`コメント明記。判定は`unsafe-reviewer`に委ねる。

## リリースビルド

- 作業完了時、コミットと並行してバックグラウンドでリリースビルドも実行する（完了を待つ必要は無い。
  ユーザーによる動作確認をスムーズにするため）
