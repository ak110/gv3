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
