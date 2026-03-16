use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context as _, Result};

/// 個々のファイル情報
#[allow(dead_code)]
pub struct FileInfo {
    pub path: PathBuf,
    pub file_name: String, // ソート用キャッシュ
    pub file_size: u64,
    pub modified: SystemTime,
    pub marked: bool,      // Phase 8で使用
    pub load_failed: bool, // デコード失敗フラグ（ナビゲーション時にスキップ）
}

impl FileInfo {
    /// パスからFileInfoを構築する
    pub fn from_path(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path)
            .with_context(|| format!("メタデータ取得失敗: {}", path.display()))?;

        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

        Ok(Self {
            path: path.to_path_buf(),
            file_name,
            file_size: metadata.len(),
            modified,
            marked: false,
            load_failed: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn from_path_valid_file() {
        let dir = std::env::temp_dir().join("gv3_test_file_info");
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("test.png");
        let mut f = std::fs::File::create(&file_path).unwrap();
        f.write_all(b"dummy content").unwrap();
        drop(f);

        let info = FileInfo::from_path(&file_path).unwrap();
        assert_eq!(info.file_name, "test.png");
        assert_eq!(info.file_size, 13);
        assert!(!info.marked);
        assert!(!info.load_failed);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_path_nonexistent() {
        let result = FileInfo::from_path(Path::new("nonexistent_file_xyz.png"));
        assert!(result.is_err());
    }
}
