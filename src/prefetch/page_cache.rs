use std::collections::HashMap;

use crate::image::DecodedImage;

/// デコード済み画像のキャッシュ
/// インデックスベースのHashMapで管理し、メモリ使用量を追跡する
pub struct PageCache {
    cache: HashMap<usize, DecodedImage>,
    max_memory: usize,
    current_memory: usize,
}

impl PageCache {
    pub fn new(max_memory: usize) -> Self {
        Self {
            cache: HashMap::new(),
            max_memory,
            current_memory: 0,
        }
    }

    /// 画像をキャッシュに格納する。メモリ超過時はfalseを返し格納しない
    pub fn insert(&mut self, index: usize, image: DecodedImage) -> bool {
        let size = image.memory_size();

        // 同じインデックスの既存エントリを除去
        if let Some(old) = self.cache.remove(&index) {
            self.current_memory -= old.memory_size();
        }

        if self.current_memory + size > self.max_memory {
            return false;
        }

        self.current_memory += size;
        self.cache.insert(index, image);
        true
    }

    /// 参照取得
    #[allow(dead_code)]
    pub fn get(&self, index: usize) -> Option<&DecodedImage> {
        self.cache.get(&index)
    }

    /// 所有権取得+キャッシュ除去+メモリ使用量更新
    pub fn take(&mut self, index: usize) -> Option<DecodedImage> {
        if let Some(image) = self.cache.remove(&index) {
            self.current_memory -= image.memory_size();
            Some(image)
        } else {
            None
        }
    }

    pub fn contains(&self, index: usize) -> bool {
        self.cache.contains_key(&index)
    }

    /// 指定範囲外のエントリを全削除
    pub fn evict_outside(&mut self, center: usize, range_back: usize, range_fwd: usize) {
        let low = center.saturating_sub(range_back);
        let high = center.saturating_add(range_fwd);

        let to_remove: Vec<usize> = self
            .cache
            .keys()
            .filter(|&&idx| idx < low || idx > high)
            .copied()
            .collect();

        for idx in to_remove {
            if let Some(image) = self.cache.remove(&idx) {
                self.current_memory -= image.memory_size();
            }
        }
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.current_memory = 0;
    }

    pub fn set_max_memory(&mut self, max: usize) {
        self.max_memory = max;
    }
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
    fn insert_and_get() {
        let mut cache = PageCache::new(1024);
        let img = make_image(100);
        assert!(cache.insert(0, img));
        assert!(cache.contains(0));
        assert!(cache.get(0).is_some());
    }

    #[test]
    fn take_removes_entry() {
        let mut cache = PageCache::new(1024);
        cache.insert(0, make_image(100));

        let taken = cache.take(0);
        assert!(taken.is_some());
        assert!(!cache.contains(0));
        assert_eq!(cache.current_memory, 0);
    }

    #[test]
    fn insert_rejects_when_over_budget() {
        let mut cache = PageCache::new(200);
        assert!(cache.insert(0, make_image(150)));
        // 150 + 100 = 250 > 200
        assert!(!cache.insert(1, make_image(100)));
        assert!(!cache.contains(1));
    }

    #[test]
    fn insert_replaces_same_index() {
        let mut cache = PageCache::new(200);
        assert!(cache.insert(0, make_image(100)));
        // 既存の100を除去してから150を追加 → 150 <= 200
        assert!(cache.insert(0, make_image(150)));
        assert_eq!(cache.current_memory, 150);
    }

    #[test]
    fn evict_outside_removes_distant_entries() {
        let mut cache = PageCache::new(10000);
        for i in 0..10 {
            cache.insert(i, make_image(100));
        }

        // center=5, back=2, fwd=2 → keep 3..=7
        cache.evict_outside(5, 2, 2);

        for i in 0..3 {
            assert!(!cache.contains(i), "index {i} should be evicted");
        }
        for i in 3..=7 {
            assert!(cache.contains(i), "index {i} should remain");
        }
        for i in 8..10 {
            assert!(!cache.contains(i), "index {i} should be evicted");
        }
        assert_eq!(cache.current_memory, 500); // 5 entries × 100
    }

    #[test]
    fn clear_resets_all() {
        let mut cache = PageCache::new(1024);
        cache.insert(0, make_image(100));
        cache.insert(1, make_image(200));

        cache.clear();
        assert!(!cache.contains(0));
        assert!(!cache.contains(1));
        assert_eq!(cache.current_memory, 0);
    }

    #[test]
    fn take_nonexistent_returns_none() {
        let mut cache = PageCache::new(1024);
        assert!(cache.take(99).is_none());
    }
}
