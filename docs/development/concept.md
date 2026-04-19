# コンセプト

Windows用の軽量な画像ビューアーである。

## 設計方針

### シンプルさの追求

画像の閲覧・ナビゲーション・ファイル操作に機能を絞る。

### 高速な画像切り替え

先読み（プリフェッチ）エンジンを中核に据え、前後の画像をバックグラウンドでデコード・キャッシュすることで、
画像切り替えを瞬時に行う。

### モダンな開発環境

`rustup` + テキストエディタ（VSCode推奨）で開発できる。Visual Studioは不要（Build Toolsのみ必要）。

## 技術選定

### 言語: Rust

- C++同等のパフォーマンス。先読みバッファの精密なメモリ管理に所有権システムが最適
- `rustup` 一発で環境構築
- コンパイル時にデータ競合を検出。先読みスレッドの安全な実装
- `libloading` + `extern "system"` で64bit Susieプラグインの動的ロードが可能

### GUI: windows-rs (Win32 API) + Direct2D

- windows-rs: Microsoft公式のRust用Win32バインディング。型安全にWin32 API、COM、Direct2Dを呼び出せる
- Direct2D: GPU加速による高速な画像描画
