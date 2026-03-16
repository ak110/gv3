//! ネットワーク更新機能
//!
//! GitHubリリースから最新版を取得し、バッチスクリプト経由でexeを置換する。

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};

const GITHUB_API_URL: &str = "https://api.github.com/repos/ak110/gv3/releases/latest";

/// 更新情報
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub download_url: String,
    pub is_newer: bool,
}

/// GitHub APIから最新リリース情報を取得し、バージョン比較する
pub fn check_for_update() -> Result<UpdateInfo> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();

    // GitHub API呼び出し
    let body = ureq::get(GITHUB_API_URL)
        .header("Accept", "application/vnd.github.v3+json")
        .header("User-Agent", &format!("gv3/{current_version}"))
        .call()
        .context("GitHub API呼び出し失敗")?
        .body_mut()
        .read_to_string()
        .context("レスポンス読み込み失敗")?;
    let response: serde_json::Value = serde_json::from_str(&body).context("JSONパース失敗")?;

    // tag_name から "v" プレフィクスを除去
    let tag = response["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("tag_nameが見つかりません"))?;
    let latest_version = tag.strip_prefix('v').unwrap_or(tag).to_string();

    // ダウンロードURL取得（gv3.exeまたは.zipアセット）
    let download_url = response["assets"]
        .as_array()
        .and_then(|assets| {
            assets.iter().find_map(|a| {
                let name = a["name"].as_str().unwrap_or("");
                if name.ends_with(".zip") || name == "gv3.exe" {
                    a["browser_download_url"].as_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| anyhow::anyhow!("ダウンロード可能なアセットが見つかりません"))?;

    let is_newer = match (
        parse_version(&current_version),
        parse_version(&latest_version),
    ) {
        (Some(cur), Some(lat)) => lat > cur,
        _ => false,
    };

    Ok(UpdateInfo {
        current_version,
        latest_version,
        download_url,
        is_newer,
    })
}

/// ダウンロード→ZIP展開→バッチスクリプト生成→起動
/// 成功すればOk(true)を返し、呼び出し元はアプリを終了する
pub fn perform_update(info: &UpdateInfo) -> Result<bool> {
    let exe_path = std::env::current_exe().context("現在のexeパス取得失敗")?;
    // ダウンロード
    let temp_dir = std::env::temp_dir().join(format!("gv3_update_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&temp_dir);

    let download_path = temp_dir.join("gv3_update_download");
    download_file(&info.download_url, &download_path)?;

    // ZIP展開またはそのまま使用
    let update_exe_path = if info.download_url.ends_with(".zip") {
        extract_exe_from_zip(&download_path, &temp_dir)?
    } else {
        // 直接exeの場合
        let dest = temp_dir.join("gv3_update.exe");
        std::fs::rename(&download_path, &dest).context("ダウンロードファイルのリネーム失敗")?;
        dest
    };

    // バッチスクリプト生成・起動
    let batch_path = temp_dir.join("gv3_update.bat");
    generate_update_batch(&batch_path, &update_exe_path, &exe_path)?;
    launch_batch(&batch_path)?;

    // 起動成功 → 呼び出し元がアプリを終了する
    Ok(true)
}

/// ファイルをダウンロードする
fn download_file(url: &str, dest: &std::path::Path) -> Result<()> {
    let data = ureq::get(url)
        .header("User-Agent", &format!("gv3/{}", env!("CARGO_PKG_VERSION")))
        .call()
        .context("ダウンロード失敗")?
        .body_mut()
        .read_to_vec()
        .context("ダウンロードデータ読み込み失敗")?;

    std::fs::write(dest, &data).context("ダウンロードファイル書き込み失敗")?;
    Ok(())
}

/// ZIPからgv3.exeを展開する
fn extract_exe_from_zip(zip_path: &std::path::Path, temp_dir: &std::path::Path) -> Result<PathBuf> {
    let file = std::fs::File::open(zip_path).context("ZIPファイルオープン失敗")?;
    let mut archive = zip::ZipArchive::new(file).context("ZIPアーカイブ読み込み失敗")?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).context("ZIPエントリ取得失敗")?;
        let name = entry.name().to_lowercase();
        if name.ends_with("gv3.exe") || name == "gv3.exe" {
            let dest = temp_dir.join("gv3_update.exe");
            let mut out = std::fs::File::create(&dest).context("展開先ファイル作成失敗")?;
            std::io::copy(&mut entry, &mut out).context("ZIPエントリ展開失敗")?;
            return Ok(dest);
        }
    }

    bail!("ZIP内にgv3.exeが見つかりません")
}

/// 更新用バッチスクリプトを生成する
fn generate_update_batch(
    batch_path: &std::path::Path,
    update_exe: &std::path::Path,
    target_exe: &std::path::Path,
) -> Result<()> {
    let content = format!(
        r#"@echo off
timeout /t 2 /nobreak >nul
move /y "{update}" "{target}"
if %errorlevel% neq 0 (
  echo 更新に失敗しました。
  pause
  exit /b 1
)
start "" "{target}"
del "%~f0"
"#,
        update = update_exe.display(),
        target = target_exe.display(),
    );

    std::fs::write(batch_path, content).context("バッチスクリプト書き込み失敗")
}

/// バッチスクリプトをバックグラウンドで起動する
fn launch_batch(batch_path: &std::path::Path) -> Result<()> {
    use std::os::windows::process::CommandExt;

    // CREATE_NO_WINDOW = 0x08000000
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    std::process::Command::new("cmd.exe")
        .args(["/c", &batch_path.display().to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("バッチスクリプト起動失敗")?;

    Ok(())
}

/// バージョン文字列を (major, minor, patch) タプルに変換
fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() >= 3 {
        Some((
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            parts[2].parse().ok()?,
        ))
    } else if parts.len() == 2 {
        Some((parts[0].parse().ok()?, parts[1].parse().ok()?, 0))
    } else {
        None
    }
}

/// 起動時にgv3.exe.oldが残っていれば削除を試みる
pub fn cleanup_old_exe() {
    if let Ok(exe) = std::env::current_exe() {
        let old = exe.with_extension("exe.old");
        if old.exists() {
            let _ = std::fs::remove_file(&old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_semver() {
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_version("10.20.30"), Some((10, 20, 30)));
    }

    #[test]
    fn parse_version_two_parts() {
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
    }

    #[test]
    fn parse_version_invalid() {
        assert_eq!(parse_version("abc"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("1"), None);
    }

    #[test]
    fn version_comparison() {
        assert!(parse_version("1.1.0").unwrap() > parse_version("1.0.0").unwrap());
        assert!(parse_version("2.0.0").unwrap() > parse_version("1.9.9").unwrap());
        assert!(parse_version("0.2.0").unwrap() > parse_version("0.1.0").unwrap());
        assert!(!(parse_version("0.1.0").unwrap() > parse_version("0.1.0").unwrap()));
    }
}
