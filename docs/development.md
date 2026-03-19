# 開発ガイド

## 必要環境

- [mise](https://mise.jdx.dev/)（タスクランナー・ツールバージョン管理）
- Visual Studio Build Tools（C++ ビルドツール）

## 初回セットアップ

```cmd
mise install
mise run setup
```

## 主要タスク

| コマンド | 内容 |
|---|---|
| `mise run build` | デバッグビルド |
| `mise run build-release` | リリースビルド → `target/release/gv3.exe` |
| `mise run run -- image.jpg` | 実行 |
| `mise run format` | 自動整形（fmt + clippy --fix） |
| `mise run test` | 全チェック（fmt + clippy + test + cargo-deny + ドキュメントlint） |
| `mise run update` | 依存パッケージを最新に更新（メジャーバージョン含む） |

`git push`時にはpre-pushフックが`mise run test`を自動実行する。

clippyのpedantic lint設定は`Cargo.toml`の`[lints.clippy]`セクションで管理している。

## キーバインド定義の管理

デフォルトキーバインドは以下の3箇所で定義されている。変更時は全箇所を同期すること。

| ファイル | 役割 |
|---------|------|
| `src/ui/key_config.rs` (`default_bindings()`) | 設定ファイル未指定時のハードコードデフォルト |
| `gv3.keys.default.toml` | ユーザー配布用のデフォルト設定テンプレート |
| `docs/keybindings.md` | ドキュメント上のデフォルトキーバインド一覧 |

## エラーハンドリング方針

### ユーザー操作起因のエラー

ユーザーが明示的に実行した操作（ファイル移動、コピー、保存、クリップボード操作など）が
失敗した場合は、必ず `show_error_title()` でタイトルバーにエラーを表示する。
`eprintln!` のみでの出力は禁止（ユーザーに見えない）。

### 内部処理のエラー・警告

Susieプラグインのロード、設定ファイルのパースなど、バックグラウンド処理や
初期化時のエラーは `eprintln!` でstderrに出力する（デバッグ用）。
フォールバック動作がある場合はそのまま続行してよい。

### Resultの伝搬

可能な限り`Result`で呼び出し元に返し、app.rsのアクションハンドラで
エラー表示を行う。中間層でエラーを握り潰さない。

## リリース手順

GitHub Actionsの `Release` ワークフローを手動実行してリリースする。

```cmd
REM リリース実行（いずれか1つ）
gh workflow run release.yml --field "bump=バグフィックス"
gh workflow run release.yml --field "bump=マイナーバージョンアップ"
gh workflow run release.yml --field "bump=メジャーバージョンアップ"

REM ワークフロー完了を待ち、バージョンバンプコミットを取り込む
for /f %i in ('gh run list --workflow=release.yml -L1 --json databaseId -q ".[0].databaseId"') do (gh run watch %i & git pull)
```

結果の確認: <https://github.com/ak110/gv3/actions>
