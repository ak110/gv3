# ぐらびゅ3 アーキテクチャ設計書

## モジュール構成

```text
gv3/
├── Cargo.toml
├── build.rs                    # リソースコンパイル（アイコン等）
├── gv3.default.toml             # デフォルト設定ファイル
├── gv3.keys.default.toml       # デフォルトキーバインド
├── spi/                        # Susieプラグイン配置ディレクトリ
├── src/
│   ├── main.rs                 # エントリーポイント、メッセージループ
│   ├── app.rs                  # AppWindow: メインウィンドウ管理、メニュー、キー処理
│   ├── document.rs             # Document: 画像・ファイルリスト・状態管理（モデル層）
│   ├── file_list.rs            # FileList: ファイル一覧管理、ソート、シャッフル
│   ├── file_info.rs            # FileInfo: 個々のファイル情報（パス、サイズ、マーク状態等）
│   ├── config.rs               # Config: TOML設定管理
│   ├── extension_registry.rs   # ExtensionRegistry: 対応拡張子の管理（Susie動的登録含む）
│   ├── bookmark.rs             # ブックマーク保存/復元
│   ├── clipboard.rs            # Win32クリップボード操作（テキスト・画像）
│   ├── file_ops.rs             # Shell APIによるファイル操作・ダイアログ
│   ├── pdf_renderer.rs         # Windows.Data.Pdf によるPDFページレンダリング
│   ├── updater.rs              # GitHub Releases からの自動更新
│   ├── temp_cleanup.rs         # 起動時の古い一時ファイル自動削除
│   ├── editing.rs              # EditingSession: 編集セッション管理（元画像バックアップ）
│   ├── selection.rs            # Selection: 矩形選択（状態マシン、座標変換）
│   ├── persistent_filter.rs    # PersistentFilter: 永続フィルタ管理（FilterOperation列）
│   ├── util.rs                 # ユーティリティ（to_wide等の共通関数）
│   ├── test_helpers.rs         # テスト用ヘルパー（テスト時のみコンパイル）
│   │
│   ├── filter/                 # 画像フィルタ
│   │   ├── mod.rs              # モジュール宣言
│   │   ├── blur.rs             # ぼかし・モザイク（box blur, median, Gaussian, unsharp mask）
│   │   ├── brightness.rs       # 明るさ・トーン調整（levels, gamma, brightness/contrast）
│   │   ├── color.rs            # 色変換（反転, グレースケール, 塗り潰し, α合成）
│   │   ├── sharpen.rs          # シャープ化（3x3 Laplacian, 強化版）
│   │   └── transform.rs        # 幾何変換（回転, 反転, クロップ, リサイズ）
│   │
│   ├── prefetch/               # 先読みエンジン
│   │   ├── mod.rs
│   │   ├── page_cache.rs       # HashMap + メモリ予算によるキャッシュ
│   │   └── loader_thread.rs    # ワーカースレッド（世代管理付き）
│   │
│   ├── image/                  # 画像デコーダ
│   │   ├── mod.rs              # trait ImageDecoder, DecoderChain
│   │   ├── standard.rs         # image crate による標準デコード (JPEG/PNG/GIF/BMP/WebP)
│   │   └── susie.rs            # Susieプラグイン画像デコーダ
│   │
│   ├── archive/                # アーカイブハンドラ
│   │   ├── mod.rs              # trait ArchiveHandler, ArchiveManager
│   │   ├── zip.rs              # ZIP/cbz
│   │   ├── rar.rs              # RAR/cbr
│   │   ├── sevenz.rs           # 7z
│   │   └── susie.rs            # Susieアーカイブプラグイン
│   │
│   ├── render/                 # 描画エンジン
│   │   ├── mod.rs
│   │   ├── d2d_renderer.rs     # Direct2D描画、ビットマップキャッシュ
│   │   └── layout.rs           # 表示モード計算（AutoShrink/AutoFit/AutoEnlarge/Original）
│   │
│   ├── ui/                     # UI関連
│   │   ├── mod.rs
│   │   ├── window.rs           # Win32ウィンドウ基本操作
│   │   ├── fullscreen.rs       # フルスクリーン/全画面切替
│   │   ├── key_config.rs       # キーバインド設定、アクション定義
│   │   ├── cursor_hider.rs     # フルスクリーン時カーソル自動非表示
│   │   ├── font.rs             # フォント管理（等幅フォント等）
│   │   ├── info_dialog.rs      # 画像情報ダイアログ
│   │   ├── page_dialog.rs      # ページ指定ジャンプダイアログ
│   │   ├── filter_dialog.rs    # フィルタパラメータ入力ダイアログ
│   │   ├── rotate_dialog.rs    # 回転角度入力ダイアログ
│   │   ├── resize_dialog.rs    # リサイズダイアログ
│   │   ├── file_list_panel.rs  # ファイルリストパネル（ListBox）
│   │   └── menu.rs             # メニューバー構築・管理
│   │
│   ├── susie/                  # Susieプラグインシステム（64bit対応）
│   │   ├── mod.rs              # SusieManager: プラグイン検出・ロード・管理
│   │   ├── plugin.rs           # SusiePlugin: DLLラッパー・FFI呼び出し
│   │   ├── ffi.rs              # Susie FFI型定義（stdcall）
│   │   └── util.rs             # DIB→RGBA変換、CP932エンコーディング、メモリ管理
│   │
│   └── shell/                  # シェル統合
│       ├── mod.rs              # register_all / unregister_all
│       ├── association.rs      # ファイル関連付け登録
│       ├── context_menu.rs     # 右クリックメニュー登録
│       └── sendto.rs           # 「送る」登録
```

## アーキテクチャパターン: Model-View (MV) 分離

Win32メッセージベースのアプリケーションにはMVVMは過剰であるため、シンプルなMV分離を採用。
Rustのチャネルで疎結合化している。

```text
┌─────────────────────────────────────────────────┐
│  AppWindow (app.rs)                             │
│  - Win32ウィンドウ管理                           │
│  - メニュー・キー入力のハンドリング               │
│  - DocumentEventの受信 → 再描画・UI更新          │
│                                                 │
│  WM_KEYDOWN → Document操作メソッド呼び出し       │
│  WM_PAINT   → Renderer.draw(document.current()) │
└─────────┬───────────────────────────┬───────────┘
          │ 操作呼び出し              │ イベント受信
          ▼                           │
┌─────────────────────────┐           │
│  Document (document.rs) │           │
│  - FileList管理          ├──────────┘
│  - 先読みエンジン制御     │  DocumentEvent送信
│  - 表示状態管理          │  (チャネル経由)
└─────────────────────────┘
```

### DocumentEvent

```rust
enum DocumentEvent {
    ImageReady,              // 先読み完了、再描画可能
    FileListChanged,         // ファイルリスト変更
    NavigationChanged {      // 表示位置変更
        index: usize,
        count: usize,
    },
    Error(String),           // エラー通知
}
```

## 先読みエンジン設計

### リングバッファキャッシュ

```text
     ← 後方キャッシュ  現在  前方キャッシュ →
     [...] [...] [...] [表示中] [...] [...] [...]
      -3    -2    -1     0      +1    +2    +3
```

- キャッシュサイズ（前方N枚 + 後方M枚）は利用可能メモリに基づいて動的に決定
- ベースサイズ（デフォルト1024×1536）の画像を基準にキャッシュ可能枚数を計算

### ワーカースレッド

```text
メインスレッド                     ワーカースレッド
     │                                  │
     ├── LoadRequest送信 ───────────────→│
     │   (index, priority)              │ デコード実行
     │                                  │ (ImageDecoder使用)
     │←─────────── ImageReady受信 ───────┤
     │  (index, decoded_image)          │
     ├── キャッシュに格納               │
     │   → DocumentEvent::ImageReady    │
     │                                  │
     ├── CancelRequest送信 ────────────→│
     │   (キャッシュ範囲外になった      │ 現在のデコードを中断
     │    画像のキャンセル)              │
```

- `crossbeam-channel` でリクエストキューを実装
- 優先度付きロード: 現在ページ > 次ページ > 前ページ > 遠いページ
- ナビゲーション時に不要なリクエストをキャンセル（世代管理）

### キャッシュデータ構造

```rust
struct DecodedImage {
    data: Vec<u8>,           // デコード済みピクセルデータ (RGBA)
    width: u32,
    height: u32,
    memory_size: usize,      // メモリ使用量（キャッシュ管理用）
}

struct PageCache {
    cache: HashMap<usize, DecodedImage>,  // index → デコード済み画像
    max_memory: usize,                     // 最大キャッシュメモリ
    current_memory: usize,                 // 現在のキャッシュメモリ使用量
}
```

## 画像デコーダ設計

```rust
trait ImageDecoder: Send + Sync {
    /// このデコーダが対応する拡張子のリスト
    fn supported_extensions(&self) -> &[&str];

    /// バイト列からデコード可能か判定
    fn can_decode(&self, data: &[u8]) -> bool;

    /// デコード実行
    fn decode(&self, data: &[u8]) -> Result<DecodedImage>;

    /// メタデータ取得（画像サイズ、コメント等）
    fn metadata(&self, data: &[u8]) -> Result<ImageMetadata>;
}
```

デコーダ登録（DecoderChain — 先に登録されたものが優先）:

1. StandardDecoder (`image` crate) — JPEG/PNG/GIF/BMP/WebP
2. SusieDecoder (`libloading`) — Susieプラグインからの動的登録

## アーカイブハンドラ設計

```rust
trait ArchiveHandler: Send + Sync {
    /// このハンドラが対応する拡張子のリスト
    fn supported_extensions(&self) -> &[&str];

    /// アーカイブ内のファイル一覧を取得
    fn list_files(&self, archive_path: &Path) -> Result<Vec<ArchiveEntry>>;

    /// アーカイブ内のファイルを読み出す
    fn extract(&self, archive_path: &Path, entry: &str) -> Result<Vec<u8>>;
}

struct ArchiveEntry {
    path: String,       // アーカイブ内パス
    size: u64,          // ファイルサイズ
    is_image: bool,     // 画像ファイルか
}
```

## Susieプラグインシステム

```rust
struct SusiePlugin {
    _lib: libloading::Library,
    plugin_type: SusiePluginType,  // Image or Archive
    // 関数ポインタ（extern "system" = stdcall、x64では cdecl と同一）
    get_plugin_info: Symbol<GetPluginInfoFn>,
    is_supported: Symbol<IsSupportedFn>,
    get_picture: Option<Symbol<GetPictureFn>>,
    get_archive_info: Option<Symbol<GetArchiveInfoFn>>,
    get_file: Option<Symbol<GetFileFn>>,
}

struct SusieManager {
    plugins: Vec<SusiePlugin>,
}
```

- exeと同階層の `spi/` からDLLを列挙して自動ロード
- 画像プラグインとアーカイブプラグインを `GetPluginInfo` の戻り値で区別
- ロード順・優先度は設定ファイルで制御

## 設定管理

```toml
# gv3.toml

[display]
auto_scale = "fit"             # shrink | fit | enlarge | original
fixed_scale = 1.0
margin = 64.0
alpha_background = "checker"   # white | black | checker

[prefetch]
cache_base_width = 1024
cache_base_height = 1536

[list]
default_sort = "name"          # name | name_nocase | size | date | natural

[window]
remember_position = false
remember_size = false
always_on_top = false

[susie]
plugin_dir = "spi"
image_plugins = []
archive_plugins = []
```
