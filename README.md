# ぐらびゅ3

Windows用画像ビューアー。

## 動作環境

- Windows 10以降 (x64)

## 使い方

```bash
# 画像ファイルを指定して起動
gv3.exe image.jpg

# ファイル関連付けやD&Dでも起動可能
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

詳細は [docs/keybindings.md](docs/keybindings.md) を参照してください。

### 設定ファイル

`gv3.toml` と `gv3.keys.toml` をexeと同じディレクトリに配置することで設定をカスタマイズできます。
テンプレートは `gv3.toml.default` / `gv3.keys.toml.default` を参照してください。

### シェル統合

```bash
# ファイル関連付け・コンテキストメニュー・「送る」を一括登録
gv3.exe --register

# 一括解除
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

## ドキュメント

- [コンセプト](docs/concept.md)
- [機能仕様](docs/features.md)
- [アーキテクチャ](docs/architecture.md)
- [キーバインド](docs/keybindings.md)
- [開発ガイド](docs/development.md)

## ライセンス

[MIT License](LICENSE)
