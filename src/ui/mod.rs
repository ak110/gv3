pub mod cursor_hider;
pub mod file_list_panel;
pub mod filter_dialog;
pub mod font;
pub mod fullscreen;
pub mod info_dialog;
pub mod key_config;
pub mod menu;
pub mod page_dialog;
pub mod resize_dialog;
pub mod rotate_dialog;
pub mod window;

use std::mem::size_of;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

/// 親ウィンドウの中心座標を計算し、モニターのワーク領域内にclampする
pub fn center_on_parent(parent: HWND, width: i32, height: i32) -> (i32, i32) {
    let mut rect = windows::Win32::Foundation::RECT::default();
    unsafe {
        let _ = GetWindowRect(parent, std::ptr::from_mut(&mut rect));
    }
    let cx = i32::midpoint(rect.left, rect.right) - width / 2;
    let cy = i32::midpoint(rect.top, rect.bottom) - height / 2;

    // 親ウィンドウが属するモニターのワーク領域内にclamp
    let monitor = unsafe { MonitorFromWindow(parent, MONITOR_DEFAULTTONEAREST) };
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, std::ptr::from_mut(&mut mi)) }.as_bool() {
        let rc = mi.rcWork;
        // ダイアログがモニターより大きい場合はワーク領域の左上に寄せる
        let x = cx.clamp(rc.left, (rc.right - width).max(rc.left));
        let y = cy.clamp(rc.top, (rc.bottom - height).max(rc.top));
        (x, y)
    } else {
        (cx, cy)
    }
}
