use std::collections::HashSet;
use std::os::windows::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use crossbeam_channel::Receiver;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{InvalidateRect, UpdateWindow, ValidateRect};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::archive::ArchiveManager;
use crate::config::Config;
use crate::document::{Document, DocumentEvent};
use crate::extension_registry::ExtensionRegistry;
use crate::image::{DecodedImage, DecoderChain, StandardDecoder};
use crate::persistent_filter::FilterOperation;
use crate::render::D2DRenderer;
use crate::render::layout::DisplayMode;
use crate::selection::{HandleKind, HitTestResult, PixelRect, Selection};
use crate::susie::SusieManager;
use crate::ui::cursor_hider::{CursorHider, TIMER_ID_CURSOR_HIDE};
use crate::ui::file_list_panel::FileListPanel;
use crate::ui::font::MonospaceFont;
use crate::ui::fullscreen::FullscreenState;
use crate::ui::info_dialog;
use crate::ui::key_config::{
    Action, InputChord, KeyConfig, Modifiers, MouseButton, WheelDirection,
};
use crate::ui::menu;
use crate::ui::window;

/// DocumentEventをUIスレッドに通知するためのカスタムメッセージ
const WM_DOCUMENT_EVENT: u32 = WM_APP + 1;

/// スライドショー用タイマーID
const TIMER_ID_SLIDESHOW: usize = 2;

/// 修飾キーVKコード
const VK_CONTROL: i32 = 0x11;
const VK_SHIFT: i32 = 0x10;
const VK_MENU: i32 = 0x12; // Alt

/// メインウィンドウ (View 層)
///
/// Win32 メッセージループから呼び出され、`Document` モデルへの操作と `D2DRenderer`
/// による描画を仲介する。`docs/architecture.md` の Model-View 分離パターン参照。
///
/// - メニューバー・キー入力・ファイルリストパネル等の UI 状態を所有
/// - `Document` から `DocumentEvent` をチャネル経由で受け取り、再描画や UI 更新を行う
/// - エラーは `show_error_title` でタイトルバーに表示する (詳細は CLAUDE.md エラー方針)
pub struct AppWindow {
    hwnd: HWND,
    document: Document,
    event_receiver: Receiver<DocumentEvent>,
    renderer: D2DRenderer,
    fullscreen: FullscreenState,
    cursor_hider: CursorHider,
    always_on_top: bool,
    key_config: KeyConfig,
    // メニューバー
    menu: HMENU,
    menu_visible: bool,
    // ファイルリストパネル
    file_list_panel: FileListPanel,
    // パネル表示中のキャッシュ状態追跡 (差分更新用)
    cached_indices: HashSet<usize>,
    // 等幅フォント (ダイアログ・ファイルリスト用)
    monospace_font: MonospaceFont,
    // 矩形選択
    selection: Selection,
    // スライドショー
    slideshow_active: bool,
    slideshow_interval_ms: u32,
    slideshow_repeat: bool,
}

impl AppWindow {
    /// AppWindowを作成しウィンドウを表示する
    pub fn create(config: Config, initial_files: &[PathBuf]) -> Result<Box<Self>> {
        let class_name = windows::core::w!("gv_main");

        // アイコンをリソースからロード (リソースID 1)
        let icon = unsafe {
            let hmodule = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok();
            let hinstance = hmodule.map(|m| windows::Win32::Foundation::HINSTANCE(m.0));
            // MAKEINTRESOURCE(1) — リソースID 1 をポインタとして渡す
            #[allow(clippy::manual_dangling_ptr)]
            LoadIconW(hinstance, windows::core::PCWSTR(1 as *const u16)).ok()
        };
        window::register_window_class_with_icon(class_name, Some(Self::wnd_proc), icon)?;

        let hwnd = window::create_window(class_name, windows::core::w!("ぐらびゅ"), 1024, 768)?;

        // ウィンドウにアイコンを設定 (タスクバー表示用)
        if let Some(ref icon) = icon {
            unsafe {
                let _ = SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(0)), // ICON_SMALL
                    Some(LPARAM(icon.0 as isize)),
                );
                let _ = SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(1)), // ICON_BIG
                    Some(LPARAM(icon.0 as isize)),
                );
            }
        }

        // D&Dを受け付ける
        unsafe {
            DragAcceptFiles(hwnd, true);
        }

        let (sender, receiver) = crossbeam_channel::unbounded();
        let renderer = D2DRenderer::new(hwnd, &config.display)?;

        // 拡張子レジストリ + Susieプラグイン + デコーダチェーン + アーカイブマネージャの初期化
        let mut registry = ExtensionRegistry::new();
        let spi_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join(&config.susie.plugin_dir)));
        let susie_manager = spi_dir
            .as_deref()
            .map(SusieManager::discover)
            .unwrap_or_default();
        susie_manager.register_extensions(&mut registry);

        let registry = Arc::new(registry);

        // デコーダチェーン: Standard → Susie画像プラグイン (フォールバック順)
        let mut decoders: Vec<Box<dyn crate::image::ImageDecoder>> =
            vec![Box::new(StandardDecoder::new())];
        for decoder in susie_manager.create_image_decoders() {
            decoders.push(decoder);
        }
        let decoder = Arc::new(DecoderChain::new(decoders));

        // アーカイブマネージャ + Susieアーカイブプラグイン
        let mut archive_manager = ArchiveManager::new(Arc::clone(&registry));
        for handler in susie_manager.create_archive_handlers(Arc::clone(&registry)) {
            archive_manager.add_handler(handler);
        }

        let document = Document::new(sender, decoder, Arc::clone(&registry), archive_manager);

        let always_on_top = config.window.always_on_top;
        let base_image_size = config.prefetch.base_image_size();

        // キーバインド設定の読み込み
        let key_config_path = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("ぐらびゅ.keys.toml")));
        let key_config = KeyConfig::load(key_config_path.as_deref());

        // メニューバー構築 (初期状態は非表示)
        let menu_handle = menu::build_menu_bar();

        // ファイルリストパネル作成 (初期状態は非表示)
        let file_list_panel = FileListPanel::create(hwnd);

        // 等幅フォント作成 + ファイルリストに適用
        let monospace_font = MonospaceFont::new(16);
        unsafe {
            let _ = SendMessageW(
                file_list_panel.listbox_hwnd(),
                WM_SETFONT,
                Some(WPARAM(monospace_font.hfont().0 as usize)),
                Some(LPARAM(1)),
            );
        }

        let mut app = Box::new(Self {
            hwnd,
            document,
            event_receiver: receiver,
            renderer,
            fullscreen: FullscreenState::new(),
            cursor_hider: CursorHider::new(),
            always_on_top,
            key_config,
            menu: menu_handle,
            menu_visible: true,
            file_list_panel,
            cached_indices: HashSet::new(),
            monospace_font,
            selection: Selection::new(),
            slideshow_active: false,
            slideshow_interval_ms: config.slideshow.interval_ms,
            slideshow_repeat: config.slideshow.repeat,
        });

        // GWLP_USERDATAにポインタを格納 (WndProcからアクセスするため)
        window::set_window_data(hwnd, std::ptr::from_mut(&mut *app));

        // 先読みエンジン起動
        // 通知コールバック: ワーカースレッドからPostMessageWでUIスレッドを起こす
        let hwnd_raw = hwnd.0 as isize;
        let notify: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || unsafe {
            let _ = PostMessageW(
                Some(HWND(hwnd_raw as *mut _)),
                WM_DOCUMENT_EVENT,
                WPARAM(0),
                LPARAM(0),
            );
        });
        let cache_budget = Self::get_cache_budget();
        if let Err(e) = app
            .document
            .start_prefetch(notify, cache_budget, base_image_size)
        {
            app.show_error_title(&format!("先読みエンジンの起動に失敗しました: {e}"));
        }

        // 設定でalways_on_topが有効な場合、ウィンドウに反映
        if always_on_top {
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE,
                );
            }
        }

        // 初期ファイルがあれば開く
        if initial_files.is_empty() {
            // ファイル未指定起動: バージョン入りタイトルを反映
            app.update_title();
        } else {
            let result = if initial_files.len() > 1 {
                // 複数パス: フォルダ・コンテナ・画像・ブックマークの混在をフラットに展開
                app.document.open_multiple(initial_files)
            } else if initial_files[0].is_dir() {
                app.document.open_folder(&initial_files[0])
            } else {
                app.document.open(&initial_files[0])
            };
            if let Err(e) = result {
                app.show_error_title(&format!("ファイルを開けませんでした: {e}"));
            }
            app.process_document_events();
        }

        // メニューバーをデフォルト表示
        unsafe {
            let _ = SetMenu(hwnd, Some(app.menu));
        }

        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = UpdateWindow(hwnd);
        }

        Ok(app)
    }

    /// 空きメモリの50%をキャッシュ予算として返す
    fn get_cache_budget() -> usize {
        let mut mem_info = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        let available = unsafe {
            if GlobalMemoryStatusEx(std::ptr::from_mut(&mut mem_info)).is_ok() {
                mem_info.ullAvailPhys as usize
            } else {
                512 * 1024 * 1024 // フォールバック: 512MB
            }
        };
        available / 2
    }

    /// DocumentEventを処理する
    fn process_document_events(&mut self) {
        // 先読みレスポンスを処理 (キャッシュ格納 + current_image更新)
        self.document.process_prefetch_responses();

        // バックグラウンドコンテナ展開結果を処理
        self.document.process_expand_results();

        // FileListChanged は同一poll内で複数届きうる (バックグラウンド統合バッチごと等)。
        // パネル更新コストを抑えるため、ループ中はフラグだけ立て、ループ抜け後に1回だけ
        // panel.update() を呼ぶ。
        let mut file_list_changed = false;
        let mut nav_changed_index: Option<usize> = None;
        while let Ok(event) = self.event_receiver.try_recv() {
            match event {
                DocumentEvent::ImageReady => unsafe {
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                },
                DocumentEvent::NavigationChanged { index, .. } => {
                    nav_changed_index = Some(index);
                }
                DocumentEvent::FileListChanged => {
                    self.stop_slideshow();
                    file_list_changed = true;
                }
                DocumentEvent::Error(msg) => {
                    self.show_error_title(&msg);
                }
            }
        }

        if file_list_changed {
            let count = self.document.file_list().len();
            self.file_list_panel.update(count);
            self.update_title();
        }
        if let Some(index) = nav_changed_index {
            self.update_title();
            self.file_list_panel.set_selection(index);
        }

        // パネル表示中ならキャッシュ状態の差分のみ更新 (該当行のみ再描画)
        if self.file_list_panel.is_visible() {
            let doc = &self.document;
            let len = doc.file_list().len();
            // 現在のキャッシュ状態をスナップショット (上限6件程度なので軽量)
            let mut new_cached = HashSet::new();
            for i in 0..len {
                if doc.is_cached(i) {
                    new_cached.insert(i);
                }
            }
            // 前回との差分だけ該当行を再描画
            for &i in self.cached_indices.symmetric_difference(&new_cached) {
                self.file_list_panel.update_item(i);
            }
            self.cached_indices = new_cached;
        }
    }

    /// タイトルバーを更新
    fn update_title(&self) {
        let title = if let Some(source) = self.document.current_source() {
            // PendingContainer 上にいる場合は「読み込み中」プレフィックスを付ける
            let loading_prefix = if source.is_pending_container() {
                "読み込み中: "
            } else {
                ""
            };
            let display = source.display_path();
            let fl = self.document.file_list();
            let page_info = if let Some(idx) = fl.current_index() {
                format!(" [{}/{}]", idx + 1, fl.len())
            } else {
                String::new()
            };
            // 選択情報をタイトルに追加
            let sel_info = if let Some(rect) = self.selection.current_rect() {
                format!(
                    " 選択: ({}, {}) {}×{}",
                    rect.x, rect.y, rect.width, rect.height
                )
            } else {
                String::new()
            };
            // バックグラウンド展開の進捗表示
            let expand_info = if let Some((done, total)) = self.document.expand_progress() {
                format!(" 読込: {done}/{total}")
            } else {
                String::new()
            };
            format!("{loading_prefix}{display}{page_info}{sel_info}{expand_info} - ぐらびゅ\0")
        } else {
            concat!("ぐらびゅ v", env!("CARGO_PKG_VERSION"), "\0").to_string()
        };

        let wide: Vec<u16> = title.encode_utf16().collect();
        unsafe {
            let _ = SetWindowTextW(self.hwnd, windows::core::PCWSTR(wide.as_ptr()));
        }
    }

    /// タイトルバーにエラーメッセージを表示する
    fn show_error_title(&self, msg: &str) {
        let title = format!("ぐらびゅ - エラー: {msg}\0");
        let wide: Vec<u16> = title.encode_utf16().collect();
        unsafe {
            let _ = SetWindowTextW(self.hwnd, windows::core::PCWSTR(wide.as_ptr()));
        }
    }

    /// 現在の修飾キー状態を取得
    fn current_modifiers() -> Modifiers {
        unsafe {
            Modifiers {
                ctrl: GetKeyState(VK_CONTROL) < 0,
                shift: GetKeyState(VK_SHIFT) < 0,
                alt: GetKeyState(VK_MENU) < 0,
            }
        }
    }

    /// 再描画をリクエスト
    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// 現在の画像サイズを返す (zoom操作用)
    fn current_image_size(&self) -> Option<(u32, u32)> {
        self.document
            .current_image()
            .map(|img| (img.width, img.height))
    }

    /// クライアント領域のサイズを返す
    fn client_size(&self) -> (f32, f32) {
        let (w, h) = window::get_client_size(self.hwnd);
        (w as f32, h as f32)
    }

    /// 常に手前に表示をトグル
    fn toggle_always_on_top(&mut self) {
        self.always_on_top = !self.always_on_top;
        // フルスクリーン中は復帰時に反映されるので今は何もしない
        if !self.fullscreen.is_fullscreen() {
            let z_order = if self.always_on_top {
                HWND_TOPMOST
            } else {
                HWND_NOTOPMOST
            };
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(z_order),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE,
                );
            }
        }
    }

    /// フルスクリーンをトグル
    fn toggle_fullscreen(&mut self) {
        let entering = !self.fullscreen.is_fullscreen();

        // フルスクリーン開始前にパネル表示状態を保存
        let panel_was_visible = self.file_list_panel.is_visible();

        self.fullscreen.toggle(self.hwnd, self.always_on_top);

        if entering {
            // フルスクリーン開始: メニュー・パネルを非表示 (フラグは保持)
            unsafe {
                let _ = SetMenu(self.hwnd, None);
            }
            if panel_was_visible {
                self.file_list_panel.hide_preserve_state();
            }
        } else {
            // フルスクリーン解除: カーソル復帰、メニュー・パネルを復元
            self.cursor_hider.force_show(self.hwnd);
            if self.menu_visible {
                unsafe {
                    let _ = SetMenu(self.hwnd, Some(self.menu));
                }
            }
            if self.file_list_panel.is_visible() {
                self.file_list_panel.show();
            }
        }
    }

    /// 最大化トグル (左ダブルクリック)
    fn toggle_maximize(&self) {
        unsafe {
            let mut placement = WINDOWPLACEMENT {
                length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            let _ = GetWindowPlacement(self.hwnd, std::ptr::from_mut(&mut placement));
            if placement.showCmd == SW_MAXIMIZE.0 as u32 {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            } else {
                let _ = ShowWindow(self.hwnd, SW_MAXIMIZE);
            }
        }
    }

    /// メニューポップアップ表示時にトグル項目のチェック状態を更新
    fn update_menu_checks(&self, popup: HMENU) {
        let pf = self.document.persistent_filter();
        let pf_enabled = pf.is_enabled();

        // 永続フィルタの有効/無効チェック
        menu::update_menu_check(popup, Action::PFilterToggle, pf_enabled);

        // 各フィルタ操作のチェックマーク + フィルタ無効時はグレーアウト
        use crate::persistent_filter::FilterOperation as FO;
        let filter_actions: &[(Action, FO)] = &[
            (Action::PFilterFlipH, FO::FlipHorizontal),
            (Action::PFilterFlipV, FO::FlipVertical),
            (Action::PFilterRotate180, FO::Rotate180),
            (Action::PFilterRotate90CW, FO::Rotate90CW),
            (Action::PFilterRotate90CCW, FO::Rotate90CCW),
            (Action::PFilterLevels, FO::Levels { low: 0, high: 0 }),
            (Action::PFilterGamma, FO::Gamma { value: 0.0 }),
            (
                Action::PFilterBrightnessContrast,
                FO::BrightnessContrast {
                    brightness: 0,
                    contrast: 0,
                },
            ),
            (Action::PFilterGrayscaleSimple, FO::GrayscaleSimple),
            (Action::PFilterGrayscaleStrict, FO::GrayscaleStrict),
            (Action::PFilterBlur, FO::Blur),
            (Action::PFilterBlurStrong, FO::BlurStrong),
            (Action::PFilterSharpen, FO::Sharpen),
            (Action::PFilterSharpenStrong, FO::SharpenStrong),
            (
                Action::PFilterGaussianBlur,
                FO::GaussianBlur { radius: 0.0 },
            ),
            (Action::PFilterUnsharpMask, FO::UnsharpMask { radius: 0.0 }),
            (Action::PFilterMedianFilter, FO::MedianFilter),
            (Action::PFilterInvertColors, FO::InvertColors),
            (Action::PFilterApplyAlpha, FO::ApplyAlpha),
        ];
        for (action, probe) in filter_actions {
            menu::update_menu_check(popup, *action, pf.has_operation(probe));
            menu::update_menu_enabled(popup, *action, pf_enabled);
        }

        // その他のトグル項目
        menu::update_menu_check(
            popup,
            Action::ToggleFileList,
            self.file_list_panel.is_visible(),
        );
        menu::update_menu_check(popup, Action::ToggleAlwaysOnTop, self.always_on_top);
        menu::update_menu_check(
            popup,
            Action::ToggleMargin,
            self.renderer.layout().margin_enabled,
        );
        menu::update_menu_check(
            popup,
            Action::ToggleCursorHide,
            self.cursor_hider.is_enabled(),
        );
        menu::update_menu_check(
            popup,
            Action::ToggleFullscreen,
            self.fullscreen.is_fullscreen(),
        );
        menu::update_menu_check(popup, Action::SlideshowToggle, self.slideshow_active);
    }

    // --- WndProc ---

    extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if let Some(app) = window::get_window_data::<Self>(hwnd) {
            match msg {
                WM_PAINT => {
                    app.on_paint();
                    return LRESULT(0);
                }
                WM_SIZE => {
                    let mut width = (lparam.0 & 0xFFFF) as u32;
                    let mut height = ((lparam.0 >> 16) & 0xFFFF) as u32;
                    // パネルのtoggleからSendMessageW (WM_SIZE, 0, 0) で呼ばれる場合
                    if width == 0 && height == 0 {
                        let (w, h) = window::get_client_size(hwnd);
                        width = w;
                        height = h;
                    }
                    app.on_size(width, height);
                    return LRESULT(0);
                }
                WM_KEYDOWN | WM_SYSKEYDOWN => {
                    // Escキー: ドラッグ操作中は選択をキャンセル (key_configより優先)
                    if wparam.0 as u16 == 0x1B && app.selection.is_dragging() {
                        app.selection.deselect();
                        unsafe {
                            windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture()
                                .unwrap_or_default();
                        }
                        app.invalidate();
                        app.update_title();
                        return LRESULT(0);
                    }

                    let chord = InputChord::Key {
                        vk: wparam.0 as u16,
                        modifiers: Self::current_modifiers(),
                    };
                    if let Some(action) = app.key_config.lookup(chord) {
                        app.execute_action(action);
                        return LRESULT(0);
                    }
                    // SYSKEYDOWNは未処理時にDefWindowProcへ渡す必要あり
                    if msg == WM_SYSKEYDOWN {
                        // fall through to DefWindowProcW
                    } else {
                        return LRESULT(0);
                    }
                }
                WM_MOUSEWHEEL => {
                    let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
                    let direction = if delta > 0 {
                        WheelDirection::Up
                    } else {
                        WheelDirection::Down
                    };
                    let chord = InputChord::Wheel {
                        direction,
                        modifiers: Self::current_modifiers(),
                    };
                    if let Some(action) = app.key_config.lookup(chord) {
                        app.execute_action(action);
                        // 同期再描画でフレームスキップ防止
                        unsafe {
                            let _ = UpdateWindow(app.hwnd);
                        }
                    }
                    return LRESULT(0);
                }
                WM_LBUTTONDOWN => {
                    app.on_lbutton_down(lparam);
                    return LRESULT(0);
                }
                WM_LBUTTONUP => {
                    app.on_lbutton_up();
                    return LRESULT(0);
                }
                WM_LBUTTONDBLCLK => {
                    let chord = InputChord::Mouse {
                        button: MouseButton::LeftDoubleClick,
                    };
                    if let Some(action) = app.key_config.lookup(chord) {
                        app.execute_action(action);
                    }
                    return LRESULT(0);
                }
                WM_MBUTTONUP => {
                    let chord = InputChord::Mouse {
                        button: MouseButton::MiddleClick,
                    };
                    if let Some(action) = app.key_config.lookup(chord) {
                        app.execute_action(action);
                    }
                    return LRESULT(0);
                }
                WM_MOUSEMOVE => {
                    if app.fullscreen.is_fullscreen() {
                        app.cursor_hider.on_mouse_move(hwnd);
                    }
                    app.on_mouse_move(lparam);
                    return LRESULT(0);
                }
                WM_SETCURSOR => {
                    // 選択状態に応じてカーソルを変更
                    if app.on_set_cursor() {
                        return LRESULT(1);
                    }
                }
                WM_TIMER => {
                    if wparam.0 == TIMER_ID_CURSOR_HIDE {
                        app.cursor_hider.on_timer(hwnd);
                        return LRESULT(0);
                    }
                    if wparam.0 == TIMER_ID_SLIDESHOW {
                        app.on_slideshow_timer();
                        return LRESULT(0);
                    }
                }
                WM_INITMENUPOPUP => {
                    // wParam = 開こうとしているポップアップメニューのHMENU
                    let popup = HMENU(wparam.0 as *mut _);
                    app.update_menu_checks(popup);
                }
                WM_COMMAND => {
                    let notify_code = ((wparam.0 as u32) >> 16) & 0xFFFF;
                    let control_id = (wparam.0 as u32) & 0xFFFF;
                    let control_hwnd = HWND(lparam.0 as *mut _);

                    // メニュー項目 (notify_code == 0 かつコントロールなし)
                    if notify_code == 0 && control_hwnd.0.is_null() {
                        if let Some(action) = menu::menu_id_to_action(control_id as u16) {
                            app.execute_action(action);
                        }
                        return LRESULT(0);
                    }

                    return LRESULT(0);
                }
                WM_NOTIFY => {
                    // SAFETY: WM_NOTIFY の lparam は OS が有効な NMHDR へのポインタを保証する
                    let nmhdr = unsafe { &*(lparam.0 as *const NMHDR) };
                    if nmhdr.hwndFrom == app.file_list_panel.listview_hwnd() {
                        return app.handle_file_list_notify(nmhdr, lparam);
                    }
                    return LRESULT(0);
                }
                WM_DROPFILES => {
                    app.on_drop_files(HDROP(wparam.0 as *mut _));
                    return LRESULT(0);
                }
                WM_ERASEBKGND => {
                    // Direct2Dが背景を描画するのでちらつき防止
                    return LRESULT(1);
                }
                WM_DESTROY => {
                    // ポインタをクリアしてダングリング参照を防止
                    window::set_window_data::<Self>(hwnd, std::ptr::null_mut());
                    unsafe { PostQuitMessage(0) };
                    return LRESULT(0);
                }
                msg if msg == WM_DOCUMENT_EVENT => {
                    app.process_document_events();
                    return LRESULT(0);
                }
                _ => {}
            }
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    fn on_paint(&mut self) {
        let sel_rect = self.selection.current_rect();
        self.renderer
            .draw(self.document.current_image(), sel_rect.as_ref());
        // WM_PAINTの無限ループを防ぐためにValidateRectを呼ぶ
        unsafe {
            let _ = ValidateRect(Some(self.hwnd), None);
        }
    }

    fn on_size(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            // ファイルリストパネルのリサイズ
            let panel_width = self.file_list_panel.panel_width() as u32;
            self.file_list_panel.resize(height as i32);

            // D2Dレンダーターゲットは全体サイズでリサイズ
            self.renderer.resize(width, height);

            // 描画オフセットを設定 (パネル幅分だけ右にずらす)
            self.renderer.set_draw_offset(panel_width as f32);

            self.invalidate();
        }
    }

    /// 未保存確認 → 選択解除 → ナビゲーション操作 → イベント処理の共通パターン
    fn navigate_with_guard(&mut self, f: impl FnOnce(&mut Document)) {
        if !self.guard_unsaved_edit() {
            return;
        }
        self.selection.deselect();
        f(&mut self.document);
        self.process_document_events();
    }

    /// パラメータなしフィルタの共通パターン (選択範囲対応)
    fn apply_simple_filter(&mut self, f: fn(&DecodedImage, Option<&PixelRect>) -> DecodedImage) {
        if let Some(img) = self.document.current_image() {
            let sel = self.selection.current_rect();
            let result = f(img, sel.as_ref());
            self.document.apply_edit(result);
            self.process_document_events();
        }
    }

    /// 画像全体に適用する変形操作の共通パターン (選択解除付き)
    fn apply_transform(&mut self, f: fn(&DecodedImage) -> DecodedImage) {
        if let Some(img) = self.document.current_image() {
            let result = f(img);
            self.selection.deselect();
            self.document.apply_edit(result);
            self.process_document_events();
        }
    }

    /// 永続フィルタのトグル (既存なら削除、なければ追加)
    fn toggle_persistent_filter(&mut self, op: crate::persistent_filter::FilterOperation) {
        let pf = self.document.persistent_filter_mut();
        if !pf.remove_operation_type(&op) {
            pf.add_operation(op);
        }
        self.document.on_persistent_filter_changed();
        self.process_document_events();
    }

    /// パラメータ付き永続フィルタのトグル (既存なら削除してtrue、なければfalse)
    fn remove_persistent_filter_if_exists(
        &mut self,
        probe: &crate::persistent_filter::FilterOperation,
    ) -> bool {
        let pf = self.document.persistent_filter_mut();
        if pf.remove_operation_type(probe) {
            self.document.on_persistent_filter_changed();
            self.process_document_events();
            true
        } else {
            false
        }
    }

    // === アクションハンドラ (execute_action から呼び出される個別メソッド) ===

    fn action_mark_set(&mut self) {
        // mark_current() は内部でnavigate_relative(1) するのでマーク元indexを先に取得
        let mark_idx = self.document.file_list().current_index();
        self.document.mark_current();
        self.process_document_events();
        if self.file_list_panel.is_visible()
            && let Some(idx) = mark_idx
        {
            self.file_list_panel.update_item(idx);
        }
    }

    fn action_mark_unset(&mut self) {
        self.document.unmark_current();
        if self.file_list_panel.is_visible()
            && let Some(idx) = self.document.file_list().current_index()
        {
            self.file_list_panel.update_item(idx);
        }
    }

    fn action_mark_invert_all(&mut self) {
        self.document.invert_all_marks();
        self.sync_file_list_panel();
    }

    fn action_mark_invert_to_here(&mut self) {
        self.document.invert_marks_to_here();
        self.sync_file_list_panel();
    }

    fn action_open_file(&mut self) {
        if !self.guard_unsaved_edit() {
            return;
        }
        self.selection.deselect();
        let initial_dir = self
            .document
            .current_source()
            .and_then(|s| s.parent_dir())
            .map(Path::to_path_buf);
        if let Ok(Some(path)) = crate::file_ops::open_file_dialog(self.hwnd, initial_dir.as_deref())
        {
            if let Err(e) = self.document.open(&path) {
                self.show_error_title(&format!("ファイルを開けませんでした: {e}"));
            }
            self.process_document_events();
        }
    }

    fn action_open_folder(&mut self) {
        if !self.guard_unsaved_edit() {
            return;
        }
        self.selection.deselect();
        let initial_dir = self
            .document
            .current_source()
            .and_then(|s| s.parent_dir())
            .map(Path::to_path_buf);
        if let Ok(Some(path)) =
            crate::file_ops::open_folder_dialog(self.hwnd, initial_dir.as_deref())
        {
            if let Err(e) = self.document.open_folder(&path) {
                self.show_error_title(&format!("フォルダを開けませんでした: {e}"));
            }
            self.process_document_events();
        }
    }

    fn action_delete_file(&mut self) {
        // コンテナ内 (アーカイブ/PDF) のファイル削除は無効
        if let Some(source) = self.document.current_source()
            && source.is_contained()
        {
            return;
        }
        if let Some(path) = self.document.current_path().map(Path::to_path_buf) {
            if let Ok(true) = crate::file_ops::delete_to_recycle_bin(self.hwnd, &[&path]) {
                self.document.remove_current_from_list();
                self.process_document_events();
            }
            // Shell APIがフォーカスを奪うことがあるため復帰
            unsafe {
                let _ = SetForegroundWindow(self.hwnd);
            }
        }
    }

    fn action_move_file(&mut self) {
        let Some(current) = self.document.file_list().current() else {
            return;
        };
        let source = current.source.clone();
        let path = current.path.clone();

        // PDFページ・未展開コンテナは移動不可
        if matches!(
            source,
            crate::file_info::FileSource::PdfPage { .. }
                | crate::file_info::FileSource::PendingContainer { .. }
        ) {
            return;
        }

        let initial_dir = source.parent_dir().map(Path::to_path_buf);
        let default_name = source.default_save_name();

        // ファイルソースに応じてダイアログのラベルを分岐
        let (dialog_title, dialog_button) = match &source {
            crate::file_info::FileSource::File(_) => ("ファイルを移動", "移動"),
            _ => ("ファイルを書き出す", "書き出す"),
        };

        if let Ok(Some(dest)) = crate::file_ops::save_file_dialog(
            self.hwnd,
            crate::file_ops::SaveFileDialogParams {
                default_name: &default_name,
                filter_name: "すべてのファイル",
                filter_ext: "*.*",
                initial_dir: initial_dir.as_deref(),
                title: Some(dialog_title),
                ok_button_label: Some(dialog_button),
                ..Default::default()
            },
        ) {
            match &source {
                crate::file_info::FileSource::File(_) => {
                    // 通常ファイル: SHFileOperationWでUndo対応の移動
                    match crate::file_ops::move_single_file(self.hwnd, &path, &dest) {
                        Ok(true) => {
                            if let Err(e) = self.document.rename_current_in_list(&dest) {
                                self.show_error_title(&format!("リストの更新に失敗しました: {e}"));
                            }
                            self.process_document_events();
                        }
                        Ok(false) => {} // ユーザーキャンセル
                        Err(e) => {
                            self.show_error_title(&format!("ファイルの移動に失敗しました: {e}"));
                        }
                    }
                    // Shell APIがフォーカスを奪うことがあるため復帰
                    unsafe {
                        let _ = SetForegroundWindow(self.hwnd);
                    }
                }
                crate::file_info::FileSource::ArchiveEntry { on_demand, .. } => {
                    // アーカイブエントリ: 書き出し (リスト除去なし)
                    let result = if *on_demand {
                        self.document.read_file_data_current().and_then(|data| {
                            std::fs::write(&dest, &data).map_err(anyhow::Error::from)
                        })
                    } else {
                        std::fs::copy(&path, &dest)
                            .map(|_| ())
                            .map_err(anyhow::Error::from)
                    };
                    if let Err(e) = result {
                        self.show_error_title(&format!("ファイルの書き出しに失敗しました: {e}"));
                    }
                }
                crate::file_info::FileSource::PdfPage { .. }
                | crate::file_info::FileSource::PendingContainer { .. } => {
                    unreachable!(); // 上でガード済み
                }
            }
        }
    }

    fn action_copy_file(&mut self) {
        if let Some(current) = self.document.file_list().current() {
            let default_name = current.source.default_save_name();
            let initial_dir = current.source.parent_dir().map(Path::to_path_buf);
            if let Ok(Some(dest)) = crate::file_ops::save_file_dialog(
                self.hwnd,
                crate::file_ops::SaveFileDialogParams {
                    default_name: &default_name,
                    filter_name: "すべてのファイル",
                    filter_ext: "*.*",
                    initial_dir: initial_dir.as_deref(),
                    title: Some("ファイルを複製"),
                    ok_button_label: Some("複製"),
                    ..Default::default()
                },
            ) {
                let result = if matches!(
                    current.source,
                    crate::file_info::FileSource::ArchiveEntry {
                        on_demand: true,
                        ..
                    }
                ) {
                    // オンデマンド: アーカイブから読み出して書き出し
                    self.document
                        .read_file_data_current()
                        .and_then(|data| std::fs::write(&dest, &data).map_err(anyhow::Error::from))
                } else {
                    // 通常ファイル/temp展開済み/PDF: 既存のfs::copy
                    std::fs::copy(&current.path, &dest)
                        .map(|_| ())
                        .map_err(anyhow::Error::from)
                };
                if let Err(e) = result {
                    self.show_error_title(&format!("ファイルのコピーに失敗しました: {e}"));
                }
            }
        }
    }

    fn action_marked_delete(&mut self) {
        // コンテナ内 (アーカイブ/PDF) は無効
        if let Some(source) = self.document.current_source()
            && source.is_contained()
        {
            return;
        }
        let paths: Vec<std::path::PathBuf> = self
            .document
            .file_list()
            .marked_indices()
            .iter()
            .map(|&i| self.document.file_list().files()[i].path.clone())
            .collect();
        let path_refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        if let Ok(true) = crate::file_ops::delete_to_recycle_bin(self.hwnd, &path_refs) {
            self.document.remove_marked_from_list();
            self.process_document_events();
        }
        // Shell APIがフォーカスを奪うことがあるため復帰
        unsafe {
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    fn action_marked_move(&mut self) {
        if let Some(source) = self.document.current_source()
            && source.is_contained()
        {
            return;
        }
        let marked = self.document.file_list().marked_indices();
        let paths: Vec<std::path::PathBuf> = marked
            .iter()
            .map(|&i| self.document.file_list().files()[i].path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }
        let initial_dir = self.document.file_list().files()[marked[0]]
            .source
            .parent_dir()
            .map(Path::to_path_buf);
        if let Ok(Some(dest)) = crate::file_ops::select_folder_dialog(
            self.hwnd,
            "移動先フォルダ",
            initial_dir.as_deref(),
        ) {
            let path_refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
            if let Ok(true) = crate::file_ops::move_files(self.hwnd, &path_refs, &dest) {
                // パス更新失敗時は従来通りリストから削除 (フォールバック)
                if let Err(e) = self.document.update_marked_paths(&dest) {
                    eprintln!("パス更新失敗、リストから削除: {e}");
                    self.document.remove_marked_from_list();
                }
                self.process_document_events();
            }
            // Shell APIがフォーカスを奪うことがあるため復帰
            unsafe {
                let _ = SetForegroundWindow(self.hwnd);
            }
        }
    }

    fn action_marked_copy(&mut self) {
        let marked = self.document.file_list().marked_indices();
        let paths: Vec<std::path::PathBuf> = marked
            .iter()
            .map(|&i| self.document.file_list().files()[i].path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }
        let initial_dir = self.document.file_list().files()[marked[0]]
            .source
            .parent_dir()
            .map(Path::to_path_buf);
        if let Ok(Some(dest)) = crate::file_ops::select_folder_dialog(
            self.hwnd,
            "コピー先フォルダ",
            initial_dir.as_deref(),
        ) {
            let path_refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
            if let Err(e) = crate::file_ops::copy_files(self.hwnd, &path_refs, &dest) {
                self.show_error_title(&format!("ファイルのコピーに失敗しました: {e}"));
            }
            // Shell APIがフォーカスを奪うことがあるため復帰
            unsafe {
                let _ = SetForegroundWindow(self.hwnd);
            }
        }
    }

    fn action_marked_copy_names(&mut self) {
        let names: Vec<String> = self
            .document
            .file_list()
            .marked_indices()
            .iter()
            .map(|&i| self.document.file_list().files()[i].source.display_path())
            .collect();
        if !names.is_empty() {
            let text = names.join("\r\n");
            if let Err(e) = crate::clipboard::copy_text_to_clipboard(self.hwnd, &text) {
                self.show_error_title(&format!("マークファイル名のコピーに失敗しました: {e}"));
            }
        }
    }

    fn action_paste_image(&mut self) {
        if !self.guard_unsaved_edit() {
            return;
        }
        self.selection.deselect();
        match crate::clipboard::paste_image_from_clipboard(self.hwnd) {
            Ok(Some(image)) => {
                // 一時ファイルに保存してから開く
                let temp_path = std::env::temp_dir().join("gv_clipboard.png");
                if let Some(img_buf) =
                    image::RgbaImage::from_raw(image.width, image.height, image.data.clone())
                    && img_buf.save(&temp_path).is_ok()
                {
                    if let Err(e) = self.document.open_single(&temp_path) {
                        self.show_error_title(&format!("貼り付けに失敗しました: {e}"));
                    }
                    self.process_document_events();
                }
            }
            Ok(None) => {} // クリップボードに画像なし
            Err(e) => self.show_error_title(&format!("貼り付け失敗: {e}")),
        }
    }

    fn action_new_window(&mut self) {
        if let Ok(exe) = std::env::current_exe() {
            // 引数なしで空のウィンドウを起動
            if let Err(e) = std::process::Command::new(&exe).spawn() {
                self.show_error_title(&format!("新規ウィンドウの起動に失敗しました: {e}"));
            }
        }
    }

    fn action_close_all(&mut self) {
        if !self.guard_unsaved_edit() {
            return;
        }
        self.selection.deselect();
        self.document.close_all();
        self.process_document_events();
        self.update_title();
    }

    fn action_open_containing_folder(&mut self) {
        if let Some(source) = self.document.current_source() {
            let target = match source {
                crate::file_info::FileSource::ArchiveEntry { archive, .. } => archive.clone(),
                crate::file_info::FileSource::PdfPage { pdf_path, .. } => pdf_path.clone(),
                crate::file_info::FileSource::PendingContainer { container_path } => {
                    container_path.clone()
                }
                crate::file_info::FileSource::File(path) => path.clone(),
            };
            let arg = format!("/select,{}", target.display());
            if let Err(e) = std::process::Command::new("explorer.exe")
                .raw_arg(&arg)
                .spawn()
            {
                self.show_error_title(&format!("エクスプローラの起動に失敗しました: {e}"));
            }
        }
    }

    fn action_open_exe_folder(&mut self) {
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
            && let Err(e) = std::process::Command::new("explorer.exe").arg(dir).spawn()
        {
            self.show_error_title(&format!("エクスプローラの起動に失敗しました: {e}"));
        }
    }

    fn action_open_bookmark_folder(&mut self) {
        let dir = crate::bookmark::bookmark_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            // ディレクトリがすでにある場合は無視されるため、ここに来るのは権限不足等。
            // explorer.exe 起動側でも失敗するため致命的にせず警告のみ。
            eprintln!(
                "警告: ブックマークディレクトリ作成失敗: {} ({e})",
                dir.display()
            );
        }
        if let Err(e) = std::process::Command::new("explorer.exe").arg(&dir).spawn() {
            self.show_error_title(&format!("エクスプローラの起動に失敗しました: {e}"));
        }
    }

    fn action_open_spi_folder(&mut self) {
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            let spi_dir = dir.join("spi");
            if let Err(e) = std::fs::create_dir_all(&spi_dir) {
                eprintln!(
                    "警告: spi ディレクトリ作成失敗: {} ({e})",
                    spi_dir.display()
                );
            }
            if let Err(e) = std::process::Command::new("explorer.exe")
                .arg(&spi_dir)
                .spawn()
            {
                self.show_error_title(&format!("エクスプローラの起動に失敗しました: {e}"));
            }
        }
    }

    fn action_open_temp_folder(&mut self) {
        let dir = std::env::temp_dir();
        if let Err(e) = std::process::Command::new("explorer.exe").arg(&dir).spawn() {
            self.show_error_title(&format!("エクスプローラの起動に失敗しました: {e}"));
        }
    }

    fn action_rotate_arbitrary(&mut self) {
        if self.document.current_image().is_some()
            && let Some(degrees) = crate::ui::rotate_dialog::show_rotate_dialog(self.hwnd)
            && let Some(img) = self.document.current_image()
        {
            let result = crate::filter::transform::rotate_arbitrary(img, degrees);
            self.selection.deselect();
            self.document.apply_edit(result);
            self.process_document_events();
        }
    }

    fn action_resize(&mut self) {
        if let Some(img) = self.document.current_image()
            && let Some((nw, nh)) =
                crate::ui::resize_dialog::show_resize_dialog(self.hwnd, img.width, img.height)
            && let Some(img) = self.document.current_image()
        {
            match crate::filter::transform::resize(img, nw, nh) {
                Ok(result) => {
                    self.selection.deselect();
                    self.document.apply_edit(result);
                    self.process_document_events();
                }
                Err(e) => {
                    self.show_error_title(&format!("リサイズに失敗しました: {e}"));
                }
            }
        }
    }

    fn action_fill(&mut self) {
        if self.document.current_image().is_some() {
            use crate::ui::filter_dialog::{FieldDef, show_filter_dialog};
            let fields = [
                FieldDef {
                    label: "赤 (0-255)",
                    default: "255".into(),
                    integer_only: true,
                },
                FieldDef {
                    label: "緑 (0-255)",
                    default: "255".into(),
                    integer_only: true,
                },
                FieldDef {
                    label: "青 (0-255)",
                    default: "255".into(),
                    integer_only: true,
                },
            ];
            if let Some(vals) = show_filter_dialog(self.hwnd, "塗り潰す", &fields)
                && let Some(img) = self.document.current_image()
            {
                let r = vals[0].parse::<u8>().unwrap_or(255);
                let g = vals[1].parse::<u8>().unwrap_or(255);
                let b = vals[2].parse::<u8>().unwrap_or(255);
                let sel = self.selection.current_rect();
                let result = crate::filter::color::fill(img, sel.as_ref(), r, g, b);
                self.document.apply_edit(result);
                self.process_document_events();
            }
        }
    }

    fn action_levels(&mut self) {
        if self.document.current_image().is_some() {
            use crate::ui::filter_dialog::{FieldDef, show_filter_dialog};
            let fields = [
                FieldDef {
                    label: "下限 (0-255)",
                    default: "0".into(),
                    integer_only: true,
                },
                FieldDef {
                    label: "上限 (0-255)",
                    default: "255".into(),
                    integer_only: true,
                },
            ];
            if let Some(vals) = show_filter_dialog(self.hwnd, "レベル補正", &fields)
                && let Some(img) = self.document.current_image()
            {
                let low = vals[0].parse::<u8>().unwrap_or(0);
                let high = vals[1].parse::<u8>().unwrap_or(255);
                let sel = self.selection.current_rect();
                let result = crate::filter::brightness::levels(img, sel.as_ref(), low, high);
                self.document.apply_edit(result);
                self.process_document_events();
            }
        }
    }

    fn action_gamma(&mut self) {
        if self.document.current_image().is_some() {
            use crate::ui::filter_dialog::{FieldDef, show_filter_dialog};
            let fields = [FieldDef {
                label: "ガンマ値 (0.1〜10.0)",
                default: "1.0".into(),
                integer_only: false,
            }];
            if let Some(vals) = show_filter_dialog(self.hwnd, "ガンマ補正", &fields)
                && let Some(img) = self.document.current_image()
            {
                let gamma = vals[0].parse::<f64>().unwrap_or(1.0).clamp(0.1, 10.0);
                let sel = self.selection.current_rect();
                let result = crate::filter::brightness::gamma(img, sel.as_ref(), gamma);
                self.document.apply_edit(result);
                self.process_document_events();
            }
        }
    }

    fn action_brightness_contrast(&mut self) {
        if self.document.current_image().is_some() {
            use crate::ui::filter_dialog::{FieldDef, show_filter_dialog};
            let fields = [
                FieldDef {
                    label: "明るさ (-128〜128)",
                    default: "0".into(),
                    integer_only: false,
                },
                FieldDef {
                    label: "コントラスト (-128〜128)",
                    default: "0".into(),
                    integer_only: false,
                },
            ];
            if let Some(vals) = show_filter_dialog(self.hwnd, "明るさとコントラスト", &fields)
                && let Some(img) = self.document.current_image()
            {
                let brightness = vals[0].parse::<i32>().unwrap_or(0).clamp(-128, 128);
                let contrast = vals[1].parse::<i32>().unwrap_or(0).clamp(-128, 128);
                let sel = self.selection.current_rect();
                let result = crate::filter::brightness::brightness_contrast(
                    img,
                    sel.as_ref(),
                    brightness,
                    contrast,
                );
                self.document.apply_edit(result);
                self.process_document_events();
            }
        }
    }

    fn action_mosaic(&mut self) {
        if self.document.current_image().is_some() {
            use crate::ui::filter_dialog::{FieldDef, show_filter_dialog};
            let fields = [FieldDef {
                label: "ブロックサイズ",
                default: "10".into(),
                integer_only: true,
            }];
            if let Some(vals) = show_filter_dialog(self.hwnd, "モザイク", &fields)
                && let Some(img) = self.document.current_image()
            {
                let size = vals[0].parse::<u32>().unwrap_or(10).max(1);
                let sel = self.selection.current_rect();
                let result = crate::filter::blur::mosaic(img, sel.as_ref(), size);
                self.document.apply_edit(result);
                self.process_document_events();
            }
        }
    }

    fn action_gaussian_blur(&mut self) {
        if self.document.current_image().is_some() {
            use crate::ui::filter_dialog::{FieldDef, show_filter_dialog};
            let fields = [FieldDef {
                label: "半径 (0.1〜10.0)",
                default: "2.0".into(),
                integer_only: false,
            }];
            if let Some(vals) = show_filter_dialog(self.hwnd, "ガウスぼかし", &fields)
                && let Some(img) = self.document.current_image()
            {
                let radius = vals[0].parse::<f64>().unwrap_or(2.0).clamp(0.1, 10.0);
                let sel = self.selection.current_rect();
                let result = crate::filter::blur::gaussian_blur(img, sel.as_ref(), radius);
                self.document.apply_edit(result);
                self.process_document_events();
            }
        }
    }

    fn action_unsharp_mask(&mut self) {
        if self.document.current_image().is_some() {
            use crate::ui::filter_dialog::{FieldDef, show_filter_dialog};
            let fields = [FieldDef {
                label: "半径 (0.1〜10.0)",
                default: "2.0".into(),
                integer_only: false,
            }];
            if let Some(vals) = show_filter_dialog(self.hwnd, "アンシャープマスク", &fields)
                && let Some(img) = self.document.current_image()
            {
                let radius = vals[0].parse::<f64>().unwrap_or(2.0).clamp(0.1, 10.0);
                let sel = self.selection.current_rect();
                let result = crate::filter::blur::unsharp_mask(img, sel.as_ref(), radius);
                self.document.apply_edit(result);
                self.process_document_events();
            }
        }
    }

    fn action_bookmark_load(&mut self) {
        if !self.guard_unsaved_edit() {
            return;
        }
        self.selection.deselect();
        // is_archive クロージャは旧形式 (.gvb) のパース時にのみ使われる。
        // self.document の共有借用のみなので、後続の load_bookmark_data の可変借用と競合しない。
        let result = {
            let is_archive = |p: &std::path::Path| self.document.is_archive_path(p);
            crate::bookmark::load_bookmark(self.hwnd, is_archive)
        };
        match result {
            Ok(Some(data)) => {
                if let Err(e) = self.document.load_bookmark_data(data) {
                    self.show_error_title(&format!("ブックマークの読み込みに失敗しました: {e}"));
                }
                self.process_document_events();
            }
            Ok(None) => {} // キャンセル
            Err(e) => self.show_error_title(&format!("ブックマーク読み込み失敗: {e}")),
        }
    }

    fn action_pfilter_levels(&mut self) {
        // 既存なら削除 (トグルオフ)
        let probe = FilterOperation::Levels { low: 0, high: 0 };
        if self.remove_persistent_filter_if_exists(&probe) {
            return;
        }
        use crate::ui::filter_dialog::{FieldDef, show_filter_dialog};
        let fields = [
            FieldDef {
                label: "下限 (0-255)",
                default: "0".into(),
                integer_only: true,
            },
            FieldDef {
                label: "上限 (0-255)",
                default: "255".into(),
                integer_only: true,
            },
        ];
        if let Some(vals) = show_filter_dialog(self.hwnd, "永続レベル補正", &fields) {
            let low = vals[0].parse::<u8>().unwrap_or(0);
            let high = vals[1].parse::<u8>().unwrap_or(255);
            self.document
                .persistent_filter_mut()
                .add_operation(FilterOperation::Levels { low, high });
            self.document.on_persistent_filter_changed();
            self.process_document_events();
        }
    }

    fn action_pfilter_gamma(&mut self) {
        let probe = FilterOperation::Gamma { value: 0.0 };
        if self.remove_persistent_filter_if_exists(&probe) {
            return;
        }
        use crate::ui::filter_dialog::{FieldDef, show_filter_dialog};
        let fields = [FieldDef {
            label: "ガンマ値 (0.1〜10.0)",
            default: "1.0".into(),
            integer_only: false,
        }];
        if let Some(vals) = show_filter_dialog(self.hwnd, "永続ガンマ補正", &fields) {
            let value = vals[0].parse::<f64>().unwrap_or(1.0).clamp(0.1, 10.0);
            self.document
                .persistent_filter_mut()
                .add_operation(FilterOperation::Gamma { value });
            self.document.on_persistent_filter_changed();
            self.process_document_events();
        }
    }

    fn action_pfilter_brightness_contrast(&mut self) {
        let probe = FilterOperation::BrightnessContrast {
            brightness: 0,
            contrast: 0,
        };
        if self.remove_persistent_filter_if_exists(&probe) {
            return;
        }
        use crate::ui::filter_dialog::{FieldDef, show_filter_dialog};
        let fields = [
            FieldDef {
                label: "明るさ (-128〜128)",
                default: "0".into(),
                integer_only: false,
            },
            FieldDef {
                label: "コントラスト (-128〜128)",
                default: "0".into(),
                integer_only: false,
            },
        ];
        if let Some(vals) = show_filter_dialog(self.hwnd, "永続明るさとコントラスト", &fields)
        {
            let brightness = vals[0].parse::<i32>().unwrap_or(0).clamp(-128, 128);
            let contrast = vals[1].parse::<i32>().unwrap_or(0).clamp(-128, 128);
            self.document.persistent_filter_mut().add_operation(
                FilterOperation::BrightnessContrast {
                    brightness,
                    contrast,
                },
            );
            self.document.on_persistent_filter_changed();
            self.process_document_events();
        }
    }

    fn action_pfilter_gaussian_blur(&mut self) {
        let probe = FilterOperation::GaussianBlur { radius: 0.0 };
        if self.remove_persistent_filter_if_exists(&probe) {
            return;
        }
        use crate::ui::filter_dialog::{FieldDef, show_filter_dialog};
        let fields = [FieldDef {
            label: "半径 (0.1〜10.0)",
            default: "2.0".into(),
            integer_only: false,
        }];
        if let Some(vals) = show_filter_dialog(self.hwnd, "永続ガウスぼかし", &fields) {
            let radius = vals[0].parse::<f64>().unwrap_or(2.0).clamp(0.1, 10.0);
            self.document
                .persistent_filter_mut()
                .add_operation(FilterOperation::GaussianBlur { radius });
            self.document.on_persistent_filter_changed();
            self.process_document_events();
        }
    }

    fn action_pfilter_unsharp_mask(&mut self) {
        let probe = FilterOperation::UnsharpMask { radius: 0.0 };
        if self.remove_persistent_filter_if_exists(&probe) {
            return;
        }
        use crate::ui::filter_dialog::{FieldDef, show_filter_dialog};
        let fields = [FieldDef {
            label: "半径 (0.1〜10.0)",
            default: "2.0".into(),
            integer_only: false,
        }];
        if let Some(vals) = show_filter_dialog(self.hwnd, "永続アンシャープマスク", &fields)
        {
            let radius = vals[0].parse::<f64>().unwrap_or(2.0).clamp(0.1, 10.0);
            self.document
                .persistent_filter_mut()
                .add_operation(FilterOperation::UnsharpMask { radius });
            self.document.on_persistent_filter_changed();
            self.process_document_events();
        }
    }

    fn action_toggle_file_list(&mut self) {
        self.file_list_panel.toggle();
        // パネルが表示状態になったら全同期 (非表示中の変更を反映)
        if self.file_list_panel.is_visible() {
            let doc = &self.document;
            let len = doc.file_list().len();
            self.file_list_panel.update(len);
            if let Some(idx) = doc.file_list().current_index() {
                self.file_list_panel.set_selection(idx);
            }
            // cached_indicesも同期
            self.cached_indices.clear();
            for i in 0..len {
                if self.document.is_cached(i) {
                    self.cached_indices.insert(i);
                }
            }
        }
        // toggle内でWM_SIZEが送られてon_sizeが呼ばれる
        // 同期再描画でちらつきを防止
        unsafe {
            let _ = UpdateWindow(self.hwnd);
        }
    }

    /// ListView (file_list_panel) からの WM_NOTIFY を処理する
    fn handle_file_list_notify(&mut self, nmhdr: &NMHDR, lparam: LPARAM) -> LRESULT {
        match nmhdr.code {
            // テキスト要求: 該当インデックスのラベルを ListView 提供のバッファへコピー
            i if i == LVN_GETDISPINFOW => {
                // SAFETY: LVN_GETDISPINFOW の lparam は OS が有効な NMLVDISPINFOW へのポインタを保証する
                let dispinfo = unsafe { &mut *(lparam.0 as *mut NMLVDISPINFOW) };
                if (dispinfo.item.mask & LVIF_TEXT).0 != 0 {
                    let idx = dispinfo.item.iItem as usize;
                    let files = self.document.file_list().files();
                    if let Some(info) = files.get(idx) {
                        let is_cached = self.document.is_cached(idx);
                        let label = FileListPanel::format_label(info, is_cached);
                        // UTF-16 化して、ListView 提供の pszText バッファへコピー
                        let max = dispinfo.item.cchTextMax as usize;
                        if max > 0 && !dispinfo.item.pszText.0.is_null() {
                            let mut wide: Vec<u16> = label.encode_utf16().collect();
                            // ヌル終端1文字分を残して切り詰める
                            if wide.len() >= max {
                                wide.truncate(max - 1);
                            }
                            wide.push(0);
                            // SAFETY: pszText は OS が確保した cchTextMax 文字分のバッファを指す。
                            // wide.len() <= max (cchTextMax) を上記で保証済みのため範囲内に収まる。
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    wide.as_ptr(),
                                    dispinfo.item.pszText.0,
                                    wide.len(),
                                );
                            }
                        }
                    }
                }
                LRESULT(0)
            }
            // 選択変更通知: ユーザーのクリック・マウスホイール等で iItem が変わった
            i if i == LVN_ITEMCHANGED => {
                // SAFETY: LVN_ITEMCHANGED の lparam は OS が有効な NMLISTVIEW へのポインタを保証する
                let nmlv = unsafe { &*(lparam.0 as *const NMLISTVIEW) };
                // 状態変化のうち SELECTED ビットが新たに立ったときだけ反応
                let became_selected = (nmlv.uChanged.0 & LVIF_STATE.0) != 0
                    && (nmlv.uOldState & LVIS_SELECTED.0) == 0
                    && (nmlv.uNewState & LVIS_SELECTED.0) != 0;
                if became_selected && nmlv.iItem >= 0 {
                    let target = nmlv.iItem as usize;
                    if self.document.file_list().current_index() != Some(target) {
                        if !self.guard_unsaved_edit() {
                            // キャンセル時は選択位置を復元
                            if let Some(idx) = self.document.file_list().current_index() {
                                self.file_list_panel.set_selection(idx);
                            }
                            return LRESULT(0);
                        }
                        self.selection.deselect();
                        self.stop_slideshow();
                        self.document.navigate_to(target);
                        self.process_document_events();
                    }
                }
                LRESULT(0)
            }
            _ => LRESULT(0),
        }
    }

    /// ファイルリストパネルの同期ヘルパー(MarkInvertAll / MarkInvertToHere で共通)
    fn sync_file_list_panel(&mut self) {
        if self.file_list_panel.is_visible() {
            let doc = &self.document;
            let len = doc.file_list().len();
            self.file_list_panel.update(len);
            if let Some(idx) = doc.file_list().current_index() {
                self.file_list_panel.set_selection(idx);
            }
        }
    }

    /// アクションを実行する
    fn execute_action(&mut self, action: Action) {
        // スライドショー中は、スライドショー関連以外のアクションで自動停止
        if self.slideshow_active
            && !matches!(
                action,
                Action::SlideshowToggle | Action::SlideshowFaster | Action::SlideshowSlower
            )
        {
            self.stop_slideshow();
        }

        match action {
            // --- ナビゲーション ---
            Action::NavigateBack => self.navigate_with_guard(|d| d.navigate_relative(-1)),
            Action::NavigateForward => self.navigate_with_guard(|d| d.navigate_relative(1)),
            Action::Navigate1Back => self.navigate_with_guard(|d| d.navigate_relative(-1)),
            Action::Navigate1Forward => self.navigate_with_guard(|d| d.navigate_relative(1)),
            Action::Navigate5Back => self.navigate_with_guard(|d| d.navigate_relative(-5)),
            Action::Navigate5Forward => self.navigate_with_guard(|d| d.navigate_relative(5)),
            Action::Navigate50Back => self.navigate_with_guard(|d| d.navigate_relative(-50)),
            Action::Navigate50Forward => self.navigate_with_guard(|d| d.navigate_relative(50)),
            Action::NavigateFirst => self.navigate_with_guard(Document::navigate_first),
            Action::NavigateLast => self.navigate_with_guard(Document::navigate_last),

            // --- 表示モード ---
            Action::DisplayAutoShrink => {
                self.renderer.layout_mut().mode = DisplayMode::AutoShrink;
                self.invalidate();
            }
            Action::DisplayAutoFit => {
                self.renderer.layout_mut().mode = DisplayMode::AutoFit;
                self.invalidate();
            }
            Action::ZoomIn => {
                if let Some((iw, ih)) = self.current_image_size() {
                    let (ww, wh) = self.client_size();
                    self.renderer.layout_mut().zoom_in(iw, ih, ww, wh);
                    self.invalidate();
                }
            }
            Action::ZoomOut => {
                if let Some((iw, ih)) = self.current_image_size() {
                    let (ww, wh) = self.client_size();
                    self.renderer.layout_mut().zoom_out(iw, ih, ww, wh);
                    self.invalidate();
                }
            }
            Action::ZoomReset => {
                self.renderer.layout_mut().zoom_reset();
                self.invalidate();
            }
            Action::ToggleMargin => {
                self.renderer.layout_mut().toggle_margin();
                self.invalidate();
            }
            Action::CycleAlphaBackground => {
                self.renderer.cycle_alpha_background();
                self.invalidate();
            }

            // --- ウィンドウ ---
            Action::ToggleFullscreen => {
                self.toggle_fullscreen();
            }
            Action::Minimize => unsafe {
                let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
            },
            Action::ToggleMaximize => {
                if !self.fullscreen.is_fullscreen() {
                    self.toggle_maximize();
                }
            }
            Action::ToggleAlwaysOnTop => {
                self.toggle_always_on_top();
            }
            Action::ToggleCursorHide => {
                self.cursor_hider.toggle_enabled(self.hwnd);
            }

            // --- マーク操作 ---
            Action::MarkSet => self.action_mark_set(),
            Action::MarkUnset => self.action_mark_unset(),
            Action::MarkInvertAll => self.action_mark_invert_all(),
            Action::MarkInvertToHere => self.action_mark_invert_to_here(),
            Action::NavigatePrevMark => self.navigate_with_guard(Document::navigate_prev_mark),
            Action::NavigateNextMark => self.navigate_with_guard(Document::navigate_next_mark),
            Action::RemoveFromList => {
                self.document.remove_current_from_list();
                self.process_document_events();
            }
            Action::MarkedRemoveFromList => {
                self.document.remove_marked_from_list();
                self.process_document_events();
            }

            // --- フォルダナビゲーション ---
            Action::NavigatePrevFolder => self.navigate_with_guard(Document::navigate_prev_folder),
            Action::NavigateNextFolder => self.navigate_with_guard(Document::navigate_next_folder),

            // --- ファイル操作 ---
            Action::OpenFile => self.action_open_file(),
            Action::OpenFolder => self.action_open_folder(),
            Action::DeleteFile => self.action_delete_file(),
            Action::MoveFile => self.action_move_file(),
            Action::CopyFile => self.action_copy_file(),
            Action::MarkedDelete => self.action_marked_delete(),
            Action::MarkedMove => self.action_marked_move(),
            Action::MarkedCopy => self.action_marked_copy(),
            Action::Reload => self.navigate_with_guard(Document::reload),

            // --- クリップボード ---
            Action::CopyImage => {
                if let Some(image) = self.document.current_image()
                    && let Err(e) = crate::clipboard::copy_image_to_clipboard(self.hwnd, image)
                {
                    self.show_error_title(&format!("画像のコピーに失敗しました: {e}"));
                }
            }
            Action::CopyFileName => {
                if let Some(source) = self.document.current_source()
                    && let Err(e) =
                        crate::clipboard::copy_text_to_clipboard(self.hwnd, &source.display_path())
                {
                    self.show_error_title(&format!("ファイル名のコピーに失敗しました: {e}"));
                }
            }
            Action::MarkedCopyNames => self.action_marked_copy_names(),
            Action::PasteImage => self.action_paste_image(),

            // --- 画像書き出し ---
            Action::ExportJpg => self.export_image(ExportFormat::Jpg),
            Action::ExportBmp => self.export_image(ExportFormat::Bmp),
            Action::ExportPng => self.export_image(ExportFormat::Png),

            // --- ユーティリティ ---
            Action::NewWindow => self.action_new_window(),
            Action::CloseAll => self.action_close_all(),
            Action::OpenContainingFolder => self.action_open_containing_folder(),
            Action::OpenExeFolder => self.action_open_exe_folder(),
            Action::OpenBookmarkFolder => self.action_open_bookmark_folder(),
            Action::OpenSpiFolder => self.action_open_spi_folder(),
            Action::OpenTempFolder => self.action_open_temp_folder(),
            Action::ShowImageInfo => {
                self.show_image_info();
            }

            // --- 編集 ---
            Action::DeselectSelection => {
                self.selection.deselect();
                self.invalidate();
                self.update_title();
            }
            Action::Crop => {
                if let Some(sel_rect) = self.selection.current_rect()
                    && let Some(img) = self.document.current_image()
                {
                    let cropped = crate::filter::transform::crop(img, &sel_rect);
                    self.selection.deselect();
                    self.document.apply_edit(cropped);
                    self.process_document_events();
                    self.update_title();
                }
            }
            Action::FlipHorizontal => {
                self.apply_transform(crate::filter::transform::flip_horizontal);
            }
            Action::FlipVertical => {
                self.apply_transform(crate::filter::transform::flip_vertical);
            }
            Action::Rotate180 => {
                self.apply_transform(crate::filter::transform::rotate_180);
            }
            Action::Rotate90CW => {
                self.apply_transform(crate::filter::transform::rotate_90);
            }
            Action::Rotate90CCW => {
                self.apply_transform(crate::filter::transform::rotate_270);
            }
            Action::RotateArbitrary => self.action_rotate_arbitrary(),
            Action::Resize => self.action_resize(),

            // --- フィルタ (パラメータあり) ---
            Action::Fill => self.action_fill(),
            Action::Levels => self.action_levels(),
            Action::Gamma => self.action_gamma(),
            Action::BrightnessContrast => self.action_brightness_contrast(),
            Action::Mosaic => self.action_mosaic(),
            Action::GaussianBlur => self.action_gaussian_blur(),
            Action::UnsharpMask => self.action_unsharp_mask(),

            // --- フィルタ (パラメータなし) ---
            Action::InvertColors => self.apply_simple_filter(crate::filter::color::invert_colors),
            Action::GrayscaleSimple => {
                self.apply_simple_filter(crate::filter::color::grayscale_simple);
            }
            Action::GrayscaleStrict => {
                self.apply_simple_filter(crate::filter::color::grayscale_strict);
            }
            Action::ApplyAlpha => self.apply_simple_filter(crate::filter::color::apply_alpha),
            Action::Blur => self.apply_simple_filter(crate::filter::blur::blur),
            Action::BlurStrong => self.apply_simple_filter(crate::filter::blur::blur_strong),
            Action::Sharpen => self.apply_simple_filter(crate::filter::sharpen::sharpen),
            Action::SharpenStrong => {
                self.apply_simple_filter(crate::filter::sharpen::sharpen_strong);
            }
            Action::MedianFilter => self.apply_simple_filter(crate::filter::blur::median_filter),

            // --- 永続フィルタ ---
            Action::PFilterToggle => {
                self.document.persistent_filter_mut().toggle_enabled();
                self.document.on_persistent_filter_changed();
                self.process_document_events();
            }
            Action::PFilterFlipH => {
                self.toggle_persistent_filter(FilterOperation::FlipHorizontal);
            }
            Action::PFilterFlipV => {
                self.toggle_persistent_filter(FilterOperation::FlipVertical);
            }
            Action::PFilterRotate180 => {
                self.toggle_persistent_filter(FilterOperation::Rotate180);
            }
            Action::PFilterRotate90CW => {
                self.toggle_persistent_filter(FilterOperation::Rotate90CW);
            }
            Action::PFilterRotate90CCW => {
                self.toggle_persistent_filter(FilterOperation::Rotate90CCW);
            }
            Action::PFilterLevels => self.action_pfilter_levels(),
            Action::PFilterGamma => self.action_pfilter_gamma(),
            Action::PFilterBrightnessContrast => self.action_pfilter_brightness_contrast(),
            Action::PFilterGrayscaleSimple => {
                self.toggle_persistent_filter(FilterOperation::GrayscaleSimple);
            }
            Action::PFilterGrayscaleStrict => {
                self.toggle_persistent_filter(FilterOperation::GrayscaleStrict);
            }
            Action::PFilterBlur => self.toggle_persistent_filter(FilterOperation::Blur),
            Action::PFilterBlurStrong => {
                self.toggle_persistent_filter(FilterOperation::BlurStrong);
            }
            Action::PFilterSharpen => self.toggle_persistent_filter(FilterOperation::Sharpen),
            Action::PFilterSharpenStrong => {
                self.toggle_persistent_filter(FilterOperation::SharpenStrong);
            }
            Action::PFilterGaussianBlur => self.action_pfilter_gaussian_blur(),
            Action::PFilterUnsharpMask => self.action_pfilter_unsharp_mask(),
            Action::PFilterMedianFilter => {
                self.toggle_persistent_filter(FilterOperation::MedianFilter);
            }
            Action::PFilterInvertColors => {
                self.toggle_persistent_filter(FilterOperation::InvertColors);
            }
            Action::PFilterApplyAlpha => {
                self.toggle_persistent_filter(FilterOperation::ApplyAlpha);
            }

            // --- ブックマーク ---
            Action::BookmarkSave => {
                // 未展開コンテナがあれば全て同期展開 (ブックマークは完全な状態で保存する)
                if self.document.file_list().has_pending() {
                    self.document.expand_all_pending_sync();
                    self.process_document_events();
                }
                let idx = self.document.file_list().current_index();
                if let Err(e) =
                    crate::bookmark::save_bookmark(self.hwnd, self.document.file_list(), idx)
                {
                    self.show_error_title(&format!("ブックマークの保存に失敗しました: {e}"));
                }
            }
            Action::BookmarkLoad => self.action_bookmark_load(),
            // --- ページ指定ナビゲーション ---
            Action::NavigateToPage => {
                if !self.guard_unsaved_edit() {
                    return;
                }
                self.selection.deselect();
                self.navigate_to_page_dialog();
            }

            // --- ソートナビゲーション ---
            Action::SortNavigateBack => self.navigate_with_guard(Document::sort_navigate_back),
            Action::SortNavigateForward => {
                self.navigate_with_guard(Document::sort_navigate_forward);
            }

            // --- シャッフル ---
            Action::ShuffleAll => {
                self.document.shuffle_all();
                self.process_document_events();
            }
            Action::ShuffleGroups => {
                self.document.shuffle_groups();
                self.process_document_events();
            }

            // --- メニューバー ---
            Action::ToggleMenuBar => {
                self.menu_visible = !self.menu_visible;
                unsafe {
                    if self.menu_visible {
                        let _ = SetMenu(self.hwnd, Some(self.menu));
                    } else {
                        let _ = SetMenu(self.hwnd, None);
                    }
                }
            }

            // --- ファイルリスト ---
            Action::ToggleFileList => self.action_toggle_file_list(),

            // --- ヘルプ ---
            Action::ShowHelp => {
                self.show_help();
            }

            // --- アップデート ---
            Action::CheckUpdate => {
                self.check_for_update();
            }

            // --- シェル統合 ---
            Action::RegisterShell => {
                self.action_register_shell();
            }
            Action::UnregisterShell => {
                self.action_unregister_shell();
            }

            // --- スライドショー ---
            Action::SlideshowToggle => self.toggle_slideshow(),
            Action::SlideshowFaster => self.adjust_slideshow_interval(-500),
            Action::SlideshowSlower => self.adjust_slideshow_interval(500),

            // --- 終了 ---
            Action::Exit => unsafe {
                let _ = DestroyWindow(self.hwnd);
            },
        }
    }

    /// ページ指定ナビゲーション
    fn navigate_to_page_dialog(&mut self) {
        let total = self.document.file_list().len();
        if total == 0 {
            return;
        }
        let current = self.document.file_list().current_index().unwrap_or(0) + 1;
        if let Some(page) = crate::ui::page_dialog::show_page_dialog(self.hwnd, current, total) {
            let index = (page.saturating_sub(1)).min(total - 1);
            self.stop_slideshow();
            self.document.navigate_to(index);
            self.process_document_events();
        }
    }

    /// 画像を指定フォーマットで書き出す
    fn export_image(&mut self, format: ExportFormat) {
        let Some(img) = self.document.current_image() else {
            return;
        };
        let (default_stem, initial_dir) = self.document.current_source().map_or_else(
            || ("image".to_string(), None),
            |s| (s.default_save_stem(), s.parent_dir().map(Path::to_path_buf)),
        );
        let default_name = format!("{default_stem}.{}", format.extension());

        let Some(save_path) = crate::file_ops::save_file_dialog(
            self.hwnd,
            crate::file_ops::SaveFileDialogParams {
                default_name: &default_name,
                filter_name: format.filter_name(),
                filter_ext: format.filter_spec(),
                default_ext: format.extension(),
                initial_dir: initial_dir.as_deref(),
                ..Default::default()
            },
        )
        .ok()
        .flatten() else {
            return;
        };

        if let Err(e) = write_image_to_path(img.width, img.height, &img.data, format, &save_path) {
            self.show_error_title(&format!("{e}"));
        }
    }

    /// 数値を3桁カンマ区切りでフォーマットする
    fn format_with_commas(n: u64) -> String {
        let s = n.to_string();
        let mut result = String::with_capacity(s.len() + s.len() / 3);
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                result.push(',');
            }
            result.push(c);
        }
        result
    }

    // --- スライドショー ---

    /// スライドショーを開始/停止する
    fn toggle_slideshow(&mut self) {
        if self.slideshow_active {
            self.stop_slideshow();
        } else {
            self.start_slideshow();
        }
    }

    /// スライドショー開始
    fn start_slideshow(&mut self) {
        if self.document.file_list().len() < 2 {
            return;
        }
        self.slideshow_active = true;
        unsafe {
            let _ = SetTimer(
                Some(self.hwnd),
                TIMER_ID_SLIDESHOW,
                self.slideshow_interval_ms,
                None,
            );
        }
    }

    /// スライドショー停止
    fn stop_slideshow(&mut self) {
        if !self.slideshow_active {
            return;
        }
        self.slideshow_active = false;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_ID_SLIDESHOW);
        }
    }

    /// スライドショーのタイマー発火時
    fn on_slideshow_timer(&mut self) {
        if !self.slideshow_active {
            return;
        }
        // 最後の画像に到達しているか確認
        let at_end = self
            .document
            .file_list()
            .current_index()
            .is_some_and(|idx| idx + 1 >= self.document.file_list().len());

        if at_end {
            if self.slideshow_repeat {
                self.document.navigate_first();
                self.process_document_events();
            } else {
                self.stop_slideshow();
            }
            return;
        }
        self.document.navigate_relative(1);
        self.process_document_events();
    }

    /// スライドショー間隔を変更 (最小500ms、最大30000ms)
    fn adjust_slideshow_interval(&mut self, delta_ms: i32) {
        let new_val =
            (i64::from(self.slideshow_interval_ms) + i64::from(delta_ms)).clamp(500, 30_000) as u32;
        self.slideshow_interval_ms = new_val;
        // 実行中ならタイマーを新しい間隔で再設定 (同一IDは上書き)
        if self.slideshow_active {
            unsafe {
                let _ = SetTimer(
                    Some(self.hwnd),
                    TIMER_ID_SLIDESHOW,
                    self.slideshow_interval_ms,
                    None,
                );
            }
        }
    }

    /// 画像情報を表示する
    fn show_image_info(&self) {
        let Some(source) = self.document.current_source() else {
            return;
        };
        let Some(file_info) = self.document.file_list().current() else {
            return;
        };

        let mut info_lines = Vec::new();
        info_lines.push(format!("パス: {}", source.display_path()));
        info_lines.push(format!(
            "ファイルサイズ: {} KiB",
            Self::format_with_commas(file_info.file_size / 1024)
        ));

        if let Some(img) = self.document.current_image() {
            info_lines.push(format!("画像サイズ: {} x {}", img.width, img.height));
        }

        // メタデータ取得 (デコーダ経由)
        if let Ok(metadata) = self.document.current_metadata() {
            info_lines.push(format!("フォーマット: {}", metadata.format));
            for comment in &metadata.comments {
                info_lines.push(comment.clone());
            }
            // EXIF情報
            if !metadata.exif.is_empty() {
                info_lines.push(String::new());
                info_lines.push("--- EXIF ---".to_string());
                for (key, value) in &metadata.exif {
                    info_lines.push(format!("{key}: {value}"));
                }
            }
        }

        let text = info_lines.join("\n\n");
        info_dialog::show_info_dialog(self.hwnd, "画像情報", &text, self.monospace_font.hfont());
    }

    /// ヘルプを表示する
    fn show_help(&self) {
        let text = "\
ぐらびゅ - Windows用画像ビューアー

【主要キーバインド】
← / →              前後の画像に移動
ホイール上/下       前後の画像に移動
PageUp / PageDown   5ページ移動
Ctrl+PageUp/Down    50ページ移動
Ctrl+Home / End     最初 / 最後へ
Ctrl+ホイール       拡大 / 縮小
Num /               自動縮小表示
Num *               自動縮小・拡大表示
A                   α背景切替
Alt+Enter           全画面表示
Esc                 メニューバー表示/非表示
F4                  ファイルリスト表示/非表示
Tab / Shift+Tab     ソート順で前後移動
Delete              マーク設定
F1                  このヘルプ

【対応フォーマット】
画像: JPEG, PNG, GIF, BMP, WebP
ドキュメント: PDF
アーカイブ: ZIP/cbz, RAR/cbr, 7z
Susieプラグイン (.sph/.spi) で拡張可能";

        info_dialog::show_info_dialog(
            self.hwnd,
            "ぐらびゅ ヘルプ",
            text,
            self.monospace_font.hfont(),
        );
    }

    /// アップデート確認・実行
    fn check_for_update(&mut self) {
        // WaitCursor表示
        let prev_cursor = unsafe { SetCursor(LoadCursorW(None, IDC_WAIT).ok()) };

        let result = crate::updater::check_for_update();

        // カーソル復元
        unsafe {
            let _ = SetCursor(Some(prev_cursor));
        }

        match result {
            Err(e) => {
                let msg = format!("更新の確認に失敗しました:\n{e}\0");
                let title = "アップデート確認\0";
                let wide_msg: Vec<u16> = msg.encode_utf16().collect();
                let wide_title: Vec<u16> = title.encode_utf16().collect();
                unsafe {
                    MessageBoxW(
                        Some(self.hwnd),
                        windows::core::PCWSTR(wide_msg.as_ptr()),
                        windows::core::PCWSTR(wide_title.as_ptr()),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
            Ok(info) if !info.is_newer => {
                let msg = format!("最新バージョンです (v{})\0", info.current_version);
                let title = "アップデート確認\0";
                let wide_msg: Vec<u16> = msg.encode_utf16().collect();
                let wide_title: Vec<u16> = title.encode_utf16().collect();
                unsafe {
                    MessageBoxW(
                        Some(self.hwnd),
                        windows::core::PCWSTR(wide_msg.as_ptr()),
                        windows::core::PCWSTR(wide_title.as_ptr()),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
            }
            Ok(info) => {
                let msg = format!(
                    "v{} が利用可能です (現在: v{})。\n更新しますか？\0",
                    info.latest_version, info.current_version
                );
                let title = "アップデート確認\0";
                let wide_msg: Vec<u16> = msg.encode_utf16().collect();
                let wide_title: Vec<u16> = title.encode_utf16().collect();
                let answer = unsafe {
                    MessageBoxW(
                        Some(self.hwnd),
                        windows::core::PCWSTR(wide_msg.as_ptr()),
                        windows::core::PCWSTR(wide_title.as_ptr()),
                        MB_YESNO | MB_ICONQUESTION,
                    )
                };

                if answer == IDYES {
                    // WaitCursor表示
                    let prev = unsafe { SetCursor(LoadCursorW(None, IDC_WAIT).ok()) };

                    match crate::updater::perform_update(&info) {
                        Ok(true) => {
                            // バッチスクリプト起動成功 → アプリ終了
                            unsafe {
                                let _ = SetCursor(Some(prev));
                                let _ = DestroyWindow(self.hwnd);
                            }
                        }
                        Ok(false) => unsafe {
                            let _ = SetCursor(Some(prev));
                        },
                        Err(e) => {
                            unsafe {
                                let _ = SetCursor(Some(prev));
                            }
                            let msg = format!("更新に失敗しました:\n{e:?}\0");
                            let title = "アップデート\0";
                            let wide_msg: Vec<u16> = msg.encode_utf16().collect();
                            let wide_title: Vec<u16> = title.encode_utf16().collect();
                            unsafe {
                                MessageBoxW(
                                    Some(self.hwnd),
                                    windows::core::PCWSTR(wide_msg.as_ptr()),
                                    windows::core::PCWSTR(wide_title.as_ptr()),
                                    MB_OK | MB_ICONERROR,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// シェル統合 (ファイル関連付け・コンテキストメニュー・「送る」) を登録
    fn action_register_shell(&self) {
        let msg = "ファイル関連付け・コンテキストメニュー・「送る」を登録しますか？\0";
        let title = "シェル統合\0";
        let wide_msg: Vec<u16> = msg.encode_utf16().collect();
        let wide_title: Vec<u16> = title.encode_utf16().collect();
        let answer = unsafe {
            MessageBoxW(
                Some(self.hwnd),
                windows::core::PCWSTR(wide_msg.as_ptr()),
                windows::core::PCWSTR(wide_title.as_ptr()),
                MB_YESNO | MB_ICONQUESTION,
            )
        };
        if answer != IDYES {
            return;
        }

        match crate::shell::register_all() {
            Ok(()) => {
                let msg = "シェル統合を登録しました。\0";
                let wide_msg: Vec<u16> = msg.encode_utf16().collect();
                unsafe {
                    MessageBoxW(
                        Some(self.hwnd),
                        windows::core::PCWSTR(wide_msg.as_ptr()),
                        windows::core::PCWSTR(wide_title.as_ptr()),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
            }
            Err(e) => {
                let msg = format!("シェル統合の登録に失敗しました:\n{e}\0");
                let wide_msg: Vec<u16> = msg.encode_utf16().collect();
                unsafe {
                    MessageBoxW(
                        Some(self.hwnd),
                        windows::core::PCWSTR(wide_msg.as_ptr()),
                        windows::core::PCWSTR(wide_title.as_ptr()),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
        }
    }

    /// シェル統合 (ファイル関連付け・コンテキストメニュー・「送る」) を解除
    fn action_unregister_shell(&self) {
        let msg = "ファイル関連付け・コンテキストメニュー・「送る」を解除しますか？\0";
        let title = "シェル統合\0";
        let wide_msg: Vec<u16> = msg.encode_utf16().collect();
        let wide_title: Vec<u16> = title.encode_utf16().collect();
        let answer = unsafe {
            MessageBoxW(
                Some(self.hwnd),
                windows::core::PCWSTR(wide_msg.as_ptr()),
                windows::core::PCWSTR(wide_title.as_ptr()),
                MB_YESNO | MB_ICONQUESTION,
            )
        };
        if answer != IDYES {
            return;
        }

        match crate::shell::unregister_all() {
            Ok(()) => {
                let msg = "シェル統合を解除しました。\0";
                let wide_msg: Vec<u16> = msg.encode_utf16().collect();
                unsafe {
                    MessageBoxW(
                        Some(self.hwnd),
                        windows::core::PCWSTR(wide_msg.as_ptr()),
                        windows::core::PCWSTR(wide_title.as_ptr()),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
            }
            Err(e) => {
                let msg = format!("シェル統合の解除に失敗しました:\n{e}\0");
                let wide_msg: Vec<u16> = msg.encode_utf16().collect();
                unsafe {
                    MessageBoxW(
                        Some(self.hwnd),
                        windows::core::PCWSTR(wide_msg.as_ptr()),
                        windows::core::PCWSTR(wide_title.as_ptr()),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
        }
    }

    /// マウス左ボタン押下: 選択ドラッグ開始
    fn on_lbutton_down(&mut self, lparam: LPARAM) {
        let Some(draw_rect) = self.renderer.last_draw_rect().copied() else {
            return;
        };
        let Some(img) = self.document.current_image() else {
            return;
        };

        let sx = (lparam.0 & 0xFFFF) as i16 as f32;
        let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;

        self.selection
            .on_mouse_down(sx, sy, &draw_rect, img.width, img.height);

        if self.selection.is_dragging() {
            // マウスキャプチャ (ウィンドウ外でもドラッグイベントを受け取る)
            unsafe {
                windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(self.hwnd);
            }
            self.invalidate();
            self.update_title();
        }
    }

    /// マウス移動: ドラッグ中の矩形更新
    fn on_mouse_move(&mut self, lparam: LPARAM) {
        if !self.selection.is_dragging() {
            return;
        }
        let Some(draw_rect) = self.renderer.last_draw_rect().copied() else {
            return;
        };
        let Some(img) = self.document.current_image() else {
            return;
        };

        let sx = (lparam.0 & 0xFFFF) as i16 as f32;
        let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;

        self.selection
            .on_mouse_move(sx, sy, &draw_rect, img.width, img.height);
        self.invalidate();
        self.update_title();
    }

    /// マウス左ボタンリリース: ドラッグ終了
    fn on_lbutton_up(&mut self) {
        if !self.selection.is_dragging() {
            return;
        }
        let Some(img) = self.document.current_image() else {
            return;
        };

        unsafe {
            windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture().unwrap_or_default();
        }
        self.selection.on_mouse_up(img.width, img.height);
        self.invalidate();
        self.update_title();
    }

    /// WM_SETCURSOR: 選択ハンドル上でカーソルを変更
    /// trueを返した場合はDefWindowProcを呼ばない
    fn on_set_cursor(&self) -> bool {
        if !self.selection.is_selected() {
            return false;
        }
        let Some(draw_rect) = self.renderer.last_draw_rect() else {
            return false;
        };
        let Some(img) = self.document.current_image() else {
            return false;
        };

        // 現在のマウス位置を取得
        let mut pt = windows::Win32::Foundation::POINT::default();
        unsafe {
            let _ =
                windows::Win32::UI::WindowsAndMessaging::GetCursorPos(std::ptr::from_mut(&mut pt));
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(
                self.hwnd,
                std::ptr::from_mut(&mut pt),
            );
        }
        let sx = pt.x as f32;
        let sy = pt.y as f32;

        let hit = self
            .selection
            .hit_test_at(sx, sy, draw_rect, img.width, img.height);
        let cursor_id = match hit {
            HitTestResult::Handle(HandleKind::TopLeft | HandleKind::BottomRight) => {
                Some(IDC_SIZENWSE)
            }
            HitTestResult::Handle(HandleKind::TopRight | HandleKind::BottomLeft) => {
                Some(IDC_SIZENESW)
            }
            HitTestResult::Handle(HandleKind::Top | HandleKind::Bottom) => Some(IDC_SIZENS),
            HitTestResult::Handle(HandleKind::Left | HandleKind::Right) => Some(IDC_SIZEWE),
            HitTestResult::Inside => Some(IDC_SIZEALL),
            _ => None,
        };

        if let Some(id) = cursor_id {
            unsafe {
                let _ = SetCursor(LoadCursorW(None, id).ok());
            }
            return true;
        }

        false
    }

    /// 未保存の編集がある場合は破棄して続行する
    fn guard_unsaved_edit(&mut self) -> bool {
        if self.document.has_unsaved_edit() {
            self.document.discard_editing_session();
            self.selection.deselect();
        }
        true
    }

    fn on_drop_files(&mut self, hdrop: HDROP) {
        if !self.guard_unsaved_edit() {
            unsafe { DragFinish(hdrop) };
            return;
        }
        self.selection.deselect();

        // ドロップされた全ファイルを収集
        let file_count = unsafe { DragQueryFileW(hdrop, 0xFFFFFFFF, None) } as usize;
        let mut paths = Vec::new();
        let mut buf = [0u16; 1024];
        for i in 0..file_count {
            let len = unsafe { DragQueryFileW(hdrop, i as u32, Some(&mut buf)) } as usize;
            if len > 0 {
                let path_str = String::from_utf16_lossy(&buf[..len]);
                paths.push(std::path::PathBuf::from(path_str));
            }
        }
        unsafe { DragFinish(hdrop) };

        if paths.is_empty() {
            return;
        }

        let result = if paths.len() > 1 {
            // 複数パス: フォルダ・コンテナ・画像の混在をすべてフラットに展開
            self.document.open_multiple(&paths)
        } else if paths[0].is_dir() {
            self.document.open_folder(&paths[0])
        } else {
            self.document.open(&paths[0])
        };

        if let Err(e) = result {
            self.show_error_title(&format!("ドロップされたファイルを開けませんでした: {e}"));
        }

        self.process_document_events();
    }
}

/// 画像書き出しフォーマット。各バリアントが拡張子・フィルタ表示・`image::ImageFormat`
/// を一元管理する。`Action::Export*` から `export_image` に渡される。
#[derive(Copy, Clone)]
enum ExportFormat {
    Png,
    Jpg,
    Bmp,
}

impl ExportFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpg => "jpg",
            Self::Bmp => "bmp",
        }
    }

    fn filter_name(self) -> &'static str {
        match self {
            Self::Png => "PNG画像",
            Self::Jpg => "JPEG画像",
            Self::Bmp => "BMP画像",
        }
    }

    fn filter_spec(self) -> &'static str {
        match self {
            Self::Png => "*.png",
            Self::Jpg => "*.jpg",
            Self::Bmp => "*.bmp",
        }
    }

    fn image_format(self) -> image::ImageFormat {
        match self {
            Self::Png => image::ImageFormat::Png,
            Self::Jpg => image::ImageFormat::Jpeg,
            Self::Bmp => image::ImageFormat::Bmp,
        }
    }
}

/// RGBA バッファを指定パスへ指定フォーマットで書き出す。
///
/// `image::ImageBuffer<Rgba<u8>, _>` を直接 `save_with_format` に渡すと、JPEG エンコーダ
/// が RGBA を受け付けず色型エラーで失敗する。`DynamicImage` を経由することで `image`
/// crate 側が必要な色変換 (RGBA→RGB 等) を自動で行う。フォーマットを引数で明示する
/// ため、保存先パスの拡張子有無に依存しない。
fn write_image_to_path(
    width: u32,
    height: u32,
    rgba: &[u8],
    format: ExportFormat,
    path: &Path,
) -> Result<()> {
    let img_buf = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| anyhow::anyhow!("画像バッファの作成に失敗しました"))?;
    let dynamic = image::DynamicImage::ImageRgba8(img_buf);
    dynamic
        .save_with_format(path, format.image_format())
        .map_err(|e| anyhow::anyhow!("画像の書き出しに失敗しました: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// テスト間で衝突しない一時パスを生成する。
    /// プロセス ID とナノ秒で並列実行に対する競合を避ける。
    fn unique_temp_path(stem: &str) -> PathBuf {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("gv_test_{stem}_{pid}_{nanos}"))
    }

    fn white_pixel() -> (u32, u32, Vec<u8>) {
        (1, 1, vec![255, 255, 255, 255])
    }

    /// 拡張子なしパスでも PNG として書き出せる (本バグ修正の回帰テスト)。
    /// 修正前は `image::RgbaImage::save()` がパスから形式を推定できず
    /// "The image format could not be determined" で失敗していた。
    #[test]
    fn write_png_with_extensionless_path_succeeds() {
        let (w, h, rgba) = white_pixel();
        let path = unique_temp_path("png_no_ext");
        let result = write_image_to_path(w, h, &rgba, ExportFormat::Png, &path);
        assert!(
            result.is_ok(),
            "extensionless path should succeed: {result:?}"
        );
        let bytes = fs::read(&path).unwrap();
        assert_eq!(
            &bytes[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
        );
        let _ = fs::remove_file(&path);
    }

    /// `.txt` のような不一致拡張子でも、指定したフォーマットでバイト列が書かれる
    /// (`save_with_format` による形式強制の挙動保証)。同時に `DynamicImage` 経由
    /// による RGBA→RGB 自動変換が JPEG エンコーダで動くことを検証する。
    #[test]
    fn write_jpg_with_txt_extension_writes_jpeg_bytes() {
        let (w, h, rgba) = white_pixel();
        let path = unique_temp_path("mismatch.txt");
        let result = write_image_to_path(w, h, &rgba, ExportFormat::Jpg, &path);
        assert!(
            result.is_ok(),
            "txt extension with Jpg format should succeed: {result:?}"
        );
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]); // JPEG SOI マーカー
        let _ = fs::remove_file(&path);
    }

    /// BMP も拡張子なしパスで成功すること。
    #[test]
    fn write_bmp_with_extensionless_path_succeeds() {
        let (w, h, rgba) = white_pixel();
        let path = unique_temp_path("bmp_no_ext");
        let result = write_image_to_path(w, h, &rgba, ExportFormat::Bmp, &path);
        assert!(
            result.is_ok(),
            "bmp extensionless should succeed: {result:?}"
        );
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], b"BM");
        let _ = fs::remove_file(&path);
    }

    /// `ExportFormat` の各バリアントが期待どおりの拡張子を返すこと。
    #[test]
    fn export_format_returns_expected_extensions() {
        assert_eq!(ExportFormat::Png.extension(), "png");
        assert_eq!(ExportFormat::Jpg.extension(), "jpg");
        assert_eq!(ExportFormat::Bmp.extension(), "bmp");
    }
}
