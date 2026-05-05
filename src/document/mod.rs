//! ドキュメント（ファイルリスト・コンテナ・画像管理）モジュール
//!
//! 複数の画像ファイル・アーカイブ・PDFを統一インターフェースで操作する。
//! - ファイルリストの管理と先読み
//! - アーカイブ（ZIP/PDF/RAR/7z）の展開とバッファキャッシュ
//! - ナビゲーションと画像デコード

pub(crate) mod container;
pub(crate) mod types;
pub(crate) mod utils;

pub use types::{DocumentEvent, ZipBuffer};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::{Context as _, Result};
use crossbeam_channel::{Receiver, Sender};
use rayon::prelude::*;

use crate::archive::ArchiveManager;
use crate::editing::EditingSession;
use crate::extension_registry::ExtensionRegistry;
use crate::file_info::FileSource;
use crate::file_list::{FileList, NavigationDirection, SortOrder};
use crate::image::{DecodedImage, DecoderChain};
use crate::persistent_filter::PersistentFilter;
use crate::prefetch::{PrefetchCoordinator, PrefetchEngine, PrefetchEvent};

use self::container::process_single_container;
use self::types::{ContainerExpandEvent, ContainerResult, ContainerState};
use self::utils::{build_expansion_pool, collect_folder_files_recursive, sort_paths_natural};

/// ドキュメント本体（画像・ファイルリスト管理、アーカイブハンドリング）
pub struct Document {
    event_sender: Sender<DocumentEvent>,
    decoder: Arc<DecoderChain>,
    current_image: Option<DecodedImage>,
    file_list: FileList,
    // 先読みエンジン
    prefetch_coord: PrefetchCoordinator,
    // アーカイブ対応
    archive_manager: Arc<ArchiveManager>,
    archive_temp_dirs: Vec<PathBuf>,
    current_containers: Vec<PathBuf>,
    /// ZIPファイルのバッファキャッシュ (オンデマンド取得用、先読みスレッドと共有)
    zip_buffers: Arc<RwLock<HashMap<PathBuf, ZipBuffer>>>,
    /// 編集セッション (編集中のみSome)
    editing_session: Option<EditingSession>,
    /// 永続フィルタ設定
    persistent_filter: PersistentFilter,
    /// バックグラウンド展開の受信チャネル
    expand_rx: Option<Receiver<ContainerExpandEvent>>,
    /// バックグラウンド展開の世代番号 (openごとにインクリメント)
    expand_generation: u64,
    /// バックグラウンド展開の現世代に紐づくキャンセルフラグ。
    /// 旧世代を無効化する経路から `cancel_expansion` を呼んで `true` をセットすると、
    /// rayon ワーカーは進行中の1件を完走したのち、残りのキューを消費せずに終了する。
    /// これにより並列度を1に抑えても再スケジュール時に旧世代の I/O が残存しない。
    expansion_cancel: Option<Arc<AtomicBool>>,
    /// コンテナの展開状態
    container_states: HashMap<PathBuf, ContainerState>,
    /// 直近のナビゲーション操作の方向 (PendingContainer 到達時の intent 構築に使う)
    last_navigation_direction: NavigationDirection,
    /// 待機中の PendingContainer ナビゲーション意図
    /// `(container_path, 到達時の方向)` を保持し、バックグラウンド展開完了時に
    /// direction-aware な current_index 配置に使う
    pending_navigation_intent: Option<(PathBuf, NavigationDirection)>,
    /// UIスレッド通知コールバック (PostMessageW経由でUIを起こす)
    ui_notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Document {
    pub fn new(
        event_sender: Sender<DocumentEvent>,
        decoder: Arc<DecoderChain>,
        registry: Arc<ExtensionRegistry>,
        archive_manager: ArchiveManager,
        default_sort: SortOrder,
    ) -> Self {
        let mut file_list = FileList::new(registry);
        file_list.set_sort_order(default_sort);
        Self {
            event_sender,
            decoder,
            current_image: None,
            file_list,
            prefetch_coord: PrefetchCoordinator::new(),
            archive_manager: Arc::new(archive_manager),
            archive_temp_dirs: Vec::new(),
            current_containers: Vec::new(),
            zip_buffers: Arc::new(RwLock::new(HashMap::new())),
            editing_session: None,
            persistent_filter: PersistentFilter::new(),
            expand_rx: None,
            expand_generation: 0,
            container_states: HashMap::new(),
            last_navigation_direction: NavigationDirection::Forward,
            pending_navigation_intent: None,
            ui_notify: None,
            expansion_cancel: None,
        }
    }

    /// 先読みエンジンを起動する
    /// `notify`: レスポンス受信時のコールバック (UIスレッド通知用)
    /// `cache_budget`: キャッシュメモリ予算 (バイト)
    /// `base_image_size`: キャッシュ枚数計算の基準となる1枚あたりのバイト数
    ///
    /// ワーカースレッドの起動に失敗した場合は `Err` を返す。呼び出し元で
    /// `show_error_title` 等へ渡すこと。失敗してもアプリ自体は起動継続可能。
    pub fn start_prefetch(
        &mut self,
        notify: Arc<dyn Fn() + Send + Sync>,
        cache_budget: usize,
        base_image_size: usize,
    ) -> Result<()> {
        // UI通知コールバックをバックグラウンド展開用にも保持
        self.ui_notify = Some(Arc::clone(&notify));
        let engine = PrefetchEngine::new(
            Box::new(move || notify()),
            Arc::clone(&self.decoder),
            Arc::clone(&self.archive_manager),
            Arc::clone(&self.zip_buffers),
        )?;
        self.prefetch_coord
            .start(engine, cache_budget, base_image_size);
        Ok(())
    }

    /// 先読みレスポンスを処理する (キャッシュ格納 + current_image更新)
    pub fn process_prefetch_responses(&mut self) {
        let events = self.prefetch_coord.process_responses(
            self.file_list.current_index(),
            self.current_image.is_some(),
            &self.persistent_filter,
        );
        for event in events {
            match event {
                PrefetchEvent::CurrentImageReady(image) => {
                    self.current_image = Some(image);
                    let _ = self.event_sender.send(DocumentEvent::ImageReady);
                }
                PrefetchEvent::Error(error) => {
                    let _ = self.event_sender.send(DocumentEvent::Error(error));
                }
            }
        }
    }

    /// 現在位置を中心に先読みをスケジュールする
    fn schedule_prefetch(&mut self) {
        let Some(center) = self.file_list.current_index() else {
            return;
        };
        self.prefetch_coord
            .reschedule(center, self.file_list.files());
    }

    /// キャッシュを無効化する (フォルダ切替、再読み込み時)
    fn invalidate_cache(&mut self) {
        self.prefetch_coord.invalidate();
        self.file_list.clear_failed();
    }

    /// ファイルを開く (親フォルダの画像を列挙し、指定ファイルを表示)
    /// PDF/アーカイブファイルの場合はそれぞれの専用パスで開く
    pub fn open(&mut self, path: &Path) -> Result<()> {
        let path = Self::canonicalize(path)?;

        // ブックマーク判定
        if crate::bookmark::is_bookmark_file(&path) {
            return self.open_bookmark(&path);
        }

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

    /// 単一ファイルを開く (フォルダスキャンしない)
    /// クリップボード貼り付けなど、tempディレクトリ内の単一ファイルを開く場合に使用
    pub fn open_single(&mut self, path: &Path) -> Result<()> {
        let path = Self::canonicalize(path)?;
        self.cleanup_archive_temp();
        self.invalidate_cache();
        self.file_list.populate_single(&path)?;
        let _ = self.event_sender.send(DocumentEvent::FileListChanged);
        self.load_current()
    }

    /// ブックマークファイルを開く
    fn open_bookmark(&mut self, bookmark_path: &Path) -> Result<()> {
        let is_archive = |p: &Path| self.archive_manager.is_archive(p);
        let data = crate::bookmark::load_bookmark_from_path(bookmark_path, &is_archive)?;
        self.load_bookmark_data(data)
    }

    /// アーカイブを開く (単一アーカイブ)
    fn open_archive(&mut self, archive_path: &Path) -> Result<()> {
        self.open_containers(&[archive_path.to_path_buf()])
    }

    /// PDFファイルを開く (単一PDF)
    fn open_pdf(&mut self, pdf_path: &Path) -> Result<()> {
        self.open_containers(&[pdf_path.to_path_buf()])
    }

    /// ドキュメント状態を初期化する (open_containers / open_multiple の共通前処理)
    fn reset_document_state(&mut self) {
        self.cleanup_archive_temp();
        self.invalidate_cache();
        self.file_list.clear();
        self.container_states.clear();
        self.pending_navigation_intent = None;
        self.cancel_expansion(); // 旧世代 rayon ジョブをキュー先頭で停止させる
        self.expand_rx = None; // 旧バックグラウンド展開を破棄
        self.expand_generation += 1;
    }

    /// 複数コンテナ (アーカイブ/PDF混在) をまとめて開く
    /// 最初のコンテナのみ即座に展開し、残りはプレースホルダとして登録後バックグラウンドで展開する。
    pub fn open_containers(&mut self, paths: &[PathBuf]) -> Result<()> {
        self.reset_document_state();

        let errors = self.process_and_integrate_containers(paths);

        // エラー処理: 部分成功を許容、全失敗のみエラー返却
        if self.file_list.len() == 0 {
            if errors.is_empty() {
                anyhow::bail!("コンテナ内に画像ファイルが存在しない");
            }
            anyhow::bail!("全てのコンテナの読み込みに失敗:\n{}", errors.join("\n"));
        }
        if !errors.is_empty() {
            let msg = format!("{}件のコンテナを開けませんでした", errors.len());
            let _ = self.event_sender.send(DocumentEvent::Error(msg));
        }

        self.file_list.sort_current();

        if self.file_list.len() > 0 {
            self.file_list.navigate_first();
        }
        let _ = self.event_sender.send(DocumentEvent::FileListChanged);

        // 未展開コンテナがあればバックグラウンド展開を起動
        self.start_background_expansion();

        self.load_current()
    }

    /// 複数パス (フォルダ・コンテナ・画像ファイル混在) をフラットに展開して開く。
    /// フォルダ入力はサブフォルダまで深さ優先で再帰走査し、配下の画像・コンテナを一括で取り込む。
    pub fn open_multiple(&mut self, paths: &[PathBuf]) -> Result<()> {
        self.reset_document_state();

        // トップレベル入力の正規化。リパースポイント (シンボリックリンク・ジャンクション) は
        // `canonicalize()` での実体化前に除外し、再帰走査時の除外規則と一貫させる。
        // 複数入力が同じ実体パスへ解決された場合は、先着を残して以降を除外する (重複走査の回避)。
        let mut canonical_inputs: Vec<PathBuf> = Vec::new();
        for raw in paths {
            if let Ok(meta) = std::fs::symlink_metadata(raw)
                && utils::is_reparse_point(&meta)
            {
                continue;
            }
            let canon = Self::canonicalize(raw)?;
            if !canonical_inputs.contains(&canon) {
                canonical_inputs.push(canon);
            }
        }

        // 入れ子関係にある入力 (ある入力が別の入力の祖先ディレクトリ) は祖先のみ残して子孫を除外する。
        // 親フォルダと配下サブフォルダを同時に受け取った際、再帰走査で子孫サブツリーが二重登録されるのを防ぐ。
        let mut filtered_inputs: Vec<PathBuf> = Vec::new();
        for path in &canonical_inputs {
            let is_descendant = canonical_inputs
                .iter()
                .any(|other| other != path && path.starts_with(other));
            if !is_descendant {
                filtered_inputs.push(path.clone());
            }
        }

        // D&D やシェルからの複数渡しは順序が選択順になるため、トップレベル入力を
        // 自然順に並べ替えて以降の push 順 (= FileList のグループ出現順) を安定化する。
        sort_paths_natural(&mut filtered_inputs);

        // コンテナは出所別に2系統で保持する。
        // - toplevel_containers: トップレベル入力が直接コンテナであるもの (最後に自然順ソート対象)
        // - folder_containers:   フォルダ再帰走査で見つけたもの (走査順を保持する)
        let mut toplevel_containers: Vec<PathBuf> = Vec::new();
        let mut folder_containers: Vec<PathBuf> = Vec::new();
        let mut bookmarks: Vec<PathBuf> = Vec::new();
        for path in &filtered_inputs {
            if path.is_dir() {
                // フォルダ: 再帰走査した全ファイルを振り分ける。
                // 再帰ヘルパー内でリパースポイント除外と自然順整列を済ませており、
                // 親階層のファイル→サブディレクトリの順に格納されている。
                for entry_path in collect_folder_files_recursive(path) {
                    if crate::bookmark::is_bookmark_file(&entry_path) {
                        bookmarks.push(entry_path);
                    } else if self.is_container(&entry_path) {
                        folder_containers.push(entry_path);
                    } else if let Some(name) = entry_path.file_name().and_then(|n| n.to_str())
                        && self.file_list.registry().is_image_extension(name)
                        && let Ok(info) = crate::file_info::FileInfo::from_path(&entry_path)
                    {
                        self.file_list.push(info);
                    }
                }
            } else if crate::bookmark::is_bookmark_file(path) {
                bookmarks.push(path.clone());
            } else if self.is_container(path) {
                toplevel_containers.push(path.clone());
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && self.file_list.registry().is_image_extension(name)
                && let Ok(info) = crate::file_info::FileInfo::from_path(path)
            {
                self.file_list.push(info);
            }
        }

        // ブックマークからエントリを展開して file_list / コンテナ群に振り分ける。
        // ブックマーク経由で得たコンテナはトップレベル相当として扱い、自然順ソート対象に含める。
        for bm_path in &bookmarks {
            let is_archive = |p: &Path| self.archive_manager.is_archive(p);
            if let Ok(data) = crate::bookmark::load_bookmark_from_path(bm_path, &is_archive) {
                for source in data.entries {
                    match source {
                        FileSource::File(path) => {
                            if path.exists()
                                && let Ok(info) = crate::file_info::FileInfo::from_path(&path)
                            {
                                self.file_list.push(info);
                            }
                        }
                        FileSource::ArchiveEntry { archive, .. }
                            if !toplevel_containers.contains(&archive)
                                && !folder_containers.contains(&archive) =>
                        {
                            toplevel_containers.push(archive);
                        }
                        FileSource::PendingContainer { container_path }
                            if !toplevel_containers.contains(&container_path)
                                && !folder_containers.contains(&container_path) =>
                        {
                            toplevel_containers.push(container_path);
                        }
                        _ => {}
                    }
                }
            }
        }

        // トップレベル入力由来のコンテナ群のみ自然順ソートし、D&D 選択順を安定化する。
        // process_and_integrate_containers は先頭を即展開するため、ここでの並び順が
        // FileList のグループ出現順 = 最終的な表示順序を左右する。
        // フォルダ再帰由来は走査順 (親→子) を壊さないよう、ソートせずに末尾へ連結する。
        sort_paths_natural(&mut toplevel_containers);
        let mut containers: Vec<PathBuf> = toplevel_containers;
        containers.extend(folder_containers);

        // コンテナを展開・統合
        let errors = self.process_and_integrate_containers(&containers);

        // エラー処理
        if self.file_list.len() == 0 {
            if errors.is_empty() {
                anyhow::bail!("画像ファイルが存在しない");
            }
            anyhow::bail!("読み込みに失敗:\n{}", errors.join("\n"));
        }
        if !errors.is_empty() {
            let msg = format!("{}件のコンテナを開けませんでした", errors.len());
            let _ = self.event_sender.send(DocumentEvent::Error(msg));
        }

        self.file_list.sort_current();

        if self.file_list.len() > 0 {
            self.file_list.navigate_first();
        }
        let _ = self.event_sender.send(DocumentEvent::FileListChanged);

        // 未展開コンテナがあればバックグラウンド展開を起動
        self.start_background_expansion();

        self.load_current()
    }

    /// コンテナの段階的読み込みヘルパー
    /// 最初のコンテナのみ即座に展開し、残りはプレースホルダとして登録する。
    /// エラーメッセージのリストを返す。
    fn process_and_integrate_containers(&mut self, paths: &[PathBuf]) -> Vec<String> {
        if paths.is_empty() {
            return Vec::new();
        }

        let mut errors: Vec<String> = Vec::new();

        // 最初のコンテナだけ即座に展開
        let result = process_single_container(&paths[0], &self.archive_manager, Self::is_pdf);
        match result {
            ContainerResult::Error { path, error } => {
                errors.push(format!("{}: {error}", path.display()));
            }
            result => {
                self.integrate_container_result(result);
            }
        }

        // 残りのコンテナはプレースホルダとして登録
        for path in &paths[1..] {
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let (file_size, modified) =
                std::fs::metadata(path).map_or((0, std::time::SystemTime::UNIX_EPOCH), |m| {
                    (
                        m.len(),
                        m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                    )
                });
            let info = crate::file_info::FileInfo {
                path: path.clone(),
                source: FileSource::PendingContainer {
                    container_path: path.clone(),
                },
                file_name,
                file_size,
                modified,
                marked: false,
                load_failed: false,
            };
            self.file_list.push(info);
            self.container_states
                .insert(path.clone(), ContainerState::Pending);
            self.current_containers.push(path.clone());
        }

        errors
    }

    /// ContainerResult をファイルリストに統合する
    fn integrate_container_result(&mut self, result: ContainerResult) {
        // FileInfo 構築を build_container_entries に委譲する。
        // 先に参照で entries を生成し、その後所有権フィールド (buffer, temp_dir) を分解して
        // self への副作用を処理する。
        let file_entries = Self::build_container_entries(&result);
        match result {
            ContainerResult::Error { .. } => {} // 呼び出し元で処理済み
            ContainerResult::Pdf { path, page_count } => {
                if page_count == 0 {
                    return;
                }
                self.container_states
                    .insert(path.clone(), ContainerState::Expanded);
                self.current_containers.push(path);
            }
            ContainerResult::Zip {
                path,
                buffer,
                entries,
            } => {
                if entries.is_empty() {
                    return;
                }
                self.zip_buffers
                    .write()
                    .expect("zip_buffers lock poisoned")
                    .insert(path.clone(), buffer);
                self.container_states
                    .insert(path.clone(), ContainerState::Expanded);
                self.current_containers.push(path);
            }
            ContainerResult::TempExtracted {
                path,
                temp_dir,
                entries,
            } => {
                if entries.is_empty() {
                    return;
                }
                self.archive_temp_dirs.push(temp_dir);
                self.container_states
                    .insert(path.clone(), ContainerState::Expanded);
                self.current_containers.push(path);
            }
        }
        for info in file_entries {
            self.file_list.push(info);
        }
    }

    /// ContainerResult からエントリの Vec を生成する共通ヘルパー
    ///
    /// Error バリアントは空の Vec を返す。呼び出し元で Error の判定・処理を行うこと。
    fn build_container_entries(result: &ContainerResult) -> Vec<crate::file_info::FileInfo> {
        let mut entries = Vec::new();
        match result {
            ContainerResult::Error { .. } => {}
            ContainerResult::Pdf { path, page_count } => {
                let pdf_file_size = std::fs::metadata(path).map_or(0, |m| m.len());
                for i in 0..*page_count {
                    entries.push(crate::file_info::FileInfo {
                        path: path.clone(),
                        source: FileSource::PdfPage {
                            pdf_path: path.clone(),
                            page_index: i,
                        },
                        file_name: format!("Page {:03}", i + 1),
                        file_size: pdf_file_size,
                        modified: std::time::SystemTime::now(),
                        marked: false,
                        load_failed: false,
                    });
                }
            }
            ContainerResult::Zip {
                path,
                entries: archive_entries,
                ..
            } => {
                for entry in archive_entries {
                    entries.push(crate::file_info::FileInfo {
                        path: path.clone(),
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
                    });
                }
            }
            ContainerResult::TempExtracted {
                path,
                entries: temp_entries,
                ..
            } => {
                for (temp_path, entry_name) in temp_entries {
                    if let Ok(mut info) = crate::file_info::FileInfo::from_path(temp_path) {
                        info.source = FileSource::ArchiveEntry {
                            archive: path.clone(),
                            entry: entry_name.clone(),
                            on_demand: false,
                        };
                        info.file_name = crate::archive::extract_filename(entry_name).to_string();
                        entries.push(info);
                    }
                }
            }
        }
        entries
    }

    /// 現世代のバックグラウンド展開を無効化する。
    ///
    /// 旧世代を破棄する全経路 ( `open_containers` / `open_multiple` /
    /// `expand_all_pending_sync` / `reschedule_background_expansion` ) から呼び、
    /// rayon ワーカーに「次の `for_each` エントリから処理を止めろ」と指示する。
    /// 進行中の1件は完走するが、それ以降のキューは消費されない。
    fn cancel_expansion(&mut self) {
        if let Some(flag) = self.expansion_cancel.take() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// バックグラウンドコンテナ展開を起動する
    /// Pending状態のコンテナを全てInFlightに遷移させてからスレッドに投入する。
    /// 既に Expanded 状態のコンテナはスキップする (再起動時の二重展開防止)。
    fn start_background_expansion(&mut self) {
        // 呼び出し側が cancel_expansion を怠った場合の二重保険
        self.cancel_expansion();

        // Pending状態のコンテナパスを収集し、現在位置からの距離でソート
        let current_idx = self.file_list.current_index().unwrap_or(0);
        let mut pending_paths: Vec<(usize, PathBuf)> = Vec::new();
        for (i, file) in self.file_list.files().iter().enumerate() {
            if let FileSource::PendingContainer { container_path } = &file.source {
                // 既に Expanded 状態のものは投入しない (reschedule 時の安全策)
                if matches!(
                    self.container_states.get(container_path),
                    Some(ContainerState::Expanded)
                ) {
                    continue;
                }
                pending_paths.push((i, container_path.clone()));
            }
        }

        if pending_paths.is_empty() {
            return;
        }

        // 現在位置からの距離でソート (近い順に展開する)
        pending_paths.sort_by_key(|(idx, _)| (*idx as isize - current_idx as isize).unsigned_abs());

        let paths: Vec<PathBuf> = pending_paths.into_iter().map(|(_, p)| p).collect();

        // ディスパッチ前にメインスレッドで全てInFlightに遷移
        for path in &paths {
            self.container_states
                .insert(path.clone(), ContainerState::InFlight);
        }

        let (tx, rx) = crossbeam_channel::unbounded();
        self.expand_rx = Some(rx);

        // 現世代用のキャンセルフラグを作成してフィールドに保持する。
        // 次に cancel_expansion が呼ばれた時点で true に遷移し、以降の par_iter
        // エントリは処理されずに終了する。
        let cancel = Arc::new(AtomicBool::new(false));
        self.expansion_cancel = Some(Arc::clone(&cancel));

        let generation = self.expand_generation;
        let archive_manager = Arc::clone(&self.archive_manager);
        let ui_notify = self.ui_notify.clone();
        // rayon プール作成失敗を UI に通知するため、event_sender をワーカースレッドへ複製する
        let event_sender = self.event_sender.clone();

        std::thread::spawn(move || {
            // rayon プールで並列展開。プール作成失敗時は DocumentEvent::Error を送って終了する。
            let pool = match build_expansion_pool() {
                Ok(pool) => pool,
                Err(e) => {
                    let _ = event_sender.send(DocumentEvent::Error(format!(
                        "バックグラウンド展開用スレッドプールの作成に失敗しました: {e}"
                    )));
                    let _ = tx.send(ContainerExpandEvent::AllDone { generation });
                    if let Some(notify) = &ui_notify {
                        notify();
                    }
                    return;
                }
            };

            pool.install(|| {
                paths.par_iter().for_each(|path| {
                    // 旧世代として無効化されていたら以降の処理をスキップする
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let result = process_single_container(path, &archive_manager, Self::is_pdf);
                    let _ = tx.send(ContainerExpandEvent::Expanded {
                        container_path: path.clone(),
                        result,
                        generation,
                    });
                    // UIスレッドを起こす (PostMessageW経由)
                    if let Some(notify) = &ui_notify {
                        notify();
                    }
                });
            });

            let _ = tx.send(ContainerExpandEvent::AllDone { generation });
            if let Some(notify) = &ui_notify {
                notify();
            }
        });
    }

    /// バックグラウンド展開を再起動して優先度を再計算する
    /// シャッフル等でリスト並べ替えが起きた後や、ユーザーが PendingContainer
    /// に到達して特定コンテナを最優先で待ち始めたタイミングで呼ぶ。
    /// 旧 rayon ジョブは `cancel_expansion` でキューの消費を停止させ、
    /// 進行中の1件だけ完走させてから終了する。結果は世代不一致で破棄される。
    fn reschedule_background_expansion(&mut self) {
        if !self.file_list.has_pending() {
            return;
        }
        self.cancel_expansion();
        self.expand_rx = None; // 古い結果は世代不一致で破棄
        self.expand_generation += 1;
        self.start_background_expansion();
    }

    /// バックグラウンド展開結果を回収して file_list に統合する
    /// 1回の poll で受信可能な完了結果を全部処理し、終了時に1回だけ
    /// `FileListChanged` を送信する (パネル全件再構築のコストを抑えるため)。
    /// 統合対象が `pending_navigation_intent` と一致したら direction を渡し、
    /// 解決時に `load_current()` をリトライして表示を更新する。
    pub fn process_expand_results(&mut self) {
        let current_gen = self.expand_generation;
        let mut applied = false;
        let mut intent_resolved = false;

        loop {
            let Some(rx) = &self.expand_rx else {
                break;
            };
            let Ok(event) = rx.try_recv() else {
                break;
            };

            match event {
                ContainerExpandEvent::Expanded {
                    container_path,
                    result,
                    generation,
                } => {
                    if generation != current_gen {
                        if let ContainerResult::TempExtracted { temp_dir, .. } = &result {
                            let _ = std::fs::remove_dir_all(temp_dir);
                        }
                        continue;
                    }

                    // file_list 内で該当 PendingContainer を探して統合
                    let Some(idx) = self.find_pending_index(&container_path) else {
                        // プレースホルダがもう存在しない (リスト変更等) → 結果を破棄する
                        if let ContainerResult::TempExtracted { temp_dir, .. } = &result {
                            let _ = std::fs::remove_dir_all(temp_dir);
                        }
                        continue;
                    };

                    // intent と一致するなら direction を引き出し、解決済みフラグを立てる
                    let (direction, was_intent) = match &self.pending_navigation_intent {
                        Some((p, d)) if *p == container_path => (*d, true),
                        _ => (NavigationDirection::Forward, false),
                    };

                    let _ = self.apply_container_result(idx, &container_path, result, direction);
                    applied = true;
                    if was_intent {
                        self.pending_navigation_intent = None;
                        intent_resolved = true;
                    }
                }
                ContainerExpandEvent::AllDone { generation } => {
                    if generation == current_gen {
                        self.expand_rx = None;
                    }
                    break;
                }
            }
        }

        if applied {
            // バッチ完了後にキャッシュ無効化と先読み再スケジュールを1回だけ実行する。
            // apply_container_result はファイルリスト更新のみ行い、これらを呼ばない。
            self.invalidate_cache();
            if intent_resolved && self.file_list.current_index().is_some() {
                let _ = self.load_current();
            } else {
                self.schedule_prefetch();
            }
            let _ = self.event_sender.send(DocumentEvent::FileListChanged);
            self.send_navigation_changed();
        }
    }

    /// 指定 container_path を持つ PendingContainer プレースホルダの index を探す
    fn find_pending_index(&self, container_path: &Path) -> Option<usize> {
        self.file_list.files().iter().position(|f| match &f.source {
            FileSource::PendingContainer { container_path: cp } => cp == container_path,
            _ => false,
        })
    }

    /// バックグラウンド展開の進捗を返す (展開済み / 全体)
    pub fn expand_progress(&self) -> Option<(usize, usize)> {
        let total = self.container_states.len();
        if total == 0 {
            return None;
        }
        let done = self
            .container_states
            .values()
            .filter(|s| **s == ContainerState::Expanded)
            .count();
        if done >= total {
            None // 全完了なら非表示
        } else {
            Some((done, total))
        }
    }

    /// ContainerResult をファイルリストに反映する
    /// `direction` は「展開位置 == 現在位置」だった場合の current_index 配置に使う。
    /// キャッシュ無効化・先読み再スケジュール・`FileListChanged` の送信は行わず、
    /// 呼び出し元 (process_expand_results / expand_all_pending_sync) が
    /// バッチ完了後にまとめて1回実行する。
    fn apply_container_result(
        &mut self,
        index: usize,
        container_path: &Path,
        result: ContainerResult,
        direction: NavigationDirection,
    ) -> Result<()> {
        match result {
            ContainerResult::Error {
                ref path,
                ref error,
            } => {
                let msg = format!("{}: {error}", path.display());
                self.file_list.remove_at(index);
                self.container_states
                    .insert(container_path.to_path_buf(), ContainerState::Expanded);
                let _ = self.event_sender.send(DocumentEvent::Error(msg.clone()));
                anyhow::bail!("コンテナ展開失敗: {msg}");
            }
            result => {
                let entries = Self::build_container_entries(&result);

                // ZIPバッファやtemp_dirを保存
                if let ContainerResult::Zip { path, buffer, .. } = result {
                    self.zip_buffers
                        .write()
                        .expect("zip_buffers lock poisoned")
                        .insert(path, buffer);
                } else if let ContainerResult::TempExtracted { temp_dir, .. } = result {
                    self.archive_temp_dirs.push(temp_dir);
                }

                self.file_list
                    .expand_container_at(index, entries, direction);
                self.container_states
                    .insert(container_path.to_path_buf(), ContainerState::Expanded);

                Ok(())
            }
        }
    }

    /// 全未展開コンテナを同期展開する (ブックマーク保存前等で全展開が必要な経路用)
    /// バックグラウンド展開と並走する可能性があるため、まず世代を進めて旧結果を破棄してから
    /// 残った PendingContainer を1つずつ直接同期展開する。
    pub fn expand_all_pending_sync(&mut self) {
        // 旧バックグラウンドジョブを即座に停止させて HDD 帯域を譲ってもらう。
        // 結果自体は expand_rx の drop と世代加算で次回 process_expand_results が破棄する。
        self.cancel_expansion();
        self.expand_rx = None;
        self.expand_generation += 1;

        // 全プレースホルダを順に同期展開
        while self.file_list.has_pending() {
            let pending = self
                .file_list
                .files()
                .iter()
                .enumerate()
                .find(|(_, f)| f.source.is_pending_container());
            let Some((idx, file)) = pending else { break };
            let container_path = match &file.source {
                FileSource::PendingContainer { container_path } => container_path.clone(),
                _ => break,
            };
            // 既に Expanded 状態 (バックグラウンドが直前に書き戻したケース) はスキップして
            // ファイルリストから単に取り除くだけにする
            if matches!(
                self.container_states.get(&container_path),
                Some(ContainerState::Expanded)
            ) {
                self.file_list.remove_at(idx);
                continue;
            }
            self.container_states
                .insert(container_path.clone(), ContainerState::InFlight);
            let result =
                process_single_container(&container_path, &self.archive_manager, Self::is_pdf);
            // expand_all_pending_sync は通常ナビゲーション前のバックエンド処理なので
            // 方向は意味を持たない。Forward を渡しておく。
            let _ = self.apply_container_result(
                idx,
                &container_path,
                result,
                NavigationDirection::Forward,
            );
        }

        self.file_list.clear_failed();
        if let Some(center) = self.file_list.current_index() {
            self.prefetch_coord
                .invalidate_and_reschedule(center, self.file_list.files());
        } else {
            self.prefetch_coord.invalidate();
        }
        let _ = self.event_sender.send(DocumentEvent::FileListChanged);
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
            // 削除失敗は無視する (ユニークdir名なので次回openに影響しない)
            let _ = std::fs::remove_dir_all(&temp_dir);
        }
        self.current_containers.clear();
        self.zip_buffers
            .write()
            .expect("zip_buffers lock poisoned")
            .clear();
    }

    /// フォルダを開く (先頭画像を表示)
    pub fn open_folder(&mut self, folder: &Path) -> Result<()> {
        // open_multiple() は read_dir エラーを無視するため、委譲前に確認して確実に伝播させる。
        std::fs::read_dir(folder)?;
        self.open_multiple(&[folder.to_path_buf()])
    }

    /// 相対移動
    pub fn navigate_relative(&mut self, offset: isize) {
        self.last_navigation_direction = if offset >= 0 {
            NavigationDirection::Forward
        } else {
            NavigationDirection::Backward
        };
        if self.file_list.navigate_relative(offset) {
            let _ = self.load_current();
        }
    }

    /// 最初へ移動
    pub fn navigate_first(&mut self) {
        self.last_navigation_direction = NavigationDirection::Forward;
        if self.file_list.navigate_first() {
            let _ = self.load_current();
        }
    }

    /// 指定インデックスへ移動
    pub fn navigate_to(&mut self, index: usize) {
        self.last_navigation_direction = NavigationDirection::Forward;
        if self.file_list.navigate_to(index) {
            let _ = self.load_current();
        }
    }

    /// 最後へ移動
    pub fn navigate_last(&mut self) {
        self.last_navigation_direction = NavigationDirection::Backward;
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

        // 0. PendingContainer なら同期展開せず、非同期待ちに入る
        //    (UIスレッドをブロックしないため。展開完了は process_expand_results が
        //    file_list へ統合し、direction-aware に current_index を調整した上で
        //    load_current() をリトライする)
        if self.file_list.is_pending_at(index)
            && let Some(container_path) = self.file_list.pending_container_path_at(index)
        {
            // 「読み込み中」状態に遷移
            self.current_image = None;

            // 直近の navigation direction を保存し、該当コンテナの展開を最優先化
            // (start_background_expansion 内で current_index 基準にソートされる)
            self.pending_navigation_intent = Some((container_path, self.last_navigation_direction));
            self.reschedule_background_expansion();

            // UI に状態変化を通知:
            //   - NavigationChanged: タイトルバーとファイルリスト選択を更新
            //     (update_title 側で「読み込み中: foo.zip」を表示する)
            //   - ImageReady: 画像領域を再描画して黒画面 (current_image=None) を表示
            self.send_navigation_changed();
            let _ = self.event_sender.send(DocumentEvent::ImageReady);
            return Ok(());
        }

        // 通常ファイルに到達したので、待機していた intent はクリアする
        self.pending_navigation_intent = None;

        // 1. キャッシュヒット → 瞬時切替 (永続フィルタはキャッシュ時に既に適用済み)
        if let Some(image) = self.prefetch_coord.take(index) {
            self.current_image = Some(image);
            let _ = self.event_sender.send(DocumentEvent::ImageReady);
            self.send_navigation_changed();
            self.schedule_prefetch();
            return Ok(());
        }

        // 2. キャッシュミス → 同期デコード (フォールバック)
        let current = self.file_list.current().expect("current_index was Some");
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
            let current = self.file_list.current().expect("current_index was Some");
            let data = self.read_file_data(current)?;
            let filename_hint = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            self.decoder.decode(&data, filename_hint)
        };

        match decode_result {
            Ok(image) => {
                // 永続フィルタを適用 (有効な場合のみ)
                let image = self.persistent_filter.apply(&image).unwrap_or(image);
                self.current_image = Some(image);
                let _ = self.event_sender.send(DocumentEvent::ImageReady);
            }
            Err(e) => {
                self.current_image = None;
                // 同期デコード失敗時はfailedマーク (ナビゲーション時にスキップ対象)
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
            let _ = self
                .event_sender
                .send(DocumentEvent::NavigationChanged { index });
        }
    }

    /// パスを正規化する (相対パスやUNCパス対応)
    /// Rustのcanonicalize() は\\?\プレフィックスを付与するが、
    /// SHFileOperationW等のShell APIが非対応のため除去する
    fn canonicalize(path: &Path) -> Result<PathBuf> {
        match std::fs::canonicalize(path) {
            Ok(canonical) => Ok(crate::util::strip_extended_length_prefix(&canonical)),
            Err(e) => {
                // UNCパスではcanonicalize() が失敗する場合がある
                // (ネットワーク遅延、DFS等)。ファイルが存在するならそのまま使う
                if path.exists() {
                    Ok(path.to_path_buf())
                } else {
                    Err(e).with_context(|| format!("パス解決失敗: {}", path.display()))
                }
            }
        }
    }

    /// FileInfoからファイルデータを取得する (オンデマンドアーカイブ対応)
    fn read_file_data(&self, info: &crate::file_info::FileInfo) -> Result<Vec<u8>> {
        match &info.source {
            FileSource::ArchiveEntry {
                archive,
                entry,
                on_demand: true,
            } => {
                // キャッシュされたZIPバッファから取得 (Stored最適化付き)
                let buffers = self.zip_buffers.read().expect("zip_buffers lock poisoned");
                if let Some(buffer) = buffers.get(archive) {
                    crate::archive::zip::ZipHandler::read_entry_from_buffer(buffer.as_ref(), entry)
                } else {
                    // キャッシュミス (通常発生しない): ファイルから直接取得
                    drop(buffers);
                    self.archive_manager.read_entry(archive, entry)
                }
            }
            FileSource::PendingContainer { .. } => {
                anyhow::bail!("未展開コンテナからは取得できない")
            }
            _ => std::fs::read(&info.path)
                .with_context(|| format!("ファイル読み込み失敗: {}", info.path.display())),
        }
    }

    /// 現在のファイルのデータを取得する (app.rsのファイル操作用)
    pub fn read_file_data_current(&self) -> Result<Vec<u8>> {
        let current = self
            .file_list
            .current()
            .ok_or_else(|| anyhow::anyhow!("ファイルが選択されていない"))?;
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

    /// パスがコンテナ (アーカイブ・PDF・ブックマーク) か判定する
    pub fn is_container(&self, path: &Path) -> bool {
        self.archive_manager.is_archive(path)
            || Self::is_pdf(path)
            || crate::bookmark::is_bookmark_file(path)
    }

    /// パスがアーカイブ拡張子を持つか判定する (ブックマーク復元時のコンテナ検出用)
    pub fn is_archive_path(&self, path: &Path) -> bool {
        self.archive_manager.is_archive(path)
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
        self.last_navigation_direction = NavigationDirection::Backward;
        if self.file_list.navigate_prev_mark() {
            let _ = self.load_current();
        }
    }

    /// 次のマーク画像へ移動
    pub fn navigate_next_mark(&mut self) {
        self.last_navigation_direction = NavigationDirection::Forward;
        if self.file_list.navigate_next_mark() {
            let _ = self.load_current();
        }
    }

    // --- フォルダナビゲーション ---

    /// 前のフォルダへ移動
    /// 「前のフォルダの先頭画像」を表示する仕様なので、PendingContainer 到達時も
    /// 展開後グループの先頭で正しい (Forward 扱い)
    pub fn navigate_prev_folder(&mut self) {
        self.last_navigation_direction = NavigationDirection::Forward;
        if self.file_list.navigate_prev_folder() {
            let _ = self.load_current();
        }
    }

    /// 次のフォルダへ移動
    pub fn navigate_next_folder(&mut self) {
        self.last_navigation_direction = NavigationDirection::Forward;
        if self.file_list.navigate_next_folder() {
            let _ = self.load_current();
        }
    }

    /// ソート順で前の画像へ移動
    pub fn sort_navigate_back(&mut self) {
        self.last_navigation_direction = NavigationDirection::Backward;
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

    /// グループ順をシャッフル (グループ内順序は保持)
    pub fn shuffle_groups(&mut self) {
        self.file_list.shuffle_groups();
        self.after_list_change();
    }

    /// ソート順で次の画像へ移動
    pub fn sort_navigate_forward(&mut self) {
        self.last_navigation_direction = NavigationDirection::Forward;
        let order = self.file_list.sort_order();
        if self.file_list.sorted_navigate(1, order) {
            let _ = self.load_current();
        }
    }

    /// 現在のファイルをリストから削除する (ファイル自体は残る)
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

    /// マーク済みファイルのパスを移動先ディレクトリに更新する
    pub fn update_marked_paths(&mut self, dest_dir: &Path) -> Result<()> {
        self.file_list.update_marked_paths(dest_dir)?;
        self.after_list_change();
        Ok(())
    }

    /// 現在のファイルをリスト内でリネーム (同一フォルダ内の移動後)
    /// 先読みキャッシュを無効化し、リスト内の位置はそのまま維持する
    pub fn rename_current_in_list(&mut self, new_path: &Path) -> Result<()> {
        let index = self
            .file_list
            .current_index()
            .ok_or_else(|| anyhow::anyhow!("ファイルが選択されていない"))?;
        self.file_list.update_file_at(index, new_path)?;
        self.invalidate_cache();
        let _ = self.event_sender.send(DocumentEvent::FileListChanged);
        if self.file_list.len() > 0 {
            let _ = self.load_current();
        }
        Ok(())
    }

    /// リスト変更後の共通処理 (キャッシュ無効化+再読込+イベント送信)
    /// 並べ替えで PendingContainer の位置関係が変わっている可能性があるため、
    /// バックグラウンド展開を再優先度付け (現在位置基準) する。
    pub fn after_list_change(&mut self) {
        self.invalidate_cache();
        // シャッフル/削除等でリスト位置が変わった可能性 → 展開キューを再ソート
        self.reschedule_background_expansion();
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
                FileSource::PdfPage { .. }
                    | FileSource::ArchiveEntry { .. }
                    | FileSource::PendingContainer { .. }
            )
        });

        if has_containers {
            // コンテナモード: ユニークなコンテナパスを収集
            let mut container_paths: Vec<PathBuf> = Vec::new();
            for source in &data.entries {
                let path = match source {
                    FileSource::PdfPage { pdf_path, .. } => Some(pdf_path),
                    FileSource::ArchiveEntry { archive, .. } => Some(archive),
                    FileSource::PendingContainer { container_path } => Some(container_path),
                    FileSource::File(_) => None,
                };
                if let Some(p) = path
                    && p.exists()
                    && !container_paths.contains(p)
                {
                    container_paths.push(p.clone());
                }
            }

            if container_paths.is_empty() {
                anyhow::bail!("ブックマーク内のコンテナが見つからない");
            }

            self.open_containers(&container_paths)?;

            // 全コンテナを同期展開してから位置復元する。
            // open_containers は先頭コンテナのみ即時展開し残りは PendingContainer のため、
            // 非先頭コンテナ内エントリを指す index の source_matches が失敗する。
            // ブックマーク読み込みはユーザー明示操作なので同期展開のブロッキングは許容する。
            self.expand_all_pending_sync();

            // 位置復元: source同一性で検索 (PDF/Archive両対応)
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
            .ok_or_else(|| anyhow::anyhow!("ファイルが選択されていない"))?;

        // PDFページの場合はcurrent_imageからメタデータを生成
        if matches!(current.source, FileSource::PdfPage { .. }) {
            return if self.current_image.is_some() {
                Ok(crate::image::ImageMetadata {
                    format: "PDF".to_string(),
                    comments: Vec::new(),
                    exif: Vec::new(),
                })
            } else {
                anyhow::bail!("PDFページが未レンダリング")
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
        self.prefetch_coord.contains(index)
            || (self.file_list.current_index() == Some(index) && self.current_image.is_some())
    }

    // --- 編集セッション ---

    /// 未保存の編集があるかどうか
    pub fn has_unsaved_edit(&self) -> bool {
        self.editing_session
            .as_ref()
            .is_some_and(EditingSession::has_unsaved_changes)
    }

    /// 編集セッションを開始する (まだ開始していない場合)
    /// 現在の画像をバックアップとして保持する
    fn ensure_editing_session(&mut self) {
        if self.editing_session.is_some() {
            return;
        }
        if let Some(img) = &self.current_image {
            let backup = DecodedImage {
                data: img.data.clone(),
                width: img.width,
                height: img.height,
            };
            self.editing_session = Some(EditingSession::new(backup));
        }
    }

    /// 編集セッションを破棄する (未保存の変更を破棄する)
    pub fn discard_editing_session(&mut self) {
        self.editing_session = None;
    }

    /// 永続フィルタへの参照
    pub fn persistent_filter(&self) -> &PersistentFilter {
        &self.persistent_filter
    }

    /// 永続フィルタへの可変参照
    pub fn persistent_filter_mut(&mut self) -> &mut PersistentFilter {
        &mut self.persistent_filter
    }

    /// 永続フィルタ設定変更後にキャッシュを全無効化して再読込する
    pub fn on_persistent_filter_changed(&mut self) {
        self.invalidate_cache();
        let _ = self.load_current();
    }

    /// current_imageを編集結果で置き換える
    pub fn apply_edit(&mut self, new_image: DecodedImage) {
        self.ensure_editing_session();
        self.current_image = Some(new_image);
        if let Some(session) = &mut self.editing_session {
            session.mark_modified();
        }
        let _ = self.event_sender.send(DocumentEvent::ImageReady);
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        self.cleanup_archive_temp();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use crate::test_helpers::{create_1x1_white_png, test_document};

    /// テスト用の一時ディレクトリにダミー画像を配置する
    fn setup_test_dir(name: &str, count: usize) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gv_test_document_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let png_data = create_1x1_white_png();
        for i in 0..count {
            let path = dir.join(format!("image_{i:03}.png"));
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(&png_data).unwrap();
        }
        dir
    }

    fn cleanup_test_dir(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn new_initial_state() {
        let (doc, _rx) = test_document();
        assert!(doc.current_image().is_none());
        assert!(doc.current_path().is_none());
        assert_eq!(doc.file_list().len(), 0);
        assert_eq!(doc.file_list().current_index(), None);
    }

    #[test]
    fn new_propagates_default_sort_to_file_list() {
        // `Document::new` が受け取った既定ソート種別がファイル一覧へ伝達されることを確認する。
        // `test_document` は `SortOrder::default()` (= `Natural`) を渡す。
        let (doc, _rx) = test_document();
        assert_eq!(doc.file_list().sort_order(), SortOrder::default());

        // 明示的に異なる種別を渡したケースも検証する。
        let (sender, _rx2) = crossbeam_channel::unbounded();
        let registry = Arc::new(ExtensionRegistry::new());
        let decoder = crate::test_helpers::test_decoder();
        let archive_manager = crate::test_helpers::test_archive_manager(&registry);
        let doc2 = Document::new(sender, decoder, registry, archive_manager, SortOrder::Date);
        assert_eq!(doc2.file_list().sort_order(), SortOrder::Date);
    }

    #[test]
    fn open_folder_populates_list() {
        let dir = setup_test_dir("open", 3);
        let (mut doc, _rx) = test_document();
        doc.open_folder(&dir).unwrap();

        assert_eq!(doc.file_list().len(), 3);
        assert_eq!(doc.file_list().current_index(), Some(0));
        cleanup_test_dir(&dir);
    }

    #[test]
    fn navigate_relative_forward_backward() {
        let dir = setup_test_dir("navigate", 5);
        let (mut doc, _rx) = test_document();
        doc.open_folder(&dir).unwrap();

        assert_eq!(doc.file_list().current_index(), Some(0));
        doc.navigate_relative(2);
        assert_eq!(doc.file_list().current_index(), Some(2));
        doc.navigate_relative(-1);
        assert_eq!(doc.file_list().current_index(), Some(1));
        cleanup_test_dir(&dir);
    }

    #[test]
    fn navigate_first_last() {
        let dir = setup_test_dir("first_last", 5);
        let (mut doc, _rx) = test_document();
        doc.open_folder(&dir).unwrap();

        doc.navigate_last();
        assert_eq!(doc.file_list().current_index(), Some(4));
        doc.navigate_first();
        assert_eq!(doc.file_list().current_index(), Some(0));
        cleanup_test_dir(&dir);
    }

    #[test]
    fn navigate_to_index() {
        let dir = setup_test_dir("to_index", 5);
        let (mut doc, _rx) = test_document();
        doc.open_folder(&dir).unwrap();

        doc.navigate_to(3);
        assert_eq!(doc.file_list().current_index(), Some(3));
        cleanup_test_dir(&dir);
    }

    #[test]
    fn mark_operations() {
        let dir = setup_test_dir("marks", 3);
        let (mut doc, _rx) = test_document();
        doc.open_folder(&dir).unwrap();

        assert_eq!(doc.file_list().marked_count(), 0);

        doc.mark_current(); // mark index 0, move to 1
        assert_eq!(doc.file_list().current_index(), Some(1));
        assert_eq!(doc.file_list().marked_count(), 1);

        doc.navigate_first();
        doc.invert_all_marks(); // toggle all marks (0 marked -> 0, 1, 2 marked; 0 is unmarked)
        let marked = doc.file_list().marked_count();
        assert!(marked > 0 && marked < 3); // should have some marked and some unmarked
        cleanup_test_dir(&dir);
    }

    #[test]
    fn is_pdf_detection() {
        assert!(Document::is_pdf(Path::new("test.pdf")));
        assert!(Document::is_pdf(Path::new("test.PDF")));
        assert!(!Document::is_pdf(Path::new("test.png")));
        assert!(!Document::is_pdf(Path::new("test.pdf.txt")));
    }

    #[test]
    fn cancel_expansion_atomic_flag() {
        let (_doc, _rx) = test_document();
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = Arc::clone(&flag);
        assert!(!flag_clone.load(Ordering::Relaxed));

        flag.store(true, Ordering::Relaxed);
        assert!(flag_clone.load(Ordering::Relaxed));
    }
}
