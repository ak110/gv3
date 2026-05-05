//! ファイル操作アクション群
//!
//! 削除、移動、コピー、マーク操作、クリップボード貼り付けなど。

use std::path::{Path, PathBuf};

use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;

use super::AppWindow;

impl AppWindow {
    pub(crate) fn action_delete_file(&mut self) {
        // コンテナ内 (アーカイブ/PDF) のファイル削除は無効
        if let Some(source) = self.document.current_source()
            && source.is_contained()
        {
            return;
        }
        if let Some(path) = self.document.current_path().map(Path::to_path_buf) {
            if let Ok(true) = crate::file_ops::delete_to_recycle_bin(self.hwnd, &[&path]) {
                self.document.remove_current_from_list();
                self.process_document_events();
            }
            // Shell APIがフォーカスを奪うことがあるため復帰
            unsafe {
                let _ = SetForegroundWindow(self.hwnd);
            }
        }
    }

    pub(crate) fn action_move_file(&mut self) {
        let Some(current) = self.document.file_list().current() else {
            return;
        };
        let source = current.source.clone();
        let path = current.path.clone();

        // PDFページ・未展開コンテナは移動不可
        if matches!(
            source,
            crate::file_info::FileSource::PdfPage { .. }
                | crate::file_info::FileSource::PendingContainer { .. }
        ) {
            return;
        }

        let initial_dir = source.parent_dir().map(Path::to_path_buf);
        let default_name = source.default_save_name();

        // ファイルソースに応じてダイアログのラベルを分岐
        let (dialog_title, dialog_button) = match &source {
            crate::file_info::FileSource::File(_) => ("ファイルを移動", "移動"),
            _ => ("ファイルを保存", "保存"),
        };

        if let Ok(Some(dest)) = crate::file_ops::save_file_dialog(
            self.hwnd,
            crate::file_ops::SaveFileDialogParams {
                default_name: &default_name,
                filter_name: "すべてのファイル",
                filter_ext: "*.*",
                initial_dir: initial_dir.as_deref(),
                title: Some(dialog_title),
                ok_button_label: Some(dialog_button),
                ..Default::default()
            },
        ) {
            match &source {
                crate::file_info::FileSource::File(_) => {
                    // 通常ファイル: SHFileOperationWでUndo対応の移動
                    match crate::file_ops::move_single_file(self.hwnd, &path, &dest) {
                        Ok(true) => {
                            if let Err(e) = self.document.rename_current_in_list(&dest) {
                                self.show_error_title(&format!("リストの更新に失敗しました: {e}"));
                            }
                            self.process_document_events();
                        }
                        Ok(false) => {} // ユーザーキャンセル
                        Err(e) => {
                            self.show_error_title(&format!("ファイルの移動に失敗しました: {e}"));
                        }
                    }
                    // Shell APIがフォーカスを奪うことがあるため復帰
                    unsafe {
                        let _ = SetForegroundWindow(self.hwnd);
                    }
                }
                crate::file_info::FileSource::ArchiveEntry { on_demand, .. } => {
                    // アーカイブエントリ: 保存 (リスト除去なし)
                    let result = if *on_demand {
                        self.document.read_file_data_current().and_then(|data| {
                            std::fs::write(&dest, &data).map_err(anyhow::Error::from)
                        })
                    } else {
                        std::fs::copy(&path, &dest)
                            .map(|_| ())
                            .map_err(anyhow::Error::from)
                    };
                    if let Err(e) = result {
                        self.show_error_title(&format!("ファイルの保存に失敗しました: {e}"));
                    }
                }
                crate::file_info::FileSource::PdfPage { .. }
                | crate::file_info::FileSource::PendingContainer { .. } => {
                    unreachable!(); // 上でガード済み
                }
            }
        }
    }

    pub(crate) fn action_copy_file(&mut self) {
        if let Some(current) = self.document.file_list().current() {
            let default_name = current.source.default_save_name();
            let initial_dir = current.source.parent_dir().map(Path::to_path_buf);
            if let Ok(Some(dest)) = crate::file_ops::save_file_dialog(
                self.hwnd,
                crate::file_ops::SaveFileDialogParams {
                    default_name: &default_name,
                    filter_name: "すべてのファイル",
                    filter_ext: "*.*",
                    initial_dir: initial_dir.as_deref(),
                    title: Some("ファイルを複製"),
                    ok_button_label: Some("複製"),
                    ..Default::default()
                },
            ) {
                let result = if matches!(
                    current.source,
                    crate::file_info::FileSource::ArchiveEntry {
                        on_demand: true,
                        ..
                    }
                ) {
                    // オンデマンド: アーカイブから読み込んで保存
                    self.document
                        .read_file_data_current()
                        .and_then(|data| std::fs::write(&dest, &data).map_err(anyhow::Error::from))
                } else {
                    // 通常ファイル/temp展開済み/PDF: 既存のfs::copy
                    std::fs::copy(&current.path, &dest)
                        .map(|_| ())
                        .map_err(anyhow::Error::from)
                };
                if let Err(e) = result {
                    self.show_error_title(&format!("ファイルのコピーに失敗しました: {e}"));
                }
            }
        }
    }

    pub(crate) fn action_marked_delete(&mut self) {
        // コンテナ内 (アーカイブ/PDF) は無効
        if let Some(source) = self.document.current_source()
            && source.is_contained()
        {
            return;
        }
        let paths: Vec<PathBuf> = self
            .document
            .file_list()
            .marked_indices()
            .iter()
            .map(|&i| self.document.file_list().files()[i].path.clone())
            .collect();
        let path_refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        if let Ok(true) = crate::file_ops::delete_to_recycle_bin(self.hwnd, &path_refs) {
            self.document.remove_marked_from_list();
            self.process_document_events();
        }
        // Shell APIがフォーカスを奪うことがあるため復帰
        unsafe {
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    pub(crate) fn action_marked_move(&mut self) {
        if let Some(source) = self.document.current_source()
            && source.is_contained()
        {
            return;
        }
        let marked = self.document.file_list().marked_indices();
        let paths: Vec<PathBuf> = marked
            .iter()
            .map(|&i| self.document.file_list().files()[i].path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }
        let initial_dir = self.document.file_list().files()[marked[0]]
            .source
            .parent_dir()
            .map(Path::to_path_buf);
        if let Ok(Some(dest)) = crate::file_ops::select_folder_dialog(
            self.hwnd,
            "移動先フォルダ",
            initial_dir.as_deref(),
        ) {
            let path_refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
            if let Ok(true) = crate::file_ops::move_files(self.hwnd, &path_refs, &dest) {
                // パス更新失敗時は従来通りリストから削除 (フォールバック)
                if let Err(e) = self.document.update_marked_paths(&dest) {
                    eprintln!("パス更新失敗、リストから削除: {e}");
                    self.document.remove_marked_from_list();
                }
                self.process_document_events();
            }
            // Shell APIがフォーカスを奪うことがあるため復帰
            unsafe {
                let _ = SetForegroundWindow(self.hwnd);
            }
        }
    }

    pub(crate) fn action_marked_copy(&mut self) {
        let marked = self.document.file_list().marked_indices();
        let paths: Vec<PathBuf> = marked
            .iter()
            .map(|&i| self.document.file_list().files()[i].path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }
        let initial_dir = self.document.file_list().files()[marked[0]]
            .source
            .parent_dir()
            .map(Path::to_path_buf);
        if let Ok(Some(dest)) = crate::file_ops::select_folder_dialog(
            self.hwnd,
            "コピー先フォルダ",
            initial_dir.as_deref(),
        ) {
            let path_refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
            if let Err(e) = crate::file_ops::copy_files(self.hwnd, &path_refs, &dest) {
                self.show_error_title(&format!("ファイルのコピーに失敗しました: {e}"));
            }
            // Shell APIがフォーカスを奪うことがあるため復帰
            unsafe {
                let _ = SetForegroundWindow(self.hwnd);
            }
        }
    }

    pub(crate) fn action_marked_copy_names(&mut self) {
        let names: Vec<String> = self
            .document
            .file_list()
            .marked_indices()
            .iter()
            .map(|&i| self.document.file_list().files()[i].source.display_path())
            .collect();
        if !names.is_empty() {
            let text = names.join("\r\n");
            if let Err(e) = crate::clipboard::copy_text_to_clipboard(self.hwnd, &text) {
                self.show_error_title(&format!("マークファイル名のコピーに失敗しました: {e}"));
            }
        }
    }

    pub(crate) fn action_paste_image(&mut self) {
        if !self.guard_unsaved_edit() {
            return;
        }
        self.selection.deselect();
        match crate::clipboard::paste_image_from_clipboard(self.hwnd) {
            Ok(Some(image)) => {
                // 一時ファイルに保存してから開く
                let temp_path = std::env::temp_dir().join("gv_clipboard.png");
                if let Some(img_buf) =
                    image::RgbaImage::from_raw(image.width, image.height, image.data.clone())
                    && img_buf.save(&temp_path).is_ok()
                {
                    if let Err(e) = self.document.open_single(&temp_path) {
                        self.show_error_title(&format!("貼り付けに失敗しました: {e}"));
                    }
                    self.process_document_events();
                }
            }
            Ok(None) => {} // クリップボードに画像なし
            Err(e) => self.show_error_title(&format!("貼り付け失敗: {e}")),
        }
    }
}
