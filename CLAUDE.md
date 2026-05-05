# CLAUDE.md: gv

Windows用画像ビューアー（Rust製）。多形式対応と高速切り替えを軸にした単一ユーザー向けGUIアプリ。
本プロジェクトは人間による開発作業がほぼ発生しないため、
コーディング規約・設計判断・実装上の注意点はCLAUDE.mdおよび`.claude/rules/`配下に集約する。

## 開発手順

- rust, node, pnpmなどはmise経由で実行する
- 普段使うのは`mise run format`（フォーマット + 軽量lint）と`mise run test`（全チェック実行）
- リリースは`gh workflow run release.yaml --field="bump=PATCH"`で実行する
- コミット前の検証方法: `uvx pyfltr run-for-agent`
  - ドキュメントなどのみの変更の場合は省略可（pre-commitで実行されるため）
  - 修正後の再実行時は、対象ファイルや対象ツールを必要に応じて限定して実行する（最終検証はCIに委ねる前提）
    - 例: `uvx pyfltr run-for-agent --commands=cargo-clippy,cargo-test path/to/file`
- 作業完了時、コミットと並行してバックグラウンドでリリースビルド（`mise run build`）も実行する。
  完了は待たなくてよい（ユーザーによる動作確認をスムーズにするため）

## アーキテクチャの参照先

アーキテクチャ概要・モジュール構成・設計判断の根拠は
[docs/development/architecture.md](docs/development/architecture.md)を参照する。

## コーディング規約

Rust実装の規約（ロックpoison・TOML SSOT・自然順比較・エラーハンドリング）は
[.claude/rules/coding-standards.md](.claude/rules/coding-standards.md)に集約する。
Windowsバッチファイル生成の規約（CP932・UTF-8 BOM・chcp・goto構文）は
[.claude/rules/windows-batch-generation.md](.claude/rules/windows-batch-generation.md)に集約する。
これらのファイルは自動ロードされないため、該当する作業の着手前に参照する。

## サブエージェント・スキル連携

`unsafe`ブロックを新規追加・変更した直後は、必ず`Task`ツールで
`subagent_type=unsafe-reviewer`を呼び出し、対象ファイルの絶対パスを与えてレビューを依頼する。
既存の`unsafe`を含むファイルを編集しても、`unsafe`部分そのものに変更がなければ対象外。

SAFETYコメントの粒度判定基準は[.claude/agents/unsafe-reviewer.md](.claude/agents/unsafe-reviewer.md)をSSOTとする。

## 注意点

- Windows用プロジェクトのため、Linux環境での検証はlint系（textlint / markdownlint / prettier）のみ確認可能。
  cargo-clippy / cargo-test / cargo-denyはWindowsターゲットのためLinuxでは失敗する
- Makefileではなく`mise.toml`のタスクを使用する。pre-commitフレームワークは`uvx pre-commit`で呼び出す
- `taiki-e/install-action@cargo-deny`はツール名タグ形式のためpinactでハッシュピン不可（`.pinact.yaml`で除外済み）
- Linux環境での検証コマンド実行時は`LOCALAPPDATA=/tmp/dummy`環境変数を付与する。
  `mise.toml`がWindows前提で`LOCALAPPDATA`を参照しているため
