//! Win32 Shell APIによるファイル操作 + ダイアログ

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{
    FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST,
    FOS_PICKFOLDERS, IFileOpenDialog, IFileSaveDialog,
};

use crate::util::to_wide;

/// IFileOperation がキャンセルされたことを示す HRESULT (HRESULT_FROM_WIN32(ERROR_CANCELLED))
const ERROR_CANCELLED_HRESULT: u32 = 0x800704C7;

/// IFileOperationによるファイル削除 (ごみ箱経由)
pub fn delete_to_recycle_bin(hwnd: HWND, paths: &[&Path]) -> Result<bool> {
    if paths.is_empty() {
        return Ok(false);
    }
    unsafe {
        let op = create_file_operation(hwnd, FOFLAG_ALLOWUNDO | FOFLAG_WANTNUKEWARNING)?;
        for path in paths {
            let item = create_shell_item_strict(path)?;
            op.DeleteItem(&item, None)
                .context("DeleteItemの設定に失敗しました")?;
        }
        perform_operations(&op, "ファイル削除に失敗しました")
    }
}

/// IFileOperationによるファイル移動 (複数→ディレクトリ)
pub fn move_files(hwnd: HWND, paths: &[&Path], dest: &Path) -> Result<bool> {
    if paths.is_empty() {
        return Ok(false);
    }
    unsafe {
        let op = create_file_operation(hwnd, FOFLAG_ALLOWUNDO)?;
        let dest_folder = create_shell_item_strict(dest)?;
        for path in paths {
            let item = create_shell_item_strict(path)?;
            op.MoveItem(&item, &dest_folder, None, None)
                .context("MoveItemの設定に失敗しました")?;
        }
        perform_operations(
            &op,
            &format!("ファイル移動に失敗しました\n  dest: {}", dest.display()),
        )
    }
}

/// IFileOperationによるファイルコピー(複数→ディレクトリ)
pub fn copy_files(hwnd: HWND, paths: &[&Path], dest: &Path) -> Result<bool> {
    if paths.is_empty() {
        return Ok(false);
    }
    unsafe {
        let op = create_file_operation(hwnd, FOFLAG_ALLOWUNDO)?;
        let dest_folder = create_shell_item_strict(dest)?;
        for path in paths {
            let item = create_shell_item_strict(path)?;
            op.CopyItem(&item, &dest_folder, None, None)
                .context("CopyItemの設定に失敗しました")?;
        }
        perform_operations(&op, "ファイルコピーに失敗しました")
    }
}

/// 単一ファイルの移動 (リネーム対応)
/// 移動先の親ディレクトリをIShellItemにし、ファイル名をMoveItemの引数で指定する
pub fn move_single_file(hwnd: HWND, src: &Path, dest: &Path) -> Result<bool> {
    unsafe {
        let op = create_file_operation(hwnd, FOFLAG_ALLOWUNDO)?;
        let src_item = create_shell_item_strict(src)?;
        let dest_parent = dest
            .parent()
            .context("移動先の親ディレクトリが存在しない")?;
        let dest_folder = create_shell_item_strict(dest_parent)?;
        let new_name = dest.file_name().context("移動先のファイル名が存在しない")?;
        let new_name_wide: Vec<u16> = new_name.encode_wide().chain(std::iter::once(0)).collect();
        op.MoveItem(
            &src_item,
            &dest_folder,
            windows::core::PCWSTR(new_name_wide.as_ptr()),
            None,
        )
        .context("MoveItemの設定に失敗しました")?;
        perform_operations(
            &op,
            &format!(
                "ファイル移動に失敗しました\n  src: {}\n  dest: {}",
                src.display(),
                dest.display()
            ),
        )
    }
}

/// ファイル選択ダイアログ (IFileOpenDialog)
pub fn open_file_dialog(hwnd: HWND, initial_dir: Option<&Path>) -> Result<Option<PathBuf>> {
    unsafe {
        let dialog: IFileOpenDialog = windows::Win32::System::Com::CoCreateInstance(
            &windows::Win32::UI::Shell::FileOpenDialog,
            None,
            windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
        )
        .context("FileOpenDialog作成失敗")?;

        let options = dialog.GetOptions()?;
        dialog.SetOptions(options | FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST | FOS_PATHMUSTEXIST)?;

        // 初期ディレクトリ設定
        if let Some(dir) = initial_dir
            && let Some(item) = create_shell_item(dir)
        {
            dialog.SetFolder(&item)?;
        }

        // 画像ファイルフィルタ
        let filter_name: Vec<u16> = "画像ファイル\0".encode_utf16().collect();
        let filter_spec: Vec<u16> = "*.jpg;*.jpeg;*.png;*.gif;*.bmp;*.webp;*.tga;*.tiff;*.ico\0"
            .encode_utf16()
            .collect();
        let all_name: Vec<u16> = "すべてのファイル\0".encode_utf16().collect();
        let all_spec: Vec<u16> = "*.*\0".encode_utf16().collect();

        let filters = [
            windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC {
                pszName: windows::core::PCWSTR(filter_name.as_ptr()),
                pszSpec: windows::core::PCWSTR(filter_spec.as_ptr()),
            },
            windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC {
                pszName: windows::core::PCWSTR(all_name.as_ptr()),
                pszSpec: windows::core::PCWSTR(all_spec.as_ptr()),
            },
        ];
        dialog.SetFileTypes(&filters)?;

        match dialog.Show(Some(hwnd)) {
            Ok(()) => {}
            Err(e) if e.code().0 as u32 == ERROR_CANCELLED_HRESULT => return Ok(None), // ユーザーキャンセル
            Err(e) => return Err(e.into()),
        }

        let result = dialog.GetResult()?;
        let path_raw = result.GetDisplayName(windows::Win32::UI::Shell::SIGDN_FILESYSPATH)?;
        let path = PathBuf::from(path_raw.to_string()?);
        windows::Win32::System::Com::CoTaskMemFree(Some(path_raw.0 as *const _));
        Ok(Some(path))
    }
}

/// フォルダ選択ダイアログ (IFileOpenDialog + FOS_PICKFOLDERS)
pub fn open_folder_dialog(hwnd: HWND, initial_dir: Option<&Path>) -> Result<Option<PathBuf>> {
    select_folder_dialog(hwnd, "フォルダを開く", initial_dir)
}

/// フォルダ選択ダイアログ (移動/コピー先選択用)
pub fn select_folder_dialog(
    hwnd: HWND,
    title: &str,
    initial_dir: Option<&Path>,
) -> Result<Option<PathBuf>> {
    unsafe {
        let dialog: IFileOpenDialog = windows::Win32::System::Com::CoCreateInstance(
            &windows::Win32::UI::Shell::FileOpenDialog,
            None,
            windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
        )
        .context("FileOpenDialog作成失敗")?;

        let options = dialog.GetOptions()?;
        dialog.SetOptions(options | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_PICKFOLDERS)?;

        let title_wide = to_wide(title);
        dialog.SetTitle(windows::core::PCWSTR(title_wide.as_ptr()))?;

        // 初期ディレクトリ設定
        if let Some(dir) = initial_dir
            && let Some(item) = create_shell_item(dir)
        {
            dialog.SetFolder(&item)?;
        }

        match dialog.Show(Some(hwnd)) {
            Ok(()) => {}
            Err(e) if e.code().0 as u32 == ERROR_CANCELLED_HRESULT => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        let result = dialog.GetResult()?;
        let path_raw = result.GetDisplayName(windows::Win32::UI::Shell::SIGDN_FILESYSPATH)?;
        let path = PathBuf::from(path_raw.to_string()?);
        windows::Win32::System::Com::CoTaskMemFree(Some(path_raw.0 as *const _));
        Ok(Some(path))
    }
}

/// `save_file_dialog` の設定パラメータ。
///
/// - `default_name`: デフォルトファイル名 (拡張子付きを推奨)。
/// - `filter_name` / `filter_ext`: ファイル種別フィルタの表示名と spec (`"*.png"` 等)。
/// - `default_ext`: 拡張子文字列 (`"png"` のように先頭ドット無し)。ユーザーがファイル名
///   から拡張子を削除した場合に Windows 側で自動補完される。補完が不要な場合 (`*.*`
///   フィルタなど) は空文字を渡す。
/// - `initial_dir`: 指定すればそのフォルダを初期表示する。
/// - `title` / `ok_button_label`: ダイアログのタイトルと OK ボタンラベルのカスタマイズ。
#[derive(Default)]
pub struct SaveFileDialogParams<'a> {
    pub default_name: &'a str,
    pub filter_name: &'a str,
    pub filter_ext: &'a str,
    pub default_ext: &'a str,
    pub initial_dir: Option<&'a Path>,
    pub title: Option<&'a str>,
    pub ok_button_label: Option<&'a str>,
}

/// 保存先ダイアログ (`IFileSaveDialog`) を表示してユーザーにパスを選択させる。
pub fn save_file_dialog(hwnd: HWND, params: SaveFileDialogParams<'_>) -> Result<Option<PathBuf>> {
    let SaveFileDialogParams {
        default_name,
        filter_name,
        filter_ext,
        default_ext,
        initial_dir,
        title,
        ok_button_label,
    } = params;
    unsafe {
        let dialog: IFileSaveDialog = windows::Win32::System::Com::CoCreateInstance(
            &windows::Win32::UI::Shell::FileSaveDialog,
            None,
            windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
        )
        .context("FileSaveDialog作成失敗")?;

        let options = dialog.GetOptions()?;
        dialog.SetOptions(options | FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT)?;

        // タイトル・OKボタンラベルのカスタマイズ
        if let Some(t) = title {
            let wide = to_wide(t);
            dialog.SetTitle(windows::core::PCWSTR(wide.as_ptr()))?;
        }
        if let Some(label) = ok_button_label {
            let wide = to_wide(label);
            dialog.SetOkButtonLabel(windows::core::PCWSTR(wide.as_ptr()))?;
        }

        // 初期ディレクトリ設定
        if let Some(dir) = initial_dir
            && let Some(item) = create_shell_item(dir)
        {
            dialog.SetFolder(&item)?;
        }

        // フィルタ設定
        let fname = to_wide(filter_name);
        let fspec = to_wide(filter_ext);
        let filters = [windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC {
            pszName: windows::core::PCWSTR(fname.as_ptr()),
            pszSpec: windows::core::PCWSTR(fspec.as_ptr()),
        }];
        dialog.SetFileTypes(&filters)?;

        // デフォルト拡張子の補完設定 (ユーザーがファイル名から拡張子を削除した場合に Windows
        // 側で自動補完される)。先頭ドット無しの拡張子文字列を渡す必要がある。
        let ext_wide;
        if !default_ext.is_empty() {
            ext_wide = to_wide(default_ext);
            dialog.SetDefaultExtension(windows::core::PCWSTR(ext_wide.as_ptr()))?;
        }

        // デフォルトファイル名
        let name_wide = to_wide(default_name);
        dialog.SetFileName(windows::core::PCWSTR(name_wide.as_ptr()))?;

        match dialog.Show(Some(hwnd)) {
            Ok(()) => {}
            Err(e) if e.code().0 as u32 == ERROR_CANCELLED_HRESULT => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        let result = dialog.GetResult()?;
        let path_raw = result.GetDisplayName(windows::Win32::UI::Shell::SIGDN_FILESYSPATH)?;
        let path = PathBuf::from(path_raw.to_string()?);
        windows::Win32::System::Com::CoTaskMemFree(Some(path_raw.0 as *const _));
        Ok(Some(path))
    }
}

/// ブックマーク読み込みダイアログ (.gvbmフィルタ + bookmarksフォルダ初期表示)
pub fn open_bookmark_dialog(hwnd: HWND) -> Result<Option<PathBuf>> {
    unsafe {
        let dialog: IFileOpenDialog = windows::Win32::System::Com::CoCreateInstance(
            &windows::Win32::UI::Shell::FileOpenDialog,
            None,
            windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
        )
        .context("FileOpenDialog作成失敗")?;

        let options = dialog.GetOptions()?;
        dialog.SetOptions(options | FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST | FOS_PATHMUSTEXIST)?;

        // 初期ディレクトリ: bookmarksフォルダ
        let bookmark_dir = crate::bookmark::bookmark_dir();
        let _ = std::fs::create_dir_all(&bookmark_dir);
        if let Some(item) = create_shell_item(&bookmark_dir) {
            dialog.SetFolder(&item)?;
        }

        // フィルタ: ブックマーク + すべてのファイル
        let filter_name: Vec<u16> = "ぐらびゅブックマーク\0".encode_utf16().collect();
        let filter_spec: Vec<u16> = "*.gvbm;*.gv3bm;*.gvb\0".encode_utf16().collect();
        let all_name: Vec<u16> = "すべてのファイル\0".encode_utf16().collect();
        let all_spec: Vec<u16> = "*.*\0".encode_utf16().collect();

        let filters = [
            windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC {
                pszName: windows::core::PCWSTR(filter_name.as_ptr()),
                pszSpec: windows::core::PCWSTR(filter_spec.as_ptr()),
            },
            windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC {
                pszName: windows::core::PCWSTR(all_name.as_ptr()),
                pszSpec: windows::core::PCWSTR(all_spec.as_ptr()),
            },
        ];
        dialog.SetFileTypes(&filters)?;

        match dialog.Show(Some(hwnd)) {
            Ok(()) => {}
            Err(e) if e.code().0 as u32 == ERROR_CANCELLED_HRESULT => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        let result = dialog.GetResult()?;
        let path_raw = result.GetDisplayName(windows::Win32::UI::Shell::SIGDN_FILESYSPATH)?;
        let path = PathBuf::from(path_raw.to_string()?);
        windows::Win32::System::Com::CoTaskMemFree(Some(path_raw.0 as *const _));
        Ok(Some(path))
    }
}

// --- IFileOperation ヘルパー ---

use windows::Win32::UI::Shell::FILEOPERATION_FLAGS;

/// IFileOperationのフラグ定数
const FOFLAG_ALLOWUNDO: FILEOPERATION_FLAGS = FILEOPERATION_FLAGS(0x0040);
const FOFLAG_WANTNUKEWARNING: FILEOPERATION_FLAGS = FILEOPERATION_FLAGS(0x4000);

/// IFileOperationを作成し、親ウィンドウとフラグを設定する
unsafe fn create_file_operation(
    hwnd: HWND,
    flags: FILEOPERATION_FLAGS,
) -> Result<windows::Win32::UI::Shell::IFileOperation> {
    unsafe {
        let op: windows::Win32::UI::Shell::IFileOperation =
            windows::Win32::System::Com::CoCreateInstance(
                &windows::Win32::UI::Shell::FileOperation,
                None,
                windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
            )
            .context("IFileOperation作成失敗")?;
        op.SetOwnerWindow(hwnd)?;
        op.SetOperationFlags(flags)?;
        Ok(op)
    }
}

/// IFileOperationの操作を実行し、結果を返す
/// 成功時はtrue、ユーザーキャンセル時はfalseを返す
unsafe fn perform_operations(
    op: &windows::Win32::UI::Shell::IFileOperation,
    error_msg: &str,
) -> Result<bool> {
    unsafe {
        op.PerformOperations().context(error_msg.to_string())?;
        let aborted = op.GetAnyOperationsAborted()?;
        Ok(!aborted.as_bool())
    }
}

/// SHCreateItemFromParsingNameでIShellItemを取得する (エラー時はResult)
/// ファイル操作用: 対象パスが存在しない場合はエラーとして扱う
fn create_shell_item_strict(path: &Path) -> Result<windows::Win32::UI::Shell::IShellItem> {
    let path = crate::util::strip_extended_length_prefix(path);
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        windows::Win32::UI::Shell::SHCreateItemFromParsingName(
            windows::core::PCWSTR(wide.as_ptr()),
            None,
        )
        .with_context(|| format!("ShellItem作成失敗: {}", path.display()))
    }
}

/// SHCreateItemFromParsingNameでIShellItemを取得するヘルパー
/// ダイアログの初期フォルダ設定用: 失敗時はNoneを返す
fn create_shell_item(dir: &Path) -> Option<windows::Win32::UI::Shell::IShellItem> {
    unsafe {
        let dir_wide: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        windows::Win32::UI::Shell::SHCreateItemFromParsingName(
            windows::core::PCWSTR(dir_wide.as_ptr()),
            None,
        )
        .ok()
    }
}
