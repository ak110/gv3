//! 明るさ・コントラスト・レベル・ガンマ補正フィルタ

use crate::image::DecodedImage;
use crate::selection::PixelRect;

/// レベル補正: low〜highの範囲を0〜255に伸張する
pub fn levels(image: &DecodedImage, region: Option<&PixelRect>, low: u8, high: u8) -> DecodedImage {
    let low = low as f32;
    let high = high.max(1) as f32;
    let range = (high - low).max(1.0);

    super::apply_to_region(image, region, |r, g, b, a| {
        let map =
            |v: u8| -> u8 { ((v as f32 - low) / range * 255.0).round().clamp(0.0, 255.0) as u8 };
        [map(r), map(g), map(b), a]
    })
}

/// ガンマ補正
pub fn gamma(image: &DecodedImage, region: Option<&PixelRect>, gamma_value: f64) -> DecodedImage {
    let inv_gamma = 1.0 / gamma_value.max(0.01);

    // ルックアップテーブルを事前計算
    let mut lut = [0u8; 256];
    for (i, entry) in lut.iter_mut().enumerate() {
        *entry = ((i as f64 / 255.0).powf(inv_gamma) * 255.0).round() as u8;
    }

    super::apply_to_region(image, region, |r, g, b, a| {
        [lut[r as usize], lut[g as usize], lut[b as usize], a]
    })
}

/// 明るさとコントラスト調整
/// brightness: -128〜+128, contrast: -128〜+128
pub fn brightness_contrast(
    image: &DecodedImage,
    region: Option<&PixelRect>,
    brightness: i32,
    contrast: i32,
) -> DecodedImage {
    // コントラスト係数: c ∈ [-128, 128] → factor
    let factor = if contrast > 0 {
        // 正のコントラスト: 128/(128-c)
        128.0 / (128.0 - contrast as f32)
    } else {
        // 負のコントラスト: (128+c)/128
        (128.0 + contrast as f32) / 128.0
    };

    // ルックアップテーブル
    let mut lut = [0u8; 256];
    for (i, entry) in lut.iter_mut().enumerate() {
        let v = i as f32 + brightness as f32;
        let v = (v - 128.0) * factor + 128.0;
        *entry = v.round().clamp(0.0, 255.0) as u8;
    }

    super::apply_to_region(image, region, |r, g, b, a| {
        [lut[r as usize], lut[g as usize], lut[b as usize], a]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_full_range() {
        let img = DecodedImage {
            data: vec![128, 128, 128, 255],
            width: 1,
            height: 1,
        };
        // 0-255の範囲はそのまま
        let result = levels(&img, None, 0, 255);
        assert_eq!(result.data[0], 128);
    }

    #[test]
    fn levels_narrow_range() {
        let img = DecodedImage {
            data: vec![128, 128, 128, 255],
            width: 1,
            height: 1,
        };
        // 100-200の範囲を伸張: (128-100)/(200-100)*255 ≈ 71
        let result = levels(&img, None, 100, 200);
        assert!((result.data[0] as i32 - 71).abs() <= 1);
    }

    #[test]
    fn gamma_1_0_identity() {
        let img = DecodedImage {
            data: vec![100, 150, 200, 255],
            width: 1,
            height: 1,
        };
        let result = gamma(&img, None, 1.0);
        assert_eq!(result.data[0], 100);
        assert_eq!(result.data[1], 150);
        assert_eq!(result.data[2], 200);
    }

    #[test]
    fn brightness_positive() {
        let img = DecodedImage {
            data: vec![100, 100, 100, 255],
            width: 1,
            height: 1,
        };
        let result = brightness_contrast(&img, None, 50, 0);
        assert_eq!(result.data[0], 150);
    }

    #[test]
    fn brightness_clamps() {
        let img = DecodedImage {
            data: vec![200, 200, 200, 255],
            width: 1,
            height: 1,
        };
        let result = brightness_contrast(&img, None, 100, 0);
        assert_eq!(result.data[0], 255); // clamped
    }
}
