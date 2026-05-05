# コンセプト

Windows用画像ビューアーである。

## 設計方針

### 機能の限定

画像の閲覧・ナビゲーション・ファイル操作に機能を限定する。

### 高速な画像切り替え

先読み（プリフェッチ）エンジンを中核に据え、前後の画像をバックグラウンドでデコード・キャッシュすることで、
画像切り替えを瞬時に行う。

### 開発環境

`rustup`とテキストエディタ（VSCode推奨）で開発できる。Visual Studio本体は不要でBuild Toolsのみ必要。

## 技術選定

### 言語: Rust

- C++同等のパフォーマンス。所有権システムにより先読みバッファのメモリ管理を安全に実装できる
- `rustup`で環境を構築できる
- コンパイル時にデータ競合を検出。先読みスレッドを安全に実装できる
- `libloading` + `extern "system"`で64bit Susieプラグインの動的ロードが可能

### GUI: windows-rs (Win32 API) + Direct2D

- windows-rs: Microsoft公式のRust用Win32バインディング。型安全にWin32 API・COM・Direct2Dを呼び出すことができる
- Direct2D: GPU加速による画像描画
