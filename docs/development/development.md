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

デフォルトキーバインドは以下の2箇所で定義されている。変更時は両方を同期すること。

| ファイル                                      | 役割                                                     |
| --------------------------------------------- | -------------------------------------------------------- |
| `src/ui/key_config.rs` (`default_bindings()`) | 設定ファイル未指定時のハードコードデフォルト             |
| `ぐらびゅ.keys.default.toml`                  | ユーザー配布用のデフォルト設定テンプレート兼リファレンス |

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

## READMEとdocsの役割分担

本プロジェクトのドキュメントは以下の構成で配置している。

- README.md: 概要・特徴・インストール手順・ドキュメントへのリンクを網羅する「玄関」。
  README.mdだけを読めばプロジェクトの目的と使い始めるための入口が把握できる状態を保つ
- docs/guide/: 利用者向けの詳細情報（使い方・操作方法・カスタマイズなど）
- docs/development/: 開発者向けの情報（セットアップ・エラーハンドリング方針・リリース手順など）

README.mdとdocs側で概要・特徴・インストール手順が部分的に重複する場合があるが、README.mdはGitHubトップとして、
docs側は公開ドキュメントの入口としてそれぞれ自己完結する必要があるため、この重複は許容する。

変更頻度が低いため二重管理のコストより一貫性・可読性のメリットが上回ると判断した。
変更時は、docs側で同じ情報を再掲している箇所があれば同じコミット内で合わせて更新する。

## ドキュメントサイト

ドキュメントは [VitePress](https://vitepress.dev/) で構築し、GitHub Pagesでホストしている。

- URL: <https://ak110.github.io/gv/>
- ローカルプレビュー: `mise run docs`
- 自動デプロイ: masterブランチへのpush時に`Docs`ワークフローが自動実行される（`docs/`以下または`package.json`の変更時のみ）

## コミットメッセージ（Conventional Commits）

Conventional Commits形式に従う。ただし記述の方向性があまり変わらないような軽微な修正は`chore`などにしてよい。

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
