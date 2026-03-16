//! 「送る」(SendTo) メニューへのショートカット登録
//!
//! SHGetKnownFolderPath(FOLDERID_SendTo) でフォルダを取得し、
//! IShellLink COMでショートカット (.lnk) を作成する。

use anyhow::{Context as _, Result};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
use windows::Win32::UI::Shell::{FOLDERID_SendTo, IShellLinkW, SHGetKnownFolderPath, ShellLink};
use windows::core::Interface;

const LNK_NAME: &str = "ぐらびゅ3.lnk";

/// 「送る」にショートカットを登録する
pub fn register() -> Result<()> {
    let sendto_dir = get_sendto_path()?;
    let lnk_path = sendto_dir.join(LNK_NAME);
    let exe = std::env::current_exe().context("exe パス取得失敗")?;

    unsafe {
        // IShellLink COMオブジェクト作成
        let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .context("IShellLink作成失敗")?;

        // ショートカットのターゲットを設定
        let wide_exe: Vec<u16> = exe
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        shell_link.SetPath(windows::core::PCWSTR(wide_exe.as_ptr()))?;

        // 説明
        let desc = "ぐらびゅ3で開く";
        let wide_desc: Vec<u16> = desc.encode_utf16().chain(std::iter::once(0)).collect();
        shell_link.SetDescription(windows::core::PCWSTR(wide_desc.as_ptr()))?;

        // IPersistFileで保存
        let persist_file: IPersistFile = shell_link.cast()?;
        let wide_lnk: Vec<u16> = lnk_path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        persist_file.Save(windows::core::PCWSTR(wide_lnk.as_ptr()), true)?;
    }

    Ok(())
}

/// 「送る」からショートカットを削除する
pub fn unregister() -> Result<()> {
    let sendto_dir = get_sendto_path()?;
    let lnk_path = sendto_dir.join(LNK_NAME);
    if lnk_path.exists() {
        std::fs::remove_file(&lnk_path)
            .with_context(|| format!("ショートカット削除失敗: {}", lnk_path.display()))?;
    }
    Ok(())
}

/// SendToフォルダのパスを取得する
fn get_sendto_path() -> Result<std::path::PathBuf> {
    unsafe {
        let pwstr = SHGetKnownFolderPath(&FOLDERID_SendTo, Default::default(), None)
            .context("SendToフォルダ取得失敗")?;
        let path = pwstr.to_string().context("SendToパス変換失敗")?;
        windows::Win32::System::Com::CoTaskMemFree(Some(pwstr.0 as *const _));
        Ok(std::path::PathBuf::from(path))
    }
}
