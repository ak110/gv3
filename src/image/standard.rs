use std::io::Cursor;

use anyhow::Context as _;

use super::{DecodedImage, ImageDecoder, ImageMetadata};

/// image crateによる標準デコーダ（JPEG/PNG/GIF/BMP/WebP）
pub struct StandardDecoder;

impl StandardDecoder {
    pub fn new() -> Self {
        Self
    }
}

impl ImageDecoder for StandardDecoder {
    fn supported_extensions(&self) -> Vec<String> {
        [".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn can_decode(&self, data: &[u8], _filename_hint: &str) -> bool {
        image::guess_format(data).is_ok()
    }

    fn decode(&self, data: &[u8], _filename_hint: &str) -> anyhow::Result<DecodedImage> {
        let img = image::load_from_memory(data).context("画像のデコードに失敗")?;
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();

        Ok(DecodedImage {
            data: rgba.into_raw(),
            width,
            height,
        })
    }

    fn metadata(&self, data: &[u8], _filename_hint: &str) -> anyhow::Result<ImageMetadata> {
        let format = image::guess_format(data)
            .map(|f| format!("{:?}", f))
            .unwrap_or_else(|_| "Unknown".to_string());

        // サイズ取得のためにデコード（ヘッダだけ読むAPIが限定的なため）
        let img = image::load_from_memory(data).context("メタデータの取得に失敗")?;

        // PNGのテキストチャンク（tEXt/zTXt/iTXt）を取得
        let comments = if matches!(image::guess_format(data), Ok(image::ImageFormat::Png)) {
            Self::read_png_text_chunks(data)
        } else {
            Vec::new()
        };

        Ok(ImageMetadata {
            width: img.width(),
            height: img.height(),
            format,
            comments,
        })
    }
}

impl StandardDecoder {
    /// PNGのテキストチャンク（tEXt/zTXt/iTXt）を読み取る
    fn read_png_text_chunks(data: &[u8]) -> Vec<String> {
        let decoder = png::Decoder::new(Cursor::new(data));
        let Ok(reader) = decoder.read_info() else {
            return Vec::new();
        };
        let info = reader.info();
        let mut texts = Vec::new();

        // tEXt（非圧縮Latin-1テキスト）
        for chunk in &info.uncompressed_latin1_text {
            texts.push(format!("{}: {}", chunk.keyword, chunk.text));
        }
        // zTXt（圧縮Latin-1テキスト）
        for chunk in &info.compressed_latin1_text {
            if let Ok(text) = chunk.get_text() {
                texts.push(format!("{}: {}", chunk.keyword, text));
            }
        }
        // iTXt（国際化テキスト、UTF-8）
        for chunk in &info.utf8_text {
            if let Ok(text) = chunk.get_text() {
                texts.push(format!("{}: {}", chunk.keyword, text));
            }
        }

        texts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_extensions_include_common_formats() {
        let decoder = StandardDecoder::new();
        let exts = decoder.supported_extensions();
        assert!(exts.contains(&".jpg".to_string()));
        assert!(exts.contains(&".png".to_string()));
        assert!(exts.contains(&".gif".to_string()));
        assert!(exts.contains(&".bmp".to_string()));
        assert!(exts.contains(&".webp".to_string()));
    }

    #[test]
    fn decode_invalid_data_returns_error() {
        let decoder = StandardDecoder::new();
        let result = decoder.decode(&[0, 1, 2, 3], "test.jpg");
        assert!(result.is_err());
    }

    #[test]
    fn can_decode_rejects_invalid_data() {
        let decoder = StandardDecoder::new();
        assert!(!decoder.can_decode(&[0, 1, 2, 3], "test.jpg"));
    }

    #[test]
    fn decode_minimal_png() {
        // 1x1 白ピクセルのPNG
        let png_data = create_1x1_white_png();
        let decoder = StandardDecoder::new();
        assert!(decoder.can_decode(&png_data, "test.png"));

        let img = decoder.decode(&png_data, "test.png").unwrap();
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 1);
        assert_eq!(img.data.len(), 4); // 1 pixel × RGBA
        assert_eq!(&img.data, &[255, 255, 255, 255]);
    }

    #[test]
    fn metadata_minimal_png() {
        let png_data = create_1x1_white_png();
        let decoder = StandardDecoder::new();
        let meta = decoder.metadata(&png_data, "test.png").unwrap();
        assert_eq!(meta.width, 1);
        assert_eq!(meta.height, 1);
        assert!(meta.format.contains("Png"));
    }

    #[test]
    fn metadata_png_with_text_chunks() {
        // tEXtチャンク付きPNGを生成
        let png_data = create_1x1_png_with_text();
        let decoder = StandardDecoder::new();
        let meta = decoder.metadata(&png_data, "test.png").unwrap();
        assert!(
            meta.comments.iter().any(|c| c.contains("Author")),
            "tEXtチャンクが取得できること: {:?}",
            meta.comments
        );
        assert!(
            meta.comments.iter().any(|c| c.contains("TestAuthor")),
            "tEXtチャンクの値が正しいこと: {:?}",
            meta.comments
        );
    }

    /// テスト用: 1x1 白ピクセルのPNGバイナリを生成
    fn create_1x1_white_png() -> Vec<u8> {
        use image::{ImageBuffer, Rgba};
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(1, 1, Rgba([255, 255, 255, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    /// テスト用: tEXtチャンク付き1x1 PNGバイナリを生成
    fn create_1x1_png_with_text() -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut encoder = png::Encoder::new(std::io::Cursor::new(&mut buf), 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            // tEXtチャンクを追加
            let text_chunk =
                png::text_metadata::TEXtChunk::new("Author".to_string(), "TestAuthor".to_string());
            encoder.add_text_chunk(text_chunk.keyword, text_chunk.text);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[255, 255, 255, 255]).unwrap();
        }
        buf
    }
}
