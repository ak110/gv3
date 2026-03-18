use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context as _, Result};

/// ファイルの論理的なソース情報
/// 通常ファイルとアーカイブ内エントリを区別する
#[derive(Debug, Clone)]
pub enum FileSource {
    /// 通常のファイルシステム上のファイル
    File(PathBuf),
    /// アーカイブ内のエントリ
    ArchiveEntry {
        archive: PathBuf,
        entry: String,
        /// trueならオンデマンド読み出し、falseならtemp展開済み
        on_demand: bool,
    },
    /// PDFのページ
    PdfPage { pdf_path: PathBuf, page_index: u32 },
}

impl FileSource {
    /// 表示用パスを生成する
    pub fn display_path(&self) -> String {
        match self {
            FileSource::File(path) => path.display().to_string(),
            FileSource::ArchiveEntry { archive, entry, .. } => {
                format!("{}/{}", archive.display(), entry)
            }
            FileSource::PdfPage {
                pdf_path,
                page_index,
            } => {
                format!("{}/Page {}", pdf_path.display(), page_index + 1)
            }
        }
    }

    /// アーカイブパスを返す（アーカイブエントリ/PDFの場合）
    #[allow(dead_code)]
    pub fn archive_path(&self) -> Option<&Path> {
        match self {
            FileSource::ArchiveEntry { archive, .. } => Some(archive),
            FileSource::PdfPage { pdf_path, .. } => Some(pdf_path),
            FileSource::File(_) => None,
        }
    }

    /// アーカイブエントリかどうか
    #[allow(dead_code)]
    pub fn is_archive_entry(&self) -> bool {
        matches!(self, FileSource::ArchiveEntry { .. })
    }

    /// コンテナ内のエントリかどうか（アーカイブまたはPDF）
    /// 破壊的ファイル操作（削除・移動等）のガードに使用
    pub fn is_contained(&self) -> bool {
        matches!(
            self,
            FileSource::ArchiveEntry { .. } | FileSource::PdfPage { .. }
        )
    }

    /// ダイアログ初期ディレクトリ用: ソースの親ディレクトリを返す
    pub fn parent_dir(&self) -> Option<&Path> {
        match self {
            FileSource::File(path) => path.parent(),
            FileSource::ArchiveEntry { archive, .. } => archive.parent(),
            FileSource::PdfPage { pdf_path, .. } => pdf_path.parent(),
        }
    }

    /// ダイアログ用デフォルトファイル名を返す
    pub fn default_save_name(&self) -> String {
        match self {
            FileSource::File(path) => path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("image")
                .to_string(),
            FileSource::ArchiveEntry { archive, entry, .. } => {
                let archive_stem = archive
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("archive");
                let entry_filename = Path::new(entry)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("image");
                format!("{archive_stem}_{entry_filename}")
            }
            FileSource::PdfPage {
                pdf_path,
                page_index,
            } => {
                let stem = pdf_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("pdf");
                format!("{stem}_page{}.png", page_index + 1)
            }
        }
    }

    /// ダイアログ用デフォルトstem（拡張子なし）を返す（エクスポート用）
    pub fn default_save_stem(&self) -> String {
        let name = self.default_save_name();
        Path::new(&name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("image")
            .to_string()
    }
}

impl fmt::Display for FileSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_path())
    }
}

/// 個々のファイル情報
pub struct FileInfo {
    pub path: PathBuf,      // 実ファイルパス（デコード/描画用。アーカイブ時はtempパス）
    pub source: FileSource, // 論理ソース（表示・保存・ブックマーク用）
    pub file_name: String,  // ソート用キャッシュ
    pub file_size: u64,
    pub modified: SystemTime,
    pub marked: bool,
    pub load_failed: bool, // デコード失敗フラグ（ナビゲーション時にスキップ）
}

impl FileInfo {
    /// パスからFileInfoを構築する（通常ファイル用）
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
            source: FileSource::File(path.to_path_buf()),
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
        assert!(!info.source.is_archive_entry());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_source_display_path() {
        let source = FileSource::File(PathBuf::from(r"C:\images\test.jpg"));
        assert_eq!(source.display_path(), r"C:\images\test.jpg");
        assert!(!source.is_archive_entry());
        assert!(source.archive_path().is_none());

        let source = FileSource::ArchiveEntry {
            archive: PathBuf::from(r"C:\archive.zip"),
            entry: "folder/image.png".to_string(),
            on_demand: false,
        };
        assert_eq!(source.display_path(), r"C:\archive.zip/folder/image.png");
        assert!(source.is_archive_entry());
        assert_eq!(source.archive_path().unwrap(), Path::new(r"C:\archive.zip"));
    }

    #[test]
    fn pdf_page_source() {
        let source = FileSource::PdfPage {
            pdf_path: PathBuf::from(r"C:\docs\test.pdf"),
            page_index: 2,
        };
        assert_eq!(source.display_path(), r"C:\docs\test.pdf/Page 3");
        assert!(!source.is_archive_entry());
        assert!(source.is_contained());
        assert_eq!(
            source.archive_path().unwrap(),
            Path::new(r"C:\docs\test.pdf")
        );

        // File は is_contained() == false
        let file_source = FileSource::File(PathBuf::from(r"C:\images\test.jpg"));
        assert!(!file_source.is_contained());

        // ArchiveEntry は is_contained() == true
        let archive_source = FileSource::ArchiveEntry {
            archive: PathBuf::from(r"C:\archive.zip"),
            entry: "img.png".to_string(),
            on_demand: false,
        };
        assert!(archive_source.is_contained());
    }

    #[test]
    fn from_path_nonexistent() {
        let result = FileInfo::from_path(Path::new("nonexistent_file_xyz.png"));
        assert!(result.is_err());
    }

    #[test]
    fn parent_dir_for_each_source() {
        let file = FileSource::File(PathBuf::from(r"C:\images\test.jpg"));
        assert_eq!(file.parent_dir().unwrap(), Path::new(r"C:\images"));

        let archive = FileSource::ArchiveEntry {
            archive: PathBuf::from(r"C:\archives\photos.zip"),
            entry: "folder/sunset.png".to_string(),
            on_demand: true,
        };
        assert_eq!(archive.parent_dir().unwrap(), Path::new(r"C:\archives"));

        let pdf = FileSource::PdfPage {
            pdf_path: PathBuf::from(r"C:\docs\report.pdf"),
            page_index: 0,
        };
        assert_eq!(pdf.parent_dir().unwrap(), Path::new(r"C:\docs"));
    }

    #[test]
    fn default_save_name_for_each_source() {
        let file = FileSource::File(PathBuf::from(r"C:\images\sunset.jpg"));
        assert_eq!(file.default_save_name(), "sunset.jpg");

        let archive = FileSource::ArchiveEntry {
            archive: PathBuf::from(r"C:\photos.zip"),
            entry: "folder/sunset.png".to_string(),
            on_demand: false,
        };
        assert_eq!(archive.default_save_name(), "photos_sunset.png");

        let pdf = FileSource::PdfPage {
            pdf_path: PathBuf::from(r"C:\docs\report.pdf"),
            page_index: 2,
        };
        assert_eq!(pdf.default_save_name(), "report_page3.png");
    }

    #[test]
    fn default_save_stem_strips_extension() {
        let file = FileSource::File(PathBuf::from(r"C:\images\sunset.jpg"));
        assert_eq!(file.default_save_stem(), "sunset");

        let archive = FileSource::ArchiveEntry {
            archive: PathBuf::from(r"C:\photos.zip"),
            entry: "img.png".to_string(),
            on_demand: false,
        };
        assert_eq!(archive.default_save_stem(), "photos_img");

        let pdf = FileSource::PdfPage {
            pdf_path: PathBuf::from(r"C:\doc.pdf"),
            page_index: 0,
        };
        assert_eq!(pdf.default_save_stem(), "doc_page1");
    }
}
