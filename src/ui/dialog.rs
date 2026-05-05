//! ダイアログ共通基盤

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

/// GWLP_USERDATA から型付きポインタを取得する
///
/// # Safety
/// `hwnd` の GWLP_USERDATA に `T` へのポインタが格納されていること。
pub unsafe fn get_window_data<T>(hwnd: HWND) -> Option<&'static mut T> {
    // SAFETY: 呼び出し元がGWLP_USERDATAにT型ポインタを格納していることを保証する（# Safety節参照）。
    // ポインタはnullチェック後にのみデリファレンスする。
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut T;
        if ptr.is_null() { None } else { Some(&mut *ptr) }
    }
}

/// 共通モーダルループ
///
/// `hwnd` のウィンドウループを `closed` が指す値が `true` になるまで実行する。
/// 終了後に `parent` を再有効化して前面へ配置する。
///
/// # Safety
/// `hwnd` と `parent` は有効な HWND であること。
/// `closed` は有効なポインタであること。
pub unsafe fn run_modal_loop(parent: HWND, hwnd: HWND, closed: *const bool) {
    // SAFETY: 呼び出し元が `closed` に有効なポインタを渡すことを保証する（# Safety節参照）。
    // `*closed` のデリファレンスはループ条件での読み取りのみで、書き込みは行わない。
    unsafe {
        let _ = EnableWindow(parent, false);
        let mut msg = MSG::default();
        #[allow(clippy::while_immutable_condition)]
        while !*closed {
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
    }
}
