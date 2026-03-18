# 開発ガイド

## 必要環境

- [rustup](https://rustup.rs/)（Rustツールチェーン）
- Visual Studio Build Tools（C++ ビルドツール）

## ビルド手順

```cmd
REM デバッグビルド
cargo build

REM リリースビルド（最適化あり）
cargo build --release
REM → target/release/gv3.exe

REM テスト
cargo test

REM 静的解析
cargo clippy

REM フォーマット
cargo fmt
```

## Git フックのセットアップ

`git push`時に自動でlint・テストを実行するpre-pushフックを用意している。

```powershell
# 初回セットアップ（リポジトリごとに1回）
powershell -ExecutionPolicy Bypass -File scripts/setup-hooks.ps1

# 手動で全チェックを実行する場合
powershell -ExecutionPolicy Bypass -File scripts/lint-all.ps1
```

`lint-all.ps1` は `cargo fmt --check`、`cargo clippy -- -D warnings`、`cargo test` を順に実行する。
pre-pushフック自体はGit for Windowsのbashで実行されるため`.sh`も同梱している。

```powershell
# フックを無効化する場合
git config --unset core.hooksPath
```

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

## 依存パッケージの更新

```cmd
REM Cargo.lock を最新に更新（semver互換範囲内）
cargo update

REM ビルド・テスト確認
cargo build && cargo test && cargo clippy

REM メジャーバージョンアップの確認（任意）
cargo install cargo-outdated
cargo outdated
```

メジャーバージョンアップがある場合は `Cargo.toml` のバージョン指定を手動で更新する。

## リリース手順

GitHub Actionsの `Release` ワークフローを手動実行してリリースする。

### GitHub CLI から実行

```cmd
REM 1. リリース実行（いずれか1つ）
gh workflow run release.yml --field "bump=バグフィックス"
gh workflow run release.yml --field "bump=マイナーバージョンアップ"
gh workflow run release.yml --field "bump=メジャーバージョンアップ"

REM 2. ワークフロー完了を待ち、バージョンバンプコミットを取り込む
for /f %i in ('gh run list --workflow=release.yml -L1 --json databaseId -q ".[0].databaseId"') do (gh run watch %i & git pull)
```

結果の確認: <https://github.com/ak110/gv3/actions>
