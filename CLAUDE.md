# カスタム指示 (プロジェクト固有)

## 開発手順

- rust, node, pnpmなどはmise経由で実行する
- ドキュメントのみの変更（`*.md`や`docs/**`の更新）をコミットする場合、事前の手動`mise run test`は省略してよい。
  `git commit`時点で`.githooks/pre-commit`が`mise run test`（`pnpm run lint`を含むフルテスト）を自動実行するため、Markdownのtextlint/markdownlint-cli2/prettierは確実にかかる
- コードに手を入れた変更では、失敗の早期検出のため従来どおり事前に`mise run test`を回すことを推奨する

## ローカルコーディング規約

`~/.claude/rules/agent-basics/rust.md` をベースに、Win32 + COM集約の本プロジェクトで現実的に運用するための補足。

### unsafe-reviewer の必須呼び出し

`unsafe`ブロックを含む`.rs`ファイルを編集・新規作成した直後は、必ず`Task`ツールで`subagent_type=unsafe-reviewer`を呼び出し、対象ファイルの絶対パスを与えてレビューを受けること。
これは`.claude/hooks/post-edit-rust.sh`のstderrリマインダとペアになっている恒久ルールである。
`unsafe`を1行も触っていない場合でも、編集したファイルに既存の`unsafe`が含まれていれば対象となる。

### Mutex / RwLock の poison 扱い

- `Mutex::lock()` / `RwLock::read()` / `RwLock::write()` のpoisonは「他スレッドがロック保持中にパニックした」ことを示し、これは不変条件違反とみなしてプロセスを止めるのが安全。
- そのため `expect("<lock 名> lock poisoned")` 形式でpanicさせてよい（Rust標準ライブラリも同様の慣例）。
- メッセージは `"<lock 名> lock poisoned"` 形式で統一する。これによりログでの追跡が容易になる。

## リリースビルド

- 作業完了時、コミットと並行してバックグラウンドでリリースビルドも実行しておいて。(完了を待つ必要は無い。ユーザーによる動作確認をスムーズにするため)

## 関連ドキュメント

- @README.md
- @docs/development/concept.md
- @docs/development/architecture.md
- @docs/development/development.md
