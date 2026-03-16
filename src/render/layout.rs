/// 表示モード
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum DisplayMode {
    /// ウィンドウに収まるよう縮小（拡大はしない）
    AutoShrink,
    /// ウィンドウに合わせて拡大・縮小
    AutoFit,
    /// ウィンドウに合わせて拡大のみ（小さい画像は原寸）
    AutoEnlarge,
    /// 原寸大表示
    Original,
    /// 固定倍率
    Fixed(f32),
}

/// 描画先矩形
#[derive(Debug, Clone, Copy)]
pub struct DrawRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

pub struct Layout {
    pub mode: DisplayMode,
    /// 余白の有効/無効
    pub margin_enabled: bool,
    /// 余白量（ピクセル）
    pub margin_amount: f32,
}

impl Layout {
    pub fn new() -> Self {
        Self {
            mode: DisplayMode::AutoShrink,
            margin_enabled: false,
            margin_amount: 20.0,
        }
    }

    /// Configから初期化
    pub fn from_config(mode: DisplayMode, margin_amount: f32) -> Self {
        Self {
            mode,
            margin_enabled: false,
            margin_amount,
        }
    }

    /// 画像サイズとウィンドウサイズから描画先矩形を計算
    pub fn calculate(
        &self,
        image_width: u32,
        image_height: u32,
        window_width: f32,
        window_height: f32,
    ) -> DrawRect {
        let img_w = image_width as f32;
        let img_h = image_height as f32;

        // margin有効時は有効領域を縮小してスケール計算
        let margin = if self.margin_enabled {
            self.margin_amount
        } else {
            0.0
        };
        let avail_w = (window_width - margin * 2.0).max(1.0);
        let avail_h = (window_height - margin * 2.0).max(1.0);

        let scale = match self.mode {
            DisplayMode::AutoShrink => {
                let scale_x = avail_w / img_w;
                let scale_y = avail_h / img_h;
                scale_x.min(scale_y).min(1.0)
            }
            DisplayMode::AutoFit => {
                let scale_x = avail_w / img_w;
                let scale_y = avail_h / img_h;
                scale_x.min(scale_y)
            }
            DisplayMode::AutoEnlarge => {
                let scale_x = avail_w / img_w;
                let scale_y = avail_h / img_h;
                // 拡大のみ（1.0未満にはしない）
                scale_x.min(scale_y).max(1.0)
            }
            DisplayMode::Original => 1.0,
            DisplayMode::Fixed(s) => s,
        };

        let draw_w = img_w * scale;
        let draw_h = img_h * scale;

        // ウィンドウ全体の中央に配置
        DrawRect {
            x: (window_width - draw_w) / 2.0,
            y: (window_height - draw_h) / 2.0,
            width: draw_w,
            height: draw_h,
        }
    }

    /// 現在のモード・marginを考慮した表示倍率を返す
    pub fn effective_scale(
        &self,
        image_width: u32,
        image_height: u32,
        window_width: f32,
        window_height: f32,
    ) -> f32 {
        let rect = self.calculate(image_width, image_height, window_width, window_height);
        rect.width / image_width as f32
    }

    /// 倍率を1.4倍にする（Fixed modeに切替）
    pub fn zoom_in(
        &mut self,
        image_width: u32,
        image_height: u32,
        window_width: f32,
        window_height: f32,
    ) {
        let current = self.effective_scale(image_width, image_height, window_width, window_height);
        self.mode = DisplayMode::Fixed(current * 1.4);
    }

    /// 倍率を0.7倍にする（Fixed modeに切替）
    pub fn zoom_out(
        &mut self,
        image_width: u32,
        image_height: u32,
        window_width: f32,
        window_height: f32,
    ) {
        let current = self.effective_scale(image_width, image_height, window_width, window_height);
        self.mode = DisplayMode::Fixed(current * 0.7);
    }

    /// 倍率を1.0にリセット（Fixed modeに切替）
    pub fn zoom_reset(&mut self) {
        self.mode = DisplayMode::Fixed(1.0);
    }

    /// 余白の有効/無効をトグル
    pub fn toggle_margin(&mut self) {
        self.margin_enabled = !self.margin_enabled;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_shrink_scales_down_large_image() {
        let layout = Layout::new();
        let rect = layout.calculate(2000, 1000, 800.0, 600.0);
        assert!(rect.width <= 800.0);
        assert!(rect.height <= 600.0);
        assert!(rect.x >= 0.0);
        assert!(rect.y >= 0.0);
    }

    #[test]
    fn auto_shrink_does_not_enlarge_small_image() {
        let layout = Layout::new();
        let rect = layout.calculate(100, 100, 800.0, 600.0);
        assert!((rect.width - 100.0).abs() < 0.01);
        assert!((rect.height - 100.0).abs() < 0.01);
    }

    #[test]
    fn auto_fit_enlarges_small_image() {
        let layout = Layout {
            mode: DisplayMode::AutoFit,
            ..Layout::new()
        };
        let rect = layout.calculate(100, 100, 800.0, 600.0);
        assert!((rect.width - 600.0).abs() < 0.01);
        assert!((rect.height - 600.0).abs() < 0.01);
    }

    #[test]
    fn original_mode_keeps_1x() {
        let layout = Layout {
            mode: DisplayMode::Original,
            ..Layout::new()
        };
        let rect = layout.calculate(400, 300, 800.0, 600.0);
        assert!((rect.width - 400.0).abs() < 0.01);
        assert!((rect.height - 300.0).abs() < 0.01);
    }

    #[test]
    fn fixed_mode_applies_scale() {
        let layout = Layout {
            mode: DisplayMode::Fixed(2.0),
            ..Layout::new()
        };
        let rect = layout.calculate(100, 100, 800.0, 600.0);
        assert!((rect.width - 200.0).abs() < 0.01);
        assert!((rect.height - 200.0).abs() < 0.01);
    }

    #[test]
    fn auto_enlarge_only_enlarges() {
        let layout = Layout {
            mode: DisplayMode::AutoEnlarge,
            ..Layout::new()
        };
        // 小さい画像 → 拡大
        let rect = layout.calculate(100, 100, 800.0, 600.0);
        assert!((rect.width - 600.0).abs() < 0.01);
        // 大きい画像 → そのまま（1.0を下回らない）
        let rect2 = layout.calculate(2000, 1000, 800.0, 600.0);
        assert!((rect2.width - 2000.0).abs() < 0.01);
    }

    #[test]
    fn margin_shrinks_available_area() {
        let layout = Layout {
            mode: DisplayMode::AutoFit,
            margin_enabled: true,
            margin_amount: 20.0,
        };
        // 800-40=760, 600-40=560 の有効領域
        let rect = layout.calculate(760, 560, 800.0, 600.0);
        assert!(rect.width <= 760.0 + 0.01);
        assert!(rect.height <= 560.0 + 0.01);
        // 中央配置はウィンドウ全体基準
        assert!((rect.x - 20.0).abs() < 0.01);
        assert!((rect.y - 20.0).abs() < 0.01);
    }

    #[test]
    fn zoom_in_increases_scale() {
        let mut layout = Layout::new();
        let scale_before = layout.effective_scale(200, 200, 800.0, 600.0);
        layout.zoom_in(200, 200, 800.0, 600.0);
        let scale_after = layout.effective_scale(200, 200, 800.0, 600.0);
        assert!(scale_after > scale_before);
    }

    #[test]
    fn zoom_out_decreases_scale() {
        let mut layout = Layout::new();
        let scale_before = layout.effective_scale(200, 200, 800.0, 600.0);
        layout.zoom_out(200, 200, 800.0, 600.0);
        let scale_after = layout.effective_scale(200, 200, 800.0, 600.0);
        assert!(scale_after < scale_before);
    }

    #[test]
    fn zoom_reset_sets_1x() {
        let mut layout = Layout {
            mode: DisplayMode::AutoFit,
            ..Layout::new()
        };
        layout.zoom_reset();
        assert_eq!(layout.mode, DisplayMode::Fixed(1.0));
    }
}
