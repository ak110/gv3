//! ぼかし・モザイクフィルタ

use crate::image::DecodedImage;
use crate::selection::PixelRect;

/// 3x3 ぼかし (平均フィルタ)
pub fn blur(image: &DecodedImage, region: Option<&PixelRect>) -> DecodedImage {
    box_blur(image, region, 1)
}

/// 5x5 ぼかし (強)
pub fn blur_strong(image: &DecodedImage, region: Option<&PixelRect>) -> DecodedImage {
    box_blur(image, region, 2)
}

/// メディアンフィルタ (3x3)
pub fn median_filter(image: &DecodedImage, region: Option<&PixelRect>) -> DecodedImage {
    // ループ範囲は i32 (clamp で負値を扱うため)、インデックス計算は usize で行う
    // (大きな画像で `i32` の `(ny * w + nx) * 4` が overflow するのを防ぐ)
    let w_u = image.width as usize;
    let w_i = image.width as i32;
    let h_i = image.height as i32;
    let mut data = image.data.clone();

    let (x0, y0, x1, y1) = super::region_bounds(region, image.width, image.height);

    for y in y0..y1 {
        for x in x0..x1 {
            for ch in 0..3 {
                let mut values = Vec::with_capacity(9);
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let nx = (x + dx).clamp(0, w_i - 1) as usize;
                        let ny = (y + dy).clamp(0, h_i - 1) as usize;
                        values.push(image.data[(ny * w_u + nx) * 4 + ch]);
                    }
                }
                values.sort_unstable();
                let offset = (y as usize * w_u + x as usize) * 4 + ch;
                data[offset] = values[4]; // 中央値
            }
        }
    }

    DecodedImage {
        data,
        width: image.width,
        height: image.height,
    }
}

/// ボックスブラー(半径指定)
fn box_blur(image: &DecodedImage, region: Option<&PixelRect>, radius: i32) -> DecodedImage {
    // ループ範囲は i32 (clamp で負値を扱うため)、インデックス計算は usize で行う。
    let w_u = image.width as usize;
    let w_i = image.width as i32;
    let h_i = image.height as i32;
    let mut data = image.data.clone();

    let (x0, y0, x1, y1) = super::region_bounds(region, image.width, image.height);
    let kernel_size = (2 * radius + 1) * (2 * radius + 1);

    for y in y0..y1 {
        for x in x0..x1 {
            for ch in 0..3 {
                let mut sum = 0u32;
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        let nx = (x + dx).clamp(0, w_i - 1) as usize;
                        let ny = (y + dy).clamp(0, h_i - 1) as usize;
                        sum += image.data[(ny * w_u + nx) * 4 + ch] as u32;
                    }
                }
                let offset = (y as usize * w_u + x as usize) * 4 + ch;
                data[offset] = (sum / kernel_size as u32) as u8;
            }
        }
    }

    DecodedImage {
        data,
        width: image.width,
        height: image.height,
    }
}

/// モザイク
pub fn mosaic(image: &DecodedImage, region: Option<&PixelRect>, block_size: u32) -> DecodedImage {
    let block = block_size.max(1) as i32;
    let w_u = image.width as usize;
    let mut data = image.data.clone();

    let (x0, y0, x1, y1) = super::region_bounds(region, image.width, image.height);

    // ブロック単位で処理
    let mut by = y0;
    while by < y1 {
        let mut bx = x0;
        while bx < x1 {
            let bx_end = (bx + block).min(x1);
            let by_end = (by + block).min(y1);
            let count = ((bx_end - bx) * (by_end - by)) as u32;

            // ブロック内の平均色を計算 (オフセットは usize で計算してオーバーフローを防ぐ)
            let mut r_sum = 0u32;
            let mut g_sum = 0u32;
            let mut b_sum = 0u32;
            for py in by..by_end {
                for px in bx..bx_end {
                    let offset = (py as usize * w_u + px as usize) * 4;
                    r_sum += image.data[offset] as u32;
                    g_sum += image.data[offset + 1] as u32;
                    b_sum += image.data[offset + 2] as u32;
                }
            }
            let avg_r = (r_sum / count) as u8;
            let avg_g = (g_sum / count) as u8;
            let avg_b = (b_sum / count) as u8;

            // ブロック内を平均色で塗り潰す
            for py in by..by_end {
                for px in bx..bx_end {
                    let offset = (py as usize * w_u + px as usize) * 4;
                    data[offset] = avg_r;
                    data[offset + 1] = avg_g;
                    data[offset + 2] = avg_b;
                }
            }

            bx += block;
        }
        by += block;
    }

    DecodedImage {
        data,
        width: image.width,
        height: image.height,
    }
}

/// ガウスぼかし (近似: 2パスのボックスブラー)
pub fn gaussian_blur(
    image: &DecodedImage,
    region: Option<&PixelRect>,
    radius: f64,
) -> DecodedImage {
    // σ ≈ radius/3 でボックスブラーを3パス (ガウシアンの近似)
    let r = (radius.max(0.1) * 1.0).round() as i32;
    let r = r.max(1);
    let pass1 = box_blur(image, region, r);
    let pass2 = box_blur(&pass1, region, r);
    box_blur(&pass2, region, r)
}

/// アンシャープマスク
pub fn unsharp_mask(image: &DecodedImage, region: Option<&PixelRect>, radius: f64) -> DecodedImage {
    let blurred = gaussian_blur(image, region, radius);
    let w_u = image.width as usize;
    let mut data = image.data.clone();

    let (x0, y0, x1, y1) = super::region_bounds(region, image.width, image.height);

    for y in y0..y1 {
        for x in x0..x1 {
            let offset = (y as usize * w_u + x as usize) * 4;
            for ch in 0..3 {
                // unsharp: original + (original - blurred)
                let orig = image.data[offset + ch] as i32;
                let blur_val = blurred.data[offset + ch] as i32;
                let v = orig + (orig - blur_val);
                data[offset + ch] = v.clamp(0, 255) as u8;
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
    fn blur_uniform_unchanged() {
        let img = uniform_image(4, 4, 100);
        let result = blur(&img, None);
        // 均一画像にぼかしをかけても値は変わらない
        for pixel in result.data.chunks_exact(4) {
            assert_eq!(pixel[0], 100);
        }
    }

    #[test]
    fn blur_strong_uniform_unchanged() {
        let img = uniform_image(6, 6, 50);
        let result = blur_strong(&img, None);
        for pixel in result.data.chunks_exact(4) {
            assert_eq!(pixel[0], 50);
        }
    }

    #[test]
    fn median_uniform_unchanged() {
        let img = uniform_image(4, 4, 200);
        let result = median_filter(&img, None);
        for pixel in result.data.chunks_exact(4) {
            assert_eq!(pixel[0], 200);
        }
    }

    #[test]
    fn blur_preserves_alpha() {
        let mut img = uniform_image(3, 3, 100);
        img.data[3] = 128; // 1ピクセルだけalpha変更
        let result = blur(&img, None);
        // alpha チャネルは変更されない
        assert_eq!(result.data[3], 128);
    }

    /// チェッカーパターン画像を生成 (偶数座標=c1, 奇数座標=c2)
    fn checker_image(w: u32, h: u32, c1: u8, c2: u8) -> DecodedImage {
        let mut data = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let offset = ((y * w + x) * 4) as usize;
                let v = if (x + y) % 2 == 0 { c1 } else { c2 };
                data[offset] = v;
                data[offset + 1] = v;
                data[offset + 2] = v;
                data[offset + 3] = 255;
            }
        }
        DecodedImage {
            data,
            width: w,
            height: h,
        }
    }

    // --- gaussian_blur テスト ---

    #[test]
    fn gaussian_blur_uniform_unchanged() {
        // 均一画像はぼかしても値が変わらない
        let img = uniform_image(8, 8, 120);
        let result = gaussian_blur(&img, None, 2.0);
        for pixel in result.data.chunks_exact(4) {
            assert_eq!(pixel[0], 120);
        }
    }

    #[test]
    fn gaussian_blur_min_radius() {
        // radius=0.1 → 内部で最小r=1にクランプされる
        let img = uniform_image(4, 4, 80);
        let result = gaussian_blur(&img, None, 0.1);
        assert_eq!(result.width, 4);
        assert_eq!(result.height, 4);
        // 均一画像なので値は変わらない
        for pixel in result.data.chunks_exact(4) {
            assert_eq!(pixel[0], 80);
        }
    }

    #[test]
    fn gaussian_blur_large_radius() {
        // radius=10.0 の大きなぼかし
        let img = checker_image(8, 8, 0, 200);
        let result = gaussian_blur(&img, None, 10.0);
        assert_eq!(result.width, 8);
        assert_eq!(result.height, 8);
        // 大きなぼかしでチェッカーパターンが平滑化され、中央付近は平均値に近づく
        let center = ((4 * 8 + 4) * 4) as usize;
        let center_val = result.data[center] as i32;
        assert!(
            (center_val - 100).abs() < 30,
            "中央ピクセルが平均値 (100) に近いはず: got {center_val}"
        );
    }

    #[test]
    fn gaussian_blur_smooths_checker() {
        // チェッカーパターンにぼかしをかけると差が縮まる
        let img = checker_image(8, 8, 0, 200);
        let result = gaussian_blur(&img, None, 2.0);
        // 結果のピクセル値の最大・最小差が元 (200) より小さくなる
        let mut min_val = 255u8;
        let mut max_val = 0u8;
        for pixel in result.data.chunks_exact(4) {
            min_val = min_val.min(pixel[0]);
            max_val = max_val.max(pixel[0]);
        }
        let range = max_val - min_val;
        assert!(
            range < 200,
            "ぼかし後のレンジが元の200未満のはず: got {range}"
        );
    }

    #[test]
    fn gaussian_blur_preserves_alpha() {
        let mut img = uniform_image(4, 4, 100);
        img.data[3] = 50; // 1ピクセルのalphaを変更
        let result = gaussian_blur(&img, None, 2.0);
        // alphaチャネルはぼかし対象外
        assert_eq!(result.data[3], 50);
    }

    // --- unsharp_mask テスト ---

    #[test]
    fn unsharp_mask_uniform_unchanged() {
        // 均一画像にアンシャープマスクをかけても変わらない
        // (original - blurred = 0 なので original + 0 = original)
        let img = uniform_image(8, 8, 150);
        let result = unsharp_mask(&img, None, 2.0);
        for pixel in result.data.chunks_exact(4) {
            assert_eq!(pixel[0], 150);
        }
    }

    #[test]
    fn unsharp_mask_min_radius() {
        // radius=0.1(最小境界値)
        let img = uniform_image(4, 4, 100);
        let result = unsharp_mask(&img, None, 0.1);
        assert_eq!(result.width, 4);
        assert_eq!(result.height, 4);
        for pixel in result.data.chunks_exact(4) {
            assert_eq!(pixel[0], 100);
        }
    }

    #[test]
    fn unsharp_mask_large_radius() {
        // radius=10.0 (大きな値) でも正常に動作する
        let img = checker_image(8, 8, 50, 200);
        let result = unsharp_mask(&img, None, 10.0);
        assert_eq!(result.width, 8);
        assert_eq!(result.height, 8);
        // アンシャープマスクはコントラストを強調する
        // → チェッカーパターンの差が広がるか、clampで0/255に固定される
        let mut min_val = 255u8;
        let mut max_val = 0u8;
        for pixel in result.data.chunks_exact(4) {
            min_val = min_val.min(pixel[0]);
            max_val = max_val.max(pixel[0]);
        }
        let original_range = 150; // 200 - 50
        let result_range = max_val - min_val;
        assert!(
            result_range >= original_range as u8,
            "アンシャープマスクでコントラスト強調されるはず: original={original_range}, result={result_range}"
        );
    }

    #[test]
    fn unsharp_mask_preserves_alpha() {
        let mut img = uniform_image(4, 4, 100);
        img.data[7] = 42; // 2番目のピクセルのalpha
        let result = unsharp_mask(&img, None, 2.0);
        assert_eq!(result.data[7], 42);
    }

    #[test]
    fn unsharp_mask_clamps_values() {
        // 極端な入力 (0と255のチェッカー) でもパニックせず正常に完了することを確認
        let img = checker_image(4, 4, 0, 255);
        let result = unsharp_mask(&img, None, 2.0);
        // 結果のデータ長が正しいこと
        assert_eq!(result.data.len(), (4 * 4 * 4) as usize);
        assert_eq!(result.width, 4);
        assert_eq!(result.height, 4);
    }

    // --- mosaic テスト ---

    #[test]
    fn mosaic_block_size_1_identity() {
        // block_size=1 は各ピクセルが自身の平均=自身なので変化しない
        let img = checker_image(4, 4, 30, 220);
        let result = mosaic(&img, None, 1);
        assert_eq!(result.data, img.data);
    }

    #[test]
    fn mosaic_block_size_0_treated_as_1() {
        // block_size=0 は内部で1にクランプされるので同様に変化しない
        let img = checker_image(4, 4, 30, 220);
        let result = mosaic(&img, None, 0);
        assert_eq!(result.data, img.data);
    }

    #[test]
    fn mosaic_uniform_unchanged() {
        // 均一画像は任意のブロックサイズでも変わらない
        let img = uniform_image(8, 8, 100);
        let result = mosaic(&img, None, 4);
        for pixel in result.data.chunks_exact(4) {
            assert_eq!(pixel[0], 100);
        }
    }

    #[test]
    fn mosaic_large_block_averages_all() {
        // ブロックサイズが画像全体を覆う場合、全ピクセルが画像全体の平均色になる
        let img = checker_image(4, 4, 0, 200);
        let result = mosaic(&img, None, 100); // 4x4画像に対して100x100ブロック
        // 4x4チェッカー: 8ピクセルが0、8ピクセルが200 → 平均=100
        let expected = 100u8;
        for pixel in result.data.chunks_exact(4) {
            assert_eq!(pixel[0], expected);
            assert_eq!(pixel[1], expected);
            assert_eq!(pixel[2], expected);
        }
    }

    #[test]
    fn mosaic_2x2_block_on_checker() {
        // 4x4チェッカーパターンに2x2モザイクをかける
        let img = checker_image(4, 4, 0, 200);
        let result = mosaic(&img, None, 2);
        // 各2x2ブロック内のピクセルはすべて同じ色になる
        for by in (0..4).step_by(2) {
            for bx in (0..4).step_by(2) {
                let offset0 = ((by * 4 + bx) * 4) as usize;
                let block_val = result.data[offset0];
                // 同じブロック内の4ピクセルが同じ値
                for dy in 0..2 {
                    for dx in 0..2 {
                        let offset = (((by + dy) * 4 + (bx + dx)) * 4) as usize;
                        assert_eq!(result.data[offset], block_val);
                        assert_eq!(result.data[offset + 1], block_val);
                        assert_eq!(result.data[offset + 2], block_val);
                    }
                }
            }
        }
    }

    #[test]
    fn mosaic_preserves_alpha() {
        let mut img = uniform_image(4, 4, 100);
        img.data[3] = 42;
        let result = mosaic(&img, None, 2);
        assert_eq!(result.data[3], 42);
    }
}
