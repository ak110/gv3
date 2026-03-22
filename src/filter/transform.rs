//! 画像の幾何変換（トリミング等）

use crate::image::DecodedImage;
use crate::selection::PixelRect;

/// 画像をトリミングする
pub fn crop(image: &DecodedImage, rect: &PixelRect) -> DecodedImage {
    let rect = rect.clamped(image.width, image.height);
    if !rect.is_valid() {
        // 空の矩形の場合は元画像をそのまま返す
        return DecodedImage {
            data: image.data.clone(),
            width: image.width,
            height: image.height,
        };
    }

    let src_w = image.width as usize;
    let dst_w = rect.width as usize;
    let dst_h = rect.height as usize;
    let mut data = Vec::with_capacity(dst_w * dst_h * 4);

    for row in 0..dst_h {
        let src_y = rect.y as usize + row;
        let src_offset = (src_y * src_w + rect.x as usize) * 4;
        let src_end = src_offset + dst_w * 4;
        data.extend_from_slice(&image.data[src_offset..src_end]);
    }

    DecodedImage {
        data,
        width: rect.width as u32,
        height: rect.height as u32,
    }
}

/// 左右反転
pub fn flip_horizontal(image: &DecodedImage) -> DecodedImage {
    let w = image.width as usize;
    let h = image.height as usize;
    let mut data = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * 4;
            let dst = (y * w + (w - 1 - x)) * 4;
            data[dst..dst + 4].copy_from_slice(&image.data[src..src + 4]);
        }
    }
    DecodedImage {
        data,
        width: image.width,
        height: image.height,
    }
}

/// 上下反転
pub fn flip_vertical(image: &DecodedImage) -> DecodedImage {
    let w = image.width as usize;
    let h = image.height as usize;
    let row_bytes = w * 4;
    let mut data = vec![0u8; w * h * 4];
    for y in 0..h {
        let src_start = y * row_bytes;
        let dst_start = (h - 1 - y) * row_bytes;
        data[dst_start..dst_start + row_bytes]
            .copy_from_slice(&image.data[src_start..src_start + row_bytes]);
    }
    DecodedImage {
        data,
        width: image.width,
        height: image.height,
    }
}

/// 時計回りに90度回転
pub fn rotate_90(image: &DecodedImage) -> DecodedImage {
    let w = image.width as usize;
    let h = image.height as usize;
    let mut data = vec![0u8; w * h * 4];
    // 新サイズ: (h, w) → width=h, height=w
    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * 4;
            let dst = (x * h + (h - 1 - y)) * 4;
            data[dst..dst + 4].copy_from_slice(&image.data[src..src + 4]);
        }
    }
    DecodedImage {
        data,
        width: image.height,
        height: image.width,
    }
}

/// 180度回転
pub fn rotate_180(image: &DecodedImage) -> DecodedImage {
    let total = image.data.len();
    let mut data = vec![0u8; total];
    let pixel_count = total / 4;
    for i in 0..pixel_count {
        let src = i * 4;
        let dst = (pixel_count - 1 - i) * 4;
        data[dst..dst + 4].copy_from_slice(&image.data[src..src + 4]);
    }
    DecodedImage {
        data,
        width: image.width,
        height: image.height,
    }
}

/// 反時計回りに90度回転
pub fn rotate_270(image: &DecodedImage) -> DecodedImage {
    let w = image.width as usize;
    let h = image.height as usize;
    let mut data = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * 4;
            let dst = ((w - 1 - x) * h + y) * 4;
            data[dst..dst + 4].copy_from_slice(&image.data[src..src + 4]);
        }
    }
    DecodedImage {
        data,
        width: image.height,
        height: image.width,
    }
}

/// 任意角度回転（度数法、時計回り）
/// 回転後の画像は元画像を包む最小矩形サイズになる
pub fn rotate_arbitrary(image: &DecodedImage, degrees: f64) -> DecodedImage {
    let rad = degrees.to_radians();
    let cos = rad.cos();
    let sin = rad.sin();
    let w = image.width as f64;
    let h = image.height as f64;

    // 回転後のバウンディングボックス
    let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)];
    let (mut min_x, mut min_y) = (f64::MAX, f64::MAX);
    let (mut max_x, mut max_y) = (f64::MIN, f64::MIN);
    for (cx, cy) in &corners {
        let rx = cx * cos - cy * sin;
        let ry = cx * sin + cy * cos;
        min_x = min_x.min(rx);
        min_y = min_y.min(ry);
        max_x = max_x.max(rx);
        max_y = max_y.max(ry);
    }

    let new_w = (max_x - min_x).ceil() as u32;
    let new_h = (max_y - min_y).ceil() as u32;
    if new_w == 0 || new_h == 0 {
        return DecodedImage {
            data: image.data.clone(),
            width: image.width,
            height: image.height,
        };
    }

    let src_w = image.width as usize;
    let mut data = vec![0u8; (new_w * new_h * 4) as usize];

    // 逆変換: 出力ピクセル→元画像座標
    let cos_inv = cos; // cos(-θ) = cos(θ)
    let sin_inv = -sin; // sin(-θ) = -sin(θ)
    for dy in 0..new_h {
        for dx in 0..new_w {
            let rx = dx as f64 + min_x;
            let ry = dy as f64 + min_y;
            let sx = rx * cos_inv - ry * sin_inv;
            let sy = rx * sin_inv + ry * cos_inv;

            // バイリニア補間
            let sx_floor = sx.floor();
            let sy_floor = sy.floor();
            let fx = sx - sx_floor;
            let fy = sy - sy_floor;
            let ix = sx_floor as i32;
            let iy = sy_floor as i32;

            if ix < 0 || iy < 0 || ix + 1 >= image.width as i32 || iy + 1 >= image.height as i32 {
                continue; // 範囲外は透明
            }

            let dst = ((dy * new_w + dx) * 4) as usize;
            for ch in 0..4 {
                let get = |x: i32, y: i32| -> f64 {
                    image.data[(y as usize * src_w + x as usize) * 4 + ch] as f64
                };
                let v00 = get(ix, iy);
                let v10 = get(ix + 1, iy);
                let v01 = get(ix, iy + 1);
                let v11 = get(ix + 1, iy + 1);
                let v = v00 * (1.0 - fx) * (1.0 - fy)
                    + v10 * fx * (1.0 - fy)
                    + v01 * (1.0 - fx) * fy
                    + v11 * fx * fy;
                data[dst + ch] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    DecodedImage {
        data,
        width: new_w,
        height: new_h,
    }
}

/// 解像度変更（Lanczos3リサイズ、SIMD加速）
pub fn resize(image: &DecodedImage, new_width: u32, new_height: u32) -> DecodedImage {
    if new_width == 0 || new_height == 0 {
        return DecodedImage {
            data: image.data.clone(),
            width: image.width,
            height: image.height,
        };
    }
    use fast_image_resize as fr;
    let mut src_buf = image.data.clone();
    let src_image = fr::images::Image::from_slice_u8(
        image.width,
        image.height,
        &mut src_buf,
        fr::PixelType::U8x4,
    )
    .expect("リサイズ用ソース画像作成失敗");
    let mut dst_image = fr::images::Image::new(new_width, new_height, fr::PixelType::U8x4);
    let options =
        fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3));
    let mut resizer = fr::Resizer::new();
    resizer
        .resize(&src_image, &mut dst_image, &options)
        .expect("画像リサイズ失敗");
    DecodedImage {
        data: dst_image.into_vec(),
        width: new_width,
        height: new_height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4x4のテスト画像を作成（各ピクセルが座標で識別可能）
    fn test_image_4x4() -> DecodedImage {
        let mut data = Vec::with_capacity(4 * 4 * 4);
        for y in 0..4u8 {
            for x in 0..4u8 {
                data.extend_from_slice(&[x * 60, y * 60, 0, 255]);
            }
        }
        DecodedImage {
            data,
            width: 4,
            height: 4,
        }
    }

    #[test]
    fn crop_center_2x2() {
        let img = test_image_4x4();
        let rect = PixelRect {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        let cropped = crop(&img, &rect);
        assert_eq!(cropped.width, 2);
        assert_eq!(cropped.height, 2);
        assert_eq!(cropped.data.len(), 2 * 2 * 4);
        // (1,1)のピクセル: R=60, G=60
        assert_eq!(cropped.data[0], 60);
        assert_eq!(cropped.data[1], 60);
    }

    #[test]
    fn crop_full_image() {
        let img = test_image_4x4();
        let rect = PixelRect {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        };
        let cropped = crop(&img, &rect);
        assert_eq!(cropped.width, 4);
        assert_eq!(cropped.height, 4);
        assert_eq!(cropped.data, img.data);
    }

    #[test]
    fn crop_clamps_to_image_bounds() {
        let img = test_image_4x4();
        let rect = PixelRect {
            x: 2,
            y: 2,
            width: 100,
            height: 100,
        };
        let cropped = crop(&img, &rect);
        assert_eq!(cropped.width, 2);
        assert_eq!(cropped.height, 2);
    }

    #[test]
    fn crop_zero_rect_returns_original() {
        let img = test_image_4x4();
        let rect = PixelRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        let cropped = crop(&img, &rect);
        assert_eq!(cropped.width, img.width);
        assert_eq!(cropped.height, img.height);
    }

    #[test]
    fn flip_horizontal_2x1() {
        let img = DecodedImage {
            data: vec![255, 0, 0, 255, 0, 255, 0, 255],
            width: 2,
            height: 1,
        };
        let flipped = flip_horizontal(&img);
        assert_eq!(flipped.data[0..4], [0, 255, 0, 255]);
        assert_eq!(flipped.data[4..8], [255, 0, 0, 255]);
    }

    #[test]
    fn flip_vertical_1x2() {
        let img = DecodedImage {
            data: vec![255, 0, 0, 255, 0, 255, 0, 255],
            width: 1,
            height: 2,
        };
        let flipped = flip_vertical(&img);
        assert_eq!(flipped.data[0..4], [0, 255, 0, 255]);
        assert_eq!(flipped.data[4..8], [255, 0, 0, 255]);
    }

    #[test]
    fn rotate_90_2x1() {
        // 2x1 → 1x2 (rotated CW)
        let img = DecodedImage {
            data: vec![255, 0, 0, 255, 0, 255, 0, 255],
            width: 2,
            height: 1,
        };
        let rotated = rotate_90(&img);
        assert_eq!(rotated.width, 1);
        assert_eq!(rotated.height, 2);
    }

    #[test]
    fn rotate_180_identity() {
        let img = test_image_4x4();
        let r1 = rotate_180(&img);
        let r2 = rotate_180(&r1);
        assert_eq!(r2.data, img.data);
    }

    #[test]
    fn rotate_270_is_3x_rotate_90() {
        let img = test_image_4x4();
        let r270 = rotate_270(&img);
        let r90_3 = rotate_90(&rotate_90(&rotate_90(&img)));
        assert_eq!(r270.width, r90_3.width);
        assert_eq!(r270.height, r90_3.height);
        assert_eq!(r270.data, r90_3.data);
    }

    #[test]
    fn resize_basic() {
        let img = test_image_4x4();
        let resized = resize(&img, 2, 2);
        assert_eq!(resized.width, 2);
        assert_eq!(resized.height, 2);
        assert_eq!(resized.data.len(), 2 * 2 * 4);
    }

    #[test]
    fn rotate_arbitrary_0_identity() {
        // 0度回転は元画像と同じサイズになる
        let img = test_image_4x4();
        let rotated = rotate_arbitrary(&img, 0.0);
        assert_eq!(rotated.width, img.width);
        assert_eq!(rotated.height, img.height);
        // 内部ピクセルも概ね一致（バイリニア補間で端は除外）
        // ピクセル(row=1, col=1)のバイトオフセット
        let center_src = (4 + 1) * 4;
        let center_dst = (rotated.width as usize + 1) * 4;
        for ch in 0..4 {
            assert!(
                (rotated.data[center_dst + ch] as i32 - img.data[center_src + ch] as i32).abs()
                    <= 1
            );
        }
    }

    #[test]
    fn crop_single_pixel() {
        let img = test_image_4x4();
        let rect = PixelRect {
            x: 3,
            y: 3,
            width: 1,
            height: 1,
        };
        let cropped = crop(&img, &rect);
        assert_eq!(cropped.width, 1);
        assert_eq!(cropped.height, 1);
        assert_eq!(cropped.data.len(), 4);
        // (3,3)のピクセル: R=3*60=180, G=3*60=180
        assert_eq!(cropped.data[0], 180);
        assert_eq!(cropped.data[1], 180);
    }
}
