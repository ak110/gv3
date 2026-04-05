---
paths:
  - "**/*.ts"
  - "**/*.tsx"
---

# TypeScript記述スタイル

- importについて
  - 型のみのimportには `import type` を使用する
  - barrel export (`index.ts`) の乱用を避ける（ツリーシェイキングを阻害するため）
- 厳格な型付けを行う (`strict: true`)
- 型について
  - `any` の使用は極力避ける。やむを得ない場合は `unknown` + 型ガードを優先する
  - `as` による型アサーションより型ガード (`is` / `satisfies`) を優先する
  - union型 (`"a" | "b"`) を `enum` より優先する（tree-shakingしやすく、型の絞り込みも自然なため）
  - `switch` の網羅性チェックには `satisfies never` を使用する
- JSDocコメントを記述する
  - ファイルの先頭に`@fileoverview`で概要を記述
  - 関数・クラス・メソッドには機能を説明するコメントを記述
  - 自明な`@param`や`@returns`は省略する
- エラーハンドリング
  - `catch` の引数は `unknown` として扱い、`instanceof` で型を絞り込む
- `null`は使わず`undefined`を使用、APIから`null`が返される場合は`?? undefined`で変換
- 未使用の変数・引数には `_` プレフィックスを付ける
- セキュリティ上の危険パターン
  - `eval()` / `new Function()` はユーザー入力に対して使わない
  - `innerHTML` / `dangerouslySetInnerHTML` を避け、テキスト挿入には `textContent` やフレームワークのエスケープ機構を使う
  - `JSON.parse()` は信頼できない入力に対してtry-catchで囲み、結果をバリデーションする（zodなどのスキーマバリデーション推奨）
  - SQLはプレースホルダやクエリビルダーを使い、テンプレートリテラルで直接組み立てない
  - オブジェクトのマージ・コピーでプロトタイプ汚染を防ぐ（`Object.create(null)` やキーの検証。`__proto__`・`constructor`・`prototype` のキーを拒否する）
  - URL・ファイルパスは文字列結合ではなく `URL` / `path.join` 等の専用APIで構築する
- 他で指定が無い場合のツール推奨:
  - パッケージマネージャー: `pnpm`（厳密な依存解決でphantom dependencyを防止）
  - リンター/フォーマッター: `Biome`（lint + formatを1ツールで高速に処理）
    - Biomeが対応していないルール（React固有等）が必要な場合のみESLint + Prettierを併用
