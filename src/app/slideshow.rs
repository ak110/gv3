//! スライドショー関連機能
//!
//! タイマー駆動のスライドショー実行、間隔調整など。

use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use super::AppWindow;

/// スライドショー用タイマーID
pub(super) const TIMER_ID_SLIDESHOW: usize = 2;

impl AppWindow {
    /// スライドショーをトグル
    pub(crate) fn toggle_slideshow(&mut self) {
        if self.slideshow_active {
            self.stop_slideshow();
        } else {
            self.start_slideshow();
        }
    }

    /// スライドショー開始
    pub(crate) fn start_slideshow(&mut self) {
        if self.document.file_list().len() < 2 {
            return;
        }
        self.slideshow_active = true;
        unsafe {
            let _ = SetTimer(
                Some(self.hwnd),
                TIMER_ID_SLIDESHOW,
                self.slideshow_interval_ms,
                None,
            );
        }
    }

    /// スライドショー停止
    pub(crate) fn stop_slideshow(&mut self) {
        if !self.slideshow_active {
            return;
        }
        self.slideshow_active = false;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_ID_SLIDESHOW);
        }
    }

    /// スライドショーのタイマー発火時
    pub(crate) fn on_slideshow_timer(&mut self) {
        if !self.slideshow_active {
            return;
        }
        // 最後の画像に到達しているか確認
        let at_end = self
            .document
            .file_list()
            .current_index()
            .is_some_and(|idx| idx + 1 >= self.document.file_list().len());

        if at_end {
            if self.slideshow_repeat {
                self.document.navigate_first();
                self.process_document_events();
            } else {
                self.stop_slideshow();
            }
            return;
        }
        self.document.navigate_relative(1);
        self.process_document_events();
    }

    /// スライドショー間隔を変更 (最小500ms、最大30000ms)
    pub(crate) fn adjust_slideshow_interval(&mut self, delta_ms: i32) {
        let new_val =
            (i64::from(self.slideshow_interval_ms) + i64::from(delta_ms)).clamp(500, 30_000) as u32;
        self.slideshow_interval_ms = new_val;
        // 実行中ならタイマーを新しい間隔で再設定 (同一IDは上書き)
        if self.slideshow_active {
            unsafe {
                let _ = SetTimer(
                    Some(self.hwnd),
                    TIMER_ID_SLIDESHOW,
                    self.slideshow_interval_ms,
                    None,
                );
            }
        }
    }
}
