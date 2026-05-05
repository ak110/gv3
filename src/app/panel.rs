//! ファイルリストパネルと UI 状態管理
//!
//! パネルの通知処理、メニュー更新、画面同期など。

use std::collections::HashSet;

use windows::Win32::Foundation::{LPARAM, LRESULT};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::Controls::{
    LVIF_STATE, LVIF_TEXT, LVIS_SELECTED, LVN_GETDISPINFOW, LVN_ITEMCHANGED, NMHDR, NMLISTVIEW,
    NMLVDISPINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::HMENU;

use crate::ui::file_list_panel::FileListPanel;
use crate::ui::key_config::Action;
use crate::ui::menu;

use super::AppWindow;

impl AppWindow {
    /// DocumentEvent を処理する
    pub(crate) fn process_document_events(&mut self) {
        // 先読みレスポンスを処理 (キャッシュ格納 + current_image更新)
        self.document.process_prefetch_responses();

        // バックグラウンドコンテナ展開結果を処理
        self.document.process_expand_results();

        // FileListChanged は同一poll内で複数届きうる (バックグラウンド統合バッチごと等)。
        // パネル更新コストを抑えるため、ループ中はフラグだけ設定し、ループ抜け後に1回だけ
        // panel.update() を呼ぶ。
        let mut file_list_changed = false;
        let mut nav_changed_index: Option<usize> = None;
        while let Ok(event) = self.event_receiver.try_recv() {
            match event {
                crate::document::DocumentEvent::ImageReady => unsafe {
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                },
                crate::document::DocumentEvent::NavigationChanged { index, .. } => {
                    nav_changed_index = Some(index);
                }
                crate::document::DocumentEvent::FileListChanged => {
                    self.stop_slideshow();
                    file_list_changed = true;
                }
                crate::document::DocumentEvent::Error(msg) => {
                    self.show_error_title(&msg);
                }
            }
        }

        if file_list_changed {
            let count = self.document.file_list().len();
            self.file_list_panel.update(count);
            self.update_title();
        }
        if let Some(index) = nav_changed_index {
            self.update_title();
            self.file_list_panel.set_selection(index);
        }

        // パネル表示中ならキャッシュ状態の差分のみ更新 (該当行のみ再描画)
        if self.file_list_panel.is_visible() {
            let doc = &self.document;
            let len = doc.file_list().len();
            // 現在のキャッシュ状態をスナップショット (上限6件程度なので軽量)
            let mut new_cached = HashSet::new();
            for i in 0..len {
                if doc.is_cached(i) {
                    new_cached.insert(i);
                }
            }
            // 前回との差分だけ該当行を再描画
            for &i in self.cached_indices.symmetric_difference(&new_cached) {
                self.file_list_panel.update_item(i);
            }
            self.cached_indices = new_cached;
        }
    }

    /// メニューポップアップ表示時にトグル項目のチェック状態を更新
    pub(crate) fn update_menu_checks(&self, popup: HMENU) {
        let pf = self.document.persistent_filter();
        let pf_enabled = pf.is_enabled();

        // 永続フィルタの有効/無効チェック
        menu::update_menu_check(popup, Action::PFilterToggle, pf_enabled);

        // 各フィルタ操作のチェックマーク + フィルタ無効時はグレーアウト
        use crate::persistent_filter::FilterOperation as FO;
        let filter_actions: &[(Action, FO)] = &[
            (Action::PFilterFlipH, FO::FlipHorizontal),
            (Action::PFilterFlipV, FO::FlipVertical),
            (Action::PFilterRotate180, FO::Rotate180),
            (Action::PFilterRotate90CW, FO::Rotate90CW),
            (Action::PFilterRotate90CCW, FO::Rotate90CCW),
            (Action::PFilterLevels, FO::Levels { low: 0, high: 0 }),
            (Action::PFilterGamma, FO::Gamma { value: 0.0 }),
            (
                Action::PFilterBrightnessContrast,
                FO::BrightnessContrast {
                    brightness: 0,
                    contrast: 0,
                },
            ),
            (Action::PFilterGrayscaleSimple, FO::GrayscaleSimple),
            (Action::PFilterGrayscaleStrict, FO::GrayscaleStrict),
            (Action::PFilterBlur, FO::Blur),
            (Action::PFilterBlurStrong, FO::BlurStrong),
            (Action::PFilterSharpen, FO::Sharpen),
            (Action::PFilterSharpenStrong, FO::SharpenStrong),
            (
                Action::PFilterGaussianBlur,
                FO::GaussianBlur { radius: 0.0 },
            ),
            (Action::PFilterUnsharpMask, FO::UnsharpMask { radius: 0.0 }),
            (Action::PFilterMedianFilter, FO::MedianFilter),
            (Action::PFilterInvertColors, FO::InvertColors),
            (Action::PFilterApplyAlpha, FO::ApplyAlpha),
        ];
        for (action, probe) in filter_actions {
            menu::update_menu_check(popup, *action, pf.has_operation(probe));
            menu::update_menu_enabled(popup, *action, pf_enabled);
        }

        // その他のトグル項目
        menu::update_menu_check(
            popup,
            Action::ToggleFileList,
            self.file_list_panel.is_visible(),
        );
        menu::update_menu_check(popup, Action::ToggleAlwaysOnTop, self.always_on_top);
        menu::update_menu_check(
            popup,
            Action::ToggleMargin,
            self.renderer.layout().margin_enabled,
        );
        menu::update_menu_check(
            popup,
            Action::ToggleCursorHide,
            self.cursor_hider.is_enabled(),
        );
        menu::update_menu_check(
            popup,
            Action::ToggleFullscreen,
            self.fullscreen.is_fullscreen(),
        );
        menu::update_menu_check(popup, Action::SlideshowToggle, self.slideshow_active);
    }

    /// ファイルリストパネルの同期ヘルパー (MarkInvertAll / MarkInvertToHere で共通)
    pub(crate) fn sync_file_list_panel(&mut self) {
        if self.file_list_panel.is_visible() {
            let doc = &self.document;
            let len = doc.file_list().len();
            self.file_list_panel.update(len);
            if let Some(idx) = doc.file_list().current_index() {
                self.file_list_panel.set_selection(idx);
            }
        }
    }

    /// ファイルリストパネルからの通知を処理
    pub(crate) fn handle_file_list_notify(&mut self, nmhdr: &NMHDR, lparam: LPARAM) -> LRESULT {
        match nmhdr.code {
            // テキスト要求: 該当インデックスのラベルを ListView 提供のバッファへコピー
            i if i == LVN_GETDISPINFOW => {
                // SAFETY: LVN_GETDISPINFOW の lparam は OS が有効な NMLVDISPINFOW へのポインタを保証する
                let dispinfo = unsafe { &mut *(lparam.0 as *mut NMLVDISPINFOW) };
                if (dispinfo.item.mask & LVIF_TEXT).0 != 0 {
                    let idx = dispinfo.item.iItem as usize;
                    let files = self.document.file_list().files();
                    if let Some(info) = files.get(idx) {
                        let is_cached = self.document.is_cached(idx);
                        let label = FileListPanel::format_label(info, is_cached);
                        // UTF-16 化して、ListView 提供の pszText バッファへコピー
                        let max = dispinfo.item.cchTextMax as usize;
                        if max > 0 && !dispinfo.item.pszText.0.is_null() {
                            let mut wide: Vec<u16> = label.encode_utf16().collect();
                            // ヌル終端1文字分を残して切り詰める
                            if wide.len() >= max {
                                wide.truncate(max - 1);
                            }
                            wide.push(0);
                            // SAFETY: pszText は OS が確保した cchTextMax 文字分のバッファを指す。
                            // wide.len() <= max (cchTextMax) を上記で保証済みのため範囲内に収まる。
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    wide.as_ptr(),
                                    dispinfo.item.pszText.0,
                                    wide.len(),
                                );
                            }
                        }
                    }
                }
                LRESULT(0)
            }
            // 選択変更通知: ユーザーのクリック・マウスホイール等で iItem が変わった
            i if i == LVN_ITEMCHANGED => {
                // SAFETY: LVN_ITEMCHANGED の lparam は OS が有効な NMLISTVIEW へのポインタを保証する
                let nmlv = unsafe { &*(lparam.0 as *const NMLISTVIEW) };
                // 状態変化のうち SELECTED ビットが新たに有効になったときだけ反応
                let became_selected = (nmlv.uChanged.0 & LVIF_STATE.0) != 0
                    && (nmlv.uOldState & LVIS_SELECTED.0) == 0
                    && (nmlv.uNewState & LVIS_SELECTED.0) != 0;
                if became_selected && nmlv.iItem >= 0 {
                    let target = nmlv.iItem as usize;
                    if self.document.file_list().current_index() != Some(target) {
                        if !self.guard_unsaved_edit() {
                            // キャンセル時は選択位置を復元
                            if let Some(idx) = self.document.file_list().current_index() {
                                self.file_list_panel.set_selection(idx);
                            }
                            return LRESULT(0);
                        }
                        self.selection.deselect();
                        self.stop_slideshow();
                        self.document.navigate_to(target);
                        self.process_document_events();
                    }
                }
                LRESULT(0)
            }
            _ => LRESULT(0),
        }
    }
}
