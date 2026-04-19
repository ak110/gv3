/// Susie画像プラグインのImageDecoderアダプタ
use anyhow::Result;

use crate::susie::plugin::SharedPlugin;

use super::{DecodedImage, ImageDecoder, ImageMetadata, read_exif_fields};

/// Susie画像プラグインをImageDecoderとして使うアダプタ
pub struct SusieImageDecoder {
    plugin: SharedPlugin,
}

impl SusieImageDecoder {
    pub fn new(plugin: SharedPlugin) -> Self {
        Self { plugin }
    }
}

impl ImageDecoder for SusieImageDecoder {
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

    fn metadata(&self, data: &[u8], _filename_hint: &str) -> Result<ImageMetadata> {
        // EXIFメタデータ (Susie経由でもraw bytesからEXIFを読み取れる)
        let exif = read_exif_fields(data);
        Ok(ImageMetadata {
            format: format!(
                "Susie ({})",
                self.plugin
                    .lock()
                    .map(|p| p.path.display().to_string())
                    .unwrap_or_default()
            ),
            comments: Vec::new(),
            exif,
        })
    }
}

// SusieImageDecoderはSend + Sync(内部のSharedPlugin = Arc<Mutex<>>がSend + Sync)
unsafe impl Send for SusieImageDecoder {}
unsafe impl Sync for SusieImageDecoder {}
