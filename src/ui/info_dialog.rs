//! 情報表示ダイアログ（画像情報・ヘルプ用）
//!
//! MessageBoxWの代わりに、readonly EDIT コントロールを持つモーダルダイアログを表示する。
//! テキストの選択・コピーが可能。

use std::sync::Once;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_BTNFACE, HFONT, UpdateWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

/// ダイアログの内部状態
struct DialogData {
    closed: bool,
}

/// ウィンドウクラス登録（一度だけ）
static REGISTER_ONCE: Once = Once::new();
const CLASS_NAME: &str = "gv_info_dialog\0";

// 子コントロールID
const ID_EDIT: u16 = 0x200;

/// ダイアログのデフォルトサイズ
const DIALOG_WIDTH: i32 = 640;
const DIALOG_HEIGHT: i32 = 480;

/// 情報ダイアログを表示する（モーダル）
///
/// `parent`: 親ウィンドウ
/// `title`: ダイアログタイトル
/// `text`: 表示テキスト
/// `font`: EDITコントロールに適用するフォント（HFONTが無効ならデフォルトフォント）
pub fn show_info_dialog(parent: HWND, title: &str, text: &str, font: HFONT) {
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

        // ダイアログデータ
        let mut data = DialogData { closed: false };
        let data_ptr = std::ptr::from_mut(&mut data);

        // 親ウィンドウの中心にダイアログを配置
        let (x, y) = super::center_on_parent(parent, DIALOG_WIDTH, DIALOG_HEIGHT);

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
            DIALOG_HEIGHT,
            Some(parent),
            None,
            None,
            Some(data_ptr as *const _),
        )
        .unwrap_or_default();

        if hwnd.is_invalid() {
            return;
        }

        // クライアント領域の実サイズを取得してEDITコントロールを配置
        let mut client_rect = windows::Win32::Foundation::RECT::default();
        let _ = GetClientRect(hwnd, std::ptr::from_mut(&mut client_rect));
        let client_w = client_rect.right - client_rect.left;
        let client_h = client_rect.bottom - client_rect.top;
        let edit_hwnd = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            windows::core::w!("EDIT"),
            None,
            WINDOW_STYLE(
                WS_CHILD.0
                    | WS_VISIBLE.0
                    | WS_VSCROLL.0
                    | ES_MULTILINE as u32
                    | ES_READONLY as u32
                    | ES_AUTOVSCROLL as u32,
            ),
            0,
            0,
            client_w,
            client_h,
            Some(hwnd),
            Some(HMENU(ID_EDIT as *mut _)),
            None,
            None,
        )
        .unwrap_or_default();

        // テキストを設定
        let text_wide: Vec<u16> = text
            .replace('\n', "\r\n")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let _ = SetWindowTextW(edit_hwnd, windows::core::PCWSTR(text_wide.as_ptr()));

        // フォント適用
        if !font.is_invalid() {
            SendMessageW(
                edit_hwnd,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }

        // 表示
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);

        // モーダルループ（data.closedはWndProc内でポインタ経由で変更される）
        let _ = EnableWindow(parent, false);
        let mut msg = MSG::default();
        #[allow(clippy::while_immutable_condition)]
        while !data.closed {
            let ret = GetMessageW(std::ptr::from_mut(&mut msg), None, 0, 0);
            if !ret.as_bool() {
                break;
            }
            // Tabキーナビゲーション対応
            if IsDialogMessageW(hwnd, std::ptr::from_ref(&msg)).as_bool() {
                continue;
            }
            let _ = TranslateMessage(std::ptr::from_ref(&msg));
            DispatchMessageW(std::ptr::from_ref(&msg));
        }
        let _ = EnableWindow(parent, true);
        let _ = SetForegroundWindow(parent);

        // ダイアログウィンドウが残っていれば破棄
        if IsWindow(Some(hwnd)).as_bool() {
            let _ = DestroyWindow(hwnd);
        }
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
                // CREATESTRUCT.lpCreateParamsからDialogDataポインタを取得してGWLP_USERDATAに保存
                let cs = &*(lparam.0 as *const CREATESTRUCTW);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
                return LRESULT(0);
            }
            WM_COMMAND => {
                let control_id = (wparam.0 as u32) & 0xFFFF;
                // Esc（IsDialogMessageWがIDCANCEL=2に変換）
                if control_id == 2 {
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

/// GWLP_USERDATAからDialogDataを取得
unsafe fn get_dialog_data(hwnd: HWND) -> Option<&'static mut DialogData> {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogData;
        if ptr.is_null() { None } else { Some(&mut *ptr) }
    }
}
