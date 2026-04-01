mod exif_reader;
mod standard;
pub mod susie;

pub use exif_reader::read_exif_fields;
pub use standard::StandardDecoder;

/// デコード済み画像データ（RGBAピクセル）
pub struct DecodedImage {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl DecodedImage {
    /// メモリ使用量（バイト）
    pub fn memory_size(&self) -> usize {
        self.data.len()
    }
}

/// 画像メタデータ（document.rs::current_metadata()で構築・返却される）
pub struct ImageMetadata {
    #[allow(dead_code)] // デコーダが設定。将来の画像情報表示拡張で使用予定
    pub width: u32,
    #[allow(dead_code)] // デコーダが設定。将来の画像情報表示拡張で使用予定
    pub height: u32,
    pub format: String,
    pub comments: Vec<String>,
    /// EXIFメタデータ（キー, フォーマット済み値）
    pub exif: Vec<(String, String)>,
}

/// 画像デコーダの共通インターフェース（DecoderChain経由でdyn dispatch）
pub trait ImageDecoder: Send + Sync {
    /// 対応する拡張子のリスト（ドット付き小文字、例: ".jpg"）
    #[allow(dead_code)] // 具象型から呼ばれるが、dyn dispatch経由の呼び出しがないため警告される
    fn supported_extensions(&self) -> Vec<String>;

    /// バイト列からデコード可能か判定
    /// `filename_hint`はSusieプラグインの`IsSupported`で使用
    fn can_decode(&self, data: &[u8], filename_hint: &str) -> bool;

    /// デコード実行
    fn decode(&self, data: &[u8], filename_hint: &str) -> anyhow::Result<DecodedImage>;

    /// メタデータ取得
    fn metadata(&self, data: &[u8], filename_hint: &str) -> anyhow::Result<ImageMetadata>;
}

/// 複数デコーダを順に試行するチェーン
pub struct DecoderChain {
    decoders: Vec<Box<dyn ImageDecoder>>,
}

impl DecoderChain {
    pub fn new(decoders: Vec<Box<dyn ImageDecoder>>) -> Self {
        Self { decoders }
    }

    /// メタデータを取得する（各デコーダを順に試行）
    pub fn metadata(&self, data: &[u8], filename_hint: &str) -> anyhow::Result<ImageMetadata> {
        let mut last_error = None;
        for decoder in &self.decoders {
            if decoder.can_decode(data, filename_hint) {
                match decoder.metadata(data, filename_hint) {
                    Ok(meta) => return Ok(meta),
                    Err(e) => last_error = Some(e),
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("対応するデコーダがありません: {filename_hint}")))
    }

    /// 各デコーダを順に試行し、最初の成功を返す
    pub fn decode(&self, data: &[u8], filename_hint: &str) -> anyhow::Result<DecodedImage> {
        let mut last_error = None;
        for decoder in &self.decoders {
            if decoder.can_decode(data, filename_hint) {
                match decoder.decode(data, filename_hint) {
                    Ok(image) => return Ok(image),
                    Err(e) => last_error = Some(e),
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("対応するデコーダがありません: {filename_hint}")))
    }
}

// DecoderChainはSend + Sync（内部のdecoderが全てSend + Syncのため）
unsafe impl Send for DecoderChain {}
unsafe impl Sync for DecoderChain {}
