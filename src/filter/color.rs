//! 色変換フィルタ

use crate::image::DecodedImage;
use crate::selection::PixelRect;

/// 色を反転
pub fn invert_colors(image: &DecodedImage, region: Option<&PixelRect>) -> DecodedImage {
    super::apply_to_region(image, region, |r, g, b, a| [255 - r, 255 - g, 255 - b, a])
}

/// 簡易グレースケール化 (R,G,Bの平均)
pub fn grayscale_simple(image: &DecodedImage, region: Option<&PixelRect>) -> DecodedImage {
    super::apply_to_region(image, region, |r, g, b, a| {
        let gray = ((r as u16 + g as u16 + b as u16) / 3) as u8;
        [gray, gray, gray, a]
    })
}

/// 厳密グレースケール化 (ITU-R BT.709 輝度)
pub fn grayscale_strict(image: &DecodedImage, region: Option<&PixelRect>) -> DecodedImage {
    super::apply_to_region(image, region, |r, g, b, a| {
        let gray = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32).round() as u8;
        [gray, gray, gray, a]
    })
}

/// 塗り潰し (指定色で領域を塗り潰す)
pub fn fill(image: &DecodedImage, region: Option<&PixelRect>, r: u8, g: u8, b: u8) -> DecodedImage {
    super::apply_to_region(image, region, |_r, _g, _b, a| [r, g, b, a])
}

/// αチャンネルをRGBに反映して不透明にする (白背景合成)
pub fn apply_alpha(image: &DecodedImage, region: Option<&PixelRect>) -> DecodedImage {
    super::apply_to_region(image, region, |r, g, b, a| {
        let af = a as f32 / 255.0;
        let blend = |c: u8| (c as f32 * af + 255.0 * (1.0 - af)).round() as u8;
        [blend(r), blend(g), blend(b), 255]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_image() -> DecodedImage {
        DecodedImage {
            data: vec![
                100, 150, 200, 255, // pixel (0,0)
                50, 100, 150, 128, // pixel (1,0)
            ],
            width: 2,
            height: 1,
        }
    }

    #[test]
    fn invert_colors_basic() {
        let img = test_image();
        let result = invert_colors(&img, None);
        assert_eq!(result.data[0], 155); // 255-100
        assert_eq!(result.data[1], 105); // 255-150
        assert_eq!(result.data[2], 55); // 255-200
        assert_eq!(result.data[3], 255); // alpha unchanged
    }

    #[test]
    fn grayscale_simple_average() {
        let img = DecodedImage {
            data: vec![30, 60, 90, 255],
            width: 1,
            height: 1,
        };
        let result = grayscale_simple(&img, None);
        assert_eq!(result.data[0], 60); // (30+60+90)/3
        assert_eq!(result.data[1], 60);
        assert_eq!(result.data[2], 60);
    }

    #[test]
    fn grayscale_strict_bt709() {
        let img = DecodedImage {
            data: vec![255, 0, 0, 255], // 純赤
            width: 1,
            height: 1,
        };
        let result = grayscale_strict(&img, None);
        // 0.2126 * 255 ≈ 54
        assert_eq!(result.data[0], 54);
    }

    #[test]
    fn apply_alpha_white_bg() {
        let img = DecodedImage {
            data: vec![100, 100, 100, 128], // 50%透過
            width: 1,
            height: 1,
        };
        let result = apply_alpha(&img, None);
        // 100 * 0.502 + 255 * 0.498 ≈ 177
        assert!((result.data[0] as i32 - 177).abs() <= 1);
        assert_eq!(result.data[3], 255);
    }

    #[test]
    fn region_only_modifies_selection() {
        let img = test_image();
        let region = PixelRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let result = invert_colors(&img, Some(&region));
        // (0,0) は反転
        assert_eq!(result.data[0], 155);
        // (1,0) は変更なし
        assert_eq!(result.data[4], 50);
    }
}
