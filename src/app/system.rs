//! システム操作アクション群
//!
//! アップデート確認、シェル統合登録・解除、各種フォルダ・ホームページを開く操作。

use windows::Win32::UI::WindowsAndMessaging::*;

use super::AppWindow;

impl AppWindow {
    /// アップデート確認・実行
    pub(crate) fn check_for_update(&mut self) {
        // WaitCursor表示
        let prev_cursor = unsafe { SetCursor(LoadCursorW(None, IDC_WAIT).ok()) };

        let result = crate::updater::check_for_update();

        // カーソル復元
        unsafe {
            let _ = SetCursor(Some(prev_cursor));
        }

        match result {
            Err(e) => unsafe {
                crate::util::show_message_box(
                    self.hwnd,
                    "アップデート確認",
                    &format!("更新の確認に失敗しました:\n{e}"),
                    MB_OK | MB_ICONERROR,
                );
            },
            Ok(info) if !info.is_newer => unsafe {
                crate::util::show_message_box(
                    self.hwnd,
                    "アップデート確認",
                    &format!("最新バージョンです (v{})", info.current_version),
                    MB_OK | MB_ICONINFORMATION,
                );
            },
            Ok(info) => {
                let answer = unsafe {
                    crate::util::show_message_box(
                        self.hwnd,
                        "アップデート確認",
                        &format!(
                            "v{} が利用可能です (現在: v{})。\n更新しますか？",
                            info.latest_version, info.current_version
                        ),
                        MB_YESNO | MB_ICONQUESTION,
                    )
                };

                if answer == IDYES {
                    // WaitCursor表示
                    let prev = unsafe { SetCursor(LoadCursorW(None, IDC_WAIT).ok()) };

                    match crate::updater::perform_update(&info) {
                        Ok(true) => {
                            // バッチスクリプト起動成功 → アプリ終了
                            unsafe {
                                let _ = SetCursor(Some(prev));
                                let _ = DestroyWindow(self.hwnd);
                            }
                        }
                        Ok(false) => unsafe {
                            let _ = SetCursor(Some(prev));
                        },
                        Err(e) => {
                            unsafe {
                                let _ = SetCursor(Some(prev));
                            }
                            unsafe {
                                crate::util::show_message_box(
                                    self.hwnd,
                                    "アップデート",
                                    &format!("更新に失敗しました:\n{e:?}"),
                                    MB_OK | MB_ICONERROR,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// シェル統合 (ファイル関連付け・コンテキストメニュー・「送る」) を登録
    pub(crate) fn action_register_shell(&self) {
        let answer = unsafe {
            crate::util::show_message_box(
                self.hwnd,
                "シェル統合",
                "ファイル関連付け・コンテキストメニュー・「送る」を登録しますか？",
                MB_YESNO | MB_ICONQUESTION,
            )
        };
        if answer != IDYES {
            return;
        }

        match crate::shell::register_all() {
            Ok(()) => unsafe {
                crate::util::show_message_box(
                    self.hwnd,
                    "シェル統合",
                    "シェル統合を登録しました。",
                    MB_OK | MB_ICONINFORMATION,
                );
            },
            Err(e) => unsafe {
                crate::util::show_message_box(
                    self.hwnd,
                    "シェル統合",
                    &format!("シェル統合の登録に失敗しました:\n{e}"),
                    MB_OK | MB_ICONERROR,
                );
            },
        }
    }

    /// シェル統合 (ファイル関連付け・コンテキストメニュー・「送る」) を解除
    pub(crate) fn action_unregister_shell(&self) {
        let answer = unsafe {
            crate::util::show_message_box(
                self.hwnd,
                "シェル統合",
                "ファイル関連付け・コンテキストメニュー・「送る」を解除しますか？",
                MB_YESNO | MB_ICONQUESTION,
            )
        };
        if answer != IDYES {
            return;
        }

        match crate::shell::unregister_all() {
            Ok(()) => unsafe {
                crate::util::show_message_box(
                    self.hwnd,
                    "シェル統合",
                    "シェル統合を解除しました。",
                    MB_OK | MB_ICONINFORMATION,
                );
            },
            Err(e) => unsafe {
                crate::util::show_message_box(
                    self.hwnd,
                    "シェル統合",
                    &format!("シェル統合の解除に失敗しました:\n{e}"),
                    MB_OK | MB_ICONERROR,
                );
            },
        }
    }

    pub(crate) fn action_open_exe_folder(&mut self) {
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            self.open_in_explorer(dir);
        }
    }

    pub(crate) fn action_open_bookmark_folder(&mut self) {
        let dir = crate::bookmark::bookmark_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            // ディレクトリがすでにある場合は無視されるため、到達するのは権限不足等。
            // explorer.exe 起動側でも失敗するため致命的にせず警告のみ。
            eprintln!(
                "警告: ブックマークディレクトリ作成失敗: {} ({e})",
                dir.display()
            );
        }
        self.open_in_explorer(&dir);
    }

    pub(crate) fn action_open_spi_folder(&mut self) {
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            let spi_dir = dir.join("spi");
            if let Err(e) = std::fs::create_dir_all(&spi_dir) {
                eprintln!(
                    "警告: spi ディレクトリ作成失敗: {} ({e})",
                    spi_dir.display()
                );
            }
            self.open_in_explorer(&spi_dir);
        }
    }

    pub(crate) fn action_open_temp_folder(&mut self) {
        let dir = std::env::temp_dir();
        self.open_in_explorer(&dir);
    }

    pub(crate) fn action_open_homepage(&mut self) {
        let url = windows::core::w!("https://github.com/ak110/gv");
        let result = unsafe {
            windows::Win32::UI::Shell::ShellExecuteW(
                Some(self.hwnd),
                windows::core::PCWSTR::null(),
                url,
                windows::core::PCWSTR::null(),
                windows::core::PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        // ShellExecuteW は戻り値が32以下の場合エラー（WinSDK仕様）
        let code = result.0 as isize;
        if code <= 32 {
            self.show_error_title(&format!("ブラウザの起動に失敗しました: {code}"));
        }
    }
}
