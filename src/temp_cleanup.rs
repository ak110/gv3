//! 起動時の孤立tempフォルダクリーンアップ
//!
//! プロセス強制終了等で残った gv3_archive_* ディレクトリを検出・削除する。

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::OpenProcess;
use windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;

/// %TEMP% 配下の孤立した gv3_archive_* ディレクトリを削除する
///
/// ディレクトリ名の形式: gv3_archive_{pid}_{timestamp_ms}
/// - 自プロセスのPIDにマッチ → スキップ（自分のtempは触らない）
/// - PIDのプロセスが生存中 → スキップ（他のgv3インスタンスかもしれない）
/// - PIDのプロセスが死亡済み → 削除（孤立temp）
pub fn cleanup_orphaned_temp_dirs() {
    let temp_dir = std::env::temp_dir();
    let my_pid = std::process::id();

    let entries = match std::fs::read_dir(&temp_dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !name_str.starts_with("gv3_archive_") {
            continue;
        }

        // PID抽出: gv3_archive_{pid}_{ms}
        let Some(pid) = parse_pid_from_dir_name(name_str) else {
            continue;
        };

        // 自プロセスはスキップ
        if pid == my_pid {
            continue;
        }

        // PID生存チェック
        if is_process_alive(pid) {
            continue;
        }

        // 孤立temp → 削除（エラー無視）
        let _ = std::fs::remove_dir_all(entry.path());
    }
}

/// ディレクトリ名からPIDを抽出する
/// 形式: gv3_archive_{pid}_{timestamp_ms}
fn parse_pid_from_dir_name(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("gv3_archive_")?;
    let pid_str = rest.split('_').next()?;
    pid_str.parse().ok()
}

/// Win32 OpenProcess でプロセスの生存をチェック
fn is_process_alive(pid: u32) -> bool {
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                true
            }
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pid_valid() {
        assert_eq!(
            parse_pid_from_dir_name("gv3_archive_12345_1700000000000"),
            Some(12345)
        );
    }

    #[test]
    fn parse_pid_no_timestamp() {
        // タイムスタンプ部分がなくてもPIDは取れる
        assert_eq!(parse_pid_from_dir_name("gv3_archive_99"), Some(99));
    }

    #[test]
    fn parse_pid_invalid_prefix() {
        assert_eq!(parse_pid_from_dir_name("gv3_other_12345_100"), None);
    }

    #[test]
    fn parse_pid_non_numeric() {
        assert_eq!(parse_pid_from_dir_name("gv3_archive_abc_100"), None);
    }

    #[test]
    fn current_process_is_alive() {
        assert!(is_process_alive(std::process::id()));
    }

    #[test]
    fn dead_process_is_not_alive() {
        // PID=0はSystem Idle Process、通常OpenProcessでアクセス拒否される
        // 非常に大きいPIDはまず存在しない
        assert!(!is_process_alive(u32::MAX));
    }
}
