use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use crossbeam_channel::Sender;

use crate::archive::ArchiveManager;
use crate::extension_registry::ExtensionRegistry;
use crate::file_info::FileSource;
use crate::file_list::FileList;
use crate::image::{DecodedImage, DecoderChain};
use crate::prefetch::{LoadResponse, PageCache, PrefetchEngine};

/// DocumentからUIへの通知イベント
#[derive(Debug)]
#[allow(dead_code)]
pub enum DocumentEvent {
    /// 画像のデコード完了、再描画可能
    ImageReady,
    /// ファイルリスト変更
    FileListChanged,
    /// 表示位置変更
    NavigationChanged { index: usize, count: usize },
    /// エラー通知
    Error(String),
}

/// 画像・ファイルリスト・状態管理（モデル層）
/// Win32 APIやHWNDへの依存は一切持たない
pub struct Document {
    event_sender: Sender<DocumentEvent>,
    decoder: Arc<DecoderChain>,
    current_image: Option<DecodedImage>,
    file_list: FileList,
    // 先読みエンジン
    cache: PageCache,
    prefetch: Option<PrefetchEngine>,
    cache_backward: usize,
    cache_forward: usize,
    // アーカイブ対応
    archive_manager: ArchiveManager,
    archive_temp_dir: Option<PathBuf>,
    current_archive: Option<PathBuf>,
}

impl Document {
    pub fn new(
        event_sender: Sender<DocumentEvent>,
        decoder: Arc<DecoderChain>,
        registry: Arc<ExtensionRegistry>,
        archive_manager: ArchiveManager,
    ) -> Self {
        Self {
            event_sender,
            decoder,
            current_image: None,
            file_list: FileList::new(registry),
            cache: PageCache::new(0),
            prefetch: None,
            cache_backward: 0,
            cache_forward: 0,
            archive_manager,
            archive_temp_dir: None,
            current_archive: None,
        }
    }

    /// 先読みエンジンを起動する
    /// `notify`: レスポンス受信時のコールバック（UIスレッド通知用）
    /// `cache_budget`: キャッシュメモリ予算（バイト）
    /// `base_image_size`: キャッシュ枚数計算の基準となる1枚あたりのバイト数
    pub fn start_prefetch(
        &mut self,
        notify: Box<dyn Fn() + Send>,
        cache_budget: usize,
        base_image_size: usize,
    ) {
        self.prefetch = Some(PrefetchEngine::new(notify, Arc::clone(&self.decoder)));
        self.update_cache_range(cache_budget, base_image_size);
    }

    /// キャッシュ範囲を再計算する
    fn update_cache_range(&mut self, cache_budget: usize, base_image_size: usize) {
        let total_slots = (cache_budget / base_image_size).max(3);
        self.cache_forward = (total_slots * 2 / 3).max(1);
        self.cache_backward = (total_slots / 3).max(1);
        self.cache.set_max_memory(cache_budget);
    }

    /// 先読みレスポンスを処理する（キャッシュ格納 + current_image更新）
    pub fn process_prefetch_responses(&mut self) {
        let Some(prefetch) = &self.prefetch else {
            return;
        };
        let current_gen = prefetch.generation();
        let responses = prefetch.drain_responses();

        for resp in responses {
            match resp {
                LoadResponse::Loaded {
                    index,
                    image,
                    generation,
                } => {
                    // 古い世代のレスポンスは破棄
                    if generation != current_gen {
                        continue;
                    }
                    // 現在表示すべきページでまだ画像がない場合、即表示
                    let is_current = self.file_list.current_index() == Some(index)
                        && self.current_image.is_none();
                    if is_current {
                        self.current_image = Some(image);
                        let _ = self.event_sender.send(DocumentEvent::ImageReady);
                    } else {
                        self.cache.insert(index, image);
                    }
                }
                LoadResponse::Failed {
                    generation, error, ..
                } => {
                    if generation != current_gen {
                        continue;
                    }
                    // 先読みの失敗ではmark_failedしない（一時的なエラーの可能性）
                    let _ = self.event_sender.send(DocumentEvent::Error(error));
                }
            }
        }
    }

    /// 現在位置を中心に先読みをスケジュールする
    fn schedule_prefetch(&mut self) {
        let Some(prefetch) = &mut self.prefetch else {
            return;
        };
        let Some(center) = self.file_list.current_index() else {
            return;
        };

        // 範囲外キャッシュを削除
        self.cache
            .evict_outside(center, self.cache_backward, self.cache_forward);

        // 世代を進行（前回のin-flightリクエストを無効化）
        prefetch.advance_generation();

        let files = self.file_list.files();
        let len = files.len();

        // 近い順にリクエスト: 前方優先（center+1, center+2, ..., center-1, center-2, ...）
        let mut indices = Vec::new();
        let fwd_end = (center + self.cache_forward + 1).min(len);
        for i in (center + 1)..fwd_end {
            indices.push(i);
        }
        let bwd_start = center.saturating_sub(self.cache_backward);
        for i in (bwd_start..center).rev() {
            indices.push(i);
        }

        for idx in indices {
            if !self.cache.contains(idx) {
                prefetch.request_load(idx, files[idx].path.clone());
            }
        }
    }

    /// キャッシュを無効化する（フォルダ切替、再読み込み時）
    fn invalidate_cache(&mut self) {
        self.cache.clear();
        self.file_list.clear_failed();
        if let Some(prefetch) = &mut self.prefetch {
            prefetch.advance_generation();
        }
    }

    /// ファイルを開く（親フォルダの画像を列挙し、指定ファイルを表示）
    /// アーカイブファイルの場合はアーカイブとして開く
    pub fn open(&mut self, path: &Path) -> Result<()> {
        let path = Self::canonicalize(path)?;

        // アーカイブ判定
        if self.archive_manager.is_archive(&path) {
            return self.open_archive(&path);
        }

        // 通常ファイル: アーカイブtempがあればクリーンアップ
        self.cleanup_archive_temp();

        // 親フォルダの画像を列挙
        if let Some(folder) = path.parent() {
            self.invalidate_cache();
            self.file_list.populate_from_folder(folder)?;
            self.file_list.set_current_by_path(&path);
            let _ = self.event_sender.send(DocumentEvent::FileListChanged);
        }

        self.load_current()
    }

    /// アーカイブを開く（画像をtempに一括展開し、ファイルリストを構築）
    fn open_archive(&mut self, archive_path: &Path) -> Result<()> {
        self.cleanup_archive_temp();
        self.invalidate_cache();

        // ユニークなtempディレクトリを作成（ワーカーが旧dirのファイルを掴んでいる可能性への対策）
        let temp_dir = std::env::temp_dir().join(format!(
            "gv3_archive_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        std::fs::create_dir_all(&temp_dir)?;

        // 画像を一括展開（tempパスと元エントリ名のマッピング付き）
        let entries = self
            .archive_manager
            .extract_images(archive_path, &temp_dir)?;
        if entries.is_empty() {
            let _ = std::fs::remove_dir_all(&temp_dir);
            anyhow::bail!(
                "アーカイブ内に画像ファイルがありません: {}",
                archive_path.display()
            );
        }

        self.archive_temp_dir = Some(temp_dir);
        self.current_archive = Some(archive_path.to_path_buf());

        // マッピング結果から直接FileListを構築（sourceにアーカイブ情報を設定）
        self.file_list.clear();
        for (temp_path, entry_name) in &entries {
            if let Ok(mut info) = crate::file_info::FileInfo::from_path(temp_path) {
                info.source = FileSource::ArchiveEntry {
                    archive: archive_path.to_path_buf(),
                    entry: entry_name.clone(),
                };
                // ソート用のfile_nameはエントリパスのファイル名部分を使う
                info.file_name = crate::archive::extract_filename(entry_name).to_string();
                self.file_list.push(info);
            }
        }
        self.file_list.sort_current();

        if self.file_list.len() > 0 {
            self.file_list.navigate_first();
        }
        let _ = self.event_sender.send(DocumentEvent::FileListChanged);
        self.load_current()
    }

    /// アーカイブ用tempディレクトリをクリーンアップする
    fn cleanup_archive_temp(&mut self) {
        if let Some(temp_dir) = self.archive_temp_dir.take() {
            // ワーカーのin-flight fs::readがファイルを掴んでいる可能性があるため、
            // 削除失敗は無視する（ユニークdir名なので次回openに影響しない）
            let _ = std::fs::remove_dir_all(&temp_dir);
        }
        self.current_archive = None;
    }

    /// フォルダを開く（先頭画像を表示）
    pub fn open_folder(&mut self, folder: &Path) -> Result<()> {
        let folder = Self::canonicalize(folder)?;
        self.cleanup_archive_temp();
        self.invalidate_cache();
        self.file_list.populate_from_folder(&folder)?;

        if self.file_list.len() > 0 {
            self.file_list.navigate_first();
        }
        let _ = self.event_sender.send(DocumentEvent::FileListChanged);
        self.load_current()
    }

    /// 相対移動
    pub fn navigate_relative(&mut self, offset: isize) {
        if self.file_list.navigate_relative(offset) {
            let _ = self.load_current();
        }
    }

    /// 最初へ移動
    pub fn navigate_first(&mut self) {
        if self.file_list.navigate_first() {
            let _ = self.load_current();
        }
    }

    /// 指定インデックスへ移動
    pub fn navigate_to(&mut self, index: usize) {
        if self.file_list.navigate_to(index) {
            let _ = self.load_current();
        }
    }

    /// 最後へ移動
    pub fn navigate_last(&mut self) {
        if self.file_list.navigate_last() {
            let _ = self.load_current();
        }
    }

    /// 現在のファイルをデコードしてイベント送信
    fn load_current(&mut self) -> Result<()> {
        let Some(index) = self.file_list.current_index() else {
            self.current_image = None;
            return Ok(());
        };

        // 1. キャッシュヒット → 瞬時切替
        if let Some(image) = self.cache.take(index) {
            self.current_image = Some(image);
            let _ = self.event_sender.send(DocumentEvent::ImageReady);
            self.send_navigation_changed();
            self.schedule_prefetch();
            return Ok(());
        }

        // 2. キャッシュミス → 同期デコード（フォールバック）
        let path = self.file_list.current().unwrap().path.clone();
        let data = std::fs::read(&path)
            .with_context(|| format!("ファイル読み込み失敗: {}", path.display()))?;

        let filename_hint = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        match self.decoder.decode(&data, filename_hint) {
            Ok(image) => {
                self.current_image = Some(image);
                let _ = self.event_sender.send(DocumentEvent::ImageReady);
            }
            Err(e) => {
                self.current_image = None;
                // 同期デコード失敗時はfailedマーク（ナビゲーション時にスキップ対象）
                self.file_list.mark_failed(index);
                let msg = format!("{}: {}", path.display(), e);
                let _ = self.event_sender.send(DocumentEvent::Error(msg));
            }
        }

        self.send_navigation_changed();
        self.schedule_prefetch();
        Ok(())
    }

    /// NavigationChangedイベントを送信
    fn send_navigation_changed(&self) {
        if let Some(index) = self.file_list.current_index() {
            let _ = self.event_sender.send(DocumentEvent::NavigationChanged {
                index,
                count: self.file_list.len(),
            });
        }
    }

    /// パスを正規化する（相対パスやUNCパス対応）
    fn canonicalize(path: &Path) -> Result<PathBuf> {
        std::fs::canonicalize(path).with_context(|| format!("パス解決失敗: {}", path.display()))
    }

    /// 現在のデコード済み画像への参照
    pub fn current_image(&self) -> Option<&DecodedImage> {
        self.current_image.as_ref()
    }

    /// 現在のファイルパス
    pub fn current_path(&self) -> Option<&Path> {
        self.file_list.current().map(|f| f.path.as_path())
    }

    /// ファイルリストへの参照
    pub fn file_list(&self) -> &FileList {
        &self.file_list
    }

    /// 現在開いているアーカイブのパス（タイトル表示用）
    pub fn current_archive(&self) -> Option<&Path> {
        self.current_archive.as_deref()
    }

    /// 現在のファイルの論理ソース
    pub fn current_source(&self) -> Option<&FileSource> {
        self.file_list.current().map(|f| &f.source)
    }

    // --- マーク操作 ---

    /// 現在のファイルをマークして次へ移動する
    pub fn mark_current(&mut self) {
        if let Some(index) = self.file_list.current_index() {
            self.file_list.mark_at(index);
            // マーク後に次へ移動
            self.navigate_relative(1);
        }
    }

    /// 現在のファイルのマークを解除する
    pub fn unmark_current(&mut self) {
        if let Some(index) = self.file_list.current_index() {
            self.file_list.unmark_at(index);
        }
    }

    /// 全マーク反転
    pub fn invert_all_marks(&mut self) {
        self.file_list.invert_all_marks();
    }

    /// 最初から現在位置までのマーク反転
    pub fn invert_marks_to_here(&mut self) {
        self.file_list.invert_marks_to_here();
    }

    /// 前のマーク画像へ移動
    pub fn navigate_prev_mark(&mut self) {
        if self.file_list.navigate_prev_mark() {
            let _ = self.load_current();
        }
    }

    /// 次のマーク画像へ移動
    pub fn navigate_next_mark(&mut self) {
        if self.file_list.navigate_next_mark() {
            let _ = self.load_current();
        }
    }

    // --- フォルダナビゲーション ---

    /// 前のフォルダへ移動
    pub fn navigate_prev_folder(&mut self) {
        if self.file_list.navigate_prev_folder() {
            let _ = self.load_current();
        }
    }

    /// 次のフォルダへ移動
    pub fn navigate_next_folder(&mut self) {
        if self.file_list.navigate_next_folder() {
            let _ = self.load_current();
        }
    }

    /// ソート順で前の画像へ移動
    pub fn sort_navigate_back(&mut self) {
        let order = self.file_list.sort_order();
        if self.file_list.sorted_navigate(-1, order) {
            let _ = self.load_current();
        }
    }

    /// ソート順で次の画像へ移動
    pub fn sort_navigate_forward(&mut self) {
        let order = self.file_list.sort_order();
        if self.file_list.sorted_navigate(1, order) {
            let _ = self.load_current();
        }
    }

    /// 現在のファイルをリストから削除する（ファイル自体は残る）
    pub fn remove_current_from_list(&mut self) {
        if let Some(index) = self.file_list.current_index() {
            self.file_list.remove_at(index);
            self.after_list_change();
        }
    }

    /// マーク済みファイルをリストから削除する
    pub fn remove_marked_from_list(&mut self) {
        if self.file_list.marked_count() == 0 {
            return;
        }
        self.file_list.remove_marked();
        self.after_list_change();
    }

    /// リスト変更後の共通処理（キャッシュ無効化+再読込+イベント送信）
    pub fn after_list_change(&mut self) {
        self.invalidate_cache();
        let _ = self.event_sender.send(DocumentEvent::FileListChanged);
        if self.file_list.len() > 0 {
            let _ = self.load_current();
        } else {
            self.current_image = None;
            let _ = self.event_sender.send(DocumentEvent::ImageReady);
        }
    }

    /// 現在のファイルを再読み込みする
    pub fn reload(&mut self) {
        self.invalidate_cache();
        let _ = self.load_current();
    }

    /// ファイルリストをクリアする
    pub fn close_all(&mut self) {
        self.cleanup_archive_temp();
        self.invalidate_cache();
        self.file_list.clear();
        self.current_image = None;
        let _ = self.event_sender.send(DocumentEvent::FileListChanged);
        let _ = self.event_sender.send(DocumentEvent::ImageReady);
    }

    /// ブックマークデータからファイルリストを復元する
    pub fn load_bookmark_data(&mut self, data: crate::bookmark::BookmarkData) -> Result<()> {
        self.cleanup_archive_temp();
        self.invalidate_cache();
        self.file_list.clear();

        // 通常ファイルのみをリストに追加（存在するもののみ）
        // アーカイブエントリは現状未対応（アーカイブを開き直す必要がある）
        for source in &data.entries {
            match source {
                FileSource::File(path) => {
                    if path.exists()
                        && let Ok(info) = crate::file_info::FileInfo::from_path(path)
                    {
                        self.file_list.push(info);
                    }
                }
                FileSource::ArchiveEntry { archive, .. } => {
                    // アーカイブを見つけたら、そのアーカイブを開く
                    // （ブックマーク内の最初のアーカイブのみ対応）
                    if archive.exists() && self.archive_manager.is_archive(archive) {
                        if let Err(e) = self.open_archive(archive) {
                            eprintln!("アーカイブ復元失敗: {e}");
                        }
                        // アーカイブを開いた場合、ブックマークの残りは無視
                        break;
                    }
                }
            }
        }

        // 指定インデックスへ移動
        if self.file_list.len() > 0 {
            let idx = data.index.min(self.file_list.len() - 1);
            self.file_list.navigate_to(idx);
        }

        let _ = self.event_sender.send(DocumentEvent::FileListChanged);
        self.load_current()
    }

    /// 現在のファイルのメタデータを取得する
    pub fn current_metadata(&self) -> Result<crate::image::ImageMetadata> {
        let path = self
            .file_list
            .current()
            .map(|f| f.path.clone())
            .ok_or_else(|| anyhow::anyhow!("ファイルが選択されていません"))?;

        let data = std::fs::read(&path)?;
        let filename_hint = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        self.decoder.metadata(&data, filename_hint)
    }

    /// 指定インデックスの画像がキャッシュ済みか判定する
    /// current_imageとして保持中の画像も「キャッシュ済み」として扱う
    pub fn is_cached(&self, index: usize) -> bool {
        self.cache.contains(index)
            || (self.file_list.current_index() == Some(index) && self.current_image.is_some())
    }

    /// ファイルリストへの可変参照（app.rsのファイル操作用）
    #[allow(dead_code)]
    pub fn file_list_mut(&mut self) -> &mut FileList {
        &mut self.file_list
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        self.cleanup_archive_temp();
    }
}
