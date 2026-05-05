use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result};

use super::{ArchiveHandler, ExtractedEntry, extract_filename, resolve_filename};
use crate::extension_registry::ExtensionRegistry;

/// 7zアーカイブハンドラ
pub struct SevenZHandler {
    registry: Arc<ExtensionRegistry>,
}

impl SevenZHandler {
    pub fn new(registry: Arc<ExtensionRegistry>) -> Self {
        Self { registry }
    }
}

impl ArchiveHandler for SevenZHandler {
    fn supported_extensions(&self) -> Vec<String> {
        vec![".7z".to_string()]
    }

    fn extract_images(
        &self,
        archive_path: &Path,
        target_dir: &Path,
    ) -> Result<Vec<ExtractedEntry>> {
        let file = File::open(archive_path)
            .with_context(|| format!("アーカイブを開けない: {}", archive_path.display()))?;

        let mut results: Vec<ExtractedEntry> = Vec::new();
        let target_dir = target_dir.to_path_buf();
        let registry = Arc::clone(&self.registry);

        // ArchiveReaderで各エントリをコールバック処理
        let mut reader = sevenz_rust2::ArchiveReader::new(file, sevenz_rust2::Password::empty())
            .with_context(|| format!("7zアーカイブ読み取り失敗: {}", archive_path.display()))?;

        reader
            .for_each_entries(|entry, data_reader| {
                let entry_path = entry.name().to_string();
                let filename = extract_filename(&entry_path);

                // ディレクトリ、空ファイル名、隠しファイルはスキップ
                if entry.is_directory() || filename.is_empty() || filename.starts_with('.') {
                    return Ok(true);
                }

                // 画像ファイルのみ展開
                if !registry.is_image_extension(filename) {
                    return Ok(true);
                }

                // エントリデータを取得
                let mut data = Vec::new();
                std::io::Read::read_to_end(data_reader, &mut data)?;

                // target_dirに保存
                let out_path = resolve_filename(&target_dir, filename);
                std::fs::write(&out_path, &data)?;
                results.push((out_path, entry_path));

                Ok(true)
            })
            .with_context(|| format!("7zアーカイブ展開失敗: {}", archive_path.display()))?;

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_extensions() {
        let reg = Arc::new(ExtensionRegistry::new());
        let handler = SevenZHandler::new(reg);
        assert!(handler.supported_extensions().contains(&".7z".to_string()));
    }

    #[test]
    fn nonexistent_7z_returns_error() {
        let reg = Arc::new(ExtensionRegistry::new());
        let handler = SevenZHandler::new(reg);
        let dir = std::env::temp_dir().join("gv_test_7z_noexist");
        let _ = std::fs::create_dir_all(&dir);
        let result: Result<Vec<super::ExtractedEntry>> =
            handler.extract_images(Path::new("nonexistent.7z"), &dir);
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
