//! シャープフィルタ

use crate::image::DecodedImage;
use crate::selection::PixelRect;

/// シャープ (3x3ラプラシアンシャープ)
pub fn sharpen(image: &DecodedImage, region: Option<&PixelRect>) -> DecodedImage {
    // カーネル: [0, -1, 0, -1, 5, -1, 0, -1, 0]
    apply_kernel(image, region, &SHARPEN_KERNEL)
}

/// シャープ (強)
pub fn sharpen_strong(image: &DecodedImage, region: Option<&PixelRect>) -> DecodedImage {
    // カーネル: [-1, -1, -1, -1, 9, -1, -1, -1, -1]
    apply_kernel(image, region, &SHARPEN_STRONG_KERNEL)
}

// 3x3カーネル (行優先、スケール1)
const SHARPEN_KERNEL: [i32; 9] = [0, -1, 0, -1, 5, -1, 0, -1, 0];
const SHARPEN_STRONG_KERNEL: [i32; 9] = [-1, -1, -1, -1, 9, -1, -1, -1, -1];

/// 3x3カーネルを適用する
fn apply_kernel(
    image: &DecodedImage,
    region: Option<&PixelRect>,
    kernel: &[i32; 9],
) -> DecodedImage {
    let w = image.width as i32;
    let h = image.height as i32;
    let mut data = image.data.clone();

    let (x0, y0, x1, y1) = super::region_bounds(region, image.width, image.height);

    for y in y0..y1 {
        for x in x0..x1 {
            for ch in 0..3 {
                let mut sum = 0i32;
                let mut ki = 0;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let nx = (x + dx).clamp(0, w - 1);
                        let ny = (y + dy).clamp(0, h - 1);
                        sum += image.data[((ny * w + nx) * 4 + ch) as usize] as i32 * kernel[ki];
                        ki += 1;
                    }
                }
                let offset = ((y * w + x) * 4 + ch) as usize;
                data[offset] = sum.clamp(0, 255) as u8;
            }
        }
    }

    DecodedImage {
        data,
        width: image.width,
        height: image.height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform_image(w: u32, h: u32, value: u8) -> DecodedImage {
        let data = vec![[value, value, value, 255u8]; (w * h) as usize]
            .into_iter()
            .flatten()
            .collect();
        DecodedImage {
            data,
            width: w,
            height: h,
        }
    }

    #[test]
    fn sharpen_uniform_unchanged() {
        // 均一画像にシャープをかけても値は変わらない (カーネルの合計が1なので)
        let img = uniform_image(4, 4, 100);
        let result = sharpen(&img, None);
        for pixel in result.data.chunks_exact(4) {
            assert_eq!(pixel[0], 100);
        }
    }

    #[test]
    fn sharpen_strong_uniform_unchanged() {
        let img = uniform_image(4, 4, 100);
        let result = sharpen_strong(&img, None);
        for pixel in result.data.chunks_exact(4) {
            assert_eq!(pixel[0], 100);
        }
    }

    #[test]
    fn sharpen_preserves_alpha() {
        let mut img = uniform_image(3, 3, 100);
        img.data[3] = 128;
        let result = sharpen(&img, None);
        assert_eq!(result.data[3], 128);
    }
}
