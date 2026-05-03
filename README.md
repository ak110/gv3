# ぐらびゅ

[![CI][ci-badge]][ci-url]
[![Release][release-badge]][release-url]

[ci-badge]: https://github.com/ak110/gv/actions/workflows/ci.yaml/badge.svg
[ci-url]: https://github.com/ak110/gv/actions/workflows/ci.yaml
[release-badge]: https://github.com/ak110/gv/actions/workflows/release.yaml/badge.svg
[release-url]: https://github.com/ak110/gv/actions/workflows/release.yaml

Windows用画像ビューアー。

## 特徴

- 前後画像の先読みによる高速切り替え
- 多形式対応: JPEG / PNG / GIF / BMP / WebP / PDF / ZIP / cbz / RAR / cbr / 7z / 64bit Susieプラグイン
- 設定ファイルとキーバインドによるカスタマイズ

## インストール

[Releases](https://github.com/ak110/gv/releases) からZIPをダウンロードし、任意のフォルダに展開する。

## 設定

`ぐらびゅ.default.toml` をコピーして `ぐらびゅ.toml` にリネームし、設定をカスタマイズできる。
キーバインドは `ぐらびゅ.keys.default.toml` をコピーして `ぐらびゅ.keys.toml` にリネームして編集する。

## ドキュメント

- <https://ak110.github.io/gv/> — 概要・使い方
- [docs/development/development.md](docs/development/development.md) — 開発者向け情報
