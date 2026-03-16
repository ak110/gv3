//! ブックマーク機能（ファイルリストの保存/復元）

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::HWND;

use crate::file_info::FileSource;

/// ブックマークデータ
pub struct BookmarkData {
    pub entries: Vec<FileSource>,
    pub index: usize,
}

/// ブックマークフォルダのパスを返す（exeと同じディレクトリの"bookmarks"）
pub fn bookmark_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("bookmarks")))
        .unwrap_or_else(|| PathBuf::from("bookmarks"))
}

/// ブックマークを保存する
pub fn save_bookmark(
    hwnd: HWND,
    file_list: &crate::file_list::FileList,
    current_index: Option<usize>,
) -> Result<()> {
    let dir = bookmark_dir();
    let _ = std::fs::create_dir_all(&dir);

    // デフォルトファイル名: 日付ベース
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let default_name = format!("bookmark_{now}.gv3bm");

    let save_path =
        crate::file_ops::save_file_dialog(hwnd, &default_name, "ぐらびゅ3ブックマーク", "*.gv3bm")?;

    let Some(save_path) = save_path else {
        return Ok(()); // キャンセル
    };

    let mut content = String::new();
    content.push_str("# gv3 bookmark v1\n");
    content.push_str(&format!("# index: {}\n", current_index.unwrap_or(0)));

    for file in file_list.files() {
        match &file.source {
            FileSource::File(path) => {
                content.push_str(&format!("file\t{}\n", path.display()));
            }
            FileSource::ArchiveEntry { archive, entry } => {
                content.push_str(&format!("archive\t{}\t{}\n", archive.display(), entry));
            }
            FileSource::PdfPage {
                pdf_path,
                page_index,
            } => {
                content.push_str(&format!("pdf\t{}\t{}\n", pdf_path.display(), page_index));
            }
        }
    }

    std::fs::write(&save_path, &content)
        .with_context(|| format!("ブックマーク保存失敗: {}", save_path.display()))?;

    Ok(())
}

/// ブックマークを読み込む
pub fn load_bookmark(hwnd: HWND) -> Result<Option<BookmarkData>> {
    let path = crate::file_ops::open_file_dialog(hwnd)?;
    let Some(path) = path else {
        return Ok(None);
    };

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("ブックマーク読み込み失敗: {}", path.display()))?;

    parse_bookmark(&content).map(Some)
}

/// ブックマークテキストをパースする
fn parse_bookmark(content: &str) -> Result<BookmarkData> {
    let mut entries = Vec::new();
    let mut index = 0;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // コメント行
        if line.starts_with('#') {
            // index指定を読み取る
            if let Some(idx_str) = line.strip_prefix("# index:")
                && let Ok(idx) = idx_str.trim().parse::<usize>()
            {
                index = idx;
            }
            continue;
        }

        // タブ区切り
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        match parts.as_slice() {
            ["file", path] => {
                entries.push(FileSource::File(PathBuf::from(path)));
            }
            ["archive", archive_path, entry] => {
                entries.push(FileSource::ArchiveEntry {
                    archive: PathBuf::from(archive_path),
                    entry: entry.to_string(),
                });
            }
            ["pdf", pdf_path, page_index_str] => {
                if let Ok(page_index) = page_index_str.parse::<u32>() {
                    entries.push(FileSource::PdfPage {
                        pdf_path: PathBuf::from(pdf_path),
                        page_index,
                    });
                }
            }
            _ => {
                // 後方互換: タブ区切りでないパスは通常ファイルとして扱う
                entries.push(FileSource::File(PathBuf::from(line)));
            }
        }
    }

    // indexがリスト範囲外ならクランプ
    if !entries.is_empty() && index >= entries.len() {
        index = entries.len() - 1;
    }

    Ok(BookmarkData { entries, index })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parse_bookmark_normal() {
        let content = r#"# gv3 bookmark v1
# index: 1
file	C:\images\test.jpg
file	C:\images\test2.png
"#;
        let data = parse_bookmark(content).unwrap();
        assert_eq!(data.entries.len(), 2);
        assert_eq!(data.index, 1);
        assert!(
            matches!(&data.entries[0], FileSource::File(p) if p == Path::new(r"C:\images\test.jpg"))
        );
    }

    #[test]
    fn parse_bookmark_with_archive() {
        let content = r#"# gv3 bookmark v1
# index: 0
file	C:\images\test.jpg
archive	C:\archive.zip	folder/image.png
"#;
        let data = parse_bookmark(content).unwrap();
        assert_eq!(data.entries.len(), 2);
        assert!(matches!(
            &data.entries[1],
            FileSource::ArchiveEntry { archive, entry }
            if archive == Path::new(r"C:\archive.zip") && entry == "folder/image.png"
        ));
    }

    #[test]
    fn parse_bookmark_with_pdf() {
        let content = r#"# gv3 bookmark v1
# index: 2
pdf	C:\docs\test.pdf	0
pdf	C:\docs\test.pdf	1
pdf	C:\docs\test.pdf	2
"#;
        let data = parse_bookmark(content).unwrap();
        assert_eq!(data.entries.len(), 3);
        assert_eq!(data.index, 2);
        assert!(matches!(
            &data.entries[1],
            FileSource::PdfPage { pdf_path, page_index }
            if pdf_path == Path::new(r"C:\docs\test.pdf") && *page_index == 1
        ));
    }

    #[test]
    fn parse_bookmark_index_clamp() {
        let content = "# index: 999\nfile\ttest.jpg\n";
        let data = parse_bookmark(content).unwrap();
        assert_eq!(data.index, 0); // clamped to 0
    }
}
