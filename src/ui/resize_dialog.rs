//! 解像度変更ダイアログ
//!
//! 幅×高さを入力し、`Option<(u32, u32)>` を返すモーダルダイアログ。

use std::sync::Once;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::COLOR_BTNFACE;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;

const EM_SETSEL: u32 = 0x00B1;

struct DialogData {
    result: Option<(u32, u32)>,
    closed: bool,
}

static REGISTER_ONCE: Once = Once::new();
const CLASS_NAME: &str = "gv_resize_dialog\0";

const ID_WIDTH_EDIT: u16 = 0x500;
const ID_HEIGHT_EDIT: u16 = 0x501;
const ID_OK: u16 = 1; // IDOK: IsDialogMessageWのEnter/ESC対応
const ID_CANCEL: u16 = 2; // IDCANCEL

const DIALOG_WIDTH: i32 = 300;
const DIALOG_HEIGHT: i32 = 180;
const BUTTON_WIDTH: i32 = 80;
const BUTTON_HEIGHT: i32 = 28;
const MARGIN: i32 = 12;

/// 解像度変更ダイアログを表示する（モーダル）
/// `current_width`, `current_height`: 現在の画像サイズ
/// 戻り値: (新幅, 新高さ)。キャンセル時は `None`。
pub fn show_resize_dialog(
    parent: HWND,
    current_width: u32,
    current_height: u32,
) -> Option<(u32, u32)> {
    unsafe {
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
            closed: false,
        };
        let data_ptr = std::ptr::from_mut(&mut data);

        let (x, y) = super::center_on_parent(parent, DIALOG_WIDTH, DIALOG_HEIGHT);
        let class_wide: Vec<u16> = CLASS_NAME.encode_utf16().collect();
        let title_wide: Vec<u16> = "解像度の変更\0".encode_utf16().collect();

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

        let frame_w = GetSystemMetrics(SM_CXFIXEDFRAME);
        let client_w = DIALOG_WIDTH - frame_w * 2;
        let edit_w = (client_w - MARGIN * 3) / 2;

        // 幅ラベル + EDIT
        let w_label: Vec<u16> = "幅\0".encode_utf16().collect();
        let _ = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            windows::core::w!("STATIC"),
            windows::core::PCWSTR(w_label.as_ptr()),
            WS_CHILD | WS_VISIBLE,
            MARGIN,
            MARGIN,
            edit_w,
            20,
            Some(hwnd),
            None,
            None,
            None,
        );

        let edit_y = MARGIN + 20;
        let w_edit_hwnd = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            windows::core::w!("EDIT"),
            None,
            WINDOW_STYLE(
                WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | ES_NUMBER as u32 | ES_AUTOHSCROLL as u32,
            ),
            MARGIN,
            edit_y,
            edit_w,
            24,
            Some(hwnd),
            Some(HMENU(ID_WIDTH_EDIT as *mut _)),
            None,
            None,
        )
        .unwrap_or_default();

        let w_text = format!("{current_width}\0");
        let w_wide: Vec<u16> = w_text.encode_utf16().collect();
        let _ = SetWindowTextW(w_edit_hwnd, windows::core::PCWSTR(w_wide.as_ptr()));
        SendMessageW(
            w_edit_hwnd,
            EM_SETSEL,
            Some(WPARAM(0)),
            Some(LPARAM(-1isize)),
        );

        // 高さラベル + EDIT
        let h_label: Vec<u16> = "高さ\0".encode_utf16().collect();
        let h_x = MARGIN * 2 + edit_w;
        let _ = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            windows::core::w!("STATIC"),
            windows::core::PCWSTR(h_label.as_ptr()),
            WS_CHILD | WS_VISIBLE,
            h_x,
            MARGIN,
            edit_w,
            20,
            Some(hwnd),
            None,
            None,
            None,
        );

        let h_edit_hwnd = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            windows::core::w!("EDIT"),
            None,
            WINDOW_STYLE(
                WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | ES_NUMBER as u32 | ES_AUTOHSCROLL as u32,
            ),
            h_x,
            edit_y,
            edit_w,
            24,
            Some(hwnd),
            Some(HMENU(ID_HEIGHT_EDIT as *mut _)),
            None,
            None,
        )
        .unwrap_or_default();

        let h_text = format!("{current_height}\0");
        let h_wide: Vec<u16> = h_text.encode_utf16().collect();
        let _ = SetWindowTextW(h_edit_hwnd, windows::core::PCWSTR(h_wide.as_ptr()));

        // ボタン行
        let button_y = edit_y + 24 + MARGIN;
        let gap = 8;
        let buttons_total = BUTTON_WIDTH * 2 + gap;
        let button_x = (client_w - buttons_total) / 2;

        let ok_label: Vec<u16> = "OK\0".encode_utf16().collect();
        let _ = CreateWindowExW(
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
        );

        let cancel_label: Vec<u16> = "キャンセル\0".encode_utf16().collect();
        let _ = CreateWindowExW(
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
        );

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = windows::Win32::Graphics::Gdi::UpdateWindow(hwnd);
        let _ = SetFocus(Some(w_edit_hwnd));

        let _ = EnableWindow(parent, false);
        let mut msg = MSG::default();
        #[allow(clippy::while_immutable_condition)]
        while !data.closed {
            let ret = GetMessageW(std::ptr::from_mut(&mut msg), None, 0, 0);
            if !ret.as_bool() {
                break;
            }
            if IsDialogMessageW(hwnd, std::ptr::from_ref(&msg)).as_bool() {
                continue;
            }
            let _ = TranslateMessage(std::ptr::from_ref(&msg));
            DispatchMessageW(std::ptr::from_ref(&msg));
        }
        let _ = EnableWindow(parent, true);
        let _ = SetForegroundWindow(parent);

        if IsWindow(Some(hwnd)).as_bool() {
            let _ = DestroyWindow(hwnd);
        }

        data.result
    }
}

unsafe extern "system" fn dialog_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_CREATE => {
                let cs = &*(lparam.0 as *const CREATESTRUCTW);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
                return LRESULT(0);
            }
            WM_COMMAND => {
                let control_id = (wparam.0 as u32) & 0xFFFF;
                if control_id == ID_OK as u32 {
                    if let Some(data) = get_dialog_data(hwnd) {
                        let w_edit =
                            GetDlgItem(Some(hwnd), ID_WIDTH_EDIT as i32).unwrap_or_default();
                        let h_edit =
                            GetDlgItem(Some(hwnd), ID_HEIGHT_EDIT as i32).unwrap_or_default();

                        let width = read_edit_u32(w_edit);
                        let height = read_edit_u32(h_edit);

                        if let (Some(w), Some(h)) = (width, height)
                            && w > 0
                            && h > 0
                        {
                            data.result = Some((w, h));
                        }
                        data.closed = true;
                    }
                    return LRESULT(0);
                }
                if control_id == ID_CANCEL as u32 {
                    if let Some(data) = get_dialog_data(hwnd) {
                        data.closed = true;
                    }
                    return LRESULT(0);
                }
            }
            WM_CLOSE => {
                if let Some(data) = get_dialog_data(hwnd) {
                    data.closed = true;
                }
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

/// EDITコントロールからu32を読み取る
unsafe fn read_edit_u32(hwnd: HWND) -> Option<u32> {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        GetWindowTextW(hwnd, &mut buf);
        let text = String::from_utf16_lossy(&buf[..len as usize]);
        text.trim().parse::<u32>().ok()
    }
}

unsafe fn get_dialog_data(hwnd: HWND) -> Option<&'static mut DialogData> {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogData;
        if ptr.is_null() { None } else { Some(&mut *ptr) }
    }
}
