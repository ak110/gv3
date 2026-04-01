---
paths:
  - "**/*.ts"
  - "**/*.tsx"
---

# TypeScript記述スタイル

- 厳格な型付けを行う (`strict: true`)
- JSDocコメントを記述する
  - ファイルの先頭に`@fileoverview`で概要を記述
  - 関数・クラス・メソッドには機能を説明するコメントを記述
  - 自明な`@param`や`@returns`は省略する
- `null`は使わず`undefined`を使用、APIから`null`が返される場合は`?? undefined`で変換
