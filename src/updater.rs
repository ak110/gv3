//! ネットワーク更新機能
//!
//! GitHubリリースから最新版を取得し、バッチスクリプト経由でexeを置換する。

use std::path::PathBuf;

use anyhow::{Context as _, Result};

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
    let extracted = if info.download_url.ends_with(".zip") {
        extract_files_from_zip(&download_path, &temp_dir)?
    } else {
        // 直接exeの場合
        let dest = temp_dir.join("gv3_update.exe");
        std::fs::rename(&download_path, &dest).context("ダウンロードファイルのリネーム失敗")?;
        ExtractedFiles {
            exe_path: dest,
            extra_files: Vec::new(),
        }
    };

    // バッチスクリプト生成・起動
    let batch_path = temp_dir.join("gv3_update.bat");
    generate_update_batch(
        &batch_path,
        &extracted.exe_path,
        &exe_path,
        &extracted.extra_files,
        std::process::id(),
    )?;
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

/// ZIP展開結果
struct ExtractedFiles {
    /// 新しいgv3.exe のパス
    exe_path: PathBuf,
    /// exe以外のファイル（展開先パス）のリスト
    extra_files: Vec<PathBuf>,
}

/// ZIPから全ファイルを展開する
fn extract_files_from_zip(
    zip_path: &std::path::Path,
    temp_dir: &std::path::Path,
) -> Result<ExtractedFiles> {
    let file = std::fs::File::open(zip_path).context("ZIPファイルオープン失敗")?;
    let mut archive = zip::ZipArchive::new(file).context("ZIPアーカイブ読み込み失敗")?;

    let mut exe_path = None;
    let mut extra_files = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).context("ZIPエントリ取得失敗")?;
        if entry.is_dir() {
            continue;
        }
        // ファイル名部分のみ取得（ZIPのパスにディレクトリが含まれる場合に対応）
        let file_name = std::path::Path::new(entry.name())
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if file_name.is_empty() {
            continue;
        }

        let lower = file_name.to_lowercase();
        if lower == "gv3.exe" {
            let dest = temp_dir.join("gv3_update.exe");
            let mut out = std::fs::File::create(&dest).context("展開先ファイル作成失敗")?;
            std::io::copy(&mut entry, &mut out).context("ZIPエントリ展開失敗")?;
            exe_path = Some(dest);
        } else {
            // exe以外のファイル（*.default.toml, README.md, LICENSE等）
            let dest = temp_dir.join(&file_name);
            let mut out = std::fs::File::create(&dest).context("展開先ファイル作成失敗")?;
            std::io::copy(&mut entry, &mut out).context("ZIPエントリ展開失敗")?;
            extra_files.push(dest);
        }
    }

    let exe_path = exe_path.ok_or_else(|| anyhow::anyhow!("ZIP内にgv3.exeが見つかりません"))?;
    Ok(ExtractedFiles {
        exe_path,
        extra_files,
    })
}

/// 更新用バッチスクリプトを生成する
///
/// rename-then-replaceパターン:
/// 1. 親プロセスの終了を待機
/// 2. 実行中のexeを.oldにリネーム（Windowsはリネームを許可する）
/// 3. 新しいexeを本来の名前で配置
/// 4. 新exeを起動
///
/// `cleanup_old_exe()`が次回起動時に.oldを削除する。
fn generate_update_batch(
    batch_path: &std::path::Path,
    update_exe: &std::path::Path,
    target_exe: &std::path::Path,
    extra_files: &[PathBuf],
    pid: u32,
) -> Result<()> {
    let old_exe = target_exe.with_extension("exe.old");
    let target_dir = target_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("exeの親ディレクトリ取得失敗"))?;

    // exe以外のファイルのコピーコマンドを生成
    let extra_copy_commands = extra_files
        .iter()
        .map(|src| {
            let file_name = src.file_name().unwrap().to_string_lossy();
            let dest = target_dir.join(file_name.as_ref());
            format!(r#"copy /y "{}" "{}""#, src.display(), dest.display())
        })
        .collect::<Vec<_>>()
        .join("\r\n");

    // cmd.exeはCP_ACPでバッチを読むため、CP932でエンコードして書き出す。
    // また、if ( ... ) ブロック内に日本語があるとDBCSトレイルバイトが
    // 特殊文字と誤認されるため、( ) ブロックを使わず goto で制御する。
    let content = format!(
        r#"@echo off
title gv3 update

echo gv3 を更新しています...
echo.

:wait_exit
tasklist /fi "PID eq {pid}" 2>nul | find "{pid}" >nul
if %errorlevel% neq 0 goto wait_done
echo アプリケーションの終了を待機中...
timeout /t 1 /nobreak >nul
goto wait_exit
:wait_done

echo アプリケーション終了確認

:rename
if exist "{old}" del /f "{old}"
rename "{target}" "{old_name}"
if %errorlevel% equ 0 goto rename_ok
echo.
echo エラー: gv3.exe のリネームに失敗しました。
echo アプリケーションがまだ実行中の可能性があります。
echo.
echo 何かキーを押すとリトライします...
pause >nul
goto rename
:rename_ok

echo リネーム完了

move /y "{update}" "{target}"
if %errorlevel% equ 0 goto move_ok
echo.
echo エラー: 新しい gv3.exe の配置に失敗しました。
echo ロールバック中...
rename "{old}" "{target_name}"
echo.
echo 何かキーを押すとリトライします...
pause >nul
goto rename
:move_ok

{extra_copy_commands}

echo.
echo 更新が完了しました。gv3 を起動します...
start "" "{target}"
del "%~f0" & exit
"#,
        pid = pid,
        update = update_exe.display(),
        target = target_exe.display(),
        old = old_exe.display(),
        old_name = old_exe.file_name().unwrap().to_string_lossy(),
        target_name = target_exe.file_name().unwrap().to_string_lossy(),
        extra_copy_commands = extra_copy_commands,
    );

    // Rustのformat!はLFのみ出力するが、cmd.exeのDBCSパーサーは
    // LF-only改行でバイト位置がずれるため、CRLFに変換する
    let content = content.replace('\n', "\r\n");
    let encoded = utf8_to_ansi(&content);
    std::fs::write(batch_path, encoded).context("バッチスクリプト書き込み失敗")
}

/// バッチスクリプトを起動する（コンソールウィンドウ表示）
fn launch_batch(batch_path: &std::path::Path) -> Result<()> {
    use std::os::windows::process::CommandExt;

    // ジョブオブジェクトから分離を試みる（ベストエフォート）
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;
    let result = std::process::Command::new("cmd.exe")
        .args(["/k", &batch_path.display().to_string()])
        .creation_flags(CREATE_BREAKAWAY_FROM_JOB)
        .spawn();

    match result {
        Ok(_) => Ok(()),
        Err(_) => {
            // ブレイクアウェイ不可の場合はフラグなしで起動
            std::process::Command::new("cmd.exe")
                .args(["/k", &batch_path.display().to_string()])
                .spawn()
                .context("バッチスクリプト起動失敗")?;
            Ok(())
        }
    }
}

/// UTF-8文字列をシステムのANSIコードページ（日本語環境ではCP932）に変換する
fn utf8_to_ansi(s: &str) -> Vec<u8> {
    use windows::Win32::Globalization::{CP_ACP, WideCharToMultiByte};

    let wide: Vec<u16> = s.encode_utf16().collect();
    if wide.is_empty() {
        return Vec::new();
    }

    unsafe {
        let len = WideCharToMultiByte(CP_ACP, Default::default(), &wide, None, None, None);
        if len == 0 {
            return s.as_bytes().to_vec();
        }
        let mut buf = vec![0u8; len as usize];
        WideCharToMultiByte(
            CP_ACP,
            Default::default(),
            &wide,
            Some(&mut buf),
            None,
            None,
        );
        // null終端が含まれていれば除去
        if buf.last() == Some(&0) {
            buf.pop();
        }
        buf
    }
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

    /// CP932バッチのバイト列から、テストに不要な行を無効化する。
    /// ASCIIプレフィクスで行を特定するためCP932でも安全に動作する。
    fn neutralize_batch_line(bytes: &[u8], ascii_prefix: &[u8]) -> Vec<u8> {
        let crlf = b"\r\n";
        let mut result = Vec::new();
        let mut pos = 0;
        while pos < bytes.len() {
            // 行末(次のCRLFまたはEOF)を探す
            let line_end = bytes[pos..]
                .windows(2)
                .position(|w| w == crlf)
                .map(|p| pos + p)
                .unwrap_or(bytes.len());
            let line = &bytes[pos..line_end];

            if line.starts_with(ascii_prefix) {
                // "rem " + 元の行でコメントアウト
                result.extend_from_slice(b"rem ");
                result.extend_from_slice(line);
            } else {
                result.extend_from_slice(line);
            }

            if line_end + 2 <= bytes.len() {
                result.extend_from_slice(crlf);
                pos = line_end + 2;
            } else {
                pos = bytes.len();
            }
        }
        result
    }

    #[test]
    fn batch_execution_renames_and_moves_files() {
        // テスト用ディレクトリとダミーファイルを作成
        let dir = std::env::temp_dir().join("gv3_test_batch_exec");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let target = dir.join("gv3.exe");
        let update = dir.join("gv3_update.exe");
        std::fs::write(&target, b"OLD_CONTENT").unwrap();
        std::fs::write(&update, b"NEW_CONTENT").unwrap();

        // 存在しないPIDでバッチ生成（wait_exitを即通過）
        let batch_path = dir.join("update.bat");
        generate_update_batch(&batch_path, &update, &target, &[], 1).unwrap();

        // start と del 行を無効化してテスト用バッチを作成
        let bytes = std::fs::read(&batch_path).unwrap();
        let bytes = neutralize_batch_line(&bytes, b"start ");
        let bytes = neutralize_batch_line(&bytes, b"del ");
        let test_batch = dir.join("update_test.bat");
        std::fs::write(&test_batch, bytes).unwrap();

        // バッチ実行
        let output = std::process::Command::new("cmd.exe")
            .args(["/c", &test_batch.display().to_string()])
            .output()
            .expect("cmd.exe実行失敗");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        // ファイル操作の結果を検証
        let old = target.with_extension("exe.old");
        assert!(
            old.exists(),
            "gv3.exe.old が存在するべき\nstdout: {stdout}\nstderr: {stderr}"
        );
        assert_eq!(
            std::fs::read(&old).unwrap(),
            b"OLD_CONTENT",
            "gv3.exe.old の中身は元のexeであるべき"
        );
        assert!(
            target.exists(),
            "gv3.exe が存在するべき（moveで配置される）\nstdout: {stdout}\nstderr: {stderr}"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"NEW_CONTENT",
            "gv3.exe の中身は新しいexeであるべき"
        );
        assert!(!update.exists(), "gv3_update.exe は move で消えているべき");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_execution_cleans_up_existing_old() {
        // .old が既に存在する場合に削除してからリネームすることを確認
        let dir = std::env::temp_dir().join("gv3_test_batch_old");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let target = dir.join("gv3.exe");
        let update = dir.join("gv3_update.exe");
        let old = target.with_extension("exe.old");
        std::fs::write(&target, b"CURRENT").unwrap();
        std::fs::write(&update, b"UPDATED").unwrap();
        std::fs::write(&old, b"STALE_OLD").unwrap();

        let batch_path = dir.join("update.bat");
        generate_update_batch(&batch_path, &update, &target, &[], 1).unwrap();

        let bytes = std::fs::read(&batch_path).unwrap();
        let bytes = neutralize_batch_line(&bytes, b"start ");
        let bytes = neutralize_batch_line(&bytes, b"del ");
        let test_batch = dir.join("update_test.bat");
        std::fs::write(&test_batch, bytes).unwrap();

        let output = std::process::Command::new("cmd.exe")
            .args(["/c", &test_batch.display().to_string()])
            .output()
            .expect("cmd.exe実行失敗");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(
            std::fs::read(&old).unwrap(),
            b"CURRENT",
            "gv3.exe.old は現在のexeであるべき（古い.oldは削除済み）\nstdout: {stdout}\nstderr: {stderr}"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"UPDATED",
            "gv3.exe は新しいexeであるべき"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_execution_copies_extra_files() {
        // exe更新と同時に追加ファイル（*.default.toml, README.md等）もコピーされることを確認
        let base_dir = std::env::temp_dir().join("gv3_test_batch_extra");
        let _ = std::fs::remove_dir_all(&base_dir);
        // exeのあるディレクトリと、展開先の一時ディレクトリを分離
        let install_dir = base_dir.join("install");
        let temp_dir = base_dir.join("temp");
        std::fs::create_dir_all(&install_dir).unwrap();
        std::fs::create_dir_all(&temp_dir).unwrap();

        let target = install_dir.join("gv3.exe");
        let update = temp_dir.join("gv3_update.exe");
        std::fs::write(&target, b"OLD_EXE").unwrap();
        std::fs::write(&update, b"NEW_EXE").unwrap();

        // 追加ファイルを一時ディレクトリに作成（ZIP展開後の状態を再現）
        let extra1 = temp_dir.join("gv3.default.toml");
        let extra2 = temp_dir.join("README.md");
        std::fs::write(&extra1, b"NEW_TOML").unwrap();
        std::fs::write(&extra2, b"NEW_README").unwrap();

        // 既存の追加ファイルも配置（上書きされることを確認）
        std::fs::write(install_dir.join("gv3.default.toml"), b"OLD_TOML").unwrap();
        std::fs::write(install_dir.join("README.md"), b"OLD_README").unwrap();

        let batch_path = temp_dir.join("update.bat");
        generate_update_batch(
            &batch_path,
            &update,
            &target,
            &[extra1.clone(), extra2.clone()],
            1,
        )
        .unwrap();

        let bytes = std::fs::read(&batch_path).unwrap();
        let bytes = neutralize_batch_line(&bytes, b"start ");
        let bytes = neutralize_batch_line(&bytes, b"del ");
        let test_batch = temp_dir.join("update_test.bat");
        std::fs::write(&test_batch, bytes).unwrap();

        let output = std::process::Command::new("cmd.exe")
            .args(["/c", &test_batch.display().to_string()])
            .output()
            .expect("cmd.exe実行失敗");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        // exe が更新されていること
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"NEW_EXE",
            "gv3.exe が更新されるべき\nstdout: {stdout}\nstderr: {stderr}"
        );

        // 追加ファイルがコピーされていること
        assert_eq!(
            std::fs::read(install_dir.join("gv3.default.toml")).unwrap(),
            b"NEW_TOML",
            "gv3.default.toml が更新されるべき\nstdout: {stdout}\nstderr: {stderr}"
        );
        assert_eq!(
            std::fs::read(install_dir.join("README.md")).unwrap(),
            b"NEW_README",
            "README.md が更新されるべき\nstdout: {stdout}\nstderr: {stderr}"
        );

        let _ = std::fs::remove_dir_all(&base_dir);
    }
}
