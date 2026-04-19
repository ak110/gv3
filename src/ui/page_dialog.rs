//! ページ指定移動ダイアログ
//!
//! 数値入力でページ番号を指定し、`Option<usize>` を返すモーダルダイアログ。

use std::sync::Once;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_BTNFACE, UpdateWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;

/// EM_SETSEL メッセージ (EDIT コントロールの選択範囲設定)
const EM_SETSEL: u32 = 0x00B1;

/// ダイアログの内部状態
struct DialogData {
    result: Option<usize>,
    total: usize,
    closed: bool,
}

/// ウィンドウクラス登録 (一度だけ)
static REGISTER_ONCE: Once = Once::new();
const CLASS_NAME: &str = "gv_page_dialog\0";

// 子コントロールID
const ID_EDIT: u16 = 0x300;
const ID_OK: u16 = 1; // IDOK: IsDialogMessageWのEnter/ESC対応
const ID_CANCEL: u16 = 2; // IDCANCEL
const ID_LABEL: u16 = 0x303;

/// ダイアログサイズ
const DIALOG_WIDTH: i32 = 280;
const DIALOG_HEIGHT: i32 = 150;
const BUTTON_WIDTH: i32 = 80;
const BUTTON_HEIGHT: i32 = 28;
const MARGIN: i32 = 12;

/// ページ指定ダイアログを表示する (モーダル)
///
/// `parent`: 親ウィンドウ
/// `current`: 現在のページ番号 (1-based)
/// `total`: 総ページ数
///
/// 戻り値: 入力されたページ番号 (1-based)。キャンセル時は `None`。
pub fn show_page_dialog(parent: HWND, current: usize, total: usize) -> Option<usize> {
    unsafe {
        // ウィンドウクラス登録
        REGISTER_ONCE.call_once(|| {
            let class_wide: Vec<u16> = CLASS_NAME.encode_utf16().collect();
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(dialog_wnd_proc),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(
                    (COLOR_BTNFACE.0 + 1) as *mut _,
                ),
                lpszClassName: windows::core::PCWSTR(class_wide.as_ptr()),
                ..Default::default()
            };
            RegisterClassExW(std::ptr::from_ref(&wc));
        });

        let mut data = DialogData {
            result: None,
            total,
            closed: false,
        };
        let data_ptr = std::ptr::from_mut(&mut data);

        // 親ウィンドウの中心に配置
        let (x, y) = super::center_on_parent(parent, DIALOG_WIDTH, DIALOG_HEIGHT);

        let class_wide: Vec<u16> = CLASS_NAME.encode_utf16().collect();
        let title_wide: Vec<u16> = "ページ指定移動\0".encode_utf16().collect();

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            windows::core::PCWSTR(class_wide.as_ptr()),
            windows::core::PCWSTR(title_wide.as_ptr()),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            x,
            y,
            DIALOG_WIDTH,
            DIALOG_HEIGHT,
            Some(parent),
            None,
            None,
            Some(data_ptr as *const _),
        )
        .unwrap_or_default();

        if hwnd.is_invalid() {
            return None;
        }

        let titlebar_h = GetSystemMetrics(SM_CYCAPTION) + GetSystemMetrics(SM_CYSIZEFRAME);
        let frame_w = GetSystemMetrics(SM_CXFIXEDFRAME);
        let client_w = DIALOG_WIDTH - frame_w * 2;

        // ラベル: "ページ番号 (1〜{total})"
        let label_text = format!("ページ番号 (1\u{301C}{total})\0");
        let label_wide: Vec<u16> = label_text.encode_utf16().collect();
        let _label_hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            windows::core::w!("STATIC"),
            windows::core::PCWSTR(label_wide.as_ptr()),
            WS_CHILD | WS_VISIBLE,
            MARGIN,
            MARGIN,
            client_w - MARGIN * 2,
            20,
            Some(hwnd),
            Some(HMENU(ID_LABEL as *mut _)),
            None,
            None,
        )
        .unwrap_or_default();

        // EDIT: 数字入力、現在ページをプリフィル
        let edit_y = MARGIN + 24;
        let edit_hwnd = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            windows::core::w!("EDIT"),
            None,
            WINDOW_STYLE(
                WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | ES_NUMBER as u32 | ES_AUTOHSCROLL as u32,
            ),
            MARGIN,
            edit_y,
            client_w - MARGIN * 2,
            24,
            Some(hwnd),
            Some(HMENU(ID_EDIT as *mut _)),
            None,
            None,
        )
        .unwrap_or_default();

        // 現在ページをプリフィル + 全選択
        let current_str = format!("{current}\0");
        let current_wide: Vec<u16> = current_str.encode_utf16().collect();
        let _ = SetWindowTextW(edit_hwnd, windows::core::PCWSTR(current_wide.as_ptr()));
        // 全選択: EM_SETSEL(0, -1)
        SendMessageW(edit_hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1isize)));

        // ボタン行
        let button_y = edit_y + 24 + MARGIN;
        let gap = 8;
        let buttons_total = BUTTON_WIDTH * 2 + gap;
        let button_x = (client_w - buttons_total) / 2;

        // OKボタン (BS_DEFPUSHBUTTON: Enterキーで発火)
        let ok_label: Vec<u16> = "OK\0".encode_utf16().collect();
        let _ok_hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            windows::core::w!("BUTTON"),
            windows::core::PCWSTR(ok_label.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32),
            button_x,
            button_y,
            BUTTON_WIDTH,
            BUTTON_HEIGHT,
            Some(hwnd),
            Some(HMENU(ID_OK as *mut _)),
            None,
            None,
        )
        .unwrap_or_default();

        // キャンセルボタン
        let cancel_label: Vec<u16> = "キャンセル\0".encode_utf16().collect();
        let _cancel_hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            windows::core::w!("BUTTON"),
            windows::core::PCWSTR(cancel_label.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP,
            button_x + BUTTON_WIDTH + gap,
            button_y,
            BUTTON_WIDTH,
            BUTTON_HEIGHT,
            Some(hwnd),
            Some(HMENU(ID_CANCEL as *mut _)),
            None,
            None,
        )
        .unwrap_or_default();

        // 表示
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        // EDITにフォーカス
        let _ = SetFocus(Some(edit_hwnd));

        // モーダルループ
        super::dialog::run_modal_loop(parent, hwnd, &raw const data.closed);

        if IsWindow(Some(hwnd)).as_bool() {
            let _ = DestroyWindow(hwnd);
        }

        // titlebar_h は上で使っているが警告を避けるため明示的に使用
        let _ = titlebar_h;

        data.result
    }
}

/// ダイアログ専用WndProc
unsafe extern "system" fn dialog_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_CREATE => {
                // SAFETY: WM_CREATE の lparam は常に有効な CREATESTRUCTW へのポインタ。
                let cs = &*(lparam.0 as *const CREATESTRUCTW);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
                return LRESULT(0);
            }
            WM_COMMAND => {
                let control_id = (wparam.0 as u32) & 0xFFFF;
                if control_id == ID_OK as u32 {
                    // EDITからテキスト取得
                    if let Some(data) = super::dialog::get_window_data::<DialogData>(hwnd) {
                        let edit_hwnd = GetDlgItem(Some(hwnd), ID_EDIT as i32).unwrap_or_default();
                        let len = GetWindowTextLengthW(edit_hwnd);
                        if len > 0 {
                            let mut buf = vec![0u16; (len + 1) as usize];
                            GetWindowTextW(edit_hwnd, &mut buf);
                            let text = String::from_utf16_lossy(&buf[..len as usize]);
                            if let Ok(page) = text.trim().parse::<usize>()
                                && page >= 1
                                && page <= data.total
                            {
                                data.result = Some(page);
                            }
                        }
                        data.closed = true;
                    }
                    return LRESULT(0);
                }
                if control_id == ID_CANCEL as u32 {
                    if let Some(data) = super::dialog::get_window_data::<DialogData>(hwnd) {
                        data.closed = true;
                    }
                    return LRESULT(0);
                }
            }
            WM_CLOSE => {
                if let Some(data) = super::dialog::get_window_data::<DialogData>(hwnd) {
                    data.closed = true;
                }
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}
