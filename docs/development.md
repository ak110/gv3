# 開発ガイド

## 必要環境

- [rustup](https://rustup.rs/)（Rust ツールチェーン）
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

## リリース手順

GitHub Actions の `Release` ワークフローを手動実行してリリースする。

### GitHub CLI から実行

```bash
gh workflow run release.yml --field "bump=バグフィックス"
gh workflow run release.yml --field "bump=マイナーバージョンアップ"
gh workflow run release.yml --field "bump=メジャーバージョンアップ"
```

<https://github.com/ak110/gv3/actions>
