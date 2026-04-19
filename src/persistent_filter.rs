//! 永続フィルタ (全画像に自動適用されるフィルタ)
//!
//! 「フィルタ」メニューで設定した操作を、全画像閲覧時に自動適用する。
//! フィルタ設定変更時はキャッシュを全無効化して再描画する。

use crate::filter;
use crate::image::DecodedImage;

/// 永続フィルタの操作種別
#[derive(Debug, Clone)]
pub enum FilterOperation {
    FlipHorizontal,
    FlipVertical,
    Rotate180,
    Rotate90CW,
    Rotate90CCW,
    // 色変換
    Levels { low: u8, high: u8 },
    Gamma { value: f64 },
    BrightnessContrast { brightness: i32, contrast: i32 },
    GrayscaleSimple,
    GrayscaleStrict,
    // フィルタ
    Blur,
    BlurStrong,
    Sharpen,
    SharpenStrong,
    GaussianBlur { radius: f64 },
    UnsharpMask { radius: f64 },
    MedianFilter,
    // 色操作
    InvertColors,
    ApplyAlpha,
}

/// 永続フィルタ設定
pub struct PersistentFilter {
    /// フィルタの有効/無効
    enabled: bool,
    /// 適用するフィルタ操作のリスト (順序通りに適用)
    operations: Vec<FilterOperation>,
    /// 設定変更時にインクリメントされる世代番号
    generation: u64,
}

impl PersistentFilter {
    pub fn new() -> Self {
        Self {
            enabled: false,
            operations: Vec::new(),
            generation: 0,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn toggle_enabled(&mut self) {
        self.enabled = !self.enabled;
        self.generation += 1;
    }

    #[cfg(test)]
    pub fn operations(&self) -> &[FilterOperation] {
        &self.operations
    }

    /// 指定したバリアントと同じ種別の操作が含まれるか判定
    pub fn has_operation(&self, probe: &FilterOperation) -> bool {
        let target = std::mem::discriminant(probe);
        self.operations
            .iter()
            .any(|op| std::mem::discriminant(op) == target)
    }

    /// フィルタ操作を追加する
    pub fn add_operation(&mut self, op: FilterOperation) {
        self.operations.push(op);
        self.generation += 1;
    }

    /// 指定バリアントと同じ種別の操作を全て除去する
    /// 除去した場合trueを返す
    pub fn remove_operation_type(&mut self, probe: &FilterOperation) -> bool {
        let target = std::mem::discriminant(probe);
        let before = self.operations.len();
        self.operations
            .retain(|op| std::mem::discriminant(op) != target);
        if self.operations.len() == before {
            false
        } else {
            self.generation += 1;
            true
        }
    }

    /// 全操作をクリアする
    #[allow(dead_code)] // 将来のUI操作で使用予定
    pub fn clear_operations(&mut self) {
        self.operations.clear();
        self.generation += 1;
    }

    /// フィルタが有効な場合、画像にフィルタを適用して返す
    /// 無効またはフィルタがない場合はNoneを返す (元画像をそのまま使う)
    pub fn apply(&self, image: &DecodedImage) -> Option<DecodedImage> {
        if !self.enabled || self.operations.is_empty() {
            return None;
        }

        let mut result = DecodedImage {
            data: image.data.clone(),
            width: image.width,
            height: image.height,
        };

        for op in &self.operations {
            result = apply_operation(&result, op);
        }

        Some(result)
    }
}

/// 単一のフィルタ操作を適用する
fn apply_operation(image: &DecodedImage, op: &FilterOperation) -> DecodedImage {
    match op {
        FilterOperation::FlipHorizontal => filter::transform::flip_horizontal(image),
        FilterOperation::FlipVertical => filter::transform::flip_vertical(image),
        FilterOperation::Rotate180 => filter::transform::rotate_180(image),
        FilterOperation::Rotate90CW => filter::transform::rotate_90(image),
        FilterOperation::Rotate90CCW => filter::transform::rotate_270(image),
        FilterOperation::Levels { low, high } => {
            filter::brightness::levels(image, None, *low, *high)
        }
        FilterOperation::Gamma { value } => filter::brightness::gamma(image, None, *value),
        FilterOperation::BrightnessContrast {
            brightness,
            contrast,
        } => filter::brightness::brightness_contrast(image, None, *brightness, *contrast),
        FilterOperation::GrayscaleSimple => filter::color::grayscale_simple(image, None),
        FilterOperation::GrayscaleStrict => filter::color::grayscale_strict(image, None),
        FilterOperation::Blur => filter::blur::blur(image, None),
        FilterOperation::BlurStrong => filter::blur::blur_strong(image, None),
        FilterOperation::Sharpen => filter::sharpen::sharpen(image, None),
        FilterOperation::SharpenStrong => filter::sharpen::sharpen_strong(image, None),
        FilterOperation::GaussianBlur { radius } => {
            filter::blur::gaussian_blur(image, None, *radius)
        }
        FilterOperation::UnsharpMask { radius } => filter::blur::unsharp_mask(image, None, *radius),
        FilterOperation::MedianFilter => filter::blur::median_filter(image, None),
        FilterOperation::InvertColors => filter::color::invert_colors(image, None),
        FilterOperation::ApplyAlpha => filter::color::apply_alpha(image, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_image() -> DecodedImage {
        DecodedImage {
            data: vec![100, 150, 200, 255],
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn disabled_returns_none() {
        let pf = PersistentFilter::new();
        assert!(pf.apply(&test_image()).is_none());
    }

    #[test]
    fn enabled_with_no_ops_returns_none() {
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        assert!(pf.apply(&test_image()).is_none());
    }

    #[test]
    fn enabled_with_ops_applies() {
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::InvertColors);
        let result = pf.apply(&test_image()).unwrap();
        assert_eq!(result.data[0], 155); // 255-100
    }

    #[test]
    fn multiple_operations_chain() {
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::InvertColors);
        pf.add_operation(FilterOperation::InvertColors);
        // 2回反転 → 元に戻る
        let img = test_image();
        let result = pf.apply(&img).unwrap();
        assert_eq!(result.data[0], img.data[0]);
    }

    #[test]
    fn clear_operations_empties_list_and_increments_generation() {
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled(); // gen=1
        pf.add_operation(FilterOperation::Blur); // gen=2
        pf.add_operation(FilterOperation::Sharpen); // gen=3
        assert_eq!(pf.operations().len(), 2);

        pf.clear_operations(); // gen=4
        assert!(pf.operations().is_empty());
        // 有効だが操作なし → None
        assert!(pf.apply(&test_image()).is_none());
    }

    #[test]
    fn three_filter_chain_grayscale_invert_levels() {
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::GrayscaleStrict);
        pf.add_operation(FilterOperation::InvertColors);
        pf.add_operation(FilterOperation::Levels { low: 0, high: 255 });

        let result = pf.apply(&test_image()).unwrap();
        // グレースケール → 反転 → レベル補正 (フルレンジ) が順に適用される
        assert_eq!(result.width, 1);
        assert_eq!(result.height, 1);
        assert_eq!(result.data.len(), 4);
        // α値は保持される
        assert_eq!(result.data[3], 255);
    }

    #[test]
    fn rotate_90cw_changes_dimensions() {
        // 2x1 の画像を90°回転すると 1x2 になる
        let img = DecodedImage {
            data: vec![
                255, 0, 0, 255, // (0,0) 赤
                0, 255, 0, 255, // (1,0) 緑
            ],
            width: 2,
            height: 1,
        };
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::Rotate90CW);

        let result = pf.apply(&img).unwrap();
        assert_eq!(result.width, 1);
        assert_eq!(result.height, 2);
    }

    #[test]
    fn rotate_90ccw_changes_dimensions() {
        let img = DecodedImage {
            data: vec![255, 0, 0, 255, 0, 255, 0, 255],
            width: 2,
            height: 1,
        };
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::Rotate90CCW);

        let result = pf.apply(&img).unwrap();
        assert_eq!(result.width, 1);
        assert_eq!(result.height, 2);
    }

    #[test]
    fn rotate_180_preserves_dimensions() {
        let img = DecodedImage {
            data: vec![255, 0, 0, 255, 0, 255, 0, 255],
            width: 2,
            height: 1,
        };
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::Rotate180);

        let result = pf.apply(&img).unwrap();
        assert_eq!(result.width, 2);
        assert_eq!(result.height, 1);
        // 180°回転で順序が反転: 元 (赤,緑) → (緑,赤)
        assert_eq!(&result.data[0..4], &[0, 255, 0, 255]);
        assert_eq!(&result.data[4..8], &[255, 0, 0, 255]);
    }

    #[test]
    fn flip_horizontal_swaps_pixels() {
        let img = DecodedImage {
            data: vec![255, 0, 0, 255, 0, 255, 0, 255],
            width: 2,
            height: 1,
        };
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::FlipHorizontal);

        let result = pf.apply(&img).unwrap();
        assert_eq!(&result.data[0..4], &[0, 255, 0, 255]);
        assert_eq!(&result.data[4..8], &[255, 0, 0, 255]);
    }

    #[test]
    fn flip_vertical_swaps_rows() {
        let img = DecodedImage {
            data: vec![
                255, 0, 0, 255, // 1行目: 赤
                0, 0, 255, 255, // 2行目: 青
            ],
            width: 1,
            height: 2,
        };
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::FlipVertical);

        let result = pf.apply(&img).unwrap();
        assert_eq!(&result.data[0..4], &[0, 0, 255, 255]); // 青が先頭に
        assert_eq!(&result.data[4..8], &[255, 0, 0, 255]);
    }

    #[test]
    fn gamma_correction_adjusts_brightness() {
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        // ガンマ1.0は変化なし
        pf.add_operation(FilterOperation::Gamma { value: 1.0 });

        let img = test_image();
        let result = pf.apply(&img).unwrap();
        assert_eq!(result.data[0], img.data[0]);
        assert_eq!(result.data[1], img.data[1]);
        assert_eq!(result.data[2], img.data[2]);
        assert_eq!(result.data[3], img.data[3]);
    }

    #[test]
    fn brightness_contrast_adjustment() {
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        // 明るさ0, コントラスト0は変化なし
        pf.add_operation(FilterOperation::BrightnessContrast {
            brightness: 0,
            contrast: 0,
        });

        let img = test_image();
        let result = pf.apply(&img).unwrap();
        assert_eq!(result.data[0], img.data[0]);
        assert_eq!(result.data[3], 255); // αは保持
    }

    #[test]
    fn grayscale_simple_converts() {
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::GrayscaleSimple);

        let result = pf.apply(&test_image()).unwrap();
        // 簡易グレースケール: RGB平均値でR=G=B
        assert_eq!(result.data[0], result.data[1]);
        assert_eq!(result.data[1], result.data[2]);
    }

    #[test]
    fn apply_alpha_composites_on_white() {
        let img = DecodedImage {
            data: vec![255, 0, 0, 128], // 半透明の赤
            width: 1,
            height: 1,
        };
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::ApplyAlpha);

        let result = pf.apply(&img).unwrap();
        // α=128で白背景に合成 → 赤みが薄くなる
        assert!(result.data[0] > 128); // 赤がある程度残る
        assert!(result.data[1] > 0); // 白背景の影響で緑成分あり
        assert_eq!(result.data[3], 255); // α合成後はα=255
    }

    #[test]
    fn gaussian_blur_runs_without_panic() {
        let img = DecodedImage {
            data: vec![
                100, 150, 200, 255, 50, 80, 120, 255, 200, 100, 50, 255, 150, 180, 210, 255,
            ],
            width: 2,
            height: 2,
        };
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::GaussianBlur { radius: 1.0 });

        let result = pf.apply(&img).unwrap();
        assert_eq!(result.width, 2);
        assert_eq!(result.height, 2);
    }

    #[test]
    fn unsharp_mask_runs_without_panic() {
        let img = DecodedImage {
            data: vec![
                100, 150, 200, 255, 50, 80, 120, 255, 200, 100, 50, 255, 150, 180, 210, 255,
            ],
            width: 2,
            height: 2,
        };
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::UnsharpMask { radius: 1.0 });

        let result = pf.apply(&img).unwrap();
        assert_eq!(result.width, 2);
        assert_eq!(result.height, 2);
    }

    #[test]
    fn median_filter_runs_without_panic() {
        let img = DecodedImage {
            data: vec![
                100, 150, 200, 255, 50, 80, 120, 255, 200, 100, 50, 255, 150, 180, 210, 255,
            ],
            width: 2,
            height: 2,
        };
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::MedianFilter);

        let result = pf.apply(&img).unwrap();
        assert_eq!(result.width, 2);
        assert_eq!(result.height, 2);
    }

    #[test]
    fn levels_clamps_range() {
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        // レベル補正: low=100, high=200 → 100未満は0, 200以上は255, 間は線形
        pf.add_operation(FilterOperation::Levels {
            low: 100,
            high: 200,
        });

        let result = pf.apply(&test_image()).unwrap();
        // 元: R=100 → low=100で下限ちょうど → 0付近
        assert_eq!(result.data[0], 0);
        // 元: G=150 → (150-100)/(200-100)*255 = 127.5
        assert!(result.data[1] > 100);
        // 元: B=200 → high=200で上限ちょうど → 255
        assert_eq!(result.data[2], 255);
    }

    #[test]
    fn four_filter_chain_transforms() {
        // 4つの幾何変換フィルタをチェーン: 水平反転 → 垂直反転 → 90°CW → 90°CCW
        // 90°CW + 90°CCW = 元に戻り、水平反転 + 垂直反転 = 180°回転と同等
        let img = DecodedImage {
            data: vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255],
            width: 2,
            height: 2,
        };
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::FlipHorizontal);
        pf.add_operation(FilterOperation::FlipVertical);
        pf.add_operation(FilterOperation::Rotate90CW);
        pf.add_operation(FilterOperation::Rotate90CCW);
        // 最終結果: 180°回転 (水平+垂直反転の効果)
        let result = pf.apply(&img).unwrap();
        assert_eq!(result.width, 2);
        assert_eq!(result.height, 2);
        // 180°回転: (0,0)↔(1,1), (1,0)↔(0,1)
        assert_eq!(&result.data[0..4], &[10, 11, 12, 255]);
        assert_eq!(&result.data[12..16], &[1, 2, 3, 255]);
    }

    #[test]
    fn toggle_twice_restores_disabled() {
        let mut pf = PersistentFilter::new();
        assert!(!pf.is_enabled());
        pf.toggle_enabled();
        assert!(pf.is_enabled());
        pf.toggle_enabled();
        assert!(!pf.is_enabled());
        // 無効状態 → 操作があってもNone
        pf.add_operation(FilterOperation::InvertColors);
        assert!(pf.apply(&test_image()).is_none());
    }

    #[test]
    fn blur_and_sharpen_variants() {
        let img = DecodedImage {
            data: vec![
                100, 150, 200, 255, 50, 80, 120, 255, 200, 100, 50, 255, 150, 180, 210, 255,
            ],
            width: 2,
            height: 2,
        };

        // Blur
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::Blur);
        assert!(pf.apply(&img).is_some());

        // BlurStrong
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::BlurStrong);
        assert!(pf.apply(&img).is_some());

        // Sharpen
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::Sharpen);
        assert!(pf.apply(&img).is_some());

        // SharpenStrong
        let mut pf = PersistentFilter::new();
        pf.toggle_enabled();
        pf.add_operation(FilterOperation::SharpenStrong);
        assert!(pf.apply(&img).is_some());
    }
}
