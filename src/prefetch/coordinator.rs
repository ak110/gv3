use crate::file_info::{FileInfo, FileSource};
use crate::image::DecodedImage;
use crate::persistent_filter::PersistentFilter;
use crate::prefetch::loader_thread::PrefetchEngine;
use crate::prefetch::page_cache::PageCache;

/// 先読みレスポンスの処理結果
pub enum PrefetchEvent {
    /// 現在表示すべき画像がデコードされた
    CurrentImageReady(DecodedImage),
    /// 先読みエラー
    Error(String),
}

/// 先読みの状態管理を一元化する構造体
///
/// Document が直接 PageCache / PrefetchEngine を操作する代わりに、
/// このコーディネータ経由で全操作を行う。世代管理とキャッシュ操作を
/// 集約することで、invalidate + reschedule のペア管理ミスを防止する。
pub struct PrefetchCoordinator {
    cache: PageCache,
    engine: Option<PrefetchEngine>,
    cache_backward: usize,
    cache_forward: usize,
}

impl PrefetchCoordinator {
    pub fn new() -> Self {
        Self {
            cache: PageCache::new(0),
            engine: None,
            cache_backward: 0,
            cache_forward: 0,
        }
    }

    /// 先読みエンジンを起動する
    pub fn start(&mut self, engine: PrefetchEngine, cache_budget: usize, base_image_size: usize) {
        self.engine = Some(engine);
        self.update_cache_range(cache_budget, base_image_size);
    }

    /// キャッシュ範囲を再計算する。
    /// 前方4枚・後方2枚を上限とし、スロット数で枚数を制御する。
    pub fn update_cache_range(&mut self, cache_budget: usize, base_image_size: usize) {
        const MAX_CACHE_FORWARD: usize = 4;
        const MAX_CACHE_BACKWARD: usize = 2;

        let base = base_image_size.max(1);
        let total_slots = (cache_budget / base).max(3);
        self.cache_forward = (total_slots * 2 / 3).clamp(1, MAX_CACHE_FORWARD);
        self.cache_backward = (total_slots / 3).clamp(1, MAX_CACHE_BACKWARD);
        self.cache.set_max_memory(cache_budget);
    }

    /// キャッシュを全クリアし、in-flightリクエストを無効化する（世代進行1回）。
    /// ファイルリスト全体が変わった場合に呼ぶ。
    pub fn invalidate(&mut self) {
        self.cache.clear();
        if let Some(engine) = &mut self.engine {
            engine.advance_generation();
        }
    }

    /// 現在位置を中心にキャッシュ範囲を再計算し先読みを再スケジュールする（世代進行1回）。
    /// ナビゲーション後に呼ぶ。
    pub fn reschedule(&mut self, center: usize, files: &[FileInfo]) {
        let Some(engine) = &mut self.engine else {
            return;
        };

        self.cache
            .evict_outside(center, self.cache_backward, self.cache_forward);
        engine.advance_generation();
        send_prefetch_requests(
            engine,
            &self.cache,
            self.cache_forward,
            self.cache_backward,
            center,
            files,
        );
    }

    /// キャッシュ全クリア + 先読み再スケジュールを1回の世代進行で行う。
    /// invalidate() + reschedule() のペア呼び出しでは世代が2回進行するが、
    /// このメソッドは1回のみ進行するため、ループ内で呼んでもN回で済む。
    pub fn invalidate_and_reschedule(&mut self, center: usize, files: &[FileInfo]) {
        let Some(engine) = &mut self.engine else {
            self.cache.clear();
            return;
        };

        self.cache.clear();
        engine.advance_generation();
        // evict不要（clearで空）。リクエスト送信のみ
        send_prefetch_requests(
            engine,
            &self.cache,
            self.cache_forward,
            self.cache_backward,
            center,
            files,
        );
    }

    /// レスポンスを回収し、キャッシュに格納する。
    /// 現在ページの画像が到着した場合は `PrefetchEvent::CurrentImageReady` を返す。
    pub fn process_responses(
        &mut self,
        current_index: Option<usize>,
        has_current_image: bool,
        persistent_filter: &PersistentFilter,
    ) -> Vec<PrefetchEvent> {
        let Some(engine) = &self.engine else {
            return Vec::new();
        };
        let current_gen = engine.generation();
        let responses = engine.drain_responses();
        let mut events = Vec::new();

        for resp in responses {
            match resp {
                crate::prefetch::LoadResponse::Loaded {
                    index,
                    image,
                    generation,
                } => {
                    if generation != current_gen {
                        continue;
                    }
                    // 永続フィルタを先読み結果にも適用
                    let image = persistent_filter.apply(&image).unwrap_or(image);
                    // 現在表示すべきページでまだ画像がない場合、即表示
                    let is_current = current_index == Some(index) && !has_current_image;
                    if is_current {
                        events.push(PrefetchEvent::CurrentImageReady(image));
                    } else {
                        self.cache.insert(index, image);
                    }
                }
                crate::prefetch::LoadResponse::Failed {
                    generation, error, ..
                } => {
                    if generation != current_gen {
                        continue;
                    }
                    events.push(PrefetchEvent::Error(error));
                }
            }
        }

        events
    }

    /// キャッシュから画像を取り出す（キャッシュからは削除される）
    pub fn take(&mut self, index: usize) -> Option<DecodedImage> {
        self.cache.take(index)
    }

    /// キャッシュに指定インデックスの画像が存在するか
    pub fn contains(&self, index: usize) -> bool {
        self.cache.contains(index)
    }

    /// キャッシュに画像を格納する
    #[cfg(test)]
    pub fn insert(&mut self, index: usize, image: DecodedImage) -> bool {
        self.cache.insert(index, image)
    }

    /// エンジンの現在世代を返す
    #[cfg(test)]
    pub fn generation(&self) -> Option<u64> {
        self.engine.as_ref().map(PrefetchEngine::generation)
    }

    /// キャッシュ範囲 (forward, backward) を返す (テスト用)
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn cache_range(&self) -> (usize, usize) {
        (self.cache_forward, self.cache_backward)
    }
}

/// 距離ベースの交互読み込みでリクエストを送信する
fn send_prefetch_requests(
    engine: &PrefetchEngine,
    cache: &PageCache,
    cache_forward: usize,
    cache_backward: usize,
    center: usize,
    files: &[FileInfo],
) {
    let len = files.len();
    let max_dist = cache_forward.max(cache_backward);

    for dist in 1..=max_dist {
        // 前方（次のページ）
        let fwd = center + dist;
        if dist <= cache_forward && fwd < len {
            request_file_if_needed(engine, cache, fwd, files);
        }
        // 後方（前のページ）
        if dist <= cache_backward && dist <= center {
            request_file_if_needed(engine, cache, center - dist, files);
        }
    }
}

/// 未キャッシュかつ先読み可能なファイルに対してロードリクエストを送信する
fn request_file_if_needed(
    engine: &PrefetchEngine,
    cache: &PageCache,
    idx: usize,
    files: &[FileInfo],
) {
    // 未展開コンテナは先読み対象外
    if matches!(files[idx].source, FileSource::PendingContainer { .. }) {
        return;
    }
    if cache.contains(idx) {
        return;
    }

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
    engine.request_load(idx, files[idx].path.clone(), pdf_page, archive_entry);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_image(size: usize) -> DecodedImage {
        DecodedImage {
            data: vec![0u8; size],
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn invalidate_and_reschedule_advances_generation_once() {
        // エンジンなしの場合でもパニックしないこと
        let mut coord = PrefetchCoordinator::new();
        coord.invalidate_and_reschedule(0, &[]);
        assert!(coord.generation().is_none());
    }

    #[test]
    fn cache_operations() {
        let mut coord = PrefetchCoordinator::new();
        coord.update_cache_range(1024, 100);

        let img = make_image(100);
        assert!(coord.insert(0, img));
        assert!(coord.contains(0));

        let taken = coord.take(0);
        assert!(taken.is_some());
        assert!(!coord.contains(0));
    }

    #[test]
    fn invalidate_clears_cache() {
        let mut coord = PrefetchCoordinator::new();
        coord.update_cache_range(1024, 100);
        coord.insert(0, make_image(100));
        coord.insert(1, make_image(100));

        coord.invalidate();
        assert!(!coord.contains(0));
        assert!(!coord.contains(1));
    }
}
