//! ソート順と比較ロジック。

use crate::file_info::FileInfo;

/// ソート順
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    /// ファイル名順
    #[default]
    Name,
    /// ファイル名順 (大文字小文字区別なし)
    #[serde(rename = "name_nocase")]
    NameNoCase,
    /// ファイルサイズ順
    Size,
    /// 最終更新日時順
    Date,
    /// 自然順ソート (数値認識)
    Natural,
}

impl SortOrder {
    /// ソート順による比較。
    ///
    /// 比較キーは論理パス (`FileSource::display_path()` 相当) を基本とする。
    /// 通常ファイルはフルパス、アーカイブはアーカイブパス＋内部パス、PDF はファイルパス＋ページ番号となる。
    ///
    /// - `Name` / `NameNoCase` / `Natural`: 論理パスを直接比較する
    /// - `Size` / `Date`: 値を主キーとし、同値時に論理パスを副キーとする (タイブレーカー)
    pub(crate) fn compare(self, a: &FileInfo, b: &FileInfo) -> std::cmp::Ordering {
        let path_a = a.source.display_path();
        let path_b = b.source.display_path();
        match self {
            SortOrder::Name => path_a.cmp(&path_b),
            SortOrder::NameNoCase => path_a.to_lowercase().cmp(&path_b.to_lowercase()),
            SortOrder::Size => a
                .file_size
                .cmp(&b.file_size)
                .then_with(|| path_a.cmp(&path_b)),
            SortOrder::Date => a
                .modified
                .cmp(&b.modified)
                .then_with(|| path_a.cmp(&path_b)),
            SortOrder::Natural => natord::compare_ignore_case(&path_a, &path_b),
        }
    }
}
