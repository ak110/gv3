# ぐらびゅ3

[![CI](https://github.com/ak110/gv3/actions/workflows/ci.yml/badge.svg)](https://github.com/ak110/gv3/actions/workflows/ci.yml)
[![Release](https://github.com/ak110/gv3/actions/workflows/release.yml/badge.svg)](https://github.com/ak110/gv3/actions/workflows/release.yml)

Windows用の軽量画像ビューアーです。先読みエンジンにより、大量の画像をサクサク閲覧できます。

## 動作環境

- Windows 10以降 (x64)

## インストール

[Releases](https://github.com/ak110/gv3/releases) からZIPをダウンロードし、任意のフォルダに展開してください。

## 使い方

```cmd
REM 画像ファイルを指定して起動
gv3.exe image.jpg

REM ファイル関連付けやD&Dでも起動可能
```

### 主要キーバインド

| キー              | 操作                    |
|-------------------|-------------------------|
| ← / →             | 前後の画像に移動        |
| ホイール上/下     | 前後の画像に移動        |
| PageUp / PageDown | 5ページ移動             |
| Ctrl+PageUp/Down  | 50ページ移動            |
| Ctrl+Home / End   | 最初 / 最後へ           |
| Ctrl+ホイール     | 拡大 / 縮小             |
| Num /             | 自動縮小表示            |
| Num *             | 自動縮小・拡大表示      |
| Num 0             | 余白トグル              |
| A                 | αチャネル背景切替       |
| Alt+Enter         | フルスクリーン          |
| Esc               | メニューバー表示/非表示 |

全キーバインドの一覧は [docs/keybindings.md](docs/keybindings.md) を参照してください。

### 設定ファイル

`gv3.toml` と `gv3.keys.toml` をexeと同じディレクトリに配置することで設定をカスタマイズできます。
同梱の `gv3.default.toml` / `gv3.keys.default.toml` をコピーしてリネームしてお使いください。

### シェル統合

```cmd
REM ファイル関連付け・コンテキストメニュー・「送る」を一括登録
gv3.exe --register

REM 一括解除
gv3.exe --unregister
```

## 対応フォーマット

### 画像（標準対応）

JPEG, PNG, GIF, BMP, WebP

### アーカイブ

ZIP / cbz, RAR / cbr, 7z

### Susieプラグイン

64bit Susieプラグイン (.sph / .spi) に対応しています。
実行ファイルと同じディレクトリの `spi/` フォルダにプラグインDLLを配置すると自動検出されます。

```text
gv3.exe
spi/
  ifXXX.sph    ← 画像プラグイン
  axXXX.spi    ← アーカイブプラグイン
```

## ライセンス

[MIT License](LICENSE)
