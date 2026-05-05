//! 旧 C++ 実装 (`.gvb`) 形式のブックマークパーサー
//!
//! 旧形式は UTF-16 LE BOM 付きテキストで、以下の構造を持つ:
//! ```text
//! [gvbookmark/ver:1]
//! current:
//! <現在表示中のファイルパス>
//! files:
//! <ファイルパス 1>
//! <ファイルパス 2>
//! ...
//! ```

use std::path::{Path, PathBuf};

use crate::bookmark::BookmarkData;
use crate::file_info::FileSource;

/// UTF-16 LE バイト列 (BOM 除去済み) から旧形式ブックマークをパースする
pub(super) fn parse_legacy_bookmark_utf16le(
    utf16le_bytes: &[u8],
    is_archive: &impl Fn(&Path) -> bool,
) -> BookmarkData {
    let text = decode_utf16le_lossy(utf16le_bytes);
    parse_legacy_bookmark(&text, is_archive)
}

/// UTF-16 LE バイト列を String にデコードする (不正シーケンスは U+FFFD に置換)
fn decode_utf16le_lossy(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// パース結果の中間表現
struct ParsedSections {
    current: Option<String>,
    files: Vec<String>,
    /// `archives:` セクションに列挙されたアーカイブ本体パス。
    /// `files:` のエントリからもアーカイブパスは推測できるが、
    /// `archives:` には files に画像が無いアーカイブも含まれうるため別途収集する。
    archives: Vec<String>,
}

/// テキストからセクションを解析する
fn parse_sections(text: &str) -> ParsedSections {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Section {
        None,
        Current,
        Files,
        Archives,
    }

    let mut section = Section::None;
    let mut current_path: Option<String> = None;
    let mut raw_files: Vec<String> = Vec::new();
    let mut raw_archives: Vec<String> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // ヘッダー行 (例: "[gvbookmark/ver:1]") はスキップ
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = Section::None;
            continue;
        }

        if trimmed == "current:" {
            section = Section::Current;
            continue;
        }
        if trimmed == "files:" {
            section = Section::Files;
            continue;
        }
        if trimmed == "archives:" {
            section = Section::Archives;
            continue;
        }

        match section {
            Section::Current => {
                // 最初の非空行のみ採用
                if current_path.is_none() {
                    current_path = Some(trimmed.to_string());
                }
            }
            Section::Files => {
                raw_files.push(trimmed.to_string());
            }
            Section::Archives => {
                raw_archives.push(trimmed.to_string());
            }
            Section::None => {
                // セクション外の行もファイルパスとして扱う (ベストエフォート)
                raw_files.push(trimmed.to_string());
            }
        }
    }

    ParsedSections {
        current: current_path,
        files: raw_files,
        archives: raw_archives,
    }
}

/// 旧形式テキストをパースして `BookmarkData` を返す
fn parse_legacy_bookmark(text: &str, is_archive: &impl Fn(&Path) -> bool) -> BookmarkData {
    let sections = parse_sections(text);

    // current パスとの大文字小文字無視比較でインデックスを決定
    let index = sections
        .current
        .as_deref()
        .and_then(|cur| {
            sections
                .files
                .iter()
                .position(|f| f.eq_ignore_ascii_case(cur))
        })
        .unwrap_or(0);

    // files セクションのエントリを分類
    let mut entries: Vec<FileSource> = sections
        .files
        .iter()
        .map(|raw| classify_raw_path(raw, is_archive))
        .collect();

    // archives セクション: files エントリから検出されないアーカイブを PendingContainer として追加。
    // files 内の ArchiveEntry から既知のアーカイブパスを収集し、archives にのみ存在するものを補完する。
    if !sections.archives.is_empty() {
        let known_archives: Vec<PathBuf> = entries
            .iter()
            .filter_map(|e| match e {
                FileSource::ArchiveEntry { archive, .. } => Some(archive.clone()),
                FileSource::PendingContainer { container_path } => Some(container_path.clone()),
                _ => None,
            })
            .collect();

        for raw_archive in &sections.archives {
            let archive_path = PathBuf::from(raw_archive.as_str());
            if !known_archives
                .iter()
                .any(|k| k.as_os_str().eq_ignore_ascii_case(archive_path.as_os_str()))
            {
                entries.push(FileSource::PendingContainer {
                    container_path: archive_path,
                });
            }
        }
    }

    // インデックスをクランプ
    let index = if entries.is_empty() {
        0
    } else {
        index.min(entries.len() - 1)
    };

    BookmarkData { entries, index }
}

/// 生パス文字列を `FileSource` に分類する
///
/// - アーカイブ内エントリ (例: `foo.zip\inside.jpg`) → `ArchiveEntry`
/// - アーカイブ本体だけ (例: `vol21.zip`) → `PendingContainer`
/// - 上記以外 → `File`
fn classify_raw_path(raw: &str, is_archive: &impl Fn(&Path) -> bool) -> FileSource {
    if let Some((archive, entry)) = split_archive_path(raw, is_archive) {
        return FileSource::ArchiveEntry {
            archive,
            entry,
            on_demand: false,
        };
    }

    let full = Path::new(raw);
    if is_archive(full) {
        return FileSource::PendingContainer {
            container_path: full.to_path_buf(),
        };
    }

    FileSource::File(PathBuf::from(raw))
}

/// パス文字列からアーカイブとエントリを分離する
///
/// `Path::ancestors()` で走査し、アーカイブ拡張子を持つ最も根に近い先祖で分割する。
/// パス自体がアーカイブの場合 (エントリ部分が空) は `None` を返す。
fn split_archive_path(raw: &str, is_archive: &impl Fn(&Path) -> bool) -> Option<(PathBuf, String)> {
    let full = Path::new(raw);

    // ancestors() は最長 (自身) → 最短 (ルート) の順で返す。
    // 上書きを続けることで最も根に近い (最短の) アーカイブを選ぶ。
    let mut matched: Option<&Path> = None;
    for ancestor in full.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() {
            break;
        }
        if is_archive(ancestor) {
            matched = Some(ancestor);
        }
    }

    let archive = matched?;
    let rel = full.strip_prefix(archive).ok()?;
    let entry = rel.to_string_lossy().replace('\\', "/");

    if entry.is_empty() {
        // パス自体がアーカイブ (エントリなし) — 呼び出し側で PendingContainer にする
        None
    } else {
        Some((archive.to_path_buf(), entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用のアーカイブ判定クロージャ
    fn test_is_archive(p: &Path) -> bool {
        p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            ["zip", "cbz", "rar", "cbr", "7z"].contains(&e.to_lowercase().as_str())
        })
    }

    /// テスト用に文字列を UTF-16 LE バイト列に変換する
    fn to_utf16le(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    // --- parse_legacy_bookmark_utf16le ---

    #[test]
    fn parse_normal() {
        let text = "[gvbookmark/ver:1]\r\ncurrent:\r\nC:\\images\\b.jpg\r\nfiles:\r\nC:\\images\\a.jpg\r\nC:\\images\\b.jpg\r\nC:\\images\\c.jpg\r\n";
        let bytes = to_utf16le(text);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        assert_eq!(data.entries.len(), 3);
        assert_eq!(data.index, 1);
        assert!(
            matches!(&data.entries[0], FileSource::File(p) if p == Path::new(r"C:\images\a.jpg"))
        );
        assert!(
            matches!(&data.entries[1], FileSource::File(p) if p == Path::new(r"C:\images\b.jpg"))
        );
        assert!(
            matches!(&data.entries[2], FileSource::File(p) if p == Path::new(r"C:\images\c.jpg"))
        );
    }

    #[test]
    fn parse_with_archive_entries() {
        let text = "[gvbookmark/ver:1]\r\ncurrent:\r\nE:\\book.zip\\01.jpg\r\nfiles:\r\nE:\\book.zip\\01.jpg\r\nE:\\book.zip\\02.jpg\r\n";
        let bytes = to_utf16le(text);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        assert_eq!(data.entries.len(), 2);
        assert_eq!(data.index, 0);
        assert!(matches!(
            &data.entries[0],
            FileSource::ArchiveEntry { archive, entry, on_demand: false }
            if archive == Path::new(r"E:\book.zip") && entry == "01.jpg"
        ));
        assert!(matches!(
            &data.entries[1],
            FileSource::ArchiveEntry { archive, entry, on_demand: false }
            if archive == Path::new(r"E:\book.zip") && entry == "02.jpg"
        ));
    }

    #[test]
    fn parse_standalone_archive_becomes_pending_container() {
        let text = "[gvbookmark/ver:1]\r\ncurrent:\r\nE:\\vol01.zip\r\nfiles:\r\nE:\\vol01.zip\r\nE:\\vol02.zip\r\n";
        let bytes = to_utf16le(text);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        assert_eq!(data.entries.len(), 2);
        assert!(matches!(
            &data.entries[0],
            FileSource::PendingContainer { container_path } if container_path == Path::new(r"E:\vol01.zip")
        ));
        assert!(matches!(
            &data.entries[1],
            FileSource::PendingContainer { container_path } if container_path == Path::new(r"E:\vol02.zip")
        ));
    }

    #[test]
    fn parse_mixed_archive_and_entries() {
        let text = "[gvbookmark/ver:1]\r\ncurrent:\r\nvol2.zip\\01.jpg\r\nfiles:\r\nvol1.zip\r\nvol2.zip\\01.jpg\r\nvol2.zip\\02.jpg\r\n";
        let bytes = to_utf16le(text);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        assert_eq!(data.entries.len(), 3);
        assert_eq!(data.index, 1);
        assert!(matches!(
            &data.entries[0],
            FileSource::PendingContainer { .. }
        ));
        assert!(matches!(&data.entries[1], FileSource::ArchiveEntry { .. }));
        assert!(matches!(&data.entries[2], FileSource::ArchiveEntry { .. }));
    }

    #[test]
    fn parse_unc_path() {
        let text = "[gvbookmark/ver:1]\r\ncurrent:\r\n\\\\server\\share\\dir\\a.jpg\r\nfiles:\r\n\\\\server\\share\\dir\\a.jpg\r\n\\\\server\\share\\dir\\b.jpg\r\n";
        let bytes = to_utf16le(text);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        assert_eq!(data.entries.len(), 2);
        assert_eq!(data.index, 0);
        assert!(matches!(
            &data.entries[0],
            FileSource::File(p) if p == Path::new(r"\\server\share\dir\a.jpg")
        ));
    }

    #[test]
    fn parse_nested_archive_ext_picks_outermost() {
        let text = "files:\r\nC:\\a.zip\\b.zip\\c.jpg\r\n";
        let bytes = to_utf16le(text);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        assert_eq!(data.entries.len(), 1);
        assert!(matches!(
            &data.entries[0],
            FileSource::ArchiveEntry { archive, entry, .. }
            if archive == Path::new(r"C:\a.zip") && entry == "b.zip/c.jpg"
        ));
    }

    #[test]
    fn parse_missing_header() {
        let text = "current:\r\ntest.jpg\r\nfiles:\r\ntest.jpg\r\ntest2.jpg\r\n";
        let bytes = to_utf16le(text);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        assert_eq!(data.entries.len(), 2);
        assert_eq!(data.index, 0);
    }

    #[test]
    fn parse_no_current_section() {
        let text = "[gvbookmark/ver:1]\r\nfiles:\r\na.jpg\r\nb.jpg\r\n";
        let bytes = to_utf16le(text);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        assert_eq!(data.entries.len(), 2);
        assert_eq!(data.index, 0);
    }

    #[test]
    fn parse_current_not_in_files() {
        let text = "[gvbookmark/ver:1]\r\ncurrent:\r\nnotexist.jpg\r\nfiles:\r\na.jpg\r\nb.jpg\r\n";
        let bytes = to_utf16le(text);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        assert_eq!(data.entries.len(), 2);
        assert_eq!(data.index, 0);
    }

    #[test]
    fn parse_index_matching_case_insensitive() {
        let text = "[gvbookmark/ver:1]\r\ncurrent:\r\nc:\\Foo.jpg\r\nfiles:\r\nC:\\foo.jpg\r\nC:\\bar.jpg\r\n";
        let bytes = to_utf16le(text);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        assert_eq!(data.index, 0);
    }

    #[test]
    fn parse_index_points_to_non_first_archive() {
        let text = "[gvbookmark/ver:1]\r\ncurrent:\r\nE:\\vol2.zip\\003.jpg\r\nfiles:\r\nE:\\vol1.zip\\001.jpg\r\nE:\\vol1.zip\\002.jpg\r\nE:\\vol2.zip\\003.jpg\r\nE:\\vol2.zip\\004.jpg\r\n";
        let bytes = to_utf16le(text);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        assert_eq!(data.entries.len(), 4);
        assert_eq!(data.index, 2);
    }

    #[test]
    fn parse_empty_after_bom() {
        let data = parse_legacy_bookmark_utf16le(&[], &test_is_archive);
        assert!(data.entries.is_empty());
        assert_eq!(data.index, 0);
    }

    #[test]
    fn parse_odd_byte_count() {
        // 奇数バイト: 末尾 1 バイトは破棄されるがパニックしない
        let mut bytes = to_utf16le("files:\r\na.jpg\r\n");
        bytes.push(0xFF);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        assert_eq!(data.entries.len(), 1);
    }

    #[test]
    fn parse_ignores_blank_lines() {
        let text = "[gvbookmark/ver:1]\r\n\r\nfiles:\r\n\r\na.jpg\r\n\r\nb.jpg\r\n\r\n";
        let bytes = to_utf16le(text);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        assert_eq!(data.entries.len(), 2);
    }

    // --- split_archive_path ---

    #[test]
    fn split_at_zip() {
        let result = split_archive_path(r"C:\data\test.zip\img\001.jpg", &test_is_archive);
        let (archive, entry) = result.unwrap();
        assert_eq!(archive, Path::new(r"C:\data\test.zip"));
        assert_eq!(entry, "img/001.jpg");
    }

    #[test]
    fn split_uppercase_cbr() {
        let result = split_archive_path(r"D:\comics\vol.CBR\page1.png", &test_is_archive);
        let (archive, entry) = result.unwrap();
        assert_eq!(archive, Path::new(r"D:\comics\vol.CBR"));
        assert_eq!(entry, "page1.png");
    }

    #[test]
    fn split_no_archive_returns_none() {
        let result = split_archive_path(r"C:\images\photo.jpg", &test_is_archive);
        assert!(result.is_none());
    }

    #[test]
    fn split_archive_itself_returns_none() {
        let result = split_archive_path(r"C:\data\test.zip", &test_is_archive);
        assert!(result.is_none());
    }

    // --- classify_raw_path ---

    #[test]
    fn classify_archive_body_becomes_pending() {
        let source = classify_raw_path(r"E:\manga\vol21.zip", &test_is_archive);
        assert!(matches!(source, FileSource::PendingContainer { .. }));
    }

    #[test]
    fn classify_archive_entry() {
        let source = classify_raw_path(r"E:\manga\vol21.zip\page001.jpg", &test_is_archive);
        assert!(matches!(source, FileSource::ArchiveEntry { .. }));
    }

    #[test]
    fn classify_normal_file() {
        let source = classify_raw_path(r"C:\images\photo.jpg", &test_is_archive);
        assert!(matches!(source, FileSource::File(_)));
    }

    // --- archives: セクション ---

    #[test]
    fn parse_archives_section_adds_unknown_containers() {
        // files にはvol1のエントリのみ、archives にはvol1とvol2が列挙される。
        // vol2は files から検出されないため PendingContainer として追加される。
        let text = "[gvbookmark/ver:1]\r\ncurrent:\r\nE:\\vol1.zip\\01.jpg\r\nfiles:\r\nE:\\vol1.zip\\01.jpg\r\nE:\\vol1.zip\\02.jpg\r\narchives:\r\nE:\\vol1.zip\r\nE:\\vol2.zip\r\n";
        let bytes = to_utf16le(text);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        // files: 2 エントリ (ArchiveEntry) + archives から 1 PendingContainer (vol2)
        assert_eq!(data.entries.len(), 3);
        assert!(matches!(&data.entries[0], FileSource::ArchiveEntry { .. }));
        assert!(matches!(&data.entries[1], FileSource::ArchiveEntry { .. }));
        assert!(matches!(
            &data.entries[2],
            FileSource::PendingContainer { container_path }
            if container_path == Path::new(r"E:\vol2.zip")
        ));
    }

    #[test]
    fn parse_archives_section_no_duplicates() {
        // files のエントリから既知のアーカイブは archives に重複があっても追加しない
        let text =
            "[gvbookmark/ver:1]\r\nfiles:\r\nE:\\vol1.zip\\01.jpg\r\narchives:\r\nE:\\vol1.zip\r\n";
        let bytes = to_utf16le(text);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        assert_eq!(data.entries.len(), 1);
        assert!(matches!(&data.entries[0], FileSource::ArchiveEntry { .. }));
    }

    #[test]
    fn parse_archives_only_no_files() {
        // files が空で archives のみ → 全て PendingContainer
        let text = "[gvbookmark/ver:1]\r\narchives:\r\nE:\\vol1.zip\r\nE:\\vol2.zip\r\n";
        let bytes = to_utf16le(text);
        let data = parse_legacy_bookmark_utf16le(&bytes, &test_is_archive);
        assert_eq!(data.entries.len(), 2);
        assert!(matches!(
            &data.entries[0],
            FileSource::PendingContainer { .. }
        ));
        assert!(matches!(
            &data.entries[1],
            FileSource::PendingContainer { .. }
        ));
    }
}
