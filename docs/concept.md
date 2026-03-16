# ぐらびゅ3 プロジェクトコンセプト

## 概要

**ぐらびゅ3** は、Windows用の軽量・高速な画像ビューアである。
**画像を見ることに特化**し、先読み（プリフェッチ）による**瞬時の画像切り替え**を最大の特長とする。

## 設計方針

### 1. シンプルさの追求

画像編集機能は一切持たない。画像の閲覧・ナビゲーション・ファイル操作に機能を絞る。

### 2. 高速な画像切り替え

先読み（プリフェッチ）エンジンを中核に据え、前後の画像をバックグラウンドでデコード・キャッシュすることで、画像切り替えを瞬時に行う。

### 3. モダンな開発環境

`rustup` + テキストエディタ（VSCode推奨）で開発できる。Visual Studioは不要（Build Toolsのみ必要）。

## 技術選定

### 言語: Rust

- C++同等のパフォーマンス。先読みバッファの精密なメモリ管理に所有権システムが最適
- `rustup` 一発で環境構築
- コンパイル時にデータ競合を検出。先読みスレッドの安全な実装
- `libloading` + `extern "system"` で64bit Susieプラグインの動的ロードが可能

### GUI: windows-rs (Win32 API) + Direct2D

- **windows-rs**: Microsoft公式のRust用Win32バインディング。型安全にWin32 API、COM、Direct2Dを呼び出せる
- **Direct2D**: GPU加速による高速な画像描画

### 主要ライブラリ

| 機能 | crate | 備考 |
|------|-------|------|
| Win32 API | `windows` (Microsoft公式) | ウィンドウ管理、Direct2D、Shell API |
| 画像デコード | `image` | JPEG/PNG/GIF/BMP/WebP |
| ZIP | `zip` | Unicode対応 |
| RAR | `unrar` | 静的リンク、外部DLL不要 |
| 7-Zip | `sevenz-rust` | Pure Rust、外部依存なし |
| DLLロード | `libloading` | Susieプラグイン用 |
| スレッド間通信 | `crossbeam-channel` | 先読みスレッドとのメッセージング |
| 設定ファイル | `serde` + `toml` | 設定のシリアライズ/デシリアライズ |
| 自然順ソート | `natord` | 数値認識ソート |
| エラーハンドリング | `anyhow` + `thiserror` | アプリ/ライブラリレベル |

### ビルド・開発フロー

```bash
# 環境構築（初回のみ）
# 1. rustup インストール（公式サイト）
# 2. Visual Studio Build Tools インストール（C++ビルドツールのみ）
# 3. VSCode + rust-analyzer 拡張

# 日常の開発
cargo build              # ビルド
cargo run                # 実行
cargo run -- image.jpg   # 画像ファイルを指定して実行
cargo test               # テスト
cargo clippy             # 静的解析
cargo fmt                # コードフォーマット

# リリースビルド
cargo build --release    # 最適化ビルド（LTO有効）
# → target/release/gv3.exe
```
