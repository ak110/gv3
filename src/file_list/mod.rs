//! ファイル一覧管理モジュール。
//!
//! ファイルの列挙、ソート、ナビゲーション、マーク管理、グループ管理などを提供する。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use crate::extension_registry::ExtensionRegistry;
use crate::file_info::{FileInfo, FileSource};

// 責務別の各モジュール
mod groups;
mod natural_sort;
pub mod navigation;
mod sort;

// 外部から使用される型・関数を再エクスポート
pub use navigation::NavigationDirection;
pub use sort::SortOrder;

use groups::{compute_group_layout, group_key};
use natural_sort::SimpleRng;

/// ファイル一覧管理
pub struct FileList {
    files: Vec<FileInfo>,
    current_index: Option<usize>,
    sort_order: SortOrder,
    registry: Arc<ExtensionRegistry>,
}

impl FileList {
    pub fn new(registry: Arc<ExtensionRegistry>) -> Self {
        Self {
            files: Vec::new(),
            current_index: None,
            sort_order: SortOrder::Natural,
            registry,
        }
    }

    /// 拡張子レジストリへの参照を返す
    pub fn registry(&self) -> &ExtensionRegistry {
        &self.registry
    }

    /// フォルダ内の画像ファイルを列挙してリストを構築する
    pub fn populate_from_folder(&mut self, folder: &Path) -> Result<()> {
        self.files.clear();
        self.current_index = None;

        let entries = std::fs::read_dir(folder)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            // 拡張子フィルタ (ExtensionRegistry経由)
            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && self.registry.is_image_extension(name)
                && let Ok(info) = FileInfo::from_path(&path)
            {
                self.files.push(info);
            }
        }

        self.sort(self.sort_order);
        Ok(())
    }

    /// 単一ファイルのみでリストを構築する (フォルダスキャンしない)
    pub fn populate_single(&mut self, path: &Path) -> Result<()> {
        self.files.clear();
        self.current_index = None;

        let info = FileInfo::from_path(path)?;
        self.files.push(info);
        self.current_index = Some(0);
        Ok(())
    }

    /// FileSourceの同一性判定 (コンテナ内エントリの位置復元用)
    pub fn source_matches(a: &FileSource, b: &FileSource) -> bool {
        match (a, b) {
            (
                FileSource::ArchiveEntry {
                    archive: a1,
                    entry: e1,
                    ..
                },
                FileSource::ArchiveEntry {
                    archive: a2,
                    entry: e2,
                    ..
                },
            ) => a1 == a2 && e1 == e2,
            (
                FileSource::PdfPage {
                    pdf_path: p1,
                    page_index: i1,
                },
                FileSource::PdfPage {
                    pdf_path: p2,
                    page_index: i2,
                },
            ) => p1 == p2 && i1 == i2,
            (
                FileSource::PendingContainer { container_path: c1 },
                FileSource::PendingContainer { container_path: c2 },
            ) => c1 == c2,
            _ => false,
        }
    }

    /// ソート/削除後の位置復元 (コンテナ内エントリはsourceで、通常ファイルはpathで復元)
    fn restore_current_position(&mut self, path: &Path, source: &FileSource) {
        if source.is_contained() {
            // コンテナ内エントリはsourceで位置復元
            if let Some(idx) = self
                .files
                .iter()
                .position(|f| Self::source_matches(&f.source, source))
            {
                self.current_index = Some(idx);
                return;
            }
        }
        // 通常ファイル or sourceマッチ失敗 → pathで復元
        if !self.set_current_by_path(path) {
            self.current_index = if self.files.is_empty() { None } else { Some(0) };
        }
    }

    /// パスで現在位置を設定する
    pub fn set_current_by_path(&mut self, path: &Path) -> bool {
        if let Some(idx) = self.files.iter().position(|f| f.path == path) {
            self.current_index = Some(idx);
            true
        } else {
            false
        }
    }

    /// 相対移動 (ラップアラウンド)
    /// 先頭で後退 → 末尾へ、末尾で前進 → 先頭へ
    /// load_failedのファイルは同方向にスキップする
    pub fn navigate_relative(&mut self, offset: isize) -> bool {
        let len = self.files.len();
        if len == 0 {
            return false;
        }
        let current = self.current_index.unwrap_or(0);

        // ラップアラウンド付きでtarget計算
        let mut target = ((current as isize + offset) % len as isize + len as isize) as usize % len;

        // スキップ方向 (offsetの符号に合わせる)
        let step: isize = if offset >= 0 { 1 } else { -1 };

        // load_failedのファイルを同方向にスキップ (最大len回で打ち切り)
        let mut attempts = 0;
        while self.files[target].load_failed && attempts < len {
            target = ((target as isize + step) % len as isize + len as isize) as usize % len;
            attempts += 1;
        }
        if attempts >= len {
            // 全てfailedで移動先がない
            return false;
        }

        if target == current {
            return false;
        }
        self.current_index = Some(target);
        true
    }

    /// 指定インデックスへ移動
    pub fn navigate_to(&mut self, index: usize) -> bool {
        if index < self.files.len() && self.current_index != Some(index) {
            self.current_index = Some(index);
            true
        } else {
            false
        }
    }

    /// 最初へ移動
    pub fn navigate_first(&mut self) -> bool {
        self.navigate_to(0)
    }

    /// 最後へ移動
    pub fn navigate_last(&mut self) -> bool {
        if self.files.is_empty() {
            return false;
        }
        self.navigate_to(self.files.len() - 1)
    }

    /// ソート実行 (現在位置を維持)
    ///
    /// 比較キーは論理パス (`FileSource::display_path()` 相当) で、フォルダ・サブフォルダ・アーカイブを
    /// またいで完全に論理パス順に並ぶ。Size/Date は値を主キーとし、同値時に論理パスをタイブレーカーとする。
    pub fn sort(&mut self, order: SortOrder) {
        self.with_position_preserved(|this| {
            this.files.sort_by(|a, b| order.compare(a, b));
            this.sort_order = order;
        });
    }

    /// 指定インデックスのファイルをデコード失敗状態にする
    pub fn mark_failed(&mut self, index: usize) {
        if let Some(info) = self.files.get_mut(index) {
            info.load_failed = true;
        }
    }

    /// 全ファイルの失敗状態をクリア (フォルダ再読み込み時用)
    pub fn clear_failed(&mut self) {
        for info in &mut self.files {
            info.load_failed = false;
        }
    }

    /// ファイルをリストに追加する
    pub fn push(&mut self, info: FileInfo) {
        self.files.push(info);
    }

    /// リストをクリアする
    pub fn clear(&mut self) {
        self.files.clear();
        self.current_index = None;
    }

    /// 指定インデックスが未展開コンテナかどうか
    pub fn is_pending_at(&self, index: usize) -> bool {
        self.files
            .get(index)
            .is_some_and(|f| f.source.is_pending_container())
    }

    /// 未展開コンテナが残っているかどうか
    pub fn has_pending(&self) -> bool {
        self.files.iter().any(|f| f.source.is_pending_container())
    }

    /// 指定インデックスの未展開コンテナパスを返す
    pub fn pending_container_path_at(&self, index: usize) -> Option<PathBuf> {
        self.files.get(index).and_then(|f| match &f.source {
            FileSource::PendingContainer { container_path } => Some(container_path.clone()),
            _ => None,
        })
    }

    /// 未展開コンテナのプレースホルダを展開結果で置換する
    ///
    /// 副次処理ではファイル一覧全体を自動再ソートしない方針のため、ここでも全体再ソートは行わない。
    /// プレースホルダは元々ファイル一覧上の固有位置を持つため、その位置に展開エントリを挿入するだけで
    /// シャッフルを含むユーザー指定の現在順序を破壊しない。展開エントリ群の内部のみ「コンテナを開いた直後の
    /// 初期表示順序」として `sort_order` でソートする (フォルダ初期構築と同じ扱い)。
    ///
    /// `direction` は「展開位置 == 現在位置」だった場合の current_index 配置に使う:
    /// `Forward` なら展開エントリの先頭、`Backward` なら末尾を選ぶ。
    pub fn expand_container_at(
        &mut self,
        index: usize,
        entries: Vec<FileInfo>,
        direction: NavigationDirection,
    ) {
        if index >= self.files.len() {
            return;
        }

        // 展開エントリ群内部のみ初期表示順序としてソートする (全体は再ソートしない)
        let order = self.sort_order;
        let mut entries = entries;
        entries.sort_by(|a, b| order.compare(a, b));

        let entries_len = entries.len();
        let current = self.current_index;

        // プレースホルダを削除して展開結果を挿入する
        self.files.splice(index..=index, entries);

        // current_index は単純なインデックス算術で更新する。
        // splice により index..(index + entries_len) が新エントリで占められ、
        // それ以降の既存ファイルは (entries_len - 1) だけずれる (削除1個・挿入N個分)。
        self.current_index = match current {
            None => None,
            _ if self.files.is_empty() => None,
            Some(c) if c < index => Some(c),
            Some(c) if c == index => {
                if entries_len == 0 {
                    // 展開対象が現在位置でエントリ空: remove_at 相当のクランプ
                    Some(c.min(self.files.len() - 1))
                } else {
                    Some(match direction {
                        NavigationDirection::Forward => index,
                        NavigationDirection::Backward => index + entries_len - 1,
                    })
                }
            }
            Some(c) => {
                // c > index: 削除1個・挿入N個分のずれを反映 (entries_len == 0 なら -1)
                Some(c + entries_len - 1)
            }
        };
    }

    /// 操作前後で現在位置を維持するヘルパー
    fn with_position_preserved<F: FnOnce(&mut Self)>(&mut self, f: F) {
        let saved = self.current().map(|f| (f.path.clone(), f.source.clone()));
        f(self);
        if let Some((path, source)) = saved {
            self.restore_current_position(&path, &source);
        }
    }

    /// 現在のソート順で再ソートする
    pub fn sort_current(&mut self) {
        self.sort(self.sort_order);
    }

    /// ファイルリスト全体をシャッフルする (Fisher-Yates)
    /// 現在位置を維持する。
    pub fn shuffle_all(&mut self) {
        if self.files.len() <= 1 {
            return;
        }
        self.with_position_preserved(|this| {
            let len = this.files.len();
            let mut rng = SimpleRng::new();
            for i in (1..len).rev() {
                let j = rng.next_usize(i + 1);
                this.files.swap(i, j);
            }
        });
    }

    /// グループ (フォルダ/アーカイブ) の並び順をシャッフルする
    ///
    /// グループ内の出現順は維持し、グループ間の順序のみシャッフルする。
    /// 同一グループのファイルがリスト上で分断されていても、`group_key` で集約してから操作する。
    pub fn shuffle_groups(&mut self) {
        if self.files.is_empty() {
            return;
        }
        self.with_position_preserved(|this| {
            // group_key の出現順を保ったままファイルをグループへ振り分ける
            let mut group_indices: HashMap<groups::GroupKey, usize> = HashMap::new();
            let mut groups: Vec<Vec<FileInfo>> = Vec::new();
            for file in this.files.drain(..) {
                let key = group_key(&file);
                let idx = *group_indices.entry(key).or_insert_with(|| {
                    let idx = groups.len();
                    groups.push(Vec::new());
                    idx
                });
                groups[idx].push(file);
            }

            // グループ単位でFisher-Yatesシャッフル
            let glen = groups.len();
            if glen > 1 {
                let mut rng = SimpleRng::new();
                for i in (1..glen).rev() {
                    let j = rng.next_usize(i + 1);
                    groups.swap(i, j);
                }
            }

            // flattenして復元
            for group in groups {
                this.files.extend(group);
            }
        });
    }

    // --- マーク操作 ---

    /// 指定インデックスのファイルをマークする
    pub fn mark_at(&mut self, index: usize) {
        if let Some(info) = self.files.get_mut(index) {
            info.marked = true;
        }
    }

    /// 指定インデックスのファイルのマークを解除する
    pub fn unmark_at(&mut self, index: usize) {
        if let Some(info) = self.files.get_mut(index) {
            info.marked = false;
        }
    }

    /// 全ファイルのマーク状態を反転する
    pub fn invert_all_marks(&mut self) {
        for info in &mut self.files {
            info.marked = !info.marked;
        }
    }

    /// 最初から現在位置 (含む) までのマーク状態を反転する
    pub fn invert_marks_to_here(&mut self) {
        let end = self.current_index.map_or(0, |i| i + 1);
        for info in &mut self.files[..end] {
            info.marked = !info.marked;
        }
    }

    /// マーク済みファイルのインデックス一覧を返す
    pub fn marked_indices(&self) -> Vec<usize> {
        self.files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.marked)
            .map(|(i, _)| i)
            .collect()
    }

    /// マーク済みファイルの数
    pub fn marked_count(&self) -> usize {
        self.files.iter().filter(|f| f.marked).count()
    }

    /// 指定インデックスのファイルをリストから削除する
    /// current_indexを適切に調整する
    pub fn remove_at(&mut self, index: usize) -> Option<FileInfo> {
        if index >= self.files.len() {
            return None;
        }
        let removed = self.files.remove(index);

        // current_indexの調整
        if self.files.is_empty() {
            self.current_index = None;
        } else if let Some(current) = self.current_index {
            if index < current {
                // 現在位置より前が削除されたのでデクリメント
                self.current_index = Some(current - 1);
            } else if index == current {
                // 現在位置が削除された→同じ位置 (末尾超過ならクランプ)
                self.current_index = Some(current.min(self.files.len() - 1));
            }
        }

        Some(removed)
    }

    /// マーク済みファイルをリストから削除する
    /// 削除されたファイル一覧を返す
    pub fn remove_marked(&mut self) -> Vec<FileInfo> {
        // 現在のパスとsourceを記憶 (位置復元用)
        let current_info = self.current().map(|f| (f.path.clone(), f.source.clone()));

        let mut removed = Vec::new();
        let mut kept = Vec::new();
        for info in self.files.drain(..) {
            if info.marked {
                removed.push(info);
            } else {
                kept.push(info);
            }
        }
        self.files = kept;

        // current_indexの復元 (コンテナ内エントリはsourceベース)
        if let Some((path, source)) = current_info {
            self.restore_current_position(&path, &source);
        } else {
            self.current_index = if self.files.is_empty() { None } else { Some(0) };
        }

        removed
    }

    /// 前のマーク済み画像へ移動する
    pub fn navigate_prev_mark(&mut self) -> bool {
        let current = self.current_index.unwrap_or(0);
        let len = self.files.len();
        if len == 0 {
            return false;
        }
        // 現在位置から逆方向に検索 (ラップアラウンド)
        for offset in 1..len {
            let idx = (current + len - offset) % len;
            if self.files[idx].marked {
                self.current_index = Some(idx);
                return true;
            }
        }
        false
    }

    /// 次のマーク済み画像へ移動する
    pub fn navigate_next_mark(&mut self) -> bool {
        let current = self.current_index.unwrap_or(0);
        let len = self.files.len();
        if len == 0 {
            return false;
        }
        // 現在位置から順方向に検索 (ラップアラウンド)
        for offset in 1..len {
            let idx = (current + offset) % len;
            if self.files[idx].marked {
                self.current_index = Some(idx);
                return true;
            }
        }
        false
    }

    /// マーク済みファイルのパスを移動先ディレクトリに更新する
    /// 各エントリを `dest_dir/元ファイル名` で再構築し、マーク状態は維持する。
    ///
    /// 副次処理ではファイル一覧全体を自動再ソートしない方針のため、リスト順序および現在位置は
    /// そのまま維持し、対象エントリのフィールドのみ書き換える。
    pub fn update_marked_paths(&mut self, dest_dir: &Path) -> Result<()> {
        for info in &mut self.files {
            if !info.marked {
                continue;
            }
            let file_name = info
                .path
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("ファイル名取得失敗: {}", info.path.display()))?;
            let new_path = dest_dir.join(file_name);
            let new_info = FileInfo::from_path(&new_path)?;
            info.path = new_info.path;
            info.source = FileSource::File(new_path);
            info.file_name = new_info.file_name;
            info.file_size = new_info.file_size;
            info.modified = new_info.modified;
            // marked状態は維持 (trueのまま)
        }
        Ok(())
    }

    /// 前のフォルダ/アーカイブの最初のファイルへ移動する
    ///
    /// `group_key` の出現順を構築してから現在グループの 1 つ前を選び、その先頭ファイルへ移動する。
    /// グループが 1 つだけの場合は何もしない。先頭グループにいる場合は末尾グループへラップアラウンドする。
    pub fn navigate_prev_folder(&mut self) -> bool {
        if self.files.is_empty() {
            return false;
        }
        let current = self.current_index.unwrap_or(0);
        let (group_first_indices, file_group_idx) = compute_group_layout(&self.files);
        let total_groups = group_first_indices.len();
        if total_groups <= 1 {
            return false;
        }

        let current_group = file_group_idx[current];
        let target_group = if current_group == 0 {
            total_groups - 1
        } else {
            current_group - 1
        };
        let target = group_first_indices[target_group];
        if target == current {
            return false;
        }
        self.current_index = Some(target);
        true
    }

    /// 次のフォルダ/アーカイブの最初のファイルへ移動する
    ///
    /// `group_key` の出現順を構築してから現在グループの 1 つ後を選び、その先頭ファイルへ移動する。
    /// グループが 1 つだけの場合は何もしない。末尾グループにいる場合は先頭グループへラップアラウンドする。
    pub fn navigate_next_folder(&mut self) -> bool {
        if self.files.is_empty() {
            return false;
        }
        let current = self.current_index.unwrap_or(0);
        let (group_first_indices, file_group_idx) = compute_group_layout(&self.files);
        let total_groups = group_first_indices.len();
        if total_groups <= 1 {
            return false;
        }

        let current_group = file_group_idx[current];
        let target_group = (current_group + 1) % total_groups;
        let target = group_first_indices[target_group];
        if target == current {
            return false;
        }
        self.current_index = Some(target);
        true
    }

    /// 一時的にソート順で前後移動する (リスト順序は変えない)
    pub fn sorted_navigate(&mut self, direction: isize, sort_order: SortOrder) -> bool {
        let Some(current) = self.current_index else {
            return false;
        };
        if self.files.is_empty() {
            return false;
        }

        // ソート済みインデックスリストを生成
        let mut sorted_indices: Vec<usize> = (0..self.files.len()).collect();
        sorted_indices.sort_by(|&a, &b| sort_order.compare(&self.files[a], &self.files[b]));

        // 現在ファイルのソート済みリスト内位置を特定
        let pos = sorted_indices.iter().position(|&i| i == current);
        let Some(pos) = pos else {
            return false;
        };

        // direction方向の次のインデックスを取得
        let new_pos = (pos as isize + direction).rem_euclid(sorted_indices.len() as isize) as usize;
        let target = sorted_indices[new_pos];

        if target == current {
            return false;
        }
        self.current_index = Some(target);
        true
    }

    /// ファイル一覧への参照
    pub fn files(&self) -> &[FileInfo] {
        &self.files
    }

    /// 現在のファイル情報
    pub fn current(&self) -> Option<&FileInfo> {
        self.current_index.and_then(|i| self.files.get(i))
    }

    /// ファイル数
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// 現在のインデックス
    pub fn current_index(&self) -> Option<usize> {
        self.current_index
    }

    /// 現在のソート順
    pub fn sort_order(&self) -> SortOrder {
        self.sort_order
    }

    /// 指定インデックスのファイルエントリを新パスで再構築する (リネーム/移動後の更新用)
    /// リスト内の位置 (current_index) はそのまま維持する
    pub fn update_file_at(&mut self, index: usize, new_path: &Path) -> Result<()> {
        let info = self
            .files
            .get_mut(index)
            .ok_or_else(|| anyhow::anyhow!("インデックス範囲外: {index}"))?;

        let new_info = FileInfo::from_path(new_path)?;
        info.path = new_info.path;
        info.source = FileSource::File(new_path.to_path_buf());
        info.file_name = new_info.file_name;
        info.file_size = new_info.file_size;
        info.modified = new_info.modified;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn test_registry() -> Arc<ExtensionRegistry> {
        Arc::new(ExtensionRegistry::new())
    }

    /// テスト用のダミー画像ファイルを作成するヘルパー
    fn create_test_files(dir: &Path, names: &[&str]) {
        let _ = std::fs::create_dir_all(dir);
        for name in names {
            let mut f = std::fs::File::create(dir.join(name)).unwrap();
            f.write_all(b"dummy").unwrap();
        }
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn populate_filters_by_extension() {
        let dir = std::env::temp_dir().join("gv_test_fl_populate");
        create_test_files(&dir, &["a.jpg", "b.png", "c.txt", "d.bmp", "readme.md"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();

        assert_eq!(fl.len(), 3); // jpg, png, bmp
        cleanup(&dir);
    }

    #[test]
    fn natural_sort_order() {
        let dir = std::env::temp_dir().join("gv_test_fl_natural");
        create_test_files(&dir, &["img2.png", "img10.png", "img1.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.sort(SortOrder::Natural);

        let names: Vec<&str> = fl.files.iter().map(|f| f.file_name.as_str()).collect();
        assert_eq!(names, vec!["img1.png", "img2.png", "img10.png"]);
        cleanup(&dir);
    }

    #[test]
    fn natural_sort_case_insensitive() {
        let dir = std::env::temp_dir().join("gv_test_fl_natural_ci");
        create_test_files(&dir, &["IMG1.png", "img2.png", "Img10.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.sort(SortOrder::Natural);

        let names: Vec<&str> = fl.files.iter().map(|f| f.file_name.as_str()).collect();
        assert_eq!(names, vec!["IMG1.png", "img2.png", "Img10.png"]);
        cleanup(&dir);
    }

    #[test]
    fn navigate_relative_wraps_around() {
        let dir = std::env::temp_dir().join("gv_test_fl_nav");
        create_test_files(&dir, &["a.png", "b.png", "c.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.navigate_to(0);

        // 先頭で後退 → 末尾へラップ
        assert!(fl.navigate_relative(-1));
        assert_eq!(fl.current_index(), Some(2));

        // 末尾で前進 → 先頭へラップ
        assert!(fl.navigate_relative(1));
        assert_eq!(fl.current_index(), Some(0));

        // 通常の相対移動
        fl.navigate_to(1);
        assert!(fl.navigate_relative(1));
        assert_eq!(fl.current_index(), Some(2));

        cleanup(&dir);
    }

    #[test]
    fn navigate_first_last() {
        let dir = std::env::temp_dir().join("gv_test_fl_firstlast");
        create_test_files(&dir, &["a.png", "b.png", "c.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.navigate_to(1);

        fl.navigate_last();
        assert_eq!(fl.current_index(), Some(2));

        fl.navigate_first();
        assert_eq!(fl.current_index(), Some(0));

        cleanup(&dir);
    }

    #[test]
    fn set_current_by_path_found() {
        let dir = std::env::temp_dir().join("gv_test_fl_setpath");
        create_test_files(&dir, &["a.png", "b.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();

        assert!(fl.set_current_by_path(&dir.join("b.png")));
        assert_eq!(fl.current().unwrap().file_name, "b.png");

        cleanup(&dir);
    }

    #[test]
    fn sort_preserves_current_position() {
        let dir = std::env::temp_dir().join("gv_test_fl_sortpreserve");
        create_test_files(&dir, &["c.png", "a.png", "b.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.set_current_by_path(&dir.join("b.png"));

        fl.sort(SortOrder::Name);
        // ソート後もb.pngが選択されている
        assert_eq!(fl.current().unwrap().file_name, "b.png");

        cleanup(&dir);
    }

    #[test]
    fn empty_list_navigation() {
        let mut fl = FileList::new(test_registry());
        assert!(!fl.navigate_relative(1));
        assert!(!fl.navigate_first());
        assert!(!fl.navigate_last());
        assert!(fl.current().is_none());
    }

    // --- マーク機能テスト ---

    #[test]
    fn mark_and_unmark() {
        let dir = std::env::temp_dir().join("gv_test_fl_mark");
        create_test_files(&dir, &["a.png", "b.png", "c.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.navigate_to(0);

        // マーク設定
        fl.mark_at(1);
        assert!(fl.files[1].marked);
        assert_eq!(fl.marked_count(), 1);

        // マーク解除
        fl.unmark_at(1);
        assert!(!fl.files[1].marked);
        assert_eq!(fl.marked_count(), 0);

        cleanup(&dir);
    }

    #[test]
    fn invert_all_marks() {
        let dir = std::env::temp_dir().join("gv_test_fl_invert");
        create_test_files(&dir, &["a.png", "b.png", "c.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.mark_at(0);

        fl.invert_all_marks();
        assert!(!fl.files[0].marked);
        assert!(fl.files[1].marked);
        assert!(fl.files[2].marked);
        assert_eq!(fl.marked_count(), 2);

        cleanup(&dir);
    }

    #[test]
    fn invert_marks_to_here() {
        let dir = std::env::temp_dir().join("gv_test_fl_invert_here");
        create_test_files(&dir, &["a.png", "b.png", "c.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.navigate_to(1);

        fl.invert_marks_to_here();
        // index 0, 1 が反転 (false→true)、index 2 は変化なし
        assert!(fl.files[0].marked);
        assert!(fl.files[1].marked);
        assert!(!fl.files[2].marked);

        cleanup(&dir);
    }

    #[test]
    fn remove_at_adjusts_current_index() {
        let dir = std::env::temp_dir().join("gv_test_fl_removeat");
        create_test_files(&dir, &["a.png", "b.png", "c.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.navigate_to(2); // 最後

        // 現在位置より前を削除 → indexデクリメント
        fl.remove_at(0);
        assert_eq!(fl.current_index(), Some(1));
        assert_eq!(fl.len(), 2);

        // 現在位置を削除 → クランプ
        fl.remove_at(1);
        assert_eq!(fl.current_index(), Some(0));
        assert_eq!(fl.len(), 1);

        cleanup(&dir);
    }

    #[test]
    fn remove_marked() {
        let dir = std::env::temp_dir().join("gv_test_fl_removemarked");
        create_test_files(&dir, &["a.png", "b.png", "c.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.navigate_to(1);
        fl.mark_at(0);
        fl.mark_at(2);

        let removed = fl.remove_marked();
        assert_eq!(removed.len(), 2);
        assert_eq!(fl.len(), 1);
        // b.pngが残っている
        assert_eq!(fl.current().unwrap().file_name, "b.png");

        cleanup(&dir);
    }

    #[test]
    fn navigate_marks() {
        let dir = std::env::temp_dir().join("gv_test_fl_navmark");
        create_test_files(&dir, &["a.png", "b.png", "c.png", "d.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.navigate_to(0);
        fl.mark_at(1);
        fl.mark_at(3);

        // 次のマーク
        assert!(fl.navigate_next_mark());
        assert_eq!(fl.current_index(), Some(1));

        assert!(fl.navigate_next_mark());
        assert_eq!(fl.current_index(), Some(3));

        // ラップアラウンド
        assert!(fl.navigate_next_mark());
        assert_eq!(fl.current_index(), Some(1));

        // 前のマーク
        assert!(fl.navigate_prev_mark());
        assert_eq!(fl.current_index(), Some(3));

        cleanup(&dir);
    }

    #[test]
    fn sorted_navigate_forward_backward() {
        let dir = std::env::temp_dir().join("gv_test_fl_sortnav");
        // 自然順: a.png, b.png, c.png
        // サイズ順は同じ (全てdummy 5バイト) なので名前順で確認
        create_test_files(&dir, &["c.png", "a.png", "b.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        // Natural sort: a.png(0), b.png(1), c.png(2)
        fl.navigate_to(0); // a.png

        // 自然順で次→ b.png
        assert!(fl.sorted_navigate(1, SortOrder::Natural));
        assert_eq!(fl.current().unwrap().file_name, "b.png");

        // 自然順で次→ c.png
        assert!(fl.sorted_navigate(1, SortOrder::Natural));
        assert_eq!(fl.current().unwrap().file_name, "c.png");

        // 自然順で次→ ラップアラウンドで a.png
        assert!(fl.sorted_navigate(1, SortOrder::Natural));
        assert_eq!(fl.current().unwrap().file_name, "a.png");

        // 自然順で前→ c.png
        assert!(fl.sorted_navigate(-1, SortOrder::Natural));
        assert_eq!(fl.current().unwrap().file_name, "c.png");

        cleanup(&dir);
    }

    #[test]
    fn navigate_marks_none_marked() {
        let dir = std::env::temp_dir().join("gv_test_fl_navmark_none");
        create_test_files(&dir, &["a.png", "b.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.navigate_to(0);

        // マークなし → 移動しない
        assert!(!fl.navigate_next_mark());
        assert!(!fl.navigate_prev_mark());

        cleanup(&dir);
    }

    #[test]
    fn populate_single_creates_one_entry() {
        let dir = std::env::temp_dir().join("gv_test_fl_single");
        create_test_files(&dir, &["target.png", "other.png", "another.jpg"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_single(&dir.join("target.png")).unwrap();

        // フォルダ内の他のファイルはリストに含まれない
        assert_eq!(fl.len(), 1);
        assert_eq!(fl.current_index(), Some(0));
        assert_eq!(fl.current().unwrap().file_name, "target.png");

        cleanup(&dir);
    }

    /// テスト用のアーカイブエントリFileInfoを作成するヘルパー
    fn make_archive_file_info(archive: &str, entry: &str, file_name: &str) -> FileInfo {
        FileInfo {
            path: std::path::PathBuf::from(format!("/tmp/{file_name}")),
            file_name: file_name.to_string(),
            file_size: 100,
            modified: std::time::SystemTime::UNIX_EPOCH,
            marked: false,
            load_failed: false,
            source: FileSource::ArchiveEntry {
                archive: std::path::PathBuf::from(archive),
                entry: entry.to_string(),
                on_demand: false,
            },
        }
    }

    #[test]
    fn sort_orders_by_logical_path_across_groups() {
        // 出現順とは無関係に、論理パス (アーカイブパス＋内部パス) で完全に並ぶ
        let registry = test_registry();
        let mut fl = FileList::new(registry);

        // 出現順は B, A だが、論理パス順では archive_a が先
        fl.push(make_archive_file_info("archive_b.zip", "x.png", "x.png"));
        fl.push(make_archive_file_info("archive_a.zip", "y.png", "y.png"));
        fl.push(make_archive_file_info("archive_a.zip", "z.png", "z.png"));

        fl.sort(SortOrder::Natural);

        let paths: Vec<String> = fl.files.iter().map(|f| f.source.display_path()).collect();
        assert_eq!(
            paths,
            vec![
                "archive_a.zip/y.png".to_string(),
                "archive_a.zip/z.png".to_string(),
                "archive_b.zip/x.png".to_string(),
            ]
        );
    }

    #[test]
    fn shuffle_all_preserves_elements_and_position() {
        let dir = std::env::temp_dir().join("gv_test_fl_shuffle_all");
        create_test_files(&dir, &["a.png", "b.png", "c.png", "d.png", "e.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.set_current_by_path(&dir.join("c.png"));

        fl.shuffle_all();

        // 全要素が保持されている
        let mut names: Vec<String> = fl.files.iter().map(|f| f.file_name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["a.png", "b.png", "c.png", "d.png", "e.png"]);

        // 現在位置がc.pngを指している
        assert_eq!(fl.current().unwrap().file_name, "c.png");

        cleanup(&dir);
    }

    #[test]
    fn shuffle_groups_preserves_intra_group_order() {
        let registry = test_registry();
        let mut fl = FileList::new(registry);

        // グループA: a1, a2, a3(この順)
        fl.push(make_archive_file_info("archive_a.zip", "a1.png", "a1.png"));
        fl.push(make_archive_file_info("archive_a.zip", "a2.png", "a2.png"));
        fl.push(make_archive_file_info("archive_a.zip", "a3.png", "a3.png"));
        // グループB: b1, b2
        fl.push(make_archive_file_info("archive_b.zip", "b1.png", "b1.png"));
        fl.push(make_archive_file_info("archive_b.zip", "b2.png", "b2.png"));

        fl.navigate_to(0);
        fl.shuffle_groups();

        // 全要素が保持されている
        assert_eq!(fl.len(), 5);

        // グループ内の順序が保持されている
        // グループAの相対順序を確認
        let positions: Vec<usize> = fl
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.file_name.starts_with('a'))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(positions.len(), 3);
        // 連続している (隣接) かつ順序が保持
        assert_eq!(positions[1] - positions[0], 1);
        assert_eq!(positions[2] - positions[1], 1);
        let a_names: Vec<&str> = positions
            .iter()
            .map(|&i| fl.files[i].file_name.as_str())
            .collect();
        assert_eq!(a_names, vec!["a1.png", "a2.png", "a3.png"]);

        // グループBの相対順序も保持
        let b_positions: Vec<usize> = fl
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.file_name.starts_with('b'))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(b_positions.len(), 2);
        assert_eq!(b_positions[1] - b_positions[0], 1);
        let b_names: Vec<&str> = b_positions
            .iter()
            .map(|&i| fl.files[i].file_name.as_str())
            .collect();
        assert_eq!(b_names, vec!["b1.png", "b2.png"]);
    }

    // --- sorted_navigate: 空リストのテスト ---

    #[test]
    fn sorted_navigate_empty_list() {
        let mut fl = FileList::new(test_registry());
        // current_index = None, files = empty
        assert!(!fl.sorted_navigate(1, SortOrder::Natural));
        assert!(!fl.sorted_navigate(-1, SortOrder::Name));
    }

    #[test]
    fn sorted_navigate_single_element() {
        let dir = std::env::temp_dir().join("gv_test_fl_sortnav_single");
        create_test_files(&dir, &["only.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.navigate_to(0);

        // 1要素のリストでは移動先 = 自分なのでfalse
        assert!(!fl.sorted_navigate(1, SortOrder::Natural));
        assert!(!fl.sorted_navigate(-1, SortOrder::Natural));

        cleanup(&dir);
    }

    // --- フォルダナビゲーション: マルチグループ境界テスト ---

    #[test]
    fn navigate_folder_three_groups() {
        // 3つのグループ: A(2件), B(1件), C(2件)
        let registry = test_registry();
        let mut fl = FileList::new(registry);

        fl.push(make_archive_file_info("archive_a.zip", "a1.png", "a1.png"));
        fl.push(make_archive_file_info("archive_a.zip", "a2.png", "a2.png"));
        fl.push(make_archive_file_info("archive_b.zip", "b1.png", "b1.png"));
        fl.push(make_archive_file_info("archive_c.zip", "c1.png", "c1.png"));
        fl.push(make_archive_file_info("archive_c.zip", "c2.png", "c2.png"));

        // グループAの2番目から次のフォルダへ
        fl.navigate_to(1); // a2.png
        assert!(fl.navigate_next_folder());
        assert_eq!(fl.current_index(), Some(2)); // b1.png(グループBの先頭)

        // グループBから次のフォルダへ
        assert!(fl.navigate_next_folder());
        assert_eq!(fl.current_index(), Some(3)); // c1.png(グループCの先頭)

        // グループCの末尾から次のフォルダへ → 先頭グループAへラップアラウンド
        assert!(fl.navigate_next_folder());
        assert_eq!(fl.current_index(), Some(0)); // a1.png(グループAの先頭)

        // グループAから前のフォルダへ → 末尾グループCへラップアラウンド
        assert!(fl.navigate_prev_folder());
        assert_eq!(fl.current_index(), Some(3)); // c1.png(グループCの先頭)

        // グループCから前のフォルダへ
        assert!(fl.navigate_prev_folder());
        assert_eq!(fl.current_index(), Some(2)); // b1.png(グループBの先頭)

        // グループBから前のフォルダへ
        assert!(fl.navigate_prev_folder());
        assert_eq!(fl.current_index(), Some(0)); // a1.png(グループAの先頭)
    }

    #[test]
    fn navigate_folder_single_group() {
        // グループが1つだけの場合
        let registry = test_registry();
        let mut fl = FileList::new(registry);

        fl.push(make_archive_file_info("only.zip", "x1.png", "x1.png"));
        fl.push(make_archive_file_info("only.zip", "x2.png", "x2.png"));
        fl.navigate_to(0);

        // 前後どちらにもグループがない
        assert!(!fl.navigate_next_folder());
        assert!(!fl.navigate_prev_folder());
    }

    #[test]
    fn navigate_folder_empty_list() {
        let mut fl = FileList::new(test_registry());
        assert!(!fl.navigate_next_folder());
        assert!(!fl.navigate_prev_folder());
    }

    #[test]
    fn navigate_prev_folder_from_middle_of_group() {
        // グループ内の途中から前のフォルダに移動すると、前グループの先頭に行く
        let registry = test_registry();
        let mut fl = FileList::new(registry);

        fl.push(make_archive_file_info("archive_a.zip", "a1.png", "a1.png"));
        fl.push(make_archive_file_info("archive_a.zip", "a2.png", "a2.png"));
        fl.push(make_archive_file_info("archive_a.zip", "a3.png", "a3.png"));
        fl.push(make_archive_file_info("archive_b.zip", "b1.png", "b1.png"));
        fl.push(make_archive_file_info("archive_b.zip", "b2.png", "b2.png"));
        fl.push(make_archive_file_info("archive_b.zip", "b3.png", "b3.png"));

        // グループBの途中 (b2.png) から前のフォルダへ
        fl.navigate_to(4); // b2.png
        assert!(fl.navigate_prev_folder());
        assert_eq!(fl.current_index(), Some(0)); // a1.png(グループAの先頭)
    }

    // --- navigate_relative: load_failedスキップのテスト ---

    #[test]
    fn navigate_relative_skips_failed() {
        let registry = test_registry();
        let mut fl = FileList::new(registry);

        fl.push(make_archive_file_info("a.zip", "a.png", "a.png"));
        fl.push(make_archive_file_info("a.zip", "b.png", "b.png"));
        fl.push(make_archive_file_info("a.zip", "c.png", "c.png"));
        fl.navigate_to(0);

        // b.pngをfailed状態にする
        fl.mark_failed(1);

        // 前進: a→b(failed)→c
        assert!(fl.navigate_relative(1));
        assert_eq!(fl.current_index(), Some(2)); // bをスキップしてcに到達
    }

    #[test]
    fn navigate_relative_all_failed() {
        let registry = test_registry();
        let mut fl = FileList::new(registry);

        fl.push(make_archive_file_info("a.zip", "a.png", "a.png"));
        fl.push(make_archive_file_info("a.zip", "b.png", "b.png"));
        fl.navigate_to(0);

        // 全てfailedにする
        fl.mark_failed(0);
        fl.mark_failed(1);

        // 移動先がないのでfalse
        assert!(!fl.navigate_relative(1));
    }

    /// テスト用の PendingContainer FileInfo を作成するヘルパー
    fn make_pending_container_info(container_path: &str) -> FileInfo {
        FileInfo {
            path: std::path::PathBuf::from(container_path),
            file_name: container_path.to_string(),
            file_size: 0,
            modified: std::time::SystemTime::UNIX_EPOCH,
            marked: false,
            load_failed: false,
            source: FileSource::PendingContainer {
                container_path: std::path::PathBuf::from(container_path),
            },
        }
    }

    #[test]
    fn expand_container_at_forward_places_current_at_first() {
        let registry = test_registry();
        let mut fl = FileList::new(registry);
        // [a.zip 展開済] [pending.zip] [c.zip 展開済]
        fl.push(make_archive_file_info("a.zip", "a1.png", "a1.png"));
        fl.push(make_pending_container_info("pending.zip"));
        fl.push(make_archive_file_info("c.zip", "c1.png", "c1.png"));
        fl.navigate_to(1); // PendingContainer 上

        let entries = vec![
            make_archive_file_info("pending.zip", "p1.png", "p1.png"),
            make_archive_file_info("pending.zip", "p2.png", "p2.png"),
            make_archive_file_info("pending.zip", "p3.png", "p3.png"),
        ];
        fl.expand_container_at(1, entries, NavigationDirection::Forward);

        assert_eq!(fl.len(), 5);
        // プレースホルダ位置に展開エントリが挿入され、全体順序は維持される
        let names: Vec<&str> = fl.files.iter().map(|f| f.file_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["a1.png", "p1.png", "p2.png", "p3.png", "c1.png"]
        );
        // Forward なので展開エントリの先頭 p1.png が現在位置となる
        assert_eq!(fl.current().unwrap().file_name, "p1.png");
        assert_eq!(fl.current_index(), Some(1));
    }

    #[test]
    fn expand_container_at_backward_places_current_at_last() {
        let registry = test_registry();
        let mut fl = FileList::new(registry);
        fl.push(make_archive_file_info("a.zip", "a1.png", "a1.png"));
        fl.push(make_pending_container_info("pending.zip"));
        fl.push(make_archive_file_info("c.zip", "c1.png", "c1.png"));
        fl.navigate_to(1); // PendingContainer 上

        let entries = vec![
            make_archive_file_info("pending.zip", "p1.png", "p1.png"),
            make_archive_file_info("pending.zip", "p2.png", "p2.png"),
            make_archive_file_info("pending.zip", "p3.png", "p3.png"),
        ];
        fl.expand_container_at(1, entries, NavigationDirection::Backward);

        assert_eq!(fl.len(), 5);
        let names: Vec<&str> = fl.files.iter().map(|f| f.file_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["a1.png", "p1.png", "p2.png", "p3.png", "c1.png"]
        );
        // Backward なので展開エントリの末尾 p3.png が現在位置となる
        assert_eq!(fl.current().unwrap().file_name, "p3.png");
        assert_eq!(fl.current_index(), Some(3));
    }

    #[test]
    fn expand_container_at_before_current_keeps_current_file() {
        let registry = test_registry();
        let mut fl = FileList::new(registry);
        fl.push(make_pending_container_info("pending.zip"));
        fl.push(make_archive_file_info("c.zip", "c1.png", "c1.png"));
        fl.push(make_archive_file_info("c.zip", "c2.png", "c2.png"));
        fl.navigate_to(2); // c2.png

        let entries = vec![
            make_archive_file_info("pending.zip", "p1.png", "p1.png"),
            make_archive_file_info("pending.zip", "p2.png", "p2.png"),
            make_archive_file_info("pending.zip", "p3.png", "p3.png"),
        ];
        // current_index は c2.png (展開位置の外)。direction は使われない
        fl.expand_container_at(0, entries, NavigationDirection::Forward);

        assert_eq!(fl.len(), 5);
        // プレースホルダ位置に展開エントリが挿入され、それ以降の既存ファイルが entries_len - 1 だけずれる
        let names: Vec<&str> = fl.files.iter().map(|f| f.file_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["p1.png", "p2.png", "p3.png", "c1.png", "c2.png"]
        );
        // 現在ファイル c2.png は維持され、index は 2 → 4 にずれる (削除1個・挿入3個)
        assert_eq!(fl.current().unwrap().file_name, "c2.png");
        assert_eq!(fl.current_index(), Some(4));
    }

    /// テスト用に任意の `FileSource` から `FileInfo` を作るヘルパー。
    /// 論理パス順を確認したいテスト用なので、`path` (実ファイルパス) は適当な代表値を入れる。
    fn make_file_info_with_source(
        source: FileSource,
        size: u64,
        modified: std::time::SystemTime,
    ) -> FileInfo {
        let path = match &source {
            FileSource::File(p) => p.clone(),
            FileSource::ArchiveEntry { archive, entry, .. } => archive.join(entry),
            FileSource::PdfPage {
                pdf_path,
                page_index,
            } => pdf_path.with_file_name(format!("page{page_index}.png")),
            FileSource::PendingContainer { container_path } => container_path.clone(),
        };
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        FileInfo {
            path,
            file_name,
            file_size: size,
            modified,
            marked: false,
            load_failed: false,
            source,
        }
    }

    #[test]
    fn sort_logical_path_orders_mixed_sources() {
        // 通常ファイル (フォルダ違い・サブフォルダ違い)、アーカイブエントリ、PDF ページが混在しても
        // 論理パス順で並ぶことを確認する
        let registry = test_registry();
        let mut fl = FileList::new(registry);
        let now = std::time::SystemTime::UNIX_EPOCH;

        // 出現順はあえて論理パス順とは異なるようにする
        fl.push(make_file_info_with_source(
            FileSource::PdfPage {
                pdf_path: PathBuf::from("/folder_b/doc.pdf"),
                page_index: 0,
            },
            100,
            now,
        ));
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/folder_a/sub/img2.png")),
            100,
            now,
        ));
        fl.push(make_file_info_with_source(
            FileSource::ArchiveEntry {
                archive: PathBuf::from("/folder_b/archive.zip"),
                entry: "inner.png".to_string(),
                on_demand: false,
            },
            100,
            now,
        ));
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/folder_a/img1.png")),
            100,
            now,
        ));

        fl.sort(SortOrder::Natural);

        let paths: Vec<String> = fl.files.iter().map(|f| f.source.display_path()).collect();
        assert_eq!(
            paths,
            vec![
                "/folder_a/img1.png".to_string(),
                "/folder_a/sub/img2.png".to_string(),
                "/folder_b/archive.zip/inner.png".to_string(),
                "/folder_b/doc.pdf/Page 1".to_string(),
            ]
        );
    }

    #[test]
    fn sort_size_uses_logical_path_as_tiebreaker() {
        // Size 順は値が主キー、同値時に論理パスをタイブレーカーとする
        let registry = test_registry();
        let mut fl = FileList::new(registry);
        let now = std::time::SystemTime::UNIX_EPOCH;

        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/x/c.png")),
            100,
            now,
        ));
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/x/b.png")),
            100,
            now,
        ));
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/x/a.png")),
            100,
            now,
        ));

        fl.sort(SortOrder::Size);

        let names: Vec<&str> = fl.files.iter().map(|f| f.file_name.as_str()).collect();
        assert_eq!(names, vec!["a.png", "b.png", "c.png"]);
    }

    #[test]
    fn sort_date_uses_logical_path_as_tiebreaker() {
        // Date 順は値が主キー、同値時に論理パスをタイブレーカーとする
        let registry = test_registry();
        let mut fl = FileList::new(registry);
        let epoch = std::time::SystemTime::UNIX_EPOCH;
        let future = epoch + std::time::Duration::from_secs(100);

        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/x/c.png")),
            100,
            future,
        ));
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/x/b.png")),
            100,
            epoch,
        ));
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/x/a.png")),
            100,
            epoch,
        ));

        fl.sort(SortOrder::Date);

        let names: Vec<&str> = fl.files.iter().map(|f| f.file_name.as_str()).collect();
        // epoch と epoch (a, b が同じ日時) → a, b の論理パス順
        // その後 future → c
        assert_eq!(names, vec!["a.png", "b.png", "c.png"]);
    }
}
