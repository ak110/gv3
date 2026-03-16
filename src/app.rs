use std::path::Path;

use anyhow::Result;
use crossbeam_channel::Receiver;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{InvalidateRect, UpdateWindow, ValidateRect};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::document::{Document, DocumentEvent};
use crate::render::D2DRenderer;
use crate::ui::window;

/// DocumentEventをUIスレッドに通知するためのカスタムメッセージ
const WM_DOCUMENT_EVENT: u32 = WM_APP + 1;

/// 仮想キーコード定数
const VK_LEFT: i32 = 0x25;
const VK_RIGHT: i32 = 0x27;
const VK_PRIOR: i32 = 0x21; // PageUp
const VK_NEXT: i32 = 0x22; // PageDown
const VK_HOME: i32 = 0x24;
const VK_END: i32 = 0x23;
const VK_CONTROL: i32 = 0x11;

/// メインウィンドウ
pub struct AppWindow {
    hwnd: HWND,
    document: Document,
    event_receiver: Receiver<DocumentEvent>,
    renderer: D2DRenderer,
}

impl AppWindow {
    /// AppWindowを作成しウィンドウを表示する
    pub fn create(initial_file: Option<&Path>) -> Result<Box<Self>> {
        let class_name = windows::core::w!("gv3_main");
        window::register_window_class(class_name, Some(Self::wnd_proc))?;

        let hwnd = window::create_window(class_name, windows::core::w!("ぐらびゅ3"), 1024, 768)?;

        // D&Dを受け付ける
        unsafe {
            DragAcceptFiles(hwnd, true);
        }

        let (sender, receiver) = crossbeam_channel::unbounded();
        let renderer = D2DRenderer::new(hwnd)?;
        let document = Document::new(sender);

        let mut app = Box::new(Self {
            hwnd,
            document,
            event_receiver: receiver,
            renderer,
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
        app.document.start_prefetch(notify, cache_budget);

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
                DocumentEvent::NavigationChanged { .. } => {
                    self.update_title();
                }
                DocumentEvent::FileListChanged => {}
                DocumentEvent::Error(msg) => {
                    eprintln!("エラー: {msg}");
                }
            }
        }
    }

    /// タイトルバーを更新
    fn update_title(&self) {
        let title = if let Some(path) = self.document.current_path() {
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("???");
            let fl = self.document.file_list();
            if let Some(idx) = fl.current_index() {
                format!("{filename} ({}/{}) - ぐらびゅ3\0", idx + 1, fl.len())
            } else {
                format!("{filename} - ぐらびゅ3\0")
            }
        } else {
            "ぐらびゅ3\0".to_string()
        };

        let wide: Vec<u16> = title.encode_utf16().collect();
        unsafe {
            let _ = SetWindowTextW(self.hwnd, windows::core::PCWSTR(wide.as_ptr()));
        }
    }

    /// Ctrlキーが押されているか判定
    fn is_ctrl_down() -> bool {
        unsafe { GetKeyState(VK_CONTROL) < 0 }
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
                    let width = (lparam.0 & 0xFFFF) as u32;
                    let height = ((lparam.0 >> 16) & 0xFFFF) as u32;
                    app.on_size(width, height);
                    return LRESULT(0);
                }
                WM_KEYDOWN => {
                    app.on_keydown(wparam.0 as i32);
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
            self.renderer.resize(width, height);
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
    }

    fn on_keydown(&mut self, vk: i32) {
        let ctrl = Self::is_ctrl_down();

        match (vk, ctrl) {
            (VK_LEFT, false) => self.document.navigate_relative(-1),
            (VK_RIGHT, false) => self.document.navigate_relative(1),
            (VK_PRIOR, false) => self.document.navigate_relative(-5), // PageUp
            (VK_NEXT, false) => self.document.navigate_relative(5),   // PageDown
            (VK_PRIOR, true) => self.document.navigate_relative(-50), // Ctrl+PageUp
            (VK_NEXT, true) => self.document.navigate_relative(50),   // Ctrl+PageDown
            (VK_HOME, true) => self.document.navigate_first(),        // Ctrl+Home
            (VK_END, true) => self.document.navigate_last(),          // Ctrl+End
            _ => return,
        }

        self.process_document_events();
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
