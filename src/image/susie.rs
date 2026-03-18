/// Susie画像プラグインのImageDecoderアダプタ
use anyhow::Result;

use crate::susie::plugin::SharedPlugin;

use super::{DecodedImage, ImageDecoder, ImageMetadata};

/// Susie画像プラグインをImageDecoderとして使うアダプタ
pub struct SusieImageDecoder {
    plugin: SharedPlugin,
    /// キャッシュした拡張子リスト
    extensions: Vec<String>,
}

impl SusieImageDecoder {
    pub fn new(plugin: SharedPlugin) -> Self {
        let extensions = plugin.lock().unwrap().supported_extensions();
        Self { plugin, extensions }
    }
}

impl ImageDecoder for SusieImageDecoder {
    fn supported_extensions(&self) -> Vec<String> {
        self.extensions.clone()
    }

    fn can_decode(&self, data: &[u8], filename_hint: &str) -> bool {
        let Ok(locked) = self.plugin.lock() else {
            return false;
        };
        locked.is_supported(filename_hint, data)
    }

    fn decode(&self, data: &[u8], filename_hint: &str) -> Result<DecodedImage> {
        let locked = self
            .plugin
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex poisoned: {e}"))?;
        locked.get_picture(data, filename_hint)
    }

    fn metadata(&self, data: &[u8], filename_hint: &str) -> Result<ImageMetadata> {
        // Susieプラグインにはメタデータ専用APIがないため、デコードして取得
        let image = self.decode(data, filename_hint)?;
        Ok(ImageMetadata {
            width: image.width,
            height: image.height,
            format: format!(
                "Susie ({})",
                self.plugin
                    .lock()
                    .map(|p| p.path.display().to_string())
                    .unwrap_or_default()
            ),
            comments: Vec::new(),
        })
    }
}

// SusieImageDecoderはSend + Sync（内部のSharedPlugin = Arc<Mutex<>>がSend + Sync）
unsafe impl Send for SusieImageDecoder {}
unsafe impl Sync for SusieImageDecoder {}
