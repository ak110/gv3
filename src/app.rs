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
use crate::image::{DecoderChain, StandardDecoder};
use crate::render::D2DRenderer;
use crate::render::layout::DisplayMode;
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
            if let Some(idx) = fl.current_index() {
                format!("{display} [{}/{}] - ぐらびゅ3\0", idx + 1, fl.len())
            } else {
                format!("{display} - ぐらびゅ3\0")
            }
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
                    return LRESULT(0);
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
        self.renderer.draw(self.document.current_image());
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

    /// アクションを実行する
    fn execute_action(&mut self, action: Action) {
        match action {
            // --- ナビゲーション ---
            Action::NavigateBack => {
                self.document.navigate_relative(-1);
                self.process_document_events();
            }
            Action::NavigateForward => {
                self.document.navigate_relative(1);
                self.process_document_events();
            }
            Action::Navigate1Back => {
                self.document.navigate_relative(-1);
                self.process_document_events();
            }
            Action::Navigate1Forward => {
                self.document.navigate_relative(1);
                self.process_document_events();
            }
            Action::Navigate5Back => {
                self.document.navigate_relative(-5);
                self.process_document_events();
            }
            Action::Navigate5Forward => {
                self.document.navigate_relative(5);
                self.process_document_events();
            }
            Action::Navigate50Back => {
                self.document.navigate_relative(-50);
                self.process_document_events();
            }
            Action::Navigate50Forward => {
                self.document.navigate_relative(50);
                self.process_document_events();
            }
            Action::NavigateFirst => {
                self.document.navigate_first();
                self.process_document_events();
            }
            Action::NavigateLast => {
                self.document.navigate_last();
                self.process_document_events();
            }

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
            Action::NavigatePrevMark => {
                self.document.navigate_prev_mark();
                self.process_document_events();
            }
            Action::NavigateNextMark => {
                self.document.navigate_next_mark();
                self.process_document_events();
            }
            Action::RemoveFromList => {
                self.document.remove_current_from_list();
                self.process_document_events();
            }
            Action::MarkedRemoveFromList => {
                self.document.remove_marked_from_list();
                self.process_document_events();
            }

            // --- フォルダナビゲーション ---
            Action::NavigatePrevFolder => {
                self.document.navigate_prev_folder();
                self.process_document_events();
            }
            Action::NavigateNextFolder => {
                self.document.navigate_next_folder();
                self.process_document_events();
            }

            // --- ファイル操作 ---
            Action::OpenFile => {
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

                if let Ok(Some(dest)) = crate::file_ops::save_file_dialog(
                    self.hwnd,
                    &default_name,
                    "すべてのファイル",
                    "*.*",
                    initial_dir.as_deref(),
                ) {
                    match &source {
                        crate::file_info::FileSource::File(_) => {
                            // 通常ファイル: SHFileOperationWでUndo対応の移動
                            if let Ok(true) =
                                crate::file_ops::move_single_file(self.hwnd, &path, &dest)
                            {
                                // 同一フォルダ内（リネーム）ならリスト内エントリを更新
                                if path.parent() == dest.parent() {
                                    if let Err(e) = self.document.rename_current_in_list(&dest) {
                                        eprintln!("リスト更新失敗: {e}");
                                    }
                                } else {
                                    self.document.remove_current_from_list();
                                }
                                self.process_document_events();
                            }
                        }
                        crate::file_info::FileSource::ArchiveEntry { on_demand, .. } => {
                            // アーカイブエントリ: 書き出し（リスト除去なし）
                            if *on_demand {
                                match self.document.read_file_data_current() {
                                    Ok(data) => {
                                        if let Err(e) = std::fs::write(&dest, &data) {
                                            eprintln!("ファイル書き出し失敗: {e}");
                                        }
                                    }
                                    Err(e) => eprintln!("ファイル書き出し失敗: {e}"),
                                }
                            } else {
                                if let Err(e) = std::fs::copy(&path, &dest) {
                                    eprintln!("ファイル書き出し失敗: {e}");
                                }
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
                    ) {
                        if matches!(
                            current.source,
                            crate::file_info::FileSource::ArchiveEntry {
                                on_demand: true,
                                ..
                            }
                        ) {
                            // オンデマンド: アーカイブから読み出して書き出し
                            match self.document.read_file_data_current() {
                                Ok(data) => {
                                    if let Err(e) = std::fs::write(&dest, &data) {
                                        eprintln!("ファイルコピー失敗: {e}");
                                    }
                                }
                                Err(e) => eprintln!("ファイルコピー失敗: {e}"),
                            }
                        } else {
                            // 通常ファイル/temp展開済み/PDF: 既存のfs::copy
                            if let Err(e) = std::fs::copy(&current.path, &dest) {
                                eprintln!("ファイルコピー失敗: {e}");
                            }
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
                        self.document.remove_marked_from_list();
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
                        eprintln!("ファイルコピー失敗: {e}");
                    }
                }
            }
            Action::Reload => {
                self.document.reload();
                self.process_document_events();
            }

            // --- クリップボード ---
            Action::CopyImage => {
                if let Some(image) = self.document.current_image()
                    && let Err(e) = crate::clipboard::copy_image_to_clipboard(self.hwnd, image)
                {
                    eprintln!("画像コピー失敗: {e}");
                }
            }
            Action::CopyFileName => {
                if let Some(source) = self.document.current_source()
                    && let Err(e) =
                        crate::clipboard::copy_text_to_clipboard(self.hwnd, &source.display_path())
                {
                    eprintln!("ファイル名コピー失敗: {e}");
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
                        eprintln!("マークファイル名コピー失敗: {e}");
                    }
                }
            }
            Action::PasteImage => {
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
                self.document.close_all();
                self.process_document_events();
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

            // --- ブックマーク ---
            Action::BookmarkSave => {
                let idx = self.document.file_list().current_index();
                if let Err(e) =
                    crate::bookmark::save_bookmark(self.hwnd, self.document.file_list(), idx)
                {
                    eprintln!("ブックマーク保存失敗: {e}");
                }
            }
            Action::BookmarkLoad => {
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
                self.navigate_to_page_dialog();
            }

            // --- ソートナビゲーション ---
            Action::SortNavigateBack => {
                self.document.sort_navigate_back();
                self.process_document_events();
            }
            Action::SortNavigateForward => {
                self.document.sort_navigate_forward();
                self.process_document_events();
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
        )
        .ok()
        .flatten() else {
            return;
        };

        // DecodedImage (RGBA) → image::RgbaImage → encode
        let Some(img_buf) = image::RgbaImage::from_raw(img.width, img.height, img.data.clone())
        else {
            eprintln!("画像バッファ作成失敗");
            return;
        };

        if let Err(e) = img_buf.save(&save_path) {
            eprintln!("画像書き出し失敗: {e}");
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

    fn on_drop_files(&mut self, hdrop: HDROP) {
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
