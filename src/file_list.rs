use std::collections::HashMap;
use std::path::Path;
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

/// ソート順
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum SortOrder {
    /// ファイル名順
    Name,
    /// ファイル名順（大文字小文字区別なし）
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

        if target != current {
            self.current_index = Some(target);
            true
        } else {
            false
        }
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

    /// ソート実行（現在位置をパスで維持）
    /// グループ（フォルダ/アーカイブ）の出現順を保持しつつ、グループ内でソートする
    pub fn sort(&mut self, order: SortOrder) {
        // 現在のパスを記憶
        let current_path = self.current().map(|f| f.path.clone());

        // グループ出現順を記録
        let mut group_order: HashMap<GroupKey, usize> = HashMap::new();
        for file in &self.files {
            let key = Self::group_key(file);
            let next_id = group_order.len();
            group_order.entry(key).or_insert(next_id);
        }

        // 第1キー=グループ出現順、第2キー=ソート順
        self.files.sort_by(|a, b| {
            let ga = group_order.get(&Self::group_key(a)).copied().unwrap_or(0);
            let gb = group_order.get(&Self::group_key(b)).copied().unwrap_or(0);
            ga.cmp(&gb)
                .then_with(|| Self::compare_by_sort_order(a, b, order))
        });

        self.sort_order = order;

        // パスで位置を復元
        if let Some(path) = current_path {
            self.set_current_by_path(&path);
        }
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

    /// 現在のソート順で再ソートする
    pub fn sort_current(&mut self) {
        self.sort(self.sort_order);
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
        let end = self.current_index.map(|i| i + 1).unwrap_or(0);
        for info in self.files[..end].iter_mut() {
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
        // 現在のパスを記憶（位置復元用）
        let current_path = self.current().map(|f| f.path.clone());

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

        // current_indexの復元
        if self.files.is_empty() {
            self.current_index = None;
        } else if let Some(path) = current_path {
            // 元の位置のファイルがまだ残っていればそれを選択
            if !self.set_current_by_path(&path) {
                // 削除されていたら先頭にフォールバック
                self.current_index = Some(0);
            }
        } else {
            self.current_index = Some(0);
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

    /// ファイルのグループキーを返す（フォルダ=親ディレクトリ or アーカイブパス）
    fn group_key(info: &FileInfo) -> GroupKey {
        match &info.source {
            FileSource::ArchiveEntry { archive, .. } => GroupKey::Archive(archive.clone()),
            FileSource::PdfPage { pdf_path, .. } => GroupKey::Archive(pdf_path.clone()),
            FileSource::File(_) => GroupKey::Folder(
                info.path
                    .parent()
                    .map(|p| p.to_path_buf())
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
        let current = match self.current_index {
            Some(idx) => idx,
            None => return false,
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

        if target != current {
            self.current_index = Some(target);
            true
        } else {
            false
        }
    }

    /// ソート順による比較
    fn compare_by_sort_order(a: &FileInfo, b: &FileInfo, order: SortOrder) -> std::cmp::Ordering {
        match order {
            SortOrder::Name => a.file_name.cmp(&b.file_name),
            SortOrder::NameNoCase => a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()),
            SortOrder::Size => a.file_size.cmp(&b.file_size),
            SortOrder::Date => a.modified.cmp(&b.modified),
            SortOrder::Natural => natord::compare(&a.file_name, &b.file_name),
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
        let dir = std::env::temp_dir().join("gv3_test_fl_populate");
        create_test_files(&dir, &["a.jpg", "b.png", "c.txt", "d.bmp", "readme.md"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();

        assert_eq!(fl.len(), 3); // jpg, png, bmp
        cleanup(&dir);
    }

    #[test]
    fn natural_sort_order() {
        let dir = std::env::temp_dir().join("gv3_test_fl_natural");
        create_test_files(&dir, &["img2.png", "img10.png", "img1.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.sort(SortOrder::Natural);

        let names: Vec<&str> = fl.files.iter().map(|f| f.file_name.as_str()).collect();
        assert_eq!(names, vec!["img1.png", "img2.png", "img10.png"]);
        cleanup(&dir);
    }

    #[test]
    fn navigate_relative_wraps_around() {
        let dir = std::env::temp_dir().join("gv3_test_fl_nav");
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
        let dir = std::env::temp_dir().join("gv3_test_fl_firstlast");
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
        let dir = std::env::temp_dir().join("gv3_test_fl_setpath");
        create_test_files(&dir, &["a.png", "b.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();

        assert!(fl.set_current_by_path(&dir.join("b.png")));
        assert_eq!(fl.current().unwrap().file_name, "b.png");

        cleanup(&dir);
    }

    #[test]
    fn sort_preserves_current_position() {
        let dir = std::env::temp_dir().join("gv3_test_fl_sortpreserve");
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
        let dir = std::env::temp_dir().join("gv3_test_fl_mark");
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
        let dir = std::env::temp_dir().join("gv3_test_fl_invert");
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
        let dir = std::env::temp_dir().join("gv3_test_fl_invert_here");
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
        let dir = std::env::temp_dir().join("gv3_test_fl_removeat");
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
        let dir = std::env::temp_dir().join("gv3_test_fl_removemarked");
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
        let dir = std::env::temp_dir().join("gv3_test_fl_navmark");
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
        let dir = std::env::temp_dir().join("gv3_test_fl_sortnav");
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
        let dir = std::env::temp_dir().join("gv3_test_fl_navmark_none");
        create_test_files(&dir, &["a.png", "b.png"]);

        let mut fl = FileList::new(test_registry());
        fl.populate_from_folder(&dir).unwrap();
        fl.navigate_to(0);

        // マークなし → 移動しない
        assert!(!fl.navigate_next_mark());
        assert!(!fl.navigate_prev_mark());

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
}
