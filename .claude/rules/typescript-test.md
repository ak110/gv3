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
- 非同期テストでは `await expect(...).resolves` / `.rejects` を活用する
- セットアップ/ティアダウン
  - `beforeAll` / `afterAll` でコストの高い初期化を共有する
  - 時間依存のテストは `vi.useFakeTimers()` で制御し、実時間の `sleep` を避ける
  - `afterEach` で `vi.restoreAllMocks()` / `vi.useRealTimers()` を確実に呼ぶ
- モック/スパイ
  - `vi.mock()` はファイル先頭へホイスティングされる点に注意（動的な値の参照不可）
  - 外部モジュール全体のモックより `vi.spyOn()` での部分モックを優先する
