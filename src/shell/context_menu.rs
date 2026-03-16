//! エクスプローラのコンテキストメニュー（右クリックメニュー）への登録
//!
//! HKCU\Software\Classes\*\shell\gv3 に「ぐらびゅ3で開く」を登録する。

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::*;

const MENU_KEY: &str = r"Software\Classes\*\shell\gv3";

/// ワイド文字列（null終端）を作成するヘルパー
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// コンテキストメニューを登録する
pub fn register() -> Result<()> {
    let exe = std::env::current_exe().context("exe パス取得失敗")?;
    let exe_str = exe.to_string_lossy();

    // メニュー項目キーを作成し、表示名を設定
    let mut hkey = HKEY::default();
    let result = unsafe {
        RegCreateKeyW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(to_wide(MENU_KEY).as_ptr()),
            &mut hkey,
        )
    };
    if result != ERROR_SUCCESS {
        anyhow::bail!("コンテキストメニュー キー作成失敗");
    }

    let display_name = to_wide("ぐらびゅ3で開く");
    let result = unsafe {
        RegSetValueExW(
            hkey,
            None,
            None,
            REG_SZ,
            Some(std::slice::from_raw_parts(
                display_name.as_ptr() as *const u8,
                display_name.len() * 2,
            )),
        )
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    if result != ERROR_SUCCESS {
        anyhow::bail!("コンテキストメニュー 表示名設定失敗");
    }

    // command サブキー
    let cmd_key = format!(r"{MENU_KEY}\command");
    let mut hkey = HKEY::default();
    let result = unsafe {
        RegCreateKeyW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(to_wide(&cmd_key).as_ptr()),
            &mut hkey,
        )
    };
    if result != ERROR_SUCCESS {
        anyhow::bail!("コンテキストメニュー command キー作成失敗");
    }

    let command = to_wide(&format!("\"{exe_str}\" \"%1\""));
    let result = unsafe {
        RegSetValueExW(
            hkey,
            None,
            None,
            REG_SZ,
            Some(std::slice::from_raw_parts(
                command.as_ptr() as *const u8,
                command.len() * 2,
            )),
        )
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    if result != ERROR_SUCCESS {
        anyhow::bail!("コンテキストメニュー command 値設定失敗");
    }

    Ok(())
}

/// コンテキストメニューを解除する
pub fn unregister() -> Result<()> {
    let result = unsafe {
        RegDeleteTreeW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(to_wide(MENU_KEY).as_ptr()),
        )
    };
    if result != ERROR_SUCCESS && result != windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
        anyhow::bail!("コンテキストメニュー キー削除失敗 (error: {result:?})");
    }
    Ok(())
}
