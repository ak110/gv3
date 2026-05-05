//! ファイル一覧内のグループ（フォルダ/アーカイブ）管理機能。

use std::collections::HashMap;
use std::path::PathBuf;

use crate::file_info::{FileInfo, FileSource};

/// フォルダ/アーカイブ単位ナビゲーション用のグループキー
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum GroupKey {
    Folder(PathBuf),
    Archive(PathBuf),
}

/// ファイルのグループキーを返す (フォルダ=親ディレクトリ or アーカイブパス)
pub(crate) fn group_key(info: &FileInfo) -> GroupKey {
    match &info.source {
        FileSource::ArchiveEntry { archive, .. } => GroupKey::Archive(archive.clone()),
        FileSource::PdfPage { pdf_path, .. } => GroupKey::Archive(pdf_path.clone()),
        FileSource::PendingContainer { container_path } => {
            GroupKey::Archive(container_path.clone())
        }
        FileSource::File(_) => GroupKey::Folder(
            info.path
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default(),
        ),
    }
}

/// 現在のファイル列から `group_key` の出現順を計算する。
///
/// リスト上での連続性に依存しないため、論理パス順で同一グループのファイルが
/// 他グループの要素が間に存在して分断されていても正しく動作する。
/// 戻り値: (各グループの最初の登場 index, 各ファイルが属するグループ index)
pub(crate) fn compute_group_layout(files: &[FileInfo]) -> (Vec<usize>, Vec<usize>) {
    let mut seen: HashMap<GroupKey, usize> = HashMap::new();
    let mut group_first_indices: Vec<usize> = Vec::new();
    let mut file_group_idx: Vec<usize> = Vec::with_capacity(files.len());
    for (i, f) in files.iter().enumerate() {
        let key = group_key(f);
        let idx = *seen.entry(key).or_insert_with(|| {
            let idx = group_first_indices.len();
            group_first_indices.push(i);
            idx
        });
        file_group_idx.push(idx);
    }
    (group_first_indices, file_group_idx)
}
