/// テスト共通ヘルパー
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::archive::ArchiveManager;
use crate::document::{Document, DocumentEvent, ZipBuffer};
use crate::extension_registry::ExtensionRegistry;
use crate::file_list::SortOrder;
use crate::image::{DecoderChain, StandardDecoder};

/// テスト用DecoderChainを生成する (StandardDecoderのみ)
pub fn test_decoder() -> Arc<DecoderChain> {
    Arc::new(DecoderChain::new(vec![Box::new(StandardDecoder::new())]))
}

/// テスト用ArchiveManagerを生成する
pub fn test_archive_manager(registry: &Arc<ExtensionRegistry>) -> ArchiveManager {
    ArchiveManager::new(Arc::clone(registry))
}

/// テスト用ZIPバッファを生成する
#[allow(dead_code)]
pub fn test_zip_buffers() -> Arc<RwLock<HashMap<PathBuf, ZipBuffer>>> {
    Arc::new(RwLock::new(HashMap::new()))
}

/// テスト用Documentとイベントレシーバーを生成する
pub fn test_document() -> (Document, crossbeam_channel::Receiver<DocumentEvent>) {
    let (sender, receiver) = crossbeam_channel::unbounded();
    let registry = Arc::new(ExtensionRegistry::new());
    let decoder = test_decoder();
    let archive_manager = test_archive_manager(&registry);
    let doc = Document::new(
        sender,
        decoder,
        registry,
        archive_manager,
        SortOrder::default(),
    );
    (doc, receiver)
}

/// テスト用: 1x1 白ピクセルのPNGバイナリを生成する
pub fn create_1x1_white_png() -> Vec<u8> {
    use image::{ImageBuffer, Rgba};
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(1, 1, Rgba([255, 255, 255, 255]));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}
