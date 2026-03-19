use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use crossbeam_channel::Receiver;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{InvalidateRect, UpdateWindow, ValidateRect};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
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

/// 修飾キーVKコード
const VK_CONTROL: i32 = 0x11;
const VK_SHIFT: i32 = 0x10;
const VK_MENU: i32 = 0x12; // Alt

/// メインウィンドウ
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
    // パネル表示中のキャッシュ状態追跡（差分更新用）
    cached_indices: HashSet<usize>,
    // 等幅フォント（ダイアログ・ファイルリスト用）
    monospace_font: MonospaceFont,
    // 矩形選択
    selection: Selection,
}

impl AppWindow {
    /// AppWindowを作成しウィンドウを表示する
    pub fn create(config: Config, initial_files: &[PathBuf]) -> Result<Box<Self>> {
        let class_name = windows::core::w!("gv3_main");

        // アイコンをリソースからロード（リソースID 1）
        let icon = unsafe {
            let hmodule = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok();
            let hinstance = hmodule.map(|m| windows::Win32::Foundation::HINSTANCE(m.0));
            // MAKEINTRESOURCE(1) — リソースID 1 をポインタとして渡す
            #[allow(clippy::manual_dangling_ptr)]
            LoadIconW(hinstance, windows::core::PCWSTR(1 as *const u16)).ok()
        };
        window::register_window_class_with_icon(class_name, Some(Self::wnd_proc), icon)?;

        let hwnd = window::create_window(class_name, windows::core::w!("ぐらびゅ3"), 1024, 768)?;

        // ウィンドウにアイコンを設定（タスクバー表示用）
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

        // デコーダチェーン: Standard → Susie画像プラグイン（フォールバック順）
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
            .and_then(|p| p.parent().map(|d| d.join("gv3.keys.toml")));
        let key_config = KeyConfig::load(key_config_path.as_deref());

        // メニューバー構築（初期状態は非表示）
        let menu_handle = menu::build_menu_bar();

        // ファイルリストパネル作成（初期状態は非表示）
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
        });

        // GWLP_USERDATAにポインタを格納（WndProcからアクセスするため）
        window::set_window_data(hwnd, &mut *app as *mut Self);

        // 先読みエンジン起動
        // 通知コールバック: ワーカースレッドからPostMessageWでUIスレッドを起こす
        let hwnd_raw = hwnd.0 as isize;
        let notify = Box::new(move || unsafe {
            let _ = PostMessageW(
                Some(HWND(hwnd_raw as *mut _)),
                WM_DOCUMENT_EVENT,
                WPARAM(0),
                LPARAM(0),
            );
        });
        let cache_budget = Self::get_cache_budget();
        app.document
            .start_prefetch(notify, cache_budget, base_image_size);

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
        if !initial_files.is_empty() {
            let result = if initial_files.len() > 1
                && initial_files.iter().all(|p| app.document.is_container(p))
            {
                // 全てコンテナなら複数コンテナをまとめて開く
                app.document.open_containers(initial_files)
            } else if initial_files[0].is_dir() {
                app.document.open_folder(&initial_files[0])
            } else {
                app.document.open(&initial_files[0])
            };
            if let Err(e) = result {
                app.show_error_title(&format!("画像を開けませんでした: {e}"));
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
            if GlobalMemoryStatusEx(&mut mem_info).is_ok() {
                mem_info.ullAvailPhys as usize
            } else {
                512 * 1024 * 1024 // フォールバック: 512MB
            }
        };
        available / 2
    }

    /// DocumentEventを処理する
    fn process_document_events(&mut self) {
        // 先読みレスポンスを処理（キャッシュ格納 + current_image更新）
        self.document.process_prefetch_responses();

        while let Ok(event) = self.event_receiver.try_recv() {
            match event {
                DocumentEvent::ImageReady => unsafe {
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                },
                DocumentEvent::NavigationChanged { index, .. } => {
                    self.update_title();
                    self.file_list_panel.set_selection(index);
                }
                DocumentEvent::FileListChanged => {
                    let doc = &self.document;
                    self.file_list_panel
                        .update(doc.file_list(), |i| doc.is_cached(i));
                    self.update_title(); // リスト変更（削除等）でタイトルを同期
                }
                DocumentEvent::Error(msg) => {
                    self.show_error_title(&msg);
                }
            }
        }

        // パネル表示中ならキャッシュ状態の差分のみ更新（全件再構築を避ける）
        if self.file_list_panel.is_visible() {
            let doc = &self.document;
            let files = doc.file_list().files();
            // 現在のキャッシュ状態をスナップショット（上限6件程度なので軽量）
            let mut new_cached = HashSet::new();
            for i in 0..files.len() {
                if doc.is_cached(i) {
                    new_cached.insert(i);
                }
            }
            // 前回との差分だけ update_item() で更新
            for &i in self.cached_indices.symmetric_difference(&new_cached) {
                if let Some(info) = files.get(i) {
                    self.file_list_panel
                        .update_item(i, info, new_cached.contains(&i));
                }
            }
            // 差分があれば選択位置を復元（LB_DELETESTRING+LB_INSERTSTRINGで崩れるため）
            if self.cached_indices != new_cached
                && let Some(idx) = doc.file_list().current_index()
            {
                self.file_list_panel.set_selection(idx);
            }
            self.cached_indices = new_cached;
        }
    }

    /// タイトルバーを更新
    fn update_title(&self) {
        let title = if let Some(source) = self.document.current_source() {
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
            format!("{display}{page_info}{sel_info} - ぐらびゅ3\0")
        } else {
            "ぐらびゅ3\0".to_string()
        };

        let wide: Vec<u16> = title.encode_utf16().collect();
        unsafe {
            let _ = SetWindowTextW(self.hwnd, windows::core::PCWSTR(wide.as_ptr()));
        }
    }

    /// タイトルバーにエラーメッセージを表示する
    fn show_error_title(&self, msg: &str) {
        let title = format!("ぐらびゅ3 - エラー: {msg}\0");
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

    /// 現在の画像サイズを返す（zoom操作用）
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
            // フルスクリーン開始: メニュー・パネルを非表示（フラグは保持）
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

    /// 最大化トグル（左ダブルクリック）
    fn toggle_maximize(&self) {
        unsafe {
            let mut placement = WINDOWPLACEMENT {
                length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            let _ = GetWindowPlacement(self.hwnd, &mut placement);
            if placement.showCmd == SW_MAXIMIZE.0 as u32 {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            } else {
                let _ = ShowWindow(self.hwnd, SW_MAXIMIZE);
            }
        }
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
                    // パネルのtoggleからSendMessageW(WM_SIZE, 0, 0)で呼ばれる場合
                    if width == 0 && height == 0 {
                        let (w, h) = window::get_client_size(hwnd);
                        width = w;
                        height = h;
                    }
                    app.on_size(width, height);
                    return LRESULT(0);
                }
                WM_KEYDOWN | WM_SYSKEYDOWN => {
                    // Escキー: ドラッグ操作中は選択をキャンセル（key_configより優先）
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
                    if let Some(action) = app.key_config.lookup(&chord) {
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
                    if let Some(action) = app.key_config.lookup(&chord) {
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
                    if let Some(action) = app.key_config.lookup(&chord) {
                        app.execute_action(action);
                    }
                    return LRESULT(0);
                }
                WM_MBUTTONUP => {
                    let chord = InputChord::Mouse {
                        button: MouseButton::MiddleClick,
                    };
                    if let Some(action) = app.key_config.lookup(&chord) {
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
                }
                WM_COMMAND => {
                    let notify_code = ((wparam.0 as u32) >> 16) & 0xFFFF;
                    let control_id = (wparam.0 as u32) & 0xFFFF;
                    let control_hwnd = HWND(lparam.0 as *mut _);

                    // ListBox の LBN_SELCHANGE
                    if notify_code == 1 // LBN_SELCHANGE
                        && control_hwnd == app.file_list_panel.listbox_hwnd()
                    {
                        let sel = unsafe {
                            SendMessageW(
                                app.file_list_panel.listbox_hwnd(),
                                LB_GETCURSEL,
                                None,
                                None,
                            )
                        };
                        if sel.0 >= 0 {
                            if !app.guard_unsaved_edit() {
                                // キャンセル時は選択位置を復元
                                if let Some(idx) = app.document.file_list().current_index() {
                                    app.file_list_panel.set_selection(idx);
                                }
                                return LRESULT(0);
                            }
                            app.selection.deselect();
                            app.document.navigate_to(sel.0 as usize);
                            app.process_document_events();
                        }
                        return LRESULT(0);
                    }

                    // メニュー項目（notify_code == 0 かつコントロールなし）
                    if notify_code == 0 && control_hwnd.0.is_null() {
                        if let Some(action) = menu::menu_id_to_action(control_id as u16) {
                            app.execute_action(action);
                        }
                        return LRESULT(0);
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

            // 描画オフセットを設定（パネル幅分だけ右にずらす）
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

    /// パラメータなしフィルタの共通パターン（選択範囲対応）
    fn apply_simple_filter(&mut self, f: fn(&DecodedImage, Option<&PixelRect>) -> DecodedImage) {
        if let Some(img) = self.document.current_image() {
            let sel = self.selection.current_rect();
            let result = f(img, sel.as_ref());
            self.document.apply_edit(result);
            self.process_document_events();
        }
    }

    /// 画像全体に適用する変形操作の共通パターン（選択解除付き）
    fn apply_transform(&mut self, f: fn(&DecodedImage) -> DecodedImage) {
        if let Some(img) = self.document.current_image() {
            let result = f(img);
            self.selection.deselect();
            self.document.apply_edit(result);
            self.process_document_events();
        }
    }

    /// パラメータなし永続フィルタの共通パターン
    fn add_persistent_filter(&mut self, op: crate::persistent_filter::FilterOperation) {
        self.document.persistent_filter_mut().add_operation(op);
        self.document.on_persistent_filter_changed();
        self.process_document_events();
    }

    /// アクションを実行する
    fn execute_action(&mut self, action: Action) {
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
            Action::NavigateFirst => self.navigate_with_guard(|d| d.navigate_first()),
            Action::NavigateLast => self.navigate_with_guard(|d| d.navigate_last()),

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
            Action::MarkSet => {
                // mark_current()は内部でnavigate_relative(1)するのでマーク元indexを先に取得
                let mark_idx = self.document.file_list().current_index();
                self.document.mark_current();
                self.process_document_events();
                if self.file_list_panel.is_visible()
                    && let Some(idx) = mark_idx
                    && let Some(info) = self.document.file_list().files().get(idx)
                {
                    let is_cached = self.document.is_cached(idx);
                    self.file_list_panel.update_item(idx, info, is_cached);
                    // update_itemでselectionが崩れるので復元
                    if let Some(cur) = self.document.file_list().current_index() {
                        self.file_list_panel.set_selection(cur);
                    }
                }
            }
            Action::MarkUnset => {
                self.document.unmark_current();
                if self.file_list_panel.is_visible()
                    && let Some(idx) = self.document.file_list().current_index()
                {
                    let info = &self.document.file_list().files()[idx];
                    let is_cached = self.document.is_cached(idx);
                    self.file_list_panel.update_item(idx, info, is_cached);
                    self.file_list_panel.set_selection(idx);
                }
            }
            Action::MarkInvertAll => {
                self.document.invert_all_marks();
                if self.file_list_panel.is_visible() {
                    let doc = &self.document;
                    self.file_list_panel
                        .update(doc.file_list(), |i| doc.is_cached(i));
                    if let Some(idx) = doc.file_list().current_index() {
                        self.file_list_panel.set_selection(idx);
                    }
                }
            }
            Action::MarkInvertToHere => {
                self.document.invert_marks_to_here();
                if self.file_list_panel.is_visible() {
                    let doc = &self.document;
                    self.file_list_panel
                        .update(doc.file_list(), |i| doc.is_cached(i));
                    if let Some(idx) = doc.file_list().current_index() {
                        self.file_list_panel.set_selection(idx);
                    }
                }
            }
            Action::NavigatePrevMark => self.navigate_with_guard(|d| d.navigate_prev_mark()),
            Action::NavigateNextMark => self.navigate_with_guard(|d| d.navigate_next_mark()),
            Action::RemoveFromList => {
                self.document.remove_current_from_list();
                self.process_document_events();
            }
            Action::MarkedRemoveFromList => {
                self.document.remove_marked_from_list();
                self.process_document_events();
            }

            // --- フォルダナビゲーション ---
            Action::NavigatePrevFolder => self.navigate_with_guard(|d| d.navigate_prev_folder()),
            Action::NavigateNextFolder => self.navigate_with_guard(|d| d.navigate_next_folder()),

            // --- ファイル操作 ---
            Action::OpenFile => {
                if !self.guard_unsaved_edit() {
                    return;
                }
                self.selection.deselect();
                let initial_dir = self
                    .document
                    .current_source()
                    .and_then(|s| s.parent_dir())
                    .map(|p| p.to_path_buf());
                if let Ok(Some(path)) =
                    crate::file_ops::open_file_dialog(self.hwnd, initial_dir.as_deref())
                {
                    if let Err(e) = self.document.open(&path) {
                        self.show_error_title(&format!("ファイルを開けませんでした: {e}"));
                    }
                    self.process_document_events();
                }
            }
            Action::OpenFolder => {
                if !self.guard_unsaved_edit() {
                    return;
                }
                self.selection.deselect();
                let initial_dir = self
                    .document
                    .current_source()
                    .and_then(|s| s.parent_dir())
                    .map(|p| p.to_path_buf());
                if let Ok(Some(path)) =
                    crate::file_ops::open_folder_dialog(self.hwnd, initial_dir.as_deref())
                {
                    if let Err(e) = self.document.open_folder(&path) {
                        self.show_error_title(&format!("フォルダを開けませんでした: {e}"));
                    }
                    self.process_document_events();
                }
            }
            Action::DeleteFile => {
                // コンテナ内（アーカイブ/PDF）のファイル削除は無効
                if let Some(source) = self.document.current_source()
                    && source.is_contained()
                {
                    return;
                }
                if let Some(path) = self.document.current_path().map(|p| p.to_path_buf())
                    && let Ok(true) = crate::file_ops::delete_to_recycle_bin(self.hwnd, &[&path])
                {
                    self.document.remove_current_from_list();
                    self.process_document_events();
                }
            }
            Action::MoveFile => {
                let Some(current) = self.document.file_list().current() else {
                    return;
                };
                let source = current.source.clone();
                let path = current.path.clone();

                // PDFページは移動不可（ページ単位の書き出しにはデコード済み画像の再エンコードが必要）
                if matches!(source, crate::file_info::FileSource::PdfPage { .. }) {
                    return;
                }

                let initial_dir = source.parent_dir().map(|p| p.to_path_buf());
                let default_name = source.default_save_name();

                // ファイルソースに応じてダイアログのラベルを分岐
                let (dialog_title, dialog_button) = match &source {
                    crate::file_info::FileSource::File(_) => ("ファイルを移動", "移動"),
                    _ => ("ファイルを書き出す", "書き出す"),
                };

                if let Ok(Some(dest)) = crate::file_ops::save_file_dialog(
                    self.hwnd,
                    &default_name,
                    "すべてのファイル",
                    "*.*",
                    initial_dir.as_deref(),
                    Some(dialog_title),
                    Some(dialog_button),
                ) {
                    match &source {
                        crate::file_info::FileSource::File(_) => {
                            // 通常ファイル: SHFileOperationWでUndo対応の移動
                            match crate::file_ops::move_single_file(self.hwnd, &path, &dest) {
                                Ok(true) => {
                                    // 移動先パスでリスト内エントリを更新（異フォルダでも追跡）
                                    if let Err(e) = self.document.rename_current_in_list(&dest) {
                                        self.show_error_title(&format!("リスト更新失敗: {e}"));
                                    }
                                    self.process_document_events();
                                }
                                Ok(false) => {} // ユーザーキャンセル
                                Err(e) => {
                                    self.show_error_title(&format!("ファイル移動失敗: {e}"));
                                }
                            }
                        }
                        crate::file_info::FileSource::ArchiveEntry { on_demand, .. } => {
                            // アーカイブエントリ: 書き出し（リスト除去なし）
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
                                self.show_error_title(&format!("ファイル書き出し失敗: {e}"));
                            }
                        }
                        crate::file_info::FileSource::PdfPage { .. } => {
                            unreachable!(); // 上でガード済み
                        }
                    }
                }
            }
            Action::CopyFile => {
                if let Some(current) = self.document.file_list().current() {
                    let default_name = current.source.default_save_name();
                    let initial_dir = current.source.parent_dir().map(|p| p.to_path_buf());
                    if let Ok(Some(dest)) = crate::file_ops::save_file_dialog(
                        self.hwnd,
                        &default_name,
                        "すべてのファイル",
                        "*.*",
                        initial_dir.as_deref(),
                        Some("ファイルを複製"),
                        Some("複製"),
                    ) {
                        let result = if matches!(
                            current.source,
                            crate::file_info::FileSource::ArchiveEntry {
                                on_demand: true,
                                ..
                            }
                        ) {
                            // オンデマンド: アーカイブから読み出して書き出し
                            self.document.read_file_data_current().and_then(|data| {
                                std::fs::write(&dest, &data).map_err(anyhow::Error::from)
                            })
                        } else {
                            // 通常ファイル/temp展開済み/PDF: 既存のfs::copy
                            std::fs::copy(&current.path, &dest)
                                .map(|_| ())
                                .map_err(anyhow::Error::from)
                        };
                        if let Err(e) = result {
                            self.show_error_title(&format!("ファイルコピー失敗: {e}"));
                        }
                    }
                }
            }
            Action::MarkedDelete => {
                // コンテナ内（アーカイブ/PDF）は無効
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
                let path_refs: Vec<&Path> = paths.iter().map(|p| p.as_path()).collect();
                if let Ok(true) = crate::file_ops::delete_to_recycle_bin(self.hwnd, &path_refs) {
                    self.document.remove_marked_from_list();
                    self.process_document_events();
                }
            }
            Action::MarkedMove => {
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
                // マーク済み最初のファイルの親ディレクトリを初期ディレクトリとして使用
                let initial_dir = self.document.file_list().files()[marked[0]]
                    .source
                    .parent_dir()
                    .map(|p| p.to_path_buf());
                if let Ok(Some(dest)) = crate::file_ops::select_folder_dialog(
                    self.hwnd,
                    "移動先フォルダ",
                    initial_dir.as_deref(),
                ) {
                    let path_refs: Vec<&Path> = paths.iter().map(|p| p.as_path()).collect();
                    if let Ok(true) = crate::file_ops::move_files(self.hwnd, &path_refs, &dest) {
                        // パス更新失敗時は従来通りリストから削除（フォールバック）
                        if let Err(e) = self.document.update_marked_paths(&dest) {
                            eprintln!("パス更新失敗、リストから削除: {e}");
                            self.document.remove_marked_from_list();
                        }
                        self.process_document_events();
                    }
                }
            }
            Action::MarkedCopy => {
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
                    .map(|p| p.to_path_buf());
                if let Ok(Some(dest)) = crate::file_ops::select_folder_dialog(
                    self.hwnd,
                    "コピー先フォルダ",
                    initial_dir.as_deref(),
                ) {
                    let path_refs: Vec<&Path> = paths.iter().map(|p| p.as_path()).collect();
                    if let Err(e) = crate::file_ops::copy_files(self.hwnd, &path_refs, &dest) {
                        self.show_error_title(&format!("ファイルコピー失敗: {e}"));
                    }
                }
            }
            Action::Reload => self.navigate_with_guard(|d| d.reload()),

            // --- クリップボード ---
            Action::CopyImage => {
                if let Some(image) = self.document.current_image()
                    && let Err(e) = crate::clipboard::copy_image_to_clipboard(self.hwnd, image)
                {
                    self.show_error_title(&format!("画像コピー失敗: {e}"));
                }
            }
            Action::CopyFileName => {
                if let Some(source) = self.document.current_source()
                    && let Err(e) =
                        crate::clipboard::copy_text_to_clipboard(self.hwnd, &source.display_path())
                {
                    self.show_error_title(&format!("ファイル名コピー失敗: {e}"));
                }
            }
            Action::MarkedCopyNames => {
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
                        self.show_error_title(&format!("マークファイル名コピー失敗: {e}"));
                    }
                }
            }
            Action::PasteImage => {
                if !self.guard_unsaved_edit() {
                    return;
                }
                self.selection.deselect();
                match crate::clipboard::paste_image_from_clipboard(self.hwnd) {
                    Ok(Some(image)) => {
                        // 一時ファイルに保存してから開く
                        let temp_path = std::env::temp_dir().join("gv3_clipboard.png");
                        if let Some(img_buf) = image::RgbaImage::from_raw(
                            image.width,
                            image.height,
                            image.data.clone(),
                        ) && img_buf.save(&temp_path).is_ok()
                        {
                            if let Err(e) = self.document.open_single(&temp_path) {
                                self.show_error_title(&format!("貼り付け失敗: {e}"));
                            }
                            self.process_document_events();
                        }
                    }
                    Ok(None) => {} // クリップボードに画像なし
                    Err(e) => self.show_error_title(&format!("貼り付け失敗: {e}")),
                }
            }

            // --- 画像書き出し ---
            Action::ExportJpg => {
                self.export_image("jpg", "JPEG画像", "*.jpg");
            }
            Action::ExportBmp => {
                self.export_image("bmp", "BMP画像", "*.bmp");
            }
            Action::ExportPng => {
                self.export_image("png", "PNG画像", "*.png");
            }

            // --- ユーティリティ ---
            Action::NewWindow => {
                // 現在のexeを新規プロセスで起動
                if let Ok(exe) = std::env::current_exe() {
                    let mut cmd = std::process::Command::new(&exe);
                    // コンテナ内の場合はコンテナパスを渡す
                    if let Some(source) = self.document.current_source() {
                        match source {
                            crate::file_info::FileSource::ArchiveEntry { archive, .. } => {
                                cmd.arg(archive);
                            }
                            crate::file_info::FileSource::PdfPage { pdf_path, .. } => {
                                cmd.arg(pdf_path);
                            }
                            crate::file_info::FileSource::File(path) => {
                                cmd.arg(path);
                            }
                        }
                    }
                    let _ = cmd.spawn();
                }
            }
            Action::CloseAll => {
                if !self.guard_unsaved_edit() {
                    return;
                }
                self.selection.deselect();
                self.document.close_all();
                self.process_document_events();
                self.update_title();
            }
            Action::OpenContainingFolder => {
                if let Some(source) = self.document.current_source() {
                    let target = match source {
                        crate::file_info::FileSource::ArchiveEntry { archive, .. } => {
                            archive.clone()
                        }
                        crate::file_info::FileSource::PdfPage { pdf_path, .. } => pdf_path.clone(),
                        crate::file_info::FileSource::File(path) => path.clone(),
                    };
                    // /select,path でエクスプローラを開く
                    let arg = format!("/select,{}", target.display());
                    let _ = std::process::Command::new("explorer.exe").arg(&arg).spawn();
                }
            }
            Action::OpenExeFolder => {
                if let Ok(exe) = std::env::current_exe()
                    && let Some(dir) = exe.parent()
                {
                    let _ = std::process::Command::new("explorer.exe").arg(dir).spawn();
                }
            }
            Action::OpenBookmarkFolder => {
                let dir = crate::bookmark::bookmark_dir();
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::process::Command::new("explorer.exe").arg(&dir).spawn();
            }
            Action::OpenSpiFolder => {
                if let Ok(exe) = std::env::current_exe()
                    && let Some(dir) = exe.parent()
                {
                    let spi_dir = dir.join("spi");
                    let _ = std::fs::create_dir_all(&spi_dir);
                    let _ = std::process::Command::new("explorer.exe")
                        .arg(&spi_dir)
                        .spawn();
                }
            }
            Action::OpenTempFolder => {
                let dir = std::env::temp_dir();
                let _ = std::process::Command::new("explorer.exe").arg(&dir).spawn();
            }
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
            Action::RotateArbitrary => {
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
            Action::Resize => {
                if let Some(img) = self.document.current_image()
                    && let Some((nw, nh)) = crate::ui::resize_dialog::show_resize_dialog(
                        self.hwnd, img.width, img.height,
                    )
                    && let Some(img) = self.document.current_image()
                {
                    let result = crate::filter::transform::resize(img, nw, nh);
                    self.selection.deselect();
                    self.document.apply_edit(result);
                    self.process_document_events();
                }
            }

            // --- フィルタ（パラメータあり） ---
            Action::Fill => {
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
            Action::Levels => {
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
                        let result =
                            crate::filter::brightness::levels(img, sel.as_ref(), low, high);
                        self.document.apply_edit(result);
                        self.process_document_events();
                    }
                }
            }
            Action::Gamma => {
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
            Action::BrightnessContrast => {
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
                    if let Some(vals) =
                        show_filter_dialog(self.hwnd, "明るさとコントラスト", &fields)
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
            Action::Mosaic => {
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
            Action::GaussianBlur => {
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
            Action::UnsharpMask => {
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

            // --- フィルタ（パラメータなし） ---
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
                self.add_persistent_filter(FilterOperation::FlipHorizontal);
            }
            Action::PFilterFlipV => {
                self.add_persistent_filter(FilterOperation::FlipVertical);
            }
            Action::PFilterRotate180 => {
                self.add_persistent_filter(FilterOperation::Rotate180);
            }
            Action::PFilterRotate90CW => {
                self.add_persistent_filter(FilterOperation::Rotate90CW);
            }
            Action::PFilterRotate90CCW => {
                self.add_persistent_filter(FilterOperation::Rotate90CCW);
            }
            Action::PFilterLevels => {
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
                if let Some(vals) = show_filter_dialog(self.hwnd, "永続レベル補正", &fields)
                {
                    let low = vals[0].parse::<u8>().unwrap_or(0);
                    let high = vals[1].parse::<u8>().unwrap_or(255);
                    self.add_persistent_filter(FilterOperation::Levels { low, high });
                }
            }
            Action::PFilterGamma => {
                use crate::ui::filter_dialog::{FieldDef, show_filter_dialog};
                let fields = [FieldDef {
                    label: "ガンマ値 (0.1〜10.0)",
                    default: "1.0".into(),
                    integer_only: false,
                }];
                if let Some(vals) = show_filter_dialog(self.hwnd, "永続ガンマ補正", &fields)
                {
                    let value = vals[0].parse::<f64>().unwrap_or(1.0).clamp(0.1, 10.0);
                    self.add_persistent_filter(FilterOperation::Gamma { value });
                }
            }
            Action::PFilterBrightnessContrast => {
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
                if let Some(vals) =
                    show_filter_dialog(self.hwnd, "永続明るさとコントラスト", &fields)
                {
                    let brightness = vals[0].parse::<i32>().unwrap_or(0).clamp(-128, 128);
                    let contrast = vals[1].parse::<i32>().unwrap_or(0).clamp(-128, 128);
                    self.add_persistent_filter(FilterOperation::BrightnessContrast {
                        brightness,
                        contrast,
                    });
                }
            }
            Action::PFilterGrayscaleSimple => {
                self.add_persistent_filter(FilterOperation::GrayscaleSimple);
            }
            Action::PFilterGrayscaleStrict => {
                self.add_persistent_filter(FilterOperation::GrayscaleStrict);
            }
            Action::PFilterBlur => self.add_persistent_filter(FilterOperation::Blur),
            Action::PFilterBlurStrong => self.add_persistent_filter(FilterOperation::BlurStrong),
            Action::PFilterSharpen => self.add_persistent_filter(FilterOperation::Sharpen),
            Action::PFilterSharpenStrong => {
                self.add_persistent_filter(FilterOperation::SharpenStrong);
            }
            Action::PFilterGaussianBlur => {
                use crate::ui::filter_dialog::{FieldDef, show_filter_dialog};
                let fields = [FieldDef {
                    label: "半径 (0.1〜10.0)",
                    default: "2.0".into(),
                    integer_only: false,
                }];
                if let Some(vals) = show_filter_dialog(self.hwnd, "永続ガウスぼかし", &fields)
                {
                    let radius = vals[0].parse::<f64>().unwrap_or(2.0).clamp(0.1, 10.0);
                    self.add_persistent_filter(FilterOperation::GaussianBlur { radius });
                }
            }
            Action::PFilterUnsharpMask => {
                use crate::ui::filter_dialog::{FieldDef, show_filter_dialog};
                let fields = [FieldDef {
                    label: "半径 (0.1〜10.0)",
                    default: "2.0".into(),
                    integer_only: false,
                }];
                if let Some(vals) = show_filter_dialog(self.hwnd, "永続アンシャープマスク", &fields)
                {
                    let radius = vals[0].parse::<f64>().unwrap_or(2.0).clamp(0.1, 10.0);
                    self.add_persistent_filter(FilterOperation::UnsharpMask { radius });
                }
            }
            Action::PFilterMedianFilter => {
                self.add_persistent_filter(FilterOperation::MedianFilter);
            }
            Action::PFilterInvertColors => {
                self.add_persistent_filter(FilterOperation::InvertColors);
            }
            Action::PFilterApplyAlpha => {
                self.add_persistent_filter(FilterOperation::ApplyAlpha);
            }

            // --- ブックマーク ---
            Action::BookmarkSave => {
                let idx = self.document.file_list().current_index();
                if let Err(e) =
                    crate::bookmark::save_bookmark(self.hwnd, self.document.file_list(), idx)
                {
                    self.show_error_title(&format!("ブックマーク保存失敗: {e}"));
                }
            }
            Action::BookmarkLoad => {
                if !self.guard_unsaved_edit() {
                    return;
                }
                self.selection.deselect();
                match crate::bookmark::load_bookmark(self.hwnd) {
                    Ok(Some(data)) => {
                        if let Err(e) = self.document.load_bookmark_data(data) {
                            self.show_error_title(&format!("ブックマーク読み込み失敗: {e}"));
                        }
                        self.process_document_events();
                    }
                    Ok(None) => {} // キャンセル
                    Err(e) => self.show_error_title(&format!("ブックマーク読み込み失敗: {e}")),
                }
            }
            // --- ページ指定ナビゲーション ---
            Action::NavigateToPage => {
                if !self.guard_unsaved_edit() {
                    return;
                }
                self.selection.deselect();
                self.navigate_to_page_dialog();
            }

            // --- ソートナビゲーション ---
            Action::SortNavigateBack => self.navigate_with_guard(|d| d.sort_navigate_back()),
            Action::SortNavigateForward => {
                self.navigate_with_guard(|d| d.sort_navigate_forward());
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
            Action::ToggleFileList => {
                self.file_list_panel.toggle();
                // パネルが表示状態になったら全同期（非表示中の変更を反映）
                if self.file_list_panel.is_visible() {
                    let doc = &self.document;
                    self.file_list_panel
                        .update(doc.file_list(), |i| doc.is_cached(i));
                    if let Some(idx) = doc.file_list().current_index() {
                        self.file_list_panel.set_selection(idx);
                    }
                    // cached_indicesも同期
                    self.cached_indices.clear();
                    let files = self.document.file_list().files();
                    for i in 0..files.len() {
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

            // --- ヘルプ ---
            Action::ShowHelp => {
                self.show_help();
            }

            // --- アップデート ---
            Action::CheckUpdate => {
                self.check_for_update();
            }

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
            self.document.navigate_to(index);
            self.process_document_events();
        }
    }

    /// 画像を指定フォーマットで書き出す
    fn export_image(&mut self, ext: &str, filter_name: &str, filter_spec: &str) {
        let Some(img) = self.document.current_image() else {
            return;
        };
        let (default_stem, initial_dir) = self
            .document
            .current_source()
            .map(|s| {
                (
                    s.default_save_stem(),
                    s.parent_dir().map(|p| p.to_path_buf()),
                )
            })
            .unwrap_or_else(|| ("image".to_string(), None));
        let default_name = format!("{default_stem}.{ext}");

        let Some(save_path) = crate::file_ops::save_file_dialog(
            self.hwnd,
            &default_name,
            filter_name,
            filter_spec,
            initial_dir.as_deref(),
            None,
            None,
        )
        .ok()
        .flatten() else {
            return;
        };

        // DecodedImage (RGBA) → image::RgbaImage → encode
        let Some(img_buf) = image::RgbaImage::from_raw(img.width, img.height, img.data.clone())
        else {
            self.show_error_title("画像バッファ作成失敗");
            return;
        };

        if let Err(e) = img_buf.save(&save_path) {
            self.show_error_title(&format!("画像書き出し失敗: {e}"));
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
        info_lines.push(format!("ファイルサイズ: {} bytes", file_info.file_size));

        if let Some(img) = self.document.current_image() {
            info_lines.push(format!("画像サイズ: {} x {}", img.width, img.height));
        }

        // メタデータ取得（デコーダ経由）
        if let Ok(metadata) = self.document.current_metadata() {
            info_lines.push(format!("フォーマット: {}", metadata.format));
            for comment in &metadata.comments {
                info_lines.push(comment.clone());
            }
        }

        let text = info_lines.join("\n");
        info_dialog::show_info_dialog(self.hwnd, "画像情報", &text, self.monospace_font.hfont());
    }

    /// ヘルプを表示する
    fn show_help(&self) {
        let text = "\
ぐらびゅ3 - Windows用画像ビューアー

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
            "ぐらびゅ3 ヘルプ",
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
                    "v{} が利用可能です（現在: v{}）。\n更新しますか？\0",
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
            // マウスキャプチャ（ウィンドウ外でもドラッグイベントを受け取る）
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
            let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt);
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut pt);
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

    /// 未保存の編集がある場合に確認ダイアログを表示する
    /// trueを返した場合は操作を続行してよい
    fn guard_unsaved_edit(&mut self) -> bool {
        if !self.document.has_unsaved_edit() {
            return true;
        }
        let msg = "編集中の画像は保存されていません。破棄しますか？\0";
        let title = "ぐらびゅ3\0";
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
            self.document.discard_editing_session();
            self.selection.deselect();
            true
        } else {
            false
        }
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

        let result = if paths.len() > 1 && paths.iter().all(|p| self.document.is_container(p)) {
            // 全てコンテナ（アーカイブ/PDF）なら複数コンテナをまとめて開く
            self.document.open_containers(&paths)
        } else if paths.len() > 1 {
            // 混在: 通知して最初のファイルのみ開く
            let msg = "複数ファイルを開く場合はアーカイブ/PDFのみ対応しています\0";
            let title = "ぐらびゅ3\0";
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
            self.document.open(&paths[0])
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
