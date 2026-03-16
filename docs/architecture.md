# ぐらびゅ3 アーキテクチャ設計書

## モジュール構成

```
gv3/
├── Cargo.toml
├── build.rs                    # リソースコンパイル（アイコン等）
├── gv3.toml.default            # デフォルト設定ファイル
├── gv3.keys.toml.default       # デフォルトキーバインド
├── spi/                        # Susieプラグイン配置ディレクトリ
├── src/
│   ├── main.rs                 # エントリーポイント、メッセージループ
│   ├── app.rs                  # AppWindow: メインウィンドウ管理、メニュー、キー処理
│   ├── document.rs             # Document: 画像・ファイルリスト・状態管理（モデル層）
│   ├── file_list.rs            # FileList: ファイル一覧管理、ソート、シャッフル
│   ├── file_info.rs            # FileInfo: 個々のファイル情報（パス、サイズ、マーク状態等）
│   ├── config.rs               # Config: TOML設定管理
│   │
│   ├── prefetch/               # 先読みエンジン
│   │   ├── mod.rs
│   │   ├── page_cache.rs       # リングバッファキャッシュ
│   │   └── loader_thread.rs    # ワーカースレッド
│   │
│   ├── image/                  # 画像デコーダ
│   │   ├── mod.rs              # trait ImageDecoder
│   │   ├── standard.rs         # image crate による標準デコード (JPEG/PNG/GIF/BMP/WebP)
│   │   ├── turbojpeg.rs        # libjpeg-turbo による高速JPEGデコード
│   │   └── susie.rs            # Susieプラグイン画像デコーダ
│   │
│   ├── archive/                # アーカイブハンドラ
│   │   ├── mod.rs              # trait ArchiveHandler
│   │   ├── zip.rs              # ZIP/cbz
│   │   ├── rar.rs              # RAR/cbr
│   │   ├── sevenz.rs           # 7z
│   │   └── susie.rs            # Susieアーカイブプラグイン
│   │
│   ├── render/                 # 描画エンジン
│   │   ├── mod.rs
│   │   ├── d2d_renderer.rs     # Direct2D描画
│   │   └── layout.rs           # 表示モード(自動縮小/拡大/原寸大/固定倍率)
│   │
│   ├── ui/                     # UI関連
│   │   ├── mod.rs
│   │   ├── window.rs           # Win32ウィンドウラッパー
│   │   ├── fullscreen.rs       # フルスクリーン/全画面切替
│   │   ├── key_config.rs       # キーバインド設定
│   │   ├── menu.rs             # メニューバー
│   │   ├── dialogs.rs          # 各種ダイアログ（ファイル選択、設定等）
│   │   ├── file_list_window.rs # ファイルリストウィンドウ
│   │   └── cursor_hider.rs     # フルスクリーン時カーソル自動非表示
│   │
│   └── shell/                  # シェル統合
│       ├── mod.rs
│       ├── association.rs      # ファイル関連付け登録
│       ├── context_menu.rs     # 右クリックメニュー登録
│       └── sendto.rs           # 「送る」登録
```

## アーキテクチャパターン: Model-View (MV) 分離

ぐらびゅ2の `CDocument` (IDocView経由) → `CAppWindow` パターンを踏襲する。
WPF的なMVVMは、Win32メッセージベースのアプリには過剰であるため採用しない。

### 通信方式

ぐらびゅ2ではコールバックインターフェース (`IDocView`) を使っていたが、
ぐらびゅ3ではRustのチャネルで疎結合化する。

```
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

ぐらびゅ2の `PageLoaderBase` + `ThreadedImageLoader` の設計を踏襲する。

### リングバッファキャッシュ

```
     ← 後方キャッシュ  現在  前方キャッシュ →
     [...] [...] [...] [表示中] [...] [...] [...]
      -3    -2    -1     0      +1    +2    +3
```

- キャッシュサイズ（前方N枚 + 後方M枚）は利用可能メモリに基づいて動的に決定
- ぐらびゅ2の `GetNowFreeMemory()` 相当の処理でシステムメモリ残量を取得
- ベースサイズ（デフォルト 1024×1536）の画像を基準にキャッシュ可能枚数を計算

### ワーカースレッド

```
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
- ナビゲーション時に不要なリクエストをキャンセル

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

ぐらびゅ2の Factory パターン (`IImageLoader` + `CJpgLoaderFactory` 等) をRustのtraitで置き換える。

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

デコーダ登録:
1. StandardDecoder (`image` crate) — JPEG/PNG/GIF/BMP/WebP/TIFF/TGA/ICO
2. TurboJpegDecoder (`turbojpeg` crate) — JPEG高速デコード（StandardDecoderより優先）
3. SusieDecoder (`libloading`) — Susieプラグインからの動的登録

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

ぐらびゅ2の `CSpi` クラスの設計を移植する。

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

- `$AppDir\spi\` からDLLを列挙して自動ロード
- 画像プラグインとアーカイブプラグインを `GetPluginInfo` の戻り値で区別
- ロード順・優先度は設定ファイルで制御

## 設定管理

```toml
# gv3.toml

[display]
auto_scale = "shrink"          # shrink | shrink_and_enlarge | enlarge | original | fixed
fixed_scale = 1.0
margin = 0
alpha_background = "checker"   # white | black | checker

[prefetch]
cache_base_width = 1024
cache_base_height = 1536

[list]
default_sort = "name"          # name | name_nocase | folder | folder_nocase | size | date | natural

[window]
remember_position = true
remember_size = true
always_on_top = false

[susie]
plugin_dir = "spi"
# プラグイン優先度（上が高優先）
image_plugins = []
archive_plugins = []
```

## 実装フェーズ

### Phase 1: 基盤 — Win32ウィンドウ + 画像表示
- Win32ウィンドウの作成、メッセージループ
- Direct2D初期化、画像のデコードと描画
- コマンドライン引数による画像表示

### Phase 2: ファイルリスト + ナビゲーション
- フォルダ内画像の列挙
- FileList実装（ソート、フィルタ）
- キーボードナビゲーション（←→, PageUp/Down等）
- D&D対応

### Phase 3: 先読みエンジン
- ワーカースレッド + リングバッファキャッシュ
- メモリ残量に基づくキャッシュサイズ動的調整
- ナビゲーション時のキャッシュ更新・キャンセル

### Phase 4: アーカイブ対応
- ZIP/cbz サポート
- RAR/cbr サポート
- 7z サポート
- アーカイブ内ナビゲーション

### Phase 5: ウィンドウ・表示機能
- フルスクリーン/全画面切替
- 表示モード（自動縮小/拡大/原寸大/固定倍率）
- 余白設定
- αチャネル背景色切替
- カーソル自動非表示

### Phase 6: Susieプラグイン
- DLL動的ロード
- 画像プラグインのImageDecoder実装
- アーカイブプラグインのArchiveHandler実装
- プラグイン設定

### Phase 7: 設定・キーバインド・シェル統合
- TOML設定ファイル読み書き
- キーバインドのカスタマイズ
- ファイル関連付け登録
- 右クリックメニュー登録
- 「送る」登録
- 設定ダイアログ

### Phase 8: ファイル操作・マーク・ブックマーク
- ファイルのコピー/移動/削除（Shell API経由）
- マーク機能（設定/解除/反転/一括操作）
- ブックマーク保存/復元
- 画像の書き出し（JPG/BMP/PNG）
- 画像情報表示
- クリップボード操作
