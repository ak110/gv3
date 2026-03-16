use std::path::Path;
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
    // 等幅フォント（ダイアログ・ファイルリスト用）
    monospace_font: MonospaceFont,
}

impl AppWindow {
    /// AppWindowを作成しウィンドウを表示する
    pub fn create(config: Config, initial_file: Option<&Path>) -> Result<Box<Self>> {
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
            menu_visible: false,
            file_list_panel,
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
        if let Some(path) = initial_file {
            if let Err(e) = app.document.open(path) {
                eprintln!("画像を開けませんでした: {e}");
            }
            app.process_document_events();
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
                    eprintln!("エラー: {msg}");
                }
            }
        }

        // ファイルリストパネルが表示中なら、キャッシュ状態をリアルタイム反映
        if self.file_list_panel.is_visible() {
            let doc = &self.document;
            self.file_list_panel
                .update(doc.file_list(), |i| doc.is_cached(i));
            // 選択位置を復元
            if let Some(idx) = self.document.file_list().current_index() {
                self.file_list_panel.set_selection(idx);
            }
        }
    }

    /// タイトルバーを更新
    fn update_title(&self) {
        let title = if let Some(source) = self.document.current_source() {
            let display = source.display_path();
            // 表示名: ファイル名部分を抽出
            let filename = display
                .rsplit(['/', '\\', '>'])
                .next()
                .map(|s| s.trim())
                .unwrap_or("???");
            let fl = self.document.file_list();
            // アーカイブ名をタイトルに付加
            let archive_suffix = source
                .archive_path()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(|name| format!(" [{name}]"))
                .unwrap_or_default();
            if let Some(idx) = fl.current_index() {
                format!(
                    "{filename} ({}/{}{archive_suffix}) - ぐらびゅ3\0",
                    idx + 1,
                    fl.len()
                )
            } else {
                format!("{filename}{archive_suffix} - ぐらびゅ3\0")
            }
        } else {
            "ぐらびゅ3\0".to_string()
        };

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
                self.document.mark_current();
                self.process_document_events();
            }
            Action::MarkUnset => {
                self.document.unmark_current();
            }
            Action::MarkInvertAll => {
                self.document.invert_all_marks();
            }
            Action::MarkInvertToHere => {
                self.document.invert_marks_to_here();
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
                if let Ok(Some(path)) = crate::file_ops::open_file_dialog(self.hwnd) {
                    if let Err(e) = self.document.open(&path) {
                        eprintln!("ファイルを開けませんでした: {e}");
                    }
                    self.process_document_events();
                }
            }
            Action::OpenFolder => {
                if let Ok(Some(path)) = crate::file_ops::open_folder_dialog(self.hwnd) {
                    if let Err(e) = self.document.open_folder(&path) {
                        eprintln!("フォルダを開けませんでした: {e}");
                    }
                    self.process_document_events();
                }
            }
            Action::DeleteFile => {
                // アーカイブ内ファイルの削除は無効
                if let Some(source) = self.document.current_source()
                    && source.is_archive_entry()
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
                if let Some(source) = self.document.current_source()
                    && source.is_archive_entry()
                {
                    return;
                }
                if let Some(path) = self.document.current_path().map(|p| p.to_path_buf())
                    && let Ok(Some(dest)) =
                        crate::file_ops::select_folder_dialog(self.hwnd, "移動先フォルダ")
                    && let Ok(true) = crate::file_ops::move_files(self.hwnd, &[&path], &dest)
                {
                    self.document.remove_current_from_list();
                    self.process_document_events();
                }
            }
            Action::CopyFile => {
                if let Some(path) = self.document.current_path().map(|p| p.to_path_buf()) {
                    let default_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("image");
                    if let Ok(Some(dest)) = crate::file_ops::save_file_dialog(
                        self.hwnd,
                        default_name,
                        "すべてのファイル",
                        "*.*",
                    ) && let Err(e) = std::fs::copy(&path, &dest)
                    {
                        eprintln!("ファイルコピー失敗: {e}");
                    }
                }
            }
            Action::MarkedDelete => {
                // アーカイブ内は無効
                if self.document.current_archive().is_some() {
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
                if self.document.current_archive().is_some() {
                    return;
                }
                let paths: Vec<std::path::PathBuf> = self
                    .document
                    .file_list()
                    .marked_indices()
                    .iter()
                    .map(|&i| self.document.file_list().files()[i].path.clone())
                    .collect();
                if paths.is_empty() {
                    return;
                }
                if let Ok(Some(dest)) =
                    crate::file_ops::select_folder_dialog(self.hwnd, "移動先フォルダ")
                {
                    let path_refs: Vec<&Path> = paths.iter().map(|p| p.as_path()).collect();
                    if let Ok(true) = crate::file_ops::move_files(self.hwnd, &path_refs, &dest) {
                        self.document.remove_marked_from_list();
                        self.process_document_events();
                    }
                }
            }
            Action::MarkedCopy => {
                let paths: Vec<std::path::PathBuf> = self
                    .document
                    .file_list()
                    .marked_indices()
                    .iter()
                    .map(|&i| self.document.file_list().files()[i].path.clone())
                    .collect();
                if paths.is_empty() {
                    return;
                }
                if let Ok(Some(dest)) =
                    crate::file_ops::select_folder_dialog(self.hwnd, "コピー先フォルダ")
                {
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
                            if let Err(e) = self.document.open(&temp_path) {
                                eprintln!("貼り付け失敗: {e}");
                            }
                            self.process_document_events();
                        }
                    }
                    Ok(None) => {} // クリップボードに画像なし
                    Err(e) => eprintln!("貼り付け失敗: {e}"),
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
                    // アーカイブ内の場合はアーカイブパスを渡す
                    if let Some(source) = self.document.current_source() {
                        match source {
                            crate::file_info::FileSource::ArchiveEntry { archive, .. } => {
                                cmd.arg(archive);
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
                            eprintln!("ブックマーク読み込み失敗: {e}");
                        }
                        self.process_document_events();
                    }
                    Ok(None) => {} // キャンセル
                    Err(e) => eprintln!("ブックマーク読み込み失敗: {e}"),
                }
            }
            Action::BookmarkOpenEditor => {
                let dir = crate::bookmark::bookmark_dir();
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::process::Command::new("explorer.exe").arg(&dir).spawn();
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
                // toggle内でWM_SIZEが送られてon_sizeが呼ばれる
            }

            // --- ヘルプ ---
            Action::ShowHelp => {
                self.show_help();
            }

            // --- 未実装スタブ ---
            Action::OpenHistory
            | Action::DialogDisplay
            | Action::DialogImage
            | Action::DialogDraw
            | Action::DialogList
            | Action::DialogGeneral
            | Action::DialogPlugin
            | Action::DialogEnvironment
            | Action::DialogKeys => {
                eprintln!("未実装: {action:?}");
            }
        }
    }

    /// ページ指定ナビゲーション
    /// 総ページ数の桁数に応じて、数字キー入力でページ移動する簡易方式
    fn navigate_to_page_dialog(&mut self) {
        let total = self.document.file_list().len();
        if total == 0 {
            return;
        }
        let current = self.document.file_list().current_index().unwrap_or(0) + 1;

        // MessageBoxで現在位置と総数を表示して入力を促す
        // （本格的な入力ダイアログは設定ダイアログフェーズで実装予定）
        let prompt = format!(
            "現在: {current} / {total}\n\nページ番号をタイトルバーに入力してEnterで移動\n（この機能は設定ダイアログフェーズで改善予定）\0"
        );
        let title = "ページ指定移動\0";
        let wide_prompt: Vec<u16> = prompt.encode_utf16().collect();
        let wide_title: Vec<u16> = title.encode_utf16().collect();
        unsafe {
            MessageBoxW(
                Some(self.hwnd),
                windows::core::PCWSTR(wide_prompt.as_ptr()),
                windows::core::PCWSTR(wide_title.as_ptr()),
                MB_OK | MB_ICONINFORMATION,
            );
        }
        // TODO: 設定ダイアログフェーズで入力ダイアログに置き換え
    }

    /// 画像を指定フォーマットで書き出す
    fn export_image(&mut self, ext: &str, filter_name: &str, filter_spec: &str) {
        let Some(img) = self.document.current_image() else {
            return;
        };
        let default_name = self
            .document
            .current_path()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .map(|s| format!("{s}.{ext}"))
            .unwrap_or_else(|| format!("image.{ext}"));

        let Some(save_path) =
            crate::file_ops::save_file_dialog(self.hwnd, &default_name, filter_name, filter_spec)
                .ok()
                .flatten()
        else {
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
ぐらびゅ3 - Windows用画像ビューア

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
アーカイブ: ZIP/cbz, RAR/cbr, 7z
Susieプラグイン (.sph/.spi) で拡張可能";

        info_dialog::show_info_dialog(
            self.hwnd,
            "ぐらびゅ3 ヘルプ",
            text,
            self.monospace_font.hfont(),
        );
    }

    fn on_drop_files(&mut self, hdrop: HDROP) {
        // ドロップされた最初のファイル/フォルダのパスを取得
        let mut buf = [0u16; 1024];
        let len = unsafe { DragQueryFileW(hdrop, 0, Some(&mut buf)) } as usize;
        unsafe { DragFinish(hdrop) };

        if len == 0 {
            return;
        }

        let path_str = String::from_utf16_lossy(&buf[..len]);
        let path = std::path::Path::new(&path_str);

        let result = if path.is_dir() {
            self.document.open_folder(path)
        } else {
            self.document.open(path)
        };

        if let Err(e) = result {
            eprintln!("ドロップされたファイルを開けませんでした: {e}");
        }

        self.process_document_events();
    }
}
