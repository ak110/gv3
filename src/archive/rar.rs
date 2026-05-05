use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use super::{ArchiveHandler, ExtractedEntry, extract_filename, resolve_filename};
use crate::extension_registry::ExtensionRegistry;

/// RAR/cbrアーカイブハンドラ
/// unrarクレートはストリーム型APIのため、1パスで全画像を展開する
pub struct RarHandler {
    registry: Arc<ExtensionRegistry>,
}

impl RarHandler {
    pub fn new(registry: Arc<ExtensionRegistry>) -> Self {
        Self { registry }
    }
}

impl ArchiveHandler for RarHandler {
    fn supported_extensions(&self) -> Vec<String> {
        vec![".rar".to_string(), ".cbr".to_string()]
    }

    fn extract_images(
        &self,
        archive_path: &Path,
        target_dir: &Path,
    ) -> Result<Vec<ExtractedEntry>> {
        let mut archive = unrar::Archive::new(archive_path)
            .open_for_processing()
            .map_err(|e| anyhow::anyhow!("RARアーカイブを開けない: {e}"))?;

        let mut results = Vec::new();

        // ストリーム型: read_header → read/skip のステートマシンで1パス展開
        loop {
            let cursor = match archive.read_header() {
                Ok(Some(cursor)) => cursor,
                Ok(None) => break, // エントリ終了
                Err(e) => {
                    eprintln!("RARヘッダ読み取り失敗: {e}");
                    break;
                }
            };

            let entry = cursor.entry();
            let entry_path = entry.filename.to_string_lossy().to_string();
            let filename = extract_filename(&entry_path);

            // ディレクトリ、空ファイル名、隠しファイルはスキップ
            let should_extract = !entry.is_directory()
                && !filename.is_empty()
                && !filename.starts_with('.')
                && self.registry.is_image_extension(filename);

            if should_extract {
                // エントリデータを取得する
                match cursor.read() {
                    Ok((data, next)) => {
                        let out_path = resolve_filename(target_dir, filename);
                        if std::fs::write(&out_path, &data).is_ok() {
                            results.push((out_path, entry_path));
                        }
                        archive = next;
                    }
                    Err(e) => {
                        eprintln!("RARエントリ読み取り失敗: {entry_path}: {e}");
                        break;
                    }
                }
            } else {
                // 画像でないエントリはスキップ
                match cursor.skip() {
                    Ok(next) => {
                        archive = next;
                    }
                    Err(e) => {
                        eprintln!("RARエントリスキップ失敗: {entry_path}: {e}");
                        break;
                    }
                }
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_extensions() {
        let reg = Arc::new(ExtensionRegistry::new());
        let handler = RarHandler::new(reg);
        let exts = handler.supported_extensions();
        assert!(exts.contains(&".rar".to_string()));
        assert!(exts.contains(&".cbr".to_string()));
    }

    #[test]
    fn nonexistent_rar_returns_error() {
        let reg = Arc::new(ExtensionRegistry::new());
        let handler = RarHandler::new(reg);
        let dir = std::env::temp_dir().join("gv_test_rar_noexist");
        let _ = std::fs::create_dir_all(&dir);
        let result: Result<Vec<super::ExtractedEntry>> =
            handler.extract_images(Path::new("nonexistent.rar"), &dir);
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
