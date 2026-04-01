# カスタマイズ

設定ファイルとキーバインドのカスタマイズ方法です。

## 設定ファイル

`ぐらびゅ.toml` をexeと同じディレクトリに配置することで設定をカスタマイズできます。
同梱の `ぐらびゅ.default.toml` をコピーしてリネームしてお使いください。

## キーバインド

`ぐらびゅ.keys.toml` をexeと同じディレクトリに配置することで、ほぼ全ての操作のキーバインドをカスタマイズできます。
同梱の `ぐらびゅ.keys.default.toml` をコピーしてリネームし、編集してください。

### TOML構造の例

```toml
# ぐらびゅ.keys.toml
# 複数キーの割り当て: カンマ区切り（例: "Ctrl+Num -, Ctrl+WheelUp"）

[file]
new_window = "Ctrl+N"
open_file = "Ctrl+O"
open_folder = "Ctrl+Shift+O"
close_all = "Ctrl+W"

[mark]
set = "Delete"
unset = "Ctrl+Delete"
invert_all = "Ctrl+Shift+I"
# ... 以下同様
```

### キー名の記法

| 記法                                    | キー                     |
| --------------------------------------- | ------------------------ |
| `←` `→` `↑` `↓`                         | 矢印キー                 |
| `PageUp` `PageDown`                     | ページアップ/ダウン      |
| `Home` `End`                            | Home / End               |
| `Enter` `Space` `Tab` `Esc` `BackSpace` | 各種キー                 |
| `Delete`                                | Delete                   |
| `F1` 〜 `F12`                           | ファンクションキー       |
| `Num0` 〜 `Num9`                        | テンキー数字             |
| `Num +` `Num -` `Num *` `Num /`         | テンキー演算子           |
| `Ctrl+` `Shift+` `Alt+`                 | 修飾キー（組み合わせ可） |
