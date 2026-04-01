---
paths:
  - "**/*.test.ts"
  - "**/*.test.tsx"
  - "**/*.spec.ts"
  - "**/*.spec.tsx"
---

# TypeScriptテストコード記述スタイル

- テストコードは`vitest`で書く
- `describe` は原則1階層、ネストは2階層まで
- 類似パターンの網羅には `it.each()` を活用する
- テストデータ生成ヘルパー (`makeXxx(overrides)`) で本質的でないセットアップを共通化する
- 時間依存のテストは `vi.useFakeTimers()` で制御し、実時間の `sleep` を避ける
- `afterEach` で `vi.restoreAllMocks()` / `vi.useRealTimers()` を確実に呼ぶ
