use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use crate::extension_registry::ExtensionRegistry;
use crate::file_info::{FileInfo, FileSource};

/// ナビゲーションの方向 (PendingContainer 展開時の current_index 配置に使う)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationDirection {
    /// 前進方向 (次へ・先頭へ・次のフォルダへ等)。展開後グループの先頭に配置する
    Forward,
    /// 後退方向 (前へ・末尾へ等)。展開後グループの末尾に配置する
    Backward,
}

/// フォルダ/アーカイブ単位ナビゲーション用のグループキー
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum GroupKey {
    Folder(std::path::PathBuf),
    Archive(std::path::PathBuf),
}

/// 軽量PRNG(xorshift64)。シャッフル用途のため暗号強度は不要。
struct SimpleRng(u64);

impl SimpleRng {
    fn new() -> Self {
        let mut buf = [0u8; 8];
        // OS 乱数源が取れない場合はシステム時刻ベースのシードにフォールバックする。
        // シャッフル用途のため暗号強度は不要で、決定的な再現を避けられれば十分。
        let seed = if getrandom::fill(&mut buf).is_ok() {
            u64::from_ne_bytes(buf)
        } else {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9E37_79B9_7F4A_7C15)
        };
        Self(seed | 1) // 0シード回避
    }

    /// xorshift64ステップを実行し、次の状態を返す
    fn step(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// [0, bound) の範囲で一様分布する乱数を返す (Lemire法)
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
            this.files
                .sort_by(|a, b| Self::compare_by_sort_order(a, b, order));
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
    /// 挿入後にリスト全体を `sort_order` で再ソートし、論理パス順を全体で維持する。
    /// `direction` は「展開位置 == 現在位置」だった場合の current_index 配置に使う:
    /// `Forward` なら展開エントリの先頭、`Backward` なら末尾を再ソート後の位置で復元する。
    /// 展開位置が現在位置でない場合は、現在ファイルそのものを再ソート後の位置に復元する。
    pub fn expand_container_at(
        &mut self,
        index: usize,
        entries: Vec<FileInfo>,
        direction: NavigationDirection,
    ) {
        if index >= self.files.len() {
            return;
        }

        let entries_len = entries.len();
        let current = self.current_index;
        let current_was_at_expand = current == Some(index);

        // 再ソート後の current 復元用に (path, source) を確定する
        // - 展開位置 == current かつエントリあり: NavigationDirection に従い展開エントリの先頭/末尾を選ぶ
        // - 展開位置 != current: 現在ファイルそのものを復元対象とする
        // - 展開位置 == current かつエントリ空: None (後続の特別処理に委ねる)
        let restore_target: Option<(PathBuf, FileSource)> = if current_was_at_expand {
            if entries_len == 0 {
                None
            } else {
                let target = match direction {
                    NavigationDirection::Forward => &entries[0],
                    NavigationDirection::Backward => &entries[entries_len - 1],
                };
                Some((target.path.clone(), target.source.clone()))
            }
        } else {
            current
                .and_then(|i| self.files.get(i))
                .map(|f| (f.path.clone(), f.source.clone()))
        };

        // プレースホルダを削除して展開結果を挿入
        self.files.splice(index..=index, entries);

        // 全体を再ソートして論理パス順を保つ
        let order = self.sort_order;
        self.files
            .sort_by(|a, b| Self::compare_by_sort_order(a, b, order));

        // current_index 復元
        if let Some((path, source)) = restore_target {
            self.restore_current_position(&path, &source);
        } else if self.files.is_empty() {
            self.current_index = None;
        } else if let Some(c) = current {
            // 展開位置 == current かつエントリ空: remove_at 相当のクランプ
            self.current_index = Some(c.min(self.files.len() - 1));
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
            let mut group_indices: HashMap<GroupKey, usize> = HashMap::new();
            let mut groups: Vec<Vec<FileInfo>> = Vec::new();
            for file in this.files.drain(..) {
                let key = Self::group_key(&file);
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
            // marked状態は維持 (trueのまま)
        }

        // 再ソートして現在位置を復元
        self.sort(self.sort_order);
        if let Some((path, source)) = current_info {
            self.restore_current_position(&path, &source);
        }
        Ok(())
    }

    /// ファイルのグループキーを返す (フォルダ=親ディレクトリ or アーカイブパス)
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

    /// 現在のファイル列から `group_key` の出現順を計算する。
    ///
    /// リスト上での連続性に依存しないため、論理パス順で同一グループのファイルが
    /// 他グループの要素を挟んで分断されていても正しく動作する。
    /// 戻り値: (各グループの最初の登場 index, 各ファイルが属するグループ index)
    fn compute_group_layout(&self) -> (Vec<usize>, Vec<usize>) {
        let mut seen: HashMap<GroupKey, usize> = HashMap::new();
        let mut group_first_indices: Vec<usize> = Vec::new();
        let mut file_group_idx: Vec<usize> = Vec::with_capacity(self.files.len());
        for (i, f) in self.files.iter().enumerate() {
            let key = Self::group_key(f);
            let idx = *seen.entry(key).or_insert_with(|| {
                let idx = group_first_indices.len();
                group_first_indices.push(i);
                idx
            });
            file_group_idx.push(idx);
        }
        (group_first_indices, file_group_idx)
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
        let (group_first_indices, file_group_idx) = self.compute_group_layout();
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
        let (group_first_indices, file_group_idx) = self.compute_group_layout();
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

    /// ソート順による比較。
    ///
    /// 比較キーは論理パス (`FileSource::display_path()` 相当) を基本とする。
    /// 通常ファイルはフルパス、アーカイブはアーカイブパス＋内部パス、PDF はファイルパス＋ページ番号となる。
    ///
    /// - `Name` / `NameNoCase` / `Natural`: 論理パスを直接比較する
    /// - `Size` / `Date`: 値を主キーとし、同値時に論理パスを副キーとする (タイブレーカー)
    fn compare_by_sort_order(a: &FileInfo, b: &FileInfo, order: SortOrder) -> std::cmp::Ordering {
        let path_a = a.source.display_path();
        let path_b = b.source.display_path();
        match order {
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
        // 全体は論理パス順で再ソートされる (a.zip, c.zip, pending.zip)
        // Forward なので展開エントリの先頭 p1.png が現在位置として復元される
        assert_eq!(fl.current().unwrap().file_name, "p1.png");
        assert_eq!(fl.current_index(), Some(2));
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
        // Backward なので展開エントリの末尾 p3.png が現在位置として復元される
        assert_eq!(fl.current().unwrap().file_name, "p3.png");
        assert_eq!(fl.current_index(), Some(4));
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
        // 全体再ソート後 (c.zip < pending.zip) でも現在ファイル c2.png が再ソート後の位置に復元される
        assert_eq!(fl.current().unwrap().file_name, "c2.png");
        assert_eq!(fl.current_index(), Some(1));
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
            FileSource::File(PathBuf::from("/x/z.png")),
            50,
            now,
        ));

        fl.sort(SortOrder::Size);

        let paths: Vec<String> = fl.files.iter().map(|f| f.source.display_path()).collect();
        assert_eq!(
            paths,
            vec![
                "/x/z.png".to_string(), // size=50 が最先頭
                "/x/b.png".to_string(), // size=100 同値内はパス順 b < c
                "/x/c.png".to_string(),
            ]
        );
    }

    #[test]
    fn sort_date_uses_logical_path_as_tiebreaker() {
        // Date 順も同値時に論理パスをタイブレーカーとする
        let registry = test_registry();
        let mut fl = FileList::new(registry);
        let t_old = std::time::SystemTime::UNIX_EPOCH;
        let t_new = t_old + std::time::Duration::from_secs(100);

        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/x/c.png")),
            100,
            t_old,
        ));
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/x/b.png")),
            100,
            t_old,
        ));
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/x/a.png")),
            100,
            t_new,
        ));

        fl.sort(SortOrder::Date);

        let paths: Vec<String> = fl.files.iter().map(|f| f.source.display_path()).collect();
        assert_eq!(
            paths,
            vec![
                "/x/b.png".to_string(), // 古い同値内ではパス順 b < c
                "/x/c.png".to_string(),
                "/x/a.png".to_string(), // 新しいので末尾
            ]
        );
    }

    #[test]
    fn shuffle_groups_handles_disjoint_groups() {
        // 論理パス順で同一グループのファイルがサブフォルダ要素を挟んで分断される配列に対しても、
        // shuffle_groups がグループ単位で集約しグループ内の出現順を維持することを確認する
        let registry = test_registry();
        let mut fl = FileList::new(registry);
        let now = std::time::SystemTime::UNIX_EPOCH;

        // 論理パス順では a/a.png → a/m/x.png → a/z.png となり、
        // group_a の 2 ファイルが group_a/m を挟んで分断される
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/a/a.png")),
            100,
            now,
        ));
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/a/m/x.png")),
            100,
            now,
        ));
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/a/z.png")),
            100,
            now,
        ));

        fl.shuffle_groups();

        assert_eq!(fl.len(), 3);

        // 全要素が保持されていることを確認
        let mut paths: Vec<String> = fl.files.iter().map(|f| f.source.display_path()).collect();
        paths.sort();
        assert_eq!(
            paths,
            vec![
                "/a/a.png".to_string(),
                "/a/m/x.png".to_string(),
                "/a/z.png".to_string(),
            ]
        );

        // group_a (a.png と z.png) がリスト上で連続し、相対順序が維持されている
        let a_pos = fl
            .files
            .iter()
            .position(|f| f.path == Path::new("/a/a.png"))
            .unwrap();
        let z_pos = fl
            .files
            .iter()
            .position(|f| f.path == Path::new("/a/z.png"))
            .unwrap();
        assert!(a_pos < z_pos);
        assert_eq!(z_pos - a_pos, 1, "group_a のファイルが連続していない");
    }

    #[test]
    fn navigate_folder_handles_disjoint_groups() {
        // 同一グループが分断される配列でも navigate_prev_folder/navigate_next_folder が
        // グループ間ジャンプとして機能する
        let registry = test_registry();
        let mut fl = FileList::new(registry);
        let now = std::time::SystemTime::UNIX_EPOCH;

        // 論理パス順: /a/a.png(0,group_a), /a/m/x.png(1,group_a/m), /a/z.png(2,group_a), /b/c.png(3,group_b)
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/a/a.png")),
            100,
            now,
        ));
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/a/m/x.png")),
            100,
            now,
        ));
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/a/z.png")),
            100,
            now,
        ));
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/b/c.png")),
            100,
            now,
        ));

        // /a/a.png から → 次のグループ (group_a/m) の先頭 (index=1)
        fl.navigate_to(0);
        assert!(fl.navigate_next_folder());
        assert_eq!(fl.current_index(), Some(1));

        // /a/m/x.png から → 次のグループ (group_b) の先頭 (index=3)
        assert!(fl.navigate_next_folder());
        assert_eq!(fl.current_index(), Some(3));

        // /b/c.png から → 先頭グループ (group_a) へラップアラウンド (index=0)
        assert!(fl.navigate_next_folder());
        assert_eq!(fl.current_index(), Some(0));

        // 分断されたグループの後半 (/a/z.png) からも、所属グループの 1 つ後 (group_a/m) へ
        fl.navigate_to(2);
        assert!(fl.navigate_next_folder());
        assert_eq!(fl.current_index(), Some(1));

        // 逆方向: /a/m/x.png (group_a/m) から prev → 前のグループ group_a の先頭 (index=0)
        fl.navigate_to(1);
        assert!(fl.navigate_prev_folder());
        assert_eq!(fl.current_index(), Some(0));
    }

    #[test]
    fn expand_container_at_resorts_in_size_order() {
        // Size 順でソート中にコンテナ展開が起きると、展開後も全体が Size 順 (同値時パス順) で並ぶ
        let registry = test_registry();
        let mut fl = FileList::new(registry);
        let now = std::time::SystemTime::UNIX_EPOCH;

        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/a.png")),
            100,
            now,
        ));
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/b.png")),
            300,
            now,
        ));
        fl.push(make_file_info_with_source(
            FileSource::PendingContainer {
                container_path: PathBuf::from("/pending.zip"),
            },
            0,
            now,
        ));
        fl.push(make_file_info_with_source(
            FileSource::File(PathBuf::from("/c.png")),
            200,
            now,
        ));

        fl.sort(SortOrder::Size);
        // 期待初期順: pending(0), a(100), c(200), b(300)
        let pending_index = fl
            .files
            .iter()
            .position(|f| matches!(f.source, FileSource::PendingContainer { .. }))
            .unwrap();
        fl.navigate_to(pending_index);

        let entries = vec![
            make_file_info_with_source(
                FileSource::ArchiveEntry {
                    archive: PathBuf::from("/pending.zip"),
                    entry: "entry1.png".to_string(),
                    on_demand: false,
                },
                150,
                now,
            ),
            make_file_info_with_source(
                FileSource::ArchiveEntry {
                    archive: PathBuf::from("/pending.zip"),
                    entry: "entry2.png".to_string(),
                    on_demand: false,
                },
                250,
                now,
            ),
        ];

        fl.expand_container_at(pending_index, entries, NavigationDirection::Forward);

        // 展開後 Size 順: a(100), entry1(150), c(200), entry2(250), b(300)
        let sizes: Vec<u64> = fl.files.iter().map(|f| f.file_size).collect();
        assert_eq!(sizes, vec![100, 150, 200, 250, 300]);

        // Forward なので展開エントリ先頭 entry1.png が現在位置として復元される
        assert_eq!(
            fl.current().unwrap().source.display_path(),
            "/pending.zip/entry1.png"
        );
    }

    #[test]
    fn update_marked_paths_moves_to_dest_dir() {
        let src_dir = std::env::temp_dir().join("gv_test_fl_update_marked_src");
        let dest_dir = std::env::temp_dir().join("gv_test_fl_update_marked_dest");
        create_test_files(&src_dir, &["a.png", "b.png", "c.png"]);
        let _ = std::fs::create_dir_all(&dest_dir);
        // 移動先にもファイルを作成 (from_pathが成功するように)
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

        // 非マークファイル (b.png) はsrc_dirのまま
        let unmarked: Vec<&FileInfo> = fl.files.iter().filter(|f| !f.marked).collect();
        assert_eq!(unmarked.len(), 1);
        assert_eq!(unmarked[0].file_name, "b.png");
        assert!(unmarked[0].path.starts_with(&src_dir));

        // 現在位置がb.pngを指している (復元されている)
        assert_eq!(fl.current().unwrap().file_name, "b.png");

        cleanup(&src_dir);
        cleanup(&dest_dir);
    }
}
