use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer, ShowCursor};

/// カーソル自動非表示のタイマーID
pub const TIMER_ID_CURSOR_HIDE: usize = 1;
/// カーソル非表示までの遅延 (ミリ秒)
const CURSOR_HIDE_DELAY_MS: u32 = 3000;

/// フルスクリーン時のカーソル自動非表示
pub struct CursorHider {
    /// カーソルが非表示状態か
    hidden: bool,
    /// カーソル自動非表示が機能しているか (Num-でトグル)
    enabled: bool,
}

impl CursorHider {
    pub fn new() -> Self {
        Self {
            hidden: false,
            enabled: true,
        }
    }

    /// マウス移動時に呼ぶ (カーソル復帰 + タイマーリセット)
    pub fn on_mouse_move(&mut self, hwnd: HWND) {
        if !self.enabled {
            return;
        }
        self.show_cursor();
        // タイマーをリセット (SetTimerは同一IDなら上書き)
        unsafe {
            let _ = SetTimer(Some(hwnd), TIMER_ID_CURSOR_HIDE, CURSOR_HIDE_DELAY_MS, None);
        }
    }

    /// タイマー発火時に呼ぶ (カーソル非表示)
    pub fn on_timer(&mut self, hwnd: HWND) {
        if !self.enabled {
            return;
        }
        self.hide_cursor();
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_ID_CURSOR_HIDE);
        }
    }

    /// カーソル自動非表示が機能しているか
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 有効/無効をトグル
    pub fn toggle_enabled(&mut self, hwnd: HWND) {
        self.enabled = !self.enabled;
        if !self.enabled {
            // 無効化時にカーソルを復帰させてタイマーも停止
            self.show_cursor();
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_ID_CURSOR_HIDE);
            }
        }
    }

    /// フルスクリーン解除時に確実にカーソルを復帰させる
    pub fn force_show(&mut self, hwnd: HWND) {
        self.show_cursor();
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_ID_CURSOR_HIDE);
        }
    }

    fn show_cursor(&mut self) {
        if self.hidden {
            unsafe {
                ShowCursor(true);
            }
            self.hidden = false;
        }
    }

    fn hide_cursor(&mut self) {
        if !self.hidden {
            unsafe {
                ShowCursor(false);
            }
            self.hidden = true;
        }
    }
}
