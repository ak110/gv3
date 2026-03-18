use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Context as _, Result};
use crossbeam_channel::Sender;

use crate::archive::ArchiveManager;
use crate::extension_registry::ExtensionRegistry;
use crate::file_info::FileSource;
use crate::file_list::FileList;
use crate::image::{DecodedImage, DecoderChain};
use crate::prefetch::{LoadResponse, PageCache, PrefetchEngine};

/// ZIPファイルのバッファ（mmapまたはメモリ読み込み）
pub(crate) enum ZipBuffer {
    /// メモリマップドファイル（OSがページフォルト駆動で必要部分のみロード）
    Mmap(memmap2::Mmap),
    /// ヒープ上のバイト列（mmapフォールバック用）
    Memory(Vec<u8>),
}

impl AsRef<[u8]> for ZipBuffer {
    fn as_ref(&self) -> &[u8] {
        match self {
            ZipBuffer::Mmap(m) => m,
            ZipBuffer::Memory(v) => v,
        }
    }
}

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
    archive_manager: Arc<ArchiveManager>,
    archive_temp_dirs: Vec<PathBuf>,
    current_containers: Vec<PathBuf>,
    /// ZIPファイルのバッファキャッシュ（オンデマンド読み出し用、先読みスレッドと共有）
    zip_buffers: Arc<RwLock<HashMap<PathBuf, ZipBuffer>>>,
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
            archive_manager: Arc::new(archive_manager),
            archive_temp_dirs: Vec::new(),
            current_containers: Vec::new(),
            zip_buffers: Arc::new(RwLock::new(HashMap::new())),
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
        self.prefetch = Some(PrefetchEngine::new(
            notify,
            Arc::clone(&self.decoder),
            Arc::clone(&self.archive_manager),
            Arc::clone(&self.zip_buffers),
        ));
        self.update_cache_range(cache_budget, base_image_size);
    }

    /// キャッシュ範囲を再計算する
    /// 前方4枚・後方2枚を上限とし、スロット数で枚数を制御する。
    /// メモリ予算はcache_budget（空きメモリの50%）をそのまま使う。
    /// base_image_sizeはスロット数の計算にのみ使用する（実画像が大きい場合でも
    /// キャッシュが機能するよう、予算の追加制限はかけない）。
    fn update_cache_range(&mut self, cache_budget: usize, base_image_size: usize) {
        const MAX_CACHE_FORWARD: usize = 4;
        const MAX_CACHE_BACKWARD: usize = 2;

        let total_slots = (cache_budget / base_image_size).max(3);
        self.cache_forward = (total_slots * 2 / 3).clamp(1, MAX_CACHE_FORWARD);
        self.cache_backward = (total_slots / 3).clamp(1, MAX_CACHE_BACKWARD);
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

        // 距離ベースの交互読み込み: 前1, 後1, 前2, 後2, ... の順でリクエスト
        let mut indices = Vec::new();
        let max_dist = self.cache_forward.max(self.cache_backward);
        for dist in 1..=max_dist {
            // 前方（次のページ）
            let fwd = center + dist;
            if dist <= self.cache_forward && fwd < len {
                indices.push(fwd);
            }
            // 後方（前のページ）
            if dist <= self.cache_backward && dist <= center {
                indices.push(center - dist);
            }
        }

        for idx in indices {
            if !self.cache.contains(idx) {
                let pdf_page = match &files[idx].source {
                    FileSource::PdfPage {
                        pdf_path,
                        page_index,
                    } => Some((pdf_path.clone(), *page_index)),
                    _ => None,
                };
                let archive_entry = match &files[idx].source {
                    FileSource::ArchiveEntry {
                        archive,
                        entry,
                        on_demand: true,
                    } => Some((archive.clone(), entry.clone())),
                    _ => None,
                };
                prefetch.request_load(idx, files[idx].path.clone(), pdf_page, archive_entry);
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
    /// PDF/アーカイブファイルの場合はそれぞれの専用パスで開く
    pub fn open(&mut self, path: &Path) -> Result<()> {
        let path = Self::canonicalize(path)?;

        // PDF判定
        if Self::is_pdf(&path) {
            return self.open_pdf(&path);
        }

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

    /// 単一ファイルを開く（フォルダスキャンしない）
    /// クリップボード貼り付けなど、tempディレクトリ内の単一ファイルを開く場合に使用
    pub fn open_single(&mut self, path: &Path) -> Result<()> {
        let path = Self::canonicalize(path)?;
        self.cleanup_archive_temp();
        self.invalidate_cache();
        self.file_list.populate_single(&path)?;
        let _ = self.event_sender.send(DocumentEvent::FileListChanged);
        self.load_current()
    }

    /// アーカイブを開く（単一アーカイブ）
    fn open_archive(&mut self, archive_path: &Path) -> Result<()> {
        self.open_containers(&[archive_path.to_path_buf()])
    }

    /// PDFファイルを開く（単一PDF）
    fn open_pdf(&mut self, pdf_path: &Path) -> Result<()> {
        self.open_containers(&[pdf_path.to_path_buf()])
    }

    /// 複数コンテナ（アーカイブ/PDF混在）をまとめて開く
    pub fn open_containers(&mut self, paths: &[PathBuf]) -> Result<()> {
        self.cleanup_archive_temp();
        self.invalidate_cache();
        self.file_list.clear();

        let mut all_empty = true;
        for path in paths {
            if Self::is_pdf(path) {
                // PDFページを追加
                let page_count = crate::pdf_renderer::get_pdf_page_count_safe(path)?;
                if page_count == 0 {
                    continue;
                }
                all_empty = false;
                for i in 0..page_count {
                    let info = crate::file_info::FileInfo {
                        path: path.clone(),
                        source: FileSource::PdfPage {
                            pdf_path: path.clone(),
                            page_index: i,
                        },
                        file_name: format!("Page {:03}", i + 1),
                        file_size: 0,
                        modified: std::time::SystemTime::now(),
                        marked: false,
                        load_failed: false,
                    };
                    self.file_list.push(info);
                }
                self.current_containers.push(path.clone());
            } else if self.archive_manager.supports_on_demand(path) {
                // オンデマンド（ZIP）: mmapで読み込み（OSがページフォルト駆動で必要部分のみロード）
                let buffer = match File::open(path).and_then(|f| unsafe { memmap2::Mmap::map(&f) })
                {
                    Ok(mmap) => ZipBuffer::Mmap(mmap),
                    Err(_) => {
                        // mmapフォールバック: ヒープに読み込み
                        let data = std::fs::read(path).with_context(|| {
                            format!("アーカイブ読み込み失敗: {}", path.display())
                        })?;
                        ZipBuffer::Memory(data)
                    }
                };
                let entries = self
                    .archive_manager
                    .list_images_from_buffer(buffer.as_ref(), path)?;
                if entries.is_empty() {
                    continue;
                }
                all_empty = false;
                for entry in &entries {
                    let info = crate::file_info::FileInfo {
                        path: path.clone(), // アーカイブパス自体（PDFと同じパターン）
                        source: FileSource::ArchiveEntry {
                            archive: path.clone(),
                            entry: entry.entry_name.clone(),
                            on_demand: true,
                        },
                        file_name: entry.file_name.clone(),
                        file_size: entry.file_size,
                        modified: std::time::SystemTime::now(),
                        marked: false,
                        load_failed: false,
                    };
                    self.file_list.push(info);
                }
                self.zip_buffers
                    .write()
                    .unwrap()
                    .insert(path.clone(), buffer);
                self.current_containers.push(path.clone());
            } else {
                // 非オンデマンド（RAR/7z/Susie）: 既存のtemp展開ロジック
                let temp_dir = std::env::temp_dir().join(format!(
                    "gv3_archive_{}_{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis()
                ));
                std::fs::create_dir_all(&temp_dir)?;

                let entries = self.archive_manager.extract_images(path, &temp_dir)?;
                if entries.is_empty() {
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    continue;
                }

                all_empty = false;
                self.archive_temp_dirs.push(temp_dir);
                self.current_containers.push(path.clone());

                for (temp_path, entry_name) in &entries {
                    if let Ok(mut info) = crate::file_info::FileInfo::from_path(temp_path) {
                        info.source = FileSource::ArchiveEntry {
                            archive: path.clone(),
                            entry: entry_name.clone(),
                            on_demand: false,
                        };
                        info.file_name = crate::archive::extract_filename(entry_name).to_string();
                        self.file_list.push(info);
                    }
                }
            }
        }

        if all_empty {
            anyhow::bail!("コンテナ内に画像ファイルがありません");
        }

        self.file_list.sort_current();

        if self.file_list.len() > 0 {
            self.file_list.navigate_first();
        }
        let _ = self.event_sender.send(DocumentEvent::FileListChanged);
        self.load_current()
    }

    /// PDFファイルかどうか判定する
    fn is_pdf(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    }

    /// アーカイブ用tempディレクトリとZIPバッファをクリーンアップする
    fn cleanup_archive_temp(&mut self) {
        for temp_dir in self.archive_temp_dirs.drain(..) {
            // ワーカーのin-flight fs::readがファイルを掴んでいる可能性があるため、
            // 削除失敗は無視する（ユニークdir名なので次回openに影響しない）
            let _ = std::fs::remove_dir_all(&temp_dir);
        }
        self.current_containers.clear();
        self.zip_buffers.write().unwrap().clear();
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
        let current = self.file_list.current().unwrap();
        let path = current.path.clone();
        let source = current.source.clone();

        let decode_result = if let FileSource::PdfPage {
            pdf_path,
            page_index,
        } = &source
        {
            // PDFページ: STAデッドロック回避のためMTAスレッドで実行
            crate::pdf_renderer::render_pdf_page_safe(pdf_path, *page_index)
        } else {
            // 通常ファイル/アーカイブエントリ: read_file_data → decode
            let current = self.file_list.current().unwrap();
            let data = self.read_file_data(current)?;
            let filename_hint = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            self.decoder.decode(&data, filename_hint)
        };

        match decode_result {
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
    /// Rustのcanonicalize()は\\?\プレフィックスを付与するが、
    /// SHFileOperationW等のShell APIが非対応のため除去する
    fn canonicalize(path: &Path) -> Result<PathBuf> {
        let canonical = std::fs::canonicalize(path)
            .with_context(|| format!("パス解決失敗: {}", path.display()))?;
        let s = canonical.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            Ok(PathBuf::from(stripped))
        } else {
            Ok(canonical)
        }
    }

    /// FileInfoからファイルデータを読み出す（オンデマンドアーカイブ対応）
    fn read_file_data(&self, info: &crate::file_info::FileInfo) -> Result<Vec<u8>> {
        match &info.source {
            FileSource::ArchiveEntry {
                archive,
                entry,
                on_demand: true,
            } => {
                // キャッシュされたZIPバッファから読み出し（Stored最適化付き）
                let buffers = self.zip_buffers.read().unwrap();
                if let Some(buffer) = buffers.get(archive) {
                    crate::archive::zip::ZipHandler::read_entry_from_buffer(buffer.as_ref(), entry)
                } else {
                    // キャッシュミス（通常発生しない）: ファイルから直接読み出し
                    drop(buffers);
                    self.archive_manager.read_entry(archive, entry)
                }
            }
            _ => std::fs::read(&info.path)
                .with_context(|| format!("ファイル読み込み失敗: {}", info.path.display())),
        }
    }

    /// 現在のファイルのデータを読み出す（app.rsのファイル操作用）
    pub fn read_file_data_current(&self) -> Result<Vec<u8>> {
        let current = self
            .file_list
            .current()
            .ok_or_else(|| anyhow::anyhow!("ファイルが選択されていません"))?;
        self.read_file_data(current)
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

    /// パスがアーカイブファイルか判定する
    #[allow(dead_code)]
    pub fn is_archive(&self, path: &Path) -> bool {
        self.archive_manager.is_archive(path)
    }

    /// パスがコンテナ（アーカイブまたはPDF）か判定する
    pub fn is_container(&self, path: &Path) -> bool {
        self.archive_manager.is_archive(path) || Self::is_pdf(path)
    }

    /// 現在開いているコンテナ（アーカイブ/PDF）のパス一覧
    #[allow(dead_code)]
    pub fn current_containers(&self) -> &[PathBuf] {
        &self.current_containers
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

    /// ファイルリスト全体をシャッフル
    pub fn shuffle_all(&mut self) {
        self.file_list.shuffle_all();
        self.after_list_change();
    }

    /// グループ順をシャッフル（グループ内順序は保持）
    pub fn shuffle_groups(&mut self) {
        self.file_list.shuffle_groups();
        self.after_list_change();
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

    /// 現在のファイルをリスト内でリネーム（同一フォルダ内の移動後）
    /// 先読みキャッシュを無効化し、再ソート後の位置に追従する
    pub fn rename_current_in_list(&mut self, new_path: &Path) -> Result<()> {
        let index = self
            .file_list
            .current_index()
            .ok_or_else(|| anyhow::anyhow!("ファイルが選択されていません"))?;
        self.file_list.update_file_at(index, new_path)?;
        self.invalidate_cache();
        let _ = self.event_sender.send(DocumentEvent::FileListChanged);
        if self.file_list.len() > 0 {
            let _ = self.load_current();
        }
        Ok(())
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
        let has_containers = data.entries.iter().any(|s| {
            matches!(
                s,
                FileSource::PdfPage { .. } | FileSource::ArchiveEntry { .. }
            )
        });

        if has_containers {
            // コンテナモード: ユニークなコンテナパスを収集
            let mut container_paths: Vec<PathBuf> = Vec::new();
            for source in &data.entries {
                let path = match source {
                    FileSource::PdfPage { pdf_path, .. } => Some(pdf_path),
                    FileSource::ArchiveEntry { archive, .. } => Some(archive),
                    _ => None,
                };
                if let Some(p) = path
                    && p.exists()
                    && !container_paths.contains(p)
                {
                    container_paths.push(p.clone());
                }
            }

            if container_paths.is_empty() {
                anyhow::bail!("ブックマーク内のコンテナが見つかりません");
            }

            self.open_containers(&container_paths)?;

            // 位置復元: source同一性で検索（PDF/Archive両対応）
            if let Some(target_source) = data.entries.get(data.index) {
                let target_idx = self
                    .file_list
                    .files()
                    .iter()
                    .position(|f| FileList::source_matches(&f.source, target_source))
                    .unwrap_or_else(|| data.index.min(self.file_list.len().saturating_sub(1)));
                self.file_list.navigate_to(target_idx);
                let _ = self.load_current();
            }
        } else {
            // 通常ファイルのみ
            self.cleanup_archive_temp();
            self.invalidate_cache();
            self.file_list.clear();

            for source in &data.entries {
                if let FileSource::File(path) = source
                    && path.exists()
                    && let Ok(info) = crate::file_info::FileInfo::from_path(path)
                {
                    self.file_list.push(info);
                }
            }

            if self.file_list.len() > 0 {
                let idx = data.index.min(self.file_list.len() - 1);
                self.file_list.navigate_to(idx);
            }

            let _ = self.event_sender.send(DocumentEvent::FileListChanged);
            self.load_current()?;
        }

        Ok(())
    }

    /// 現在のファイルのメタデータを取得する
    pub fn current_metadata(&self) -> Result<crate::image::ImageMetadata> {
        let current = self
            .file_list
            .current()
            .ok_or_else(|| anyhow::anyhow!("ファイルが選択されていません"))?;

        // PDFページの場合はcurrent_imageからメタデータを生成
        if matches!(current.source, FileSource::PdfPage { .. }) {
            return if let Some(img) = &self.current_image {
                Ok(crate::image::ImageMetadata {
                    width: img.width,
                    height: img.height,
                    format: "PDF".to_string(),
                    comments: Vec::new(),
                })
            } else {
                anyhow::bail!("PDFページがまだレンダリングされていません")
            };
        }

        let data = self.read_file_data(current)?;
        let filename_hint = current
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
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
