/// Susieアーカイブプラグインの ArchiveHandler アダプタ
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use super::{ArchiveHandler, ExtractedEntry, extract_filename, resolve_filename};
use crate::extension_registry::ExtensionRegistry;
use crate::susie::plugin::SharedPlugin;
use crate::susie::util::from_ansi;

/// SusieアーカイブプラグインをArchiveHandlerとして使うアダプタ
pub struct SusieArchiveHandler {
    plugin: SharedPlugin,
    registry: Arc<ExtensionRegistry>,
    /// キャッシュした拡張子リスト
    extensions: Vec<String>,
}

impl SusieArchiveHandler {
    pub fn new(plugin: SharedPlugin, registry: Arc<ExtensionRegistry>) -> Self {
        let extensions = plugin
            .lock()
            .expect("Susie plugin lock poisoned")
            .supported_extensions();
        Self {
            plugin,
            registry,
            extensions,
        }
    }
}

impl ArchiveHandler for SusieArchiveHandler {
    fn supported_extensions(&self) -> Vec<String> {
        self.extensions.clone()
    }

    fn extract_images(
        &self,
        archive_path: &Path,
        target_dir: &Path,
    ) -> Result<Vec<ExtractedEntry>> {
        let path_str = archive_path.to_string_lossy().to_string();
        let locked = self.plugin.lock().expect("Susie plugin lock poisoned");

        // アーカイブ内のエントリ一覧を取得
        let entries = locked.get_archive_info(&path_str)?;

        let mut results = Vec::new();

        for entry in &entries {
            // ファイル名を取得 (ANSI → UTF-8)
            let raw_filename = from_ansi(&entry.filename);
            let filename = extract_filename(&raw_filename);

            // 空ファイル名、隠しファイルはスキップ
            if filename.is_empty() || filename.starts_with('.') {
                continue;
            }

            // 画像ファイルのみ展開
            if !self.registry.is_image_extension(filename) {
                continue;
            }

            // メモリに展開
            let position = { entry.position };
            match locked.get_file_to_memory(&path_str, position) {
                Ok(data) => {
                    let out_path = resolve_filename(target_dir, filename);
                    if std::fs::write(&out_path, &data).is_ok() {
                        results.push((out_path, raw_filename));
                    }
                }
                Err(e) => {
                    eprintln!("Susieアーカイブエントリ展開失敗: {filename}: {e}");
                }
            }
        }

        Ok(results)
    }
}
