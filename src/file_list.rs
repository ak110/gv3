use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use crate::extension_registry::ExtensionRegistry;
use crate::file_info::{FileInfo, FileSource};

/// フォルダ/アーカイブ単位ナビゲーション用のグループキー
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum GroupKey {
    Folder(std::path::PathBuf),
    Archive(std::path::PathBuf),
}

/// 軽量PRNG（xorshift64）。シャッフル用途のため暗号強度は不要。
struct SimpleRng(u64);

impl SimpleRng {
    fn new() -> Self {
        let mut buf = [0u8; 8];
        getrandom::fill(&mut buf).expect("OS乱数ソースの取得に失敗");
        let seed = u64::from_ne_bytes(buf);
        Self(seed | 1) // 0シード回避
    }

    /// xorshift64ステップを実行し、次の状態を返す
    fn step(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// [0, bound) の範囲で一様分布する乱数を返す（Lemire法）
    ///
    /// 参考: Daniel Lemire, "Fast Random Integer Generation in an Interval",
    /// ACM Trans. Model. Comput. Simul., 2019
    fn next_usize(&mut self, bound: usize) -> usize {
        let s = bound as u64;
        let mut m = self.step() as u128 * s as u128;
        let mut l = m as u64;
        if l < s {
            // rejection threshold: (2^64 - s) % s
            let t = s.wrapping_neg() % s;
            while l < t {
                m = self.step() as u128 * s as u128;
                l = m as u64;
            }
        }
        (m >> 64) as usize
    }
}

/// ソート順
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    /// ファイル名順
    #[default]
    Name,
    /// ファイル名順（大文字小文字区別なし）
    #[serde(rename = "name_nocase")]
    NameNoCase,
    /// ファイルサイズ順
    Size,
    /// 最終更新日時順
    Date,
    /// 自然順ソート（数値認識）
    Natural,
}

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
            // 拡張子フィルタ（ExtensionRegistry経由）
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

    /// 単一ファイルのみでリストを構築する（フォルダスキャンしない）
    pub fn populate_single(&mut self, path: &Path) -> Result<()> {
        self.files.clear();
        self.current_index = None;

        let info = FileInfo::from_path(path)?;
        self.files.push(info);
        self.current_index = Some(0);
        Ok(())
    }

    /// FileSourceの同一性判定（コンテナ内エントリの位置復元用）
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

    /// ソート/削除後の位置復元（コンテナ内エントリはsourceで、通常ファイルはpathで復元）
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

    /// 相対移動（ラップアラウンド）
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

        // スキップ方向（offsetの符号に合わせる）
        let step: isize = if offset >= 0 { 1 } else { -1 };

        // load_failedのファイルを同方向にスキップ（最大len回で打ち切り）
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

    /// ソート実行（現在位置を維持）
    /// グループ（フォルダ/アーカイブ）の出現順を保持しつつ、グループ内でソートする
    pub fn sort(&mut self, order: SortOrder) {
        self.with_position_preserved(|this| {
            // グループ出現順を記録
            let mut group_order: HashMap<GroupKey, usize> = HashMap::new();
            for file in &this.files {
                let key = Self::group_key(file);
                let next_id = group_order.len();
                group_order.entry(key).or_insert(next_id);
            }

            // 第1キー=グループ出現順、第2キー=ソート順
            this.files.sort_by(|a, b| {
                let ga = group_order.get(&Self::group_key(a)).copied().unwrap_or(0);
                let gb = group_order.get(&Self::group_key(b)).copied().unwrap_or(0);
                ga.cmp(&gb)
                    .then_with(|| Self::compare_by_sort_order(a, b, order))
            });

            this.sort_order = order;
        });
    }

    /// 指定インデックスのファイルをデコード失敗状態にする
    pub fn mark_failed(&mut self, index: usize) {
        if let Some(info) = self.files.get_mut(index) {
            info.load_failed = true;
        }
    }

    /// 全ファイルの失敗状態をクリア（フォルダ再読み込み時用）
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
    /// 挿入前にグループ内ソートを適用する。
    /// current_indexを適切に調整する。
    pub fn expand_container_at(&mut self, index: usize, mut entries: Vec<FileInfo>) {
        if index >= self.files.len() {
            return;
        }

        // 挿入前にグループ内ソートを適用
        let order = self.sort_order;
        entries.sort_by(|a, b| Self::compare_by_sort_order(a, b, order));

        let entries_len = entries.len();

        // プレースホルダを削除して展開結果を挿入
        self.files.splice(index..=index, entries);

        // current_indexの調整
        if let Some(current) = self.current_index {
            if entries_len == 0 {
                // エントリ無し（空コンテナ）: remove_at相当の調整
                if self.files.is_empty() {
                    self.current_index = None;
                } else if index < current {
                    self.current_index = Some(current - 1);
                } else if index == current {
                    self.current_index = Some(current.min(self.files.len() - 1));
                }
            } else if index < current {
                // 展開位置が現在位置より前: entries_len - 1 分だけシフト
                self.current_index = Some(current + entries_len - 1);
            } else if index == current {
                // 展開位置が現在位置: 展開グループの先頭を指す
                // （current_indexはそのまま = 展開結果の先頭）
            }
        }
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

    /// ファイルリスト全体をシャッフルする（Fisher-Yates）
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

    /// グループ（フォルダ/アーカイブ）の並び順をシャッフルする
    /// グループ内の順序は保持し、グループ間の順序のみ変更する。
    pub fn shuffle_groups(&mut self) {
        if self.files.is_empty() {
            return;
        }
        self.with_position_preserved(|this| {
            // グループを出現順に分割
            let mut groups: Vec<Vec<FileInfo>> = Vec::new();
            let mut current_key = Self::group_key(&this.files[0]);
            let mut current_group = Vec::new();

            for file in this.files.drain(..) {
                let key = Self::group_key(&file);
                if key == current_key {
                    current_group.push(file);
                } else {
                    groups.push(current_group);
                    current_group = vec![file];
                    current_key = key;
                }
            }
            groups.push(current_group);

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

    /// 最初から現在位置（含む）までのマーク状態を反転する
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
                // 現在位置が削除された→同じ位置（末尾超過ならクランプ）
                self.current_index = Some(current.min(self.files.len() - 1));
            }
        }

        Some(removed)
    }

    /// マーク済みファイルをリストから削除する
    /// 削除されたファイル一覧を返す
    pub fn remove_marked(&mut self) -> Vec<FileInfo> {
        // 現在のパスとsourceを記憶（位置復元用）
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

        // current_indexの復元（コンテナ内エントリはsourceベース）
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
        // 現在位置から逆方向に検索（ラップアラウンド）
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
        // 現在位置から順方向に検索（ラップアラウンド）
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
    /// 各エントリを `dest_dir/元ファイル名` で再構築し、マーク状態は維持する
    pub fn update_marked_paths(&mut self, dest_dir: &Path) -> Result<()> {
        let current_info = self.current().map(|f| (f.path.clone(), f.source.clone()));

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
            // marked状態は維持（trueのまま）
        }

        // 再ソートして現在位置を復元
        self.sort(self.sort_order);
        if let Some((path, source)) = current_info {
            self.restore_current_position(&path, &source);
        }
        Ok(())
    }

    /// ファイルのグループキーを返す（フォルダ=親ディレクトリ or アーカイブパス）
    fn group_key(info: &FileInfo) -> GroupKey {
        match &info.source {
            FileSource::ArchiveEntry { archive, .. } => GroupKey::Archive(archive.clone()),
            FileSource::PdfPage { pdf_path, .. } => GroupKey::Archive(pdf_path.clone()),
            FileSource::PendingContainer { container_path } => {
                GroupKey::Archive(container_path.clone())
            }
            FileSource::File(_) => GroupKey::Folder(
                info.path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default(),
            ),
        }
    }

    /// 前のフォルダ/アーカイブの最初のファイルへ移動する
    pub fn navigate_prev_folder(&mut self) -> bool {
        let current = self.current_index.unwrap_or(0);
        if self.files.is_empty() {
            return false;
        }

        let current_key = Self::group_key(&self.files[current]);

        // 現在グループの先頭を探す
        let mut group_start = current;
        while group_start > 0 && Self::group_key(&self.files[group_start - 1]) == current_key {
            group_start -= 1;
        }

        if group_start == 0 {
            return false;
        }

        // 前のグループの先頭へ
        let prev_key = Self::group_key(&self.files[group_start - 1]);
        let mut target = group_start - 1;
        while target > 0 && Self::group_key(&self.files[target - 1]) == prev_key {
            target -= 1;
        }

        self.current_index = Some(target);
        true
    }

    /// 次のフォルダ/アーカイブの最初のファイルへ移動する
    pub fn navigate_next_folder(&mut self) -> bool {
        let current = self.current_index.unwrap_or(0);
        if self.files.is_empty() {
            return false;
        }

        let current_key = Self::group_key(&self.files[current]);

        // 現在グループの末尾+1を探す
        let mut next_start = current + 1;
        while next_start < self.files.len()
            && Self::group_key(&self.files[next_start]) == current_key
        {
            next_start += 1;
        }

        if next_start >= self.files.len() {
            return false;
        }

        self.current_index = Some(next_start);
        true
    }

    /// 一時的にソート順で前後移動する（リスト順序は変えない）
    pub fn sorted_navigate(&mut self, direction: isize, sort_order: SortOrder) -> bool {
        let Some(current) = self.current_index else {
            return false;
        };
        if self.files.is_empty() {
            return false;
        }

        // ソート済みインデックスリストを生成
        let mut sorted_indices: Vec<usize> = (0..self.files.len()).collect();
        sorted_indices.sort_by(|&a, &b| {
            Self::compare_by_sort_order(&self.files[a], &self.files[b], sort_order)
        });

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

    /// ソート順による比較
    fn compare_by_sort_order(a: &FileInfo, b: &FileInfo, order: SortOrder) -> std::cmp::Ordering {
        match order {
            SortOrder::Name => a.file_name.cmp(&b.file_name),
            SortOrder::NameNoCase => a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()),
            SortOrder::Size => a.file_size.cmp(&b.file_size),
            SortOrder::Date => a.modified.cmp(&b.modified),
            SortOrder::Natural => natord::compare_ignore_case(&a.file_name, &b.file_name),
        }
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

    /// 指定インデックスのファイルエントリを新パスで再構築する（リネーム/移動後の更新用）
    /// リスト内の位置（current_index）はそのまま維持する
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
        // index 0, 1 が反転（false→true）、index 2 は変化なし
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
        // サイズ順は同じ（全てdummy 5バイト）なので名前順で確認
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
    fn sort_preserves_group_order() {
        // 複数グループ（異なるアーカイブ）のファイルがソート後もグループ順を保持する
        let registry = test_registry();
        let mut fl = FileList::new(registry);

        // グループA → グループB の出現順で追加
        fl.push(make_archive_file_info(
            "archive_a.zip",
            "z_last.png",
            "z_last.png",
        ));
        fl.push(make_archive_file_info(
            "archive_a.zip",
            "a_first.png",
            "a_first.png",
        ));
        fl.push(make_archive_file_info(
            "archive_b.zip",
            "b_middle.png",
            "b_middle.png",
        ));

        fl.sort(SortOrder::Natural);

        let names: Vec<&str> = fl.files.iter().map(|f| f.file_name.as_str()).collect();
        // グループA（出現順0）が先、グループ内はソート済み
        // グループB（出現順1）が後
        assert_eq!(names, vec!["a_first.png", "z_last.png", "b_middle.png"]);
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

        // グループA: a1, a2, a3（この順）
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
        // 連続している（隣接）かつ順序が保持
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
        assert_eq!(fl.current_index(), Some(2)); // b1.png（グループBの先頭）

        // グループBから次のフォルダへ
        assert!(fl.navigate_next_folder());
        assert_eq!(fl.current_index(), Some(3)); // c1.png（グループCの先頭）

        // グループCの末尾、次のフォルダはない
        assert!(!fl.navigate_next_folder());

        // グループCから前のフォルダへ
        assert!(fl.navigate_prev_folder());
        assert_eq!(fl.current_index(), Some(2)); // b1.png（グループBの先頭）

        // グループBから前のフォルダへ
        assert!(fl.navigate_prev_folder());
        assert_eq!(fl.current_index(), Some(0)); // a1.png（グループAの先頭）

        // グループAの先頭、前のフォルダはない
        assert!(!fl.navigate_prev_folder());
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

        // グループBの途中（b2.png）から前のフォルダへ
        fl.navigate_to(4); // b2.png
        assert!(fl.navigate_prev_folder());
        assert_eq!(fl.current_index(), Some(0)); // a1.png（グループAの先頭）
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

    #[test]
    fn update_marked_paths_moves_to_dest_dir() {
        let src_dir = std::env::temp_dir().join("gv_test_fl_update_marked_src");
        let dest_dir = std::env::temp_dir().join("gv_test_fl_update_marked_dest");
        create_test_files(&src_dir, &["a.png", "b.png", "c.png"]);
        let _ = std::fs::create_dir_all(&dest_dir);
        // 移動先にもファイルを作成（from_pathが成功するように）
        create_test_files(&dest_dir, &["a.png", "c.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&src_dir).unwrap();
        fl.navigate_to(1); // b.png を選択

        // a.png と c.png をマーク
        fl.mark_at(0);
        fl.mark_at(2);

        fl.update_marked_paths(&dest_dir).unwrap();

        // マーク済みファイルのパスが更新されている
        let marked: Vec<&FileInfo> = fl.files.iter().filter(|f| f.marked).collect();
        assert_eq!(marked.len(), 2);
        for f in &marked {
            assert!(f.path.starts_with(&dest_dir));
        }

        // 非マークファイル（b.png）はsrc_dirのまま
        let unmarked: Vec<&FileInfo> = fl.files.iter().filter(|f| !f.marked).collect();
        assert_eq!(unmarked.len(), 1);
        assert_eq!(unmarked[0].file_name, "b.png");
        assert!(unmarked[0].path.starts_with(&src_dir));

        // 現在位置がb.pngを指している（復元されている）
        assert_eq!(fl.current().unwrap().file_name, "b.png");

        cleanup(&src_dir);
        cleanup(&dest_dir);
    }
}
