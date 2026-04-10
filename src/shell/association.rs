//! ファイル関連付け登録 (OpenWithProgids方式)
//!
//! 既存の既定アプリを上書きせず、「プログラムから開く」候補にぐらびゅを追加する安全な方式。
//! - HKCU\Software\Classes\gv.ImageFile  → 画像用ProgID
//! - HKCU\Software\Classes\gv.ArchiveFile → アーカイブ用ProgID
//! - 各拡張子の OpenWithProgids に上記ProgIDを追加

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};

/// exeの絶対パスを返す
fn exe_path() -> Result<String> {
    let path = std::env::current_exe().context("exe パス取得失敗")?;
    Ok(path.to_string_lossy().into_owned())
}

use crate::util::to_wide;

/// レジストリキーを再帰的に削除する
pub(super) fn delete_key_tree(hkey: HKEY, subkey: &str) -> Result<()> {
    let result = unsafe { RegDeleteTreeW(hkey, windows::core::PCWSTR(to_wide(subkey).as_ptr())) };
    if result != ERROR_SUCCESS && result != windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
        anyhow::bail!("レジストリキー削除失敗: {subkey} (error: {result:?})");
    }
    Ok(())
}

/// レジストリキーを作成してデフォルト値を設定する
pub(super) fn set_key_value(root: HKEY, subkey: &str, value: &str) -> Result<()> {
    let wide_key = to_wide(subkey);
    let wide_val = to_wide(value);

    // RegCreateKeyWでキーを作成 (存在すれば開く)
    let mut hkey = HKEY::default();
    let result = unsafe {
        RegCreateKeyW(
            root,
            windows::core::PCWSTR(wide_key.as_ptr()),
            std::ptr::from_mut(&mut hkey),
        )
    };
    if result != ERROR_SUCCESS {
        anyhow::bail!("レジストリキー作成失敗: {subkey}");
    }

    // デフォルト値を設定
    let result = unsafe {
        RegSetValueExW(
            hkey,
            None,
            None,
            REG_SZ,
            Some(std::slice::from_raw_parts(
                wide_val.as_ptr().cast::<u8>(),
                wide_val.len() * 2,
            )),
        )
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    if result != ERROR_SUCCESS {
        anyhow::bail!("レジストリ値設定失敗: {subkey}");
    }
    Ok(())
}

/// 拡張子のOpenWithProgidsにProgIDを追加する
fn add_open_with_progid(extension: &str, progid: &str) -> Result<()> {
    let subkey = format!(r"Software\Classes\{extension}\OpenWithProgids");
    let wide_key = to_wide(&subkey);
    let wide_progid = to_wide(progid);

    let mut hkey = HKEY::default();
    let result = unsafe {
        RegCreateKeyW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(wide_key.as_ptr()),
            std::ptr::from_mut(&mut hkey),
        )
    };
    if result != ERROR_SUCCESS {
        anyhow::bail!("OpenWithProgids キー作成失敗: {extension}");
    }

    // 空のREG_NONE値を設定 (値の存在がProgID登録を意味する)
    let result = unsafe {
        RegSetValueExW(
            hkey,
            windows::core::PCWSTR(wide_progid.as_ptr()),
            None,
            REG_NONE,
            None,
        )
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    if result != ERROR_SUCCESS {
        anyhow::bail!("OpenWithProgids 値設定失敗: {extension} -> {progid}");
    }
    Ok(())
}

/// 拡張子のOpenWithProgidsからProgIDを削除する
fn remove_open_with_progid(extension: &str, progid: &str) {
    let subkey = format!(r"Software\Classes\{extension}\OpenWithProgids");

    let mut hkey = HKEY::default();
    let result = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(to_wide(&subkey).as_ptr()),
            None,
            KEY_SET_VALUE,
            std::ptr::from_mut(&mut hkey),
        )
    };
    if result != ERROR_SUCCESS {
        return; // キーがなければ何もしない
    }

    let _ = unsafe { RegDeleteValueW(hkey, windows::core::PCWSTR(to_wide(progid).as_ptr())) };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
}

/// 画像拡張子リスト
const IMAGE_EXTENSIONS: &[&str] = &[".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp"];

/// アーカイブ拡張子リスト (コミック用のみ関連付け)
const ARCHIVE_EXTENSIONS: &[&str] = &[".cbz", ".cbr"];

/// 汎用アーカイブ拡張子 (関連付け解除のみ、登録はしない)
const GENERIC_ARCHIVE_EXTENSIONS: &[&str] = &[".zip", ".rar", ".7z"];

const IMAGE_PROGID: &str = "gv.ImageFile";
const ARCHIVE_PROGID: &str = "gv.ArchiveFile";
const BOOKMARK_PROGID: &str = "gv.Bookmark";

// 旧ProgID(マイグレーション用)
const OLD_IMAGE_PROGID: &str = "gv.ImageFile";
const OLD_ARCHIVE_PROGID: &str = "gv.ArchiveFile";
const OLD_BOOKMARK_PROGID: &str = "gv3.Bookmark";

/// ブックマーク拡張子
const BOOKMARK_EXTENSION: &str = ".gvbm";
/// 旧ブックマーク拡張子 (後方互換)
const OLD_BOOKMARK_EXTENSION: &str = ".gv3bm";
/// 旧 C++ 実装のブックマーク拡張子
const LEGACY_BOOKMARK_EXTENSION: &str = ".gvb";

/// ファイル関連付けを登録する
pub fn register() -> Result<()> {
    // 旧ProgIDのクリーンアップ
    let _ = cleanup_old_progids();

    let exe = exe_path()?;

    // 画像用ProgID
    let progid_key = format!(r"Software\Classes\{IMAGE_PROGID}");
    set_key_value(HKEY_CURRENT_USER, &progid_key, "ぐらびゅ 画像ファイル")?;
    set_key_value(
        HKEY_CURRENT_USER,
        &format!(r"{progid_key}\shell\open\command"),
        &format!("\"{exe}\" \"%1\""),
    )?;

    // アーカイブ用ProgID
    let progid_key = format!(r"Software\Classes\{ARCHIVE_PROGID}");
    set_key_value(
        HKEY_CURRENT_USER,
        &progid_key,
        "ぐらびゅ アーカイブファイル",
    )?;
    set_key_value(
        HKEY_CURRENT_USER,
        &format!(r"{progid_key}\shell\open\command"),
        &format!("\"{exe}\" \"%1\""),
    )?;

    // DefaultIcon設定 (リソースID: 0=icon1, 1=icon2, 2=icon3)
    set_key_value(
        HKEY_CURRENT_USER,
        &format!(r"Software\Classes\{IMAGE_PROGID}\DefaultIcon"),
        &format!("\"{exe}\",1"),
    )?;
    set_key_value(
        HKEY_CURRENT_USER,
        &format!(r"Software\Classes\{ARCHIVE_PROGID}\DefaultIcon"),
        &format!("\"{exe}\",2"),
    )?;

    // ブックマーク用ProgID (.gv3bm)
    let bm_key = format!(r"Software\Classes\{BOOKMARK_PROGID}");
    set_key_value(HKEY_CURRENT_USER, &bm_key, "ぐらびゅ ブックマーク")?;
    set_key_value(
        HKEY_CURRENT_USER,
        &format!(r"{bm_key}\shell\open\command"),
        &format!("\"{exe}\" \"%1\""),
    )?;
    set_key_value(
        HKEY_CURRENT_USER,
        &format!(r"{bm_key}\DefaultIcon"),
        &format!("\"{exe}\",2"),
    )?;

    // 汎用アーカイブの既存関連付けをクリーンアップ (再登録時に古い関連付けが残らないように)
    for ext in GENERIC_ARCHIVE_EXTENSIONS {
        remove_open_with_progid(ext, ARCHIVE_PROGID);
    }

    // 各拡張子にOpenWithProgidsを登録
    for ext in IMAGE_EXTENSIONS {
        add_open_with_progid(ext, IMAGE_PROGID)?;
    }
    for ext in ARCHIVE_EXTENSIONS {
        add_open_with_progid(ext, ARCHIVE_PROGID)?;
    }
    add_open_with_progid(BOOKMARK_EXTENSION, BOOKMARK_PROGID)?;
    // 旧ブックマーク拡張子も新ProgIDに関連付け (既存ファイルを開けるように)
    add_open_with_progid(OLD_BOOKMARK_EXTENSION, BOOKMARK_PROGID)?;
    add_open_with_progid(LEGACY_BOOKMARK_EXTENSION, BOOKMARK_PROGID)?;

    Ok(())
}

/// 旧ProgID (gv3.*) を削除する
fn cleanup_old_progids() -> Result<()> {
    // 旧ProgIDキーを削除
    delete_key_tree(
        HKEY_CURRENT_USER,
        &format!(r"Software\Classes\{OLD_IMAGE_PROGID}"),
    )?;
    delete_key_tree(
        HKEY_CURRENT_USER,
        &format!(r"Software\Classes\{OLD_ARCHIVE_PROGID}"),
    )?;
    delete_key_tree(
        HKEY_CURRENT_USER,
        &format!(r"Software\Classes\{OLD_BOOKMARK_PROGID}"),
    )?;

    // 各拡張子のOpenWithProgidsから旧ProgIDを削除
    for ext in IMAGE_EXTENSIONS {
        remove_open_with_progid(ext, OLD_IMAGE_PROGID);
    }
    for ext in ARCHIVE_EXTENSIONS {
        remove_open_with_progid(ext, OLD_ARCHIVE_PROGID);
    }
    for ext in GENERIC_ARCHIVE_EXTENSIONS {
        remove_open_with_progid(ext, OLD_ARCHIVE_PROGID);
    }
    remove_open_with_progid(OLD_BOOKMARK_EXTENSION, OLD_BOOKMARK_PROGID);
    Ok(())
}

/// ファイル関連付けを解除する
pub fn unregister() -> Result<()> {
    // 旧ProgIDのクリーンアップ
    let _ = cleanup_old_progids();

    // ProgIDキーを削除
    delete_key_tree(
        HKEY_CURRENT_USER,
        &format!(r"Software\Classes\{IMAGE_PROGID}"),
    )?;
    delete_key_tree(
        HKEY_CURRENT_USER,
        &format!(r"Software\Classes\{ARCHIVE_PROGID}"),
    )?;
    delete_key_tree(
        HKEY_CURRENT_USER,
        &format!(r"Software\Classes\{BOOKMARK_PROGID}"),
    )?;

    // 各拡張子のOpenWithProgidsからProgIDを削除
    for ext in IMAGE_EXTENSIONS {
        remove_open_with_progid(ext, IMAGE_PROGID);
    }
    for ext in ARCHIVE_EXTENSIONS {
        remove_open_with_progid(ext, ARCHIVE_PROGID);
    }
    for ext in GENERIC_ARCHIVE_EXTENSIONS {
        remove_open_with_progid(ext, ARCHIVE_PROGID);
    }
    remove_open_with_progid(BOOKMARK_EXTENSION, BOOKMARK_PROGID);
    remove_open_with_progid(OLD_BOOKMARK_EXTENSION, BOOKMARK_PROGID);
    remove_open_with_progid(LEGACY_BOOKMARK_EXTENSION, BOOKMARK_PROGID);

    Ok(())
}

/// シェルに関連付け変更を通知する
pub fn notify_shell() {
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}
