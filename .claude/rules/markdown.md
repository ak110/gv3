---
paths:
  - "**/*.md"
  - "**/*.mdx"
---

# Markdown記述スタイル

- `**`は強調したい箇所のみとし、箇条書きの見出しなどでの使用は禁止
  - NG例: `1. **xx機能**: xxをyyする`
- できるだけmarkdownlintが通るように書く
  - 特に注意するルール:
    - MD022 - Headings should be surrounded by blank lines
    - MD031 - Fenced code blocks should be surrounded by blank lines
    - MD040 - Fenced code blocks should have a language specified
- 図はMermaid記法で書く
- 別のMarkdownファイルへのリンクは、基本的に`[プロジェクトルートからのパス](記述個所からの相対パス)`で書く
  - 例: `docs/api.md` から `docs/guide/setup.md` へリンク → `[docs/guide/setup.md](guide/setup.md)`
