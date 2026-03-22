//! フィルタパラメータ入力ダイアログ
//!
//! 複数のラベル+数値入力フィールドを持つ汎用モーダルダイアログ。

use std::sync::Once;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::COLOR_BTNFACE;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;

const EM_SETSEL: u32 = 0x00B1;

/// フィールド定義
pub struct FieldDef {
    pub label: &'static str,
    pub default: String,
    /// trueならES_NUMBERを適用（整数のみ）
    pub integer_only: bool,
}

struct DialogData {
    results: Option<Vec<String>>,
    field_count: usize,
    closed: bool,
}

static REGISTER_ONCE: Once = Once::new();
const CLASS_NAME: &str = "gv_filter_dialog\0";

const ID_EDIT_BASE: u16 = 0x600;
const ID_OK: u16 = 1; // IDOK: IsDialogMessageWのEnter/ESC対応
const ID_CANCEL: u16 = 2; // IDCANCEL

const DIALOG_WIDTH: i32 = 320;
const BUTTON_WIDTH: i32 = 80;
const BUTTON_HEIGHT: i32 = 28;
const MARGIN: i32 = 12;
const FIELD_HEIGHT: i32 = 48; // ラベル(20) + EDIT(24) + gap(4)

/// フィルタパラメータダイアログを表示する
/// 戻り値: 各フィールドの入力値。キャンセル時はNone。
pub fn show_filter_dialog(parent: HWND, title: &str, fields: &[FieldDef]) -> Option<Vec<String>> {
    if fields.is_empty() {
        return None;
    }

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

        let dialog_height = MARGIN
            + fields.len() as i32 * FIELD_HEIGHT
            + MARGIN
            + BUTTON_HEIGHT
            + MARGIN * 2
            + GetSystemMetrics(SM_CYCAPTION)
            + GetSystemMetrics(SM_CYSIZEFRAME);

        let mut data = DialogData {
            results: None,
            field_count: fields.len(),
            closed: false,
        };
        let data_ptr = std::ptr::from_mut(&mut data);

        let (x, y) = super::center_on_parent(parent, DIALOG_WIDTH, dialog_height);
        let class_wide: Vec<u16> = CLASS_NAME.encode_utf16().collect();
        let title_wide: Vec<u16> = format!("{title}\0").encode_utf16().collect();

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            windows::core::PCWSTR(class_wide.as_ptr()),
            windows::core::PCWSTR(title_wide.as_ptr()),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            x,
            y,
            DIALOG_WIDTH,
            dialog_height,
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
        let edit_w = client_w - MARGIN * 2;

        let mut first_edit = HWND::default();

        for (i, field) in fields.iter().enumerate() {
            let field_y = MARGIN + i as i32 * FIELD_HEIGHT;

            // ラベル
            let label: Vec<u16> = format!("{}\0", field.label).encode_utf16().collect();
            let _ = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                windows::core::w!("STATIC"),
                windows::core::PCWSTR(label.as_ptr()),
                WS_CHILD | WS_VISIBLE,
                MARGIN,
                field_y,
                edit_w,
                20,
                Some(hwnd),
                None,
                None,
                None,
            );

            // EDIT
            let mut style = WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32;
            if field.integer_only {
                style |= ES_NUMBER as u32;
            }
            let edit_hwnd = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                windows::core::w!("EDIT"),
                None,
                WINDOW_STYLE(style),
                MARGIN,
                field_y + 20,
                edit_w,
                24,
                Some(hwnd),
                Some(HMENU((ID_EDIT_BASE + i as u16) as *mut _)),
                None,
                None,
            )
            .unwrap_or_default();

            let default_text: Vec<u16> = format!("{}\0", field.default).encode_utf16().collect();
            let _ = SetWindowTextW(edit_hwnd, windows::core::PCWSTR(default_text.as_ptr()));
            SendMessageW(edit_hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1isize)));

            if i == 0 {
                first_edit = edit_hwnd;
            }
        }

        // ボタン行
        let button_y = MARGIN + fields.len() as i32 * FIELD_HEIGHT + MARGIN;
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
        if !first_edit.is_invalid() {
            let _ = SetFocus(Some(first_edit));
        }

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

        data.results
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
                        let mut results = Vec::new();
                        for i in 0..data.field_count {
                            let edit = GetDlgItem(Some(hwnd), (ID_EDIT_BASE + i as u16) as i32)
                                .unwrap_or_default();
                            results.push(read_edit_text(edit));
                        }
                        data.results = Some(results);
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

unsafe fn read_edit_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len == 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

unsafe fn get_dialog_data(hwnd: HWND) -> Option<&'static mut DialogData> {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogData;
        if ptr.is_null() { None } else { Some(&mut *ptr) }
    }
}
