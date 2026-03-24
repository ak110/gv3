# ぐらびゅ

[![CI](https://github.com/ak110/gv/actions/workflows/ci.yaml/badge.svg)](https://github.com/ak110/gv/actions/workflows/ci.yaml)
[![Release](https://github.com/ak110/gv/actions/workflows/release.yaml/badge.svg)](https://github.com/ak110/gv/actions/workflows/release.yaml)

Windows用画像ビューアー。

## 動作環境

- Windows 10以降 (x64)

## インストール

[Releases](https://github.com/ak110/gv/releases) からZIPをダウンロードし、任意のフォルダに展開してください。

## 使い方

ファイル関連付けやドラッグ&ドロップで画像ファイルを開けます。
キーバインドや詳しい使い方は [ユーザーガイド](docs/user-guide.md) を参照してください。

## 対応フォーマット

- 画像: JPEG, PNG, GIF, BMP, WebP
- PDF
- アーカイブ: ZIP/cbz, RAR/cbr, 7z

64bit Susieプラグイン (.sph / .spi) にも対応しています。詳細は [ユーザーガイド](docs/user-guide.md) を参照してください。

## カスタマイズ

`ぐらびゅ.toml` と `ぐらびゅ.keys.toml` をexeと同じディレクトリに配置することで設定やキーバインドをカスタマイズできます。
同梱の `ぐらびゅ.default.toml` / `ぐらびゅ.keys.default.toml` をコピーしてリネームしてお使いください。

## コマンドライン

```cmd
REM 画像ファイルを指定して起動
ぐらびゅ.exe image.jpg

REM ファイル関連付け・コンテキストメニュー・「送る」を一括登録
ぐらびゅ.exe --register

REM 一括解除
ぐらびゅ.exe --unregister
```
