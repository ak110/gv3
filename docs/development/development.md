# 開発ガイド

## 必要環境

- [mise](https://mise.jdx.dev/)（タスクランナー・ツールバージョン管理）
- Visual Studio Build Tools（C++ ビルドツール）

## 初回セットアップ

```cmd
mise install && mise run setup
```

`cargo`や`node`、`pnpm`などのコマンドはシステムにインストールされたものではなく、必ずmise経由で実行すること。
具体的には`mise run`タスク経由、またはmiseが管理するPATH上のバイナリを使用する。

## miseタスク

普段使うのはこの2つ。

| コマンド          | 内容                                                        |
| ----------------- | ----------------------------------------------------------- |
| `mise run format` | フォーマット + 軽量lint（開発時の手動実行用。自動修正あり） |
| `mise run test`   | 全チェック実行（これを通過すればコミット可能）              |

`git commit`時にはpre-commitフックが`mise run test`を自動実行する。

その他のタスク。

| コマンド          | 説明                             |
| ----------------- | -------------------------------- |
| `mise run setup`  | 開発環境のセットアップ           |
| `mise run build`  | リリースビルド                   |
| `mise run clean`  | ビルド成果物の削除               |
| `mise run update` | 依存パッケージの更新             |
| `mise run docs`   | ドキュメントのローカルプレビュー |

## Windowsバッチファイル生成時の注意

cmd.exeはバッチファイルをシステムのANSIコードページ（日本語環境ではCP932）で読む。
UTF-8で書くとDBCSバイト列が改行やコマンド構文を破壊する。

- ファイル名に日本語を含むバッチファイルは **UTF-8 BOM + `chcp 65001`** で書き出す。
  CP932だとcopyコマンド等のパス引数で日本語ファイル名を正しく解釈できない場合がある
- Rustの`format!`はLFのみ出力するので`replace('\n', "\r\n")`でCRLFに変換が必要
- `if ( ... )` ブロック内に日本語テキストがあると、DBCSトレイルバイトが特殊文字と誤認される。
  日本語を含む場合は `goto` で制御フローを構成する
- CP932変換が必要な場合は `WideCharToMultiByte(CP_ACP, ...)` を使う
- バッチファイルのテストは実際に `cmd /c` で実行して結果を検証する。
  テスト不可能な副作用 (`start`、`del "%~f0"`等) はヘルパーで無効化してコアロジックを検証可能にする

## キーバインド定義の管理

デフォルトキーバインドは `ぐらびゅ.keys.default.toml` を唯一のSSOTとして管理する。
ソースコードは起動時にこのTOMLを `include_str!` で取り込み、`KeyConfig::parse_toml`
でパースして反映する。
ハードコード版のデフォルト定義は持たない。配布TOMLが正しくパースできることは
`KeyConfig::parse_toml` の単体テスト (`default_toml_parses_and_resolves`) で保証する。

| ファイル                     | 役割                                                                                |
| ---------------------------- | ----------------------------------------------------------------------------------- |
| `ぐらびゅ.keys.default.toml` | デフォルトキーバインドの正規定義。配布物にも同梱され、`include_str!` で取り込まれる |
| `src/ui/key_config.rs`       | TOMLパーサー本体。セクション認識（`[persistent_filter]` 等）を含む                  |

## リネーム作業時の点検

製品名・パッケージ名のリネーム（例: `gv3` → `gv`）を行うときは、
変更後にリポジトリ全体を `grep -rn` で全文検索し、コメント・定数値・ドキュメント・配布物
（TOMLテンプレートなど）に旧名が残っていないか必ず点検する。
ProgID・ファイル拡張子のように後方互換のため意図的に旧名を残す箇所は、
近傍コメントでその旨を明記する。

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

## Clippy設定

clippyのpedantic lint設定は`Cargo.toml`の`[lints.clippy]`セクションで管理している。

## ドキュメントサイト

ドキュメントは [VitePress](https://vitepress.dev/) で構築し、GitHub Pagesでホストしている。

- URL: <https://ak110.github.io/gv/>
- ローカルプレビュー: `mise run docs`
- 自動デプロイ: masterブランチへのpush時に`Docs`ワークフローが自動実行される（`docs/`以下または`package.json`の変更時のみ）

## リリース手順

GitHub Actionsの`Release`ワークフローを手動実行してリリースする。

```cmd
rem リリース実行 (いずれか1つ)
gh workflow run release.yaml --field "bump=PATCH"
gh workflow run release.yaml --field "bump=MINOR"
gh workflow run release.yaml --field "bump=MAJOR"

rem ワークフロー完了を待ち、バージョンバンプコミットを取り込む
for /f "usebackq" %i in (`gh run list --workflow=release.yaml -L1 --json databaseId -q ".[0].databaseId"`) do gh run watch %i && git pull
```

結果の確認: <https://github.com/ak110/gv/actions>
