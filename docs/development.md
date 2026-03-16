# 開発ガイド

## 必要環境

- [rustup](https://rustup.rs/)（Rustツールチェーン）
- Visual Studio Build Tools（C++ ビルドツール）

## ビルド手順

```bash
# デバッグビルド
cargo build

# リリースビルド（最適化あり）
cargo build --release
# → target/release/gv3.exe

# テスト
cargo test

# 静的解析
cargo clippy

# フォーマット
cargo fmt
```

## 依存パッケージの更新

```bash
# Cargo.lock を最新に更新（semver互換範囲内）
cargo update

# ビルド・テスト確認
cargo build && cargo test && cargo clippy

# メジャーバージョンアップの確認（任意）
cargo install cargo-outdated
cargo outdated
```

メジャーバージョンアップがある場合は `Cargo.toml` のバージョン指定を手動で更新する。

## リリース手順

GitHub Actionsの `Release` ワークフローを手動実行してリリースする。

### GitHub CLI から実行

```bash
gh workflow run release.yml --field "bump=バグフィックス"
gh workflow run release.yml --field "bump=マイナーバージョンアップ"
gh workflow run release.yml --field "bump=メジャーバージョンアップ"
```

<https://github.com/ak110/gv3/actions>
