//! 画像フィルタ・変換モジュール

pub mod blur;
pub mod brightness;
pub mod color;
pub mod sharpen;
pub mod transform;

use crate::image::DecodedImage;
use crate::selection::PixelRect;

/// 選択領域の境界 (x0, y0, x1, y1) を計算する。選択なしなら全画像を返す。
pub(super) fn region_bounds(
    region: Option<&PixelRect>,
    width: u32,
    height: u32,
) -> (i32, i32, i32, i32) {
    if let Some(r) = region {
        let r = r.clamped(width, height);
        (r.x, r.y, r.right(), r.bottom())
    } else {
        (0, 0, width as i32, height as i32)
    }
}

/// 選択領域内のピクセルに変換関数を適用する (選択なしなら全画像)
pub(super) fn apply_to_region(
    image: &DecodedImage,
    region: Option<&PixelRect>,
    f: impl Fn(u8, u8, u8, u8) -> [u8; 4],
) -> DecodedImage {
    let mut data = image.data.clone();
    let w = image.width as i32;

    let (x0, y0, x1, y1) = region_bounds(region, image.width, image.height);

    for y in y0..y1 {
        for x in x0..x1 {
            let offset = ((y * w + x) * 4) as usize;
            let [r, g, b, a] = [
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ];
            let result = f(r, g, b, a);
            data[offset..offset + 4].copy_from_slice(&result);
        }
    }

    DecodedImage {
        data,
        width: image.width,
        height: image.height,
    }
}
