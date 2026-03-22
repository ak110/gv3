//! 角度指定回転ダイアログ
//!
//! 浮動小数点入力で回転角度を指定し、`Option<f64>` を返すモーダルダイアログ。

use std::sync::Once;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::COLOR_BTNFACE;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;

const EM_SETSEL: u32 = 0x00B1;

struct DialogData {
    result: Option<f64>,
    closed: bool,
}

static REGISTER_ONCE: Once = Once::new();
const CLASS_NAME: &str = "gv_rotate_dialog\0";

const ID_EDIT: u16 = 0x400;
const ID_OK: u16 = 1; // IDOK: IsDialogMessageWのEnter/ESC対応
const ID_CANCEL: u16 = 2; // IDCANCEL

const DIALOG_WIDTH: i32 = 280;
const DIALOG_HEIGHT: i32 = 150;
const BUTTON_WIDTH: i32 = 80;
const BUTTON_HEIGHT: i32 = 28;
const MARGIN: i32 = 12;

/// 角度指定回転ダイアログを表示する（モーダル）
/// 戻り値: 入力された角度（度数法）。キャンセル時は `None`。
pub fn show_rotate_dialog(parent: HWND) -> Option<f64> {
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
        let title_wide: Vec<u16> = "角度指定回転\0".encode_utf16().collect();

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

        // ラベル
        let label: Vec<u16> = "回転角度（度）\0".encode_utf16().collect();
        let _label_hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            windows::core::w!("STATIC"),
            windows::core::PCWSTR(label.as_ptr()),
            WS_CHILD | WS_VISIBLE,
            MARGIN,
            MARGIN,
            client_w - MARGIN * 2,
            20,
            Some(hwnd),
            None,
            None,
            None,
        );

        // EDIT
        let edit_y = MARGIN + 24;
        let edit_hwnd = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            windows::core::w!("EDIT"),
            None,
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32),
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

        let default_text: Vec<u16> = "0\0".encode_utf16().collect();
        let _ = SetWindowTextW(edit_hwnd, windows::core::PCWSTR(default_text.as_ptr()));
        SendMessageW(edit_hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1isize)));

        // ボタン行
        let button_y = edit_y + 24 + MARGIN;
        let gap = 8;
        let buttons_total = BUTTON_WIDTH * 2 + gap;
        let button_x = (client_w - buttons_total) / 2;

        let ok_label: Vec<u16> = "OK\0".encode_utf16().collect();
        let _ok = CreateWindowExW(
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
        let _cancel = CreateWindowExW(
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
        let _ = SetFocus(Some(edit_hwnd));

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
                        let edit_hwnd = GetDlgItem(Some(hwnd), ID_EDIT as i32).unwrap_or_default();
                        let len = GetWindowTextLengthW(edit_hwnd);
                        if len > 0 {
                            let mut buf = vec![0u16; (len + 1) as usize];
                            GetWindowTextW(edit_hwnd, &mut buf);
                            let text = String::from_utf16_lossy(&buf[..len as usize]);
                            if let Ok(degrees) = text.trim().parse::<f64>() {
                                data.result = Some(degrees);
                            }
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

unsafe fn get_dialog_data(hwnd: HWND) -> Option<&'static mut DialogData> {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogData;
        if ptr.is_null() { None } else { Some(&mut *ptr) }
    }
}
