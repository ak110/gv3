use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result};

use super::{ArchiveHandler, ExtractedEntry, extract_filename, resolve_filename};
use crate::extension_registry::ExtensionRegistry;

/// ZIP/cbzアーカイブハンドラ
pub struct ZipHandler {
    registry: Arc<ExtensionRegistry>,
}

impl ZipHandler {
    pub fn new(registry: Arc<ExtensionRegistry>) -> Self {
        Self { registry }
    }
}

impl ZipHandler {
    /// インメモリバッファからエントリ一覧を取得する
    pub fn list_images_from_buffer(
        buffer: &[u8],
        registry: &ExtensionRegistry,
    ) -> Result<Vec<super::ArchiveImageEntry>> {
        let cursor = std::io::Cursor::new(buffer);
        let archive = zip::ZipArchive::new(cursor).context("ZIPバッファの読み取りに失敗")?;
        Ok(Self::list_images_from_archive(archive, registry))
    }

    /// インメモリバッファからエントリを読み出す（Stored最適化付き）
    pub fn read_entry_from_buffer(buffer: &[u8], entry_name: &str) -> Result<Vec<u8>> {
        let cursor = std::io::Cursor::new(buffer);
        let mut archive = zip::ZipArchive::new(cursor).context("ZIPバッファの読み取りに失敗")?;
        let mut entry = archive
            .by_name(entry_name)
            .with_context(|| format!("エントリが見つかりません: {entry_name}"))?;

        // Storedエントリ: バッファから直接スライス（zip Readerのオーバーヘッド回避）
        if entry.compression() == zip::CompressionMethod::Stored {
            let start = entry.data_start().context("データ開始位置の取得に失敗")? as usize;
            let size = entry.size() as usize;
            drop(entry);
            return Ok(buffer[start..start + size].to_vec());
        }

        // 圧縮エントリ: 通常の展開
        let mut data = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut data)?;
        Ok(data)
    }

    /// ファイルパスからエントリ一覧を取得する
    #[cfg(test)]
    pub fn list_images(&self, archive_path: &Path) -> Result<Vec<super::ArchiveImageEntry>> {
        let file = File::open(archive_path)
            .with_context(|| format!("アーカイブを開けません: {}", archive_path.display()))?;
        let archive = zip::ZipArchive::new(file)
            .with_context(|| format!("ZIP読み取り失敗: {}", archive_path.display()))?;
        Ok(Self::list_images_from_archive(archive, &self.registry))
    }

    /// ZipArchiveからエントリ一覧を取得する共通実装
    fn list_images_from_archive<R: std::io::Read + std::io::Seek>(
        mut archive: zip::ZipArchive<R>,
        registry: &ExtensionRegistry,
    ) -> Vec<super::ArchiveImageEntry> {
        let mut results = Vec::new();
        for i in 0..archive.len() {
            let Ok(entry) = archive.by_index_raw(i) else {
                continue;
            };
            if entry.is_dir() {
                continue;
            }
            let entry_name = entry.name().to_string();
            let file_size = entry.size();
            let filename = extract_filename(&entry_name).to_string();
            if filename.is_empty() || filename.starts_with('.') {
                continue;
            }
            if !registry.is_image_extension(&filename) {
                continue;
            }
            results.push(super::ArchiveImageEntry {
                entry_name,
                file_name: filename,
                file_size,
            });
        }
        results
    }
}

impl ArchiveHandler for ZipHandler {
    fn supported_extensions(&self) -> Vec<String> {
        vec![".zip".to_string(), ".cbz".to_string()]
    }

    fn supports_on_demand(&self) -> bool {
        true
    }

    fn read_entry(&self, archive_path: &Path, entry_name: &str) -> Result<Vec<u8>> {
        let file = File::open(archive_path)
            .with_context(|| format!("アーカイブを開けません: {}", archive_path.display()))?;
        let mut archive = zip::ZipArchive::new(file)
            .with_context(|| format!("ZIP読み取り失敗: {}", archive_path.display()))?;
        let mut entry = archive
            .by_name(entry_name)
            .with_context(|| format!("エントリが見つかりません: {entry_name}"))?;
        let mut data = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut data)?;
        Ok(data)
    }

    fn extract_images(
        &self,
        archive_path: &Path,
        target_dir: &Path,
    ) -> Result<Vec<ExtractedEntry>> {
        let file = File::open(archive_path)
            .with_context(|| format!("アーカイブを開けません: {}", archive_path.display()))?;
        let mut archive = zip::ZipArchive::new(file)
            .with_context(|| format!("ZIP読み取り失敗: {}", archive_path.display()))?;

        let mut results = Vec::new();

        for i in 0..archive.len() {
            let Ok(mut entry) = archive.by_index(i) else {
                continue;
            };

            // ディレクトリエントリはスキップ
            if entry.is_dir() {
                continue;
            }

            let entry_name = entry.name().to_string();
            let filename = extract_filename(&entry_name);

            // 空ファイル名やドット始まりの隠しファイルはスキップ
            if filename.is_empty() || filename.starts_with('.') {
                continue;
            }

            // 画像ファイルのみ展開
            if !self.registry.is_image_extension(filename) {
                continue;
            }

            // ファイルデータを読み出し
            let mut data = Vec::new();
            if entry.read_to_end(&mut data).is_err() {
                continue;
            }

            // target_dirに書き出し（重複時はリネーム）
            let out_path = resolve_filename(target_dir, filename);
            if std::fs::write(&out_path, &data).is_ok() {
                results.push((out_path, entry_name));
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// テスト用ZIPをメモリ上で作成してtempに書き出す
    fn create_test_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn extract_images_from_zip() {
        let dir = std::env::temp_dir().join("gv_test_zip_extract");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let zip_path = dir.join("test.zip");
        create_test_zip(
            &zip_path,
            &[
                ("image1.jpg", b"fake-jpg-data"),
                ("subfolder/image2.png", b"fake-png-data"),
                ("readme.txt", b"not an image"),
                ("image3.bmp", b"fake-bmp-data"),
            ],
        );

        let out_dir = dir.join("out");
        std::fs::create_dir_all(&out_dir).unwrap();

        let reg = Arc::new(ExtensionRegistry::new());
        let handler = ZipHandler::new(reg);
        let entries = handler.extract_images(&zip_path, &out_dir).unwrap();

        assert_eq!(entries.len(), 3);
        assert!(out_dir.join("image1.jpg").exists());
        assert!(out_dir.join("image2.png").exists()); // サブフォルダはフラット化
        assert!(out_dir.join("image3.bmp").exists());
        assert!(!out_dir.join("readme.txt").exists());

        // エントリパスが元のアーカイブ内パスを保持していることを確認
        let entry_paths: Vec<&str> = entries.iter().map(|(_, e)| e.as_str()).collect();
        assert!(entry_paths.contains(&"image1.jpg"));
        assert!(entry_paths.contains(&"subfolder/image2.png"));
        assert!(entry_paths.contains(&"image3.bmp"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_handles_duplicate_filenames() {
        let dir = std::env::temp_dir().join("gv_test_zip_dup");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let zip_path = dir.join("dup.zip");
        create_test_zip(
            &zip_path,
            &[("a/image.jpg", b"data1"), ("b/image.jpg", b"data2")],
        );

        let out_dir = dir.join("out");
        std::fs::create_dir_all(&out_dir).unwrap();

        let reg = Arc::new(ExtensionRegistry::new());
        let handler = ZipHandler::new(reg);
        let entries = handler.extract_images(&zip_path, &out_dir).unwrap();

        assert_eq!(entries.len(), 2);
        assert!(out_dir.join("image.jpg").exists());
        assert!(out_dir.join("image_2.jpg").exists());

        // 元エントリパスはそれぞれ異なる
        assert_eq!(entries[0].1, "a/image.jpg");
        assert_eq!(entries[1].1, "b/image.jpg");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ZIPをメモリ上で作成しバイト列として返す
    fn create_test_zip_buffer(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buf);
            let mut writer = zip::ZipWriter::new(cursor);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, data) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buf
    }

    #[test]
    fn list_images_returns_image_entries_only() {
        let dir = std::env::temp_dir().join("gv_test_zip_list");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("test.zip");
        create_test_zip(
            &zip_path,
            &[
                ("image1.jpg", b"fake-jpg"),
                ("subfolder/image2.png", b"fake-png"),
                ("readme.txt", b"text"),
            ],
        );

        let reg = Arc::new(ExtensionRegistry::new());
        let handler = ZipHandler::new(reg);
        let entries = handler.list_images(&zip_path).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry_name, "image1.jpg");
        assert_eq!(entries[0].file_name, "image1.jpg");
        assert_eq!(entries[1].entry_name, "subfolder/image2.png");
        assert_eq!(entries[1].file_name, "image2.png");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_entry_returns_data() {
        let dir = std::env::temp_dir().join("gv_test_zip_read");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("test.zip");
        create_test_zip(&zip_path, &[("image.jpg", b"fake-jpg-data-123")]);

        let reg = Arc::new(ExtensionRegistry::new());
        let handler = ZipHandler::new(reg);
        let data = handler.read_entry(&zip_path, "image.jpg").unwrap();
        assert_eq!(data, b"fake-jpg-data-123");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_images_from_buffer_works() {
        let reg = Arc::new(ExtensionRegistry::new());
        let buffer = create_test_zip_buffer(&[
            ("a.jpg", b"jpg-data"),
            ("b.png", b"png-data"),
            ("c.txt", b"text"),
        ]);
        let entries = ZipHandler::list_images_from_buffer(&buffer, &reg).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].file_name, "a.jpg");
        assert_eq!(entries[1].file_name, "b.png");
    }

    #[test]
    fn read_entry_from_buffer_stored() {
        let buffer = create_test_zip_buffer(&[("img.jpg", b"stored-data")]);
        let data = ZipHandler::read_entry_from_buffer(&buffer, "img.jpg").unwrap();
        assert_eq!(data, b"stored-data");
    }

    #[test]
    fn read_entry_from_buffer_compressed() {
        // Deflate圧縮のZIPを作成
        let mut buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buf);
            let mut writer = zip::ZipWriter::new(cursor);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("img.jpg", options).unwrap();
            writer.write_all(b"deflated-data-content").unwrap();
            writer.finish().unwrap();
        }
        let data = ZipHandler::read_entry_from_buffer(&buf, "img.jpg").unwrap();
        assert_eq!(data, b"deflated-data-content");
    }
}
