//! ブックマーク機能（ファイルリストの保存/復元）

use std::fmt::Write as _;
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
    let default_name = format!("bookmark_{now}.gvbm");

    let save_path = crate::file_ops::save_file_dialog(
        hwnd,
        &default_name,
        "ぐらびゅブックマーク",
        "*.gvbm",
        Some(&dir),
        None,
        None,
    )?;

    let Some(save_path) = save_path else {
        return Ok(()); // キャンセル
    };

    let mut content = String::new();
    content.push_str("# gv3 bookmark v1\n");
    let _ = writeln!(content, "# index: {}", current_index.unwrap_or(0));

    for file in file_list.files() {
        match &file.source {
            FileSource::File(path) => {
                let _ = writeln!(content, "file\t{}", path.display());
            }
            FileSource::ArchiveEntry { archive, entry, .. } => {
                // on_demandフラグは保存しない（復元時にopen_containersで再判定）
                let _ = writeln!(content, "archive\t{}\t{}", archive.display(), entry);
            }
            FileSource::PdfPage {
                pdf_path,
                page_index,
            } => {
                let _ = writeln!(content, "pdf\t{}\t{}", pdf_path.display(), page_index);
            }
            FileSource::PendingContainer { .. } => {
                // 未展開コンテナはスキップ（保存前にexpand_all_pending_syncで展開済みのはず）
            }
        }
    }

    std::fs::write(&save_path, &content)
        .with_context(|| format!("ブックマーク保存失敗: {}", save_path.display()))?;

    Ok(())
}

/// ブックマークを読み込む
pub fn load_bookmark(hwnd: HWND) -> Result<Option<BookmarkData>> {
    let path = crate::file_ops::open_bookmark_dialog(hwnd)?;
    let Some(path) = path else {
        return Ok(None);
    };

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("ブックマーク読み込み失敗: {}", path.display()))?;

    Ok(Some(parse_bookmark(&content)))
}

/// ブックマークテキストをパースする
fn parse_bookmark(content: &str) -> BookmarkData {
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
                    on_demand: false, // 復元時にopen_containersで再判定
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

    BookmarkData { entries, index }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parse_bookmark_normal() {
        let content = r"# gv3 bookmark v1
# index: 1
file	C:\images\test.jpg
file	C:\images\test2.png
";
        let data = parse_bookmark(content);
        assert_eq!(data.entries.len(), 2);
        assert_eq!(data.index, 1);
        assert!(
            matches!(&data.entries[0], FileSource::File(p) if p == Path::new(r"C:\images\test.jpg"))
        );
    }

    #[test]
    fn parse_bookmark_with_archive() {
        let content = r"# gv3 bookmark v1
# index: 0
file	C:\images\test.jpg
archive	C:\archive.zip	folder/image.png
";
        let data = parse_bookmark(content);
        assert_eq!(data.entries.len(), 2);
        assert!(matches!(
            &data.entries[1],
            FileSource::ArchiveEntry { archive, entry, .. }
            if archive == Path::new(r"C:\archive.zip") && entry == "folder/image.png"
        ));
    }

    #[test]
    fn parse_bookmark_with_pdf() {
        let content = r"# gv3 bookmark v1
# index: 2
pdf	C:\docs\test.pdf	0
pdf	C:\docs\test.pdf	1
pdf	C:\docs\test.pdf	2
";
        let data = parse_bookmark(content);
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
        let data = parse_bookmark(content);
        assert_eq!(data.index, 0); // clamped to 0
    }

    #[test]
    fn parse_bookmark_empty_content() {
        let data = parse_bookmark("");
        assert!(data.entries.is_empty());
        assert_eq!(data.index, 0);
    }

    #[test]
    fn parse_bookmark_header_only_no_entries() {
        let content = "# gv3 bookmark v1\n# index: 5\n";
        let data = parse_bookmark(content);
        assert!(data.entries.is_empty());
        assert_eq!(data.index, 5); // エントリが空なのでクランプされず、パースされたindex値がそのまま残る
    }

    #[test]
    fn parse_bookmark_no_index_comment() {
        let content = "# gv3 bookmark v1\nfile\ttest.jpg\nfile\ttest2.jpg\n";
        let data = parse_bookmark(content);
        assert_eq!(data.entries.len(), 2);
        assert_eq!(data.index, 0); // デフォルト0
    }

    #[test]
    fn parse_bookmark_invalid_index_value() {
        // index行のパースに失敗 → デフォルト0のまま
        let content = "# index: abc\nfile\ttest.jpg\n";
        let data = parse_bookmark(content);
        assert_eq!(data.index, 0);
        assert_eq!(data.entries.len(), 1);
    }

    #[test]
    fn parse_bookmark_negative_index() {
        let content = "# index: -1\nfile\ttest.jpg\n";
        let data = parse_bookmark(content);
        assert_eq!(data.index, 0); // usize::parseに失敗 → デフォルト0
    }

    #[test]
    fn parse_bookmark_blank_lines_ignored() {
        let content = "# gv3 bookmark v1\n\n\nfile\ttest.jpg\n\nfile\ttest2.jpg\n\n";
        let data = parse_bookmark(content);
        assert_eq!(data.entries.len(), 2);
    }

    #[test]
    fn parse_bookmark_whitespace_lines_ignored() {
        let content = "   \n\tfile\ta.jpg\n   \n";
        let data = parse_bookmark(content);
        // "file\ta.jpg" はtrim後に正しくパースされるはず…
        // ただし "\tfile\ta.jpg" をtrimすると "file\ta.jpg" になり、
        // splitn(3, '\t') → ["file", "a.jpg"] にマッチ
        assert_eq!(data.entries.len(), 1);
        assert!(matches!(&data.entries[0], FileSource::File(p) if p == Path::new("a.jpg")));
    }

    #[test]
    fn parse_bookmark_unknown_type_falls_back_to_file() {
        // 未知のタイプ名は後方互換でFileとして扱われる
        let content = "unknown\tpath\textra\n";
        let data = parse_bookmark(content);
        assert_eq!(data.entries.len(), 1);
        // 後方互換: タブ区切りでない行 or マッチしない行はそのままFileとして扱う
        assert!(matches!(&data.entries[0], FileSource::File(_)));
    }

    #[test]
    fn parse_bookmark_bare_path_without_type() {
        // タブ区切りでない行 → 後方互換でそのままファイルパスとして扱う
        let content = r"C:\images\photo.jpg";
        let data = parse_bookmark(content);
        assert_eq!(data.entries.len(), 1);
        assert!(
            matches!(&data.entries[0], FileSource::File(p) if p == Path::new(r"C:\images\photo.jpg"))
        );
    }

    #[test]
    fn parse_bookmark_pdf_invalid_page_index() {
        // page_indexがu32にパースできない → エントリはスキップされる
        let content = "pdf\tC:\\test.pdf\tabc\n";
        let data = parse_bookmark(content);
        assert!(data.entries.is_empty());
    }

    #[test]
    fn parse_bookmark_pdf_negative_page_index() {
        let content = "pdf\tC:\\test.pdf\t-1\n";
        let data = parse_bookmark(content);
        assert!(data.entries.is_empty()); // u32パース失敗
    }

    #[test]
    fn parse_bookmark_archive_on_demand_is_false() {
        let content = "archive\tC:\\test.zip\timg.png\n";
        let data = parse_bookmark(content);
        assert_eq!(data.entries.len(), 1);
        assert!(matches!(
            &data.entries[0],
            FileSource::ArchiveEntry { on_demand, .. } if !on_demand
        ));
    }

    #[test]
    fn parse_bookmark_mixed_entry_types() {
        let content = r"# gv3 bookmark v1
# index: 2
file	C:\images\a.jpg
archive	C:\archive.zip	inner/b.png
pdf	C:\docs\doc.pdf	3
file	C:\images\c.bmp
";
        let data = parse_bookmark(content);
        assert_eq!(data.entries.len(), 4);
        assert_eq!(data.index, 2);
        assert!(matches!(&data.entries[0], FileSource::File(_)));
        assert!(matches!(&data.entries[1], FileSource::ArchiveEntry { .. }));
        assert!(matches!(
            &data.entries[2],
            FileSource::PdfPage { page_index, .. } if *page_index == 3
        ));
        assert!(matches!(&data.entries[3], FileSource::File(_)));
    }

    #[test]
    fn parse_bookmark_index_clamp_exact_boundary() {
        // index == entries.len() → len-1にクランプ
        let content = "# index: 3\nfile\ta.jpg\nfile\tb.jpg\nfile\tc.jpg\n";
        let data = parse_bookmark(content);
        assert_eq!(data.entries.len(), 3);
        assert_eq!(data.index, 2); // 3 >= 3 なのでクランプ
    }

    #[test]
    fn parse_bookmark_index_within_range() {
        let content = "# index: 2\nfile\ta.jpg\nfile\tb.jpg\nfile\tc.jpg\n";
        let data = parse_bookmark(content);
        assert_eq!(data.index, 2); // 範囲内なのでそのまま
    }

    #[test]
    fn parse_bookmark_comment_lines_ignored() {
        let content = "# gv3 bookmark v1\n# some comment\n# another\nfile\ta.jpg\n";
        let data = parse_bookmark(content);
        assert_eq!(data.entries.len(), 1);
    }
}
