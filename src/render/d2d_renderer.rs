use anyhow::{Context as _, Result};
use fast_image_resize as fr;
use rayon::prelude::*;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
    D2D1_BITMAP_PROPERTIES, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1CreateFactory, ID2D1Bitmap,
    ID2D1BitmapBrush, ID2D1Factory, ID2D1HwndRenderTarget,
};
use windows::Win32::Graphics::Direct2D::{D2D1_BITMAP_BRUSH_PROPERTIES, D2D1_EXTEND_MODE_WRAP};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

use super::layout::{DrawRect, Layout};
use crate::config::DisplayConfig;
use crate::image::DecodedImage;
use crate::selection::{self, HANDLE_DRAW_SIZE, PixelRect};

/// αチャネル背景モード
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaBackground {
    White,
    Black,
    #[default]
    Checker,
}

/// プリスケール済みビットマップのキャッシュエントリ
struct PrescaledEntry {
    source_key: usize, // 元画像データのポインタ（同一性判定用）
    width: u32,        // プリスケール後の幅
    height: u32,       // プリスケール後の高さ
    bitmap: ID2D1Bitmap,
}

/// Direct2D描画エンジン
pub struct D2DRenderer {
    #[allow(dead_code)] // COM参照をDrop時まで保持する必要がある
    factory: ID2D1Factory,
    render_target: ID2D1HwndRenderTarget,
    layout: Layout,
    /// 現在キャッシュ中のD2Dビットマップとそのソースポインタ（同一画像の再描画を高速化）
    cached_bitmap: Option<(usize, ID2D1Bitmap)>,
    /// CPU Lanczos3プリスケール済みビットマップのキャッシュ
    prescaled: Option<PrescaledEntry>,
    /// αチャネル背景モード
    alpha_bg: AlphaBackground,
    /// チェッカーパターンブラシ（遅延初期化）
    checker_brush: Option<ID2D1BitmapBrush>,
    /// 描画領域の左オフセット（ファイルリストパネル分）
    draw_offset_x: f32,
    /// 最後に描画した画像の描画矩形（選択の座標変換用）
    last_draw_rect: Option<DrawRect>,
}

// 背景色: ダークグレー (#333333)
const BG_COLOR: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 0.2,
    g: 0.2,
    b: 0.2,
    a: 1.0,
};

impl D2DRenderer {
    pub fn new(hwnd: HWND, display_config: &DisplayConfig) -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
                .context("D2D1Factory作成失敗")?;

            let render_target = Self::create_render_target(&factory, hwnd)?;
            let layout =
                Layout::from_config(display_config.to_display_mode(), display_config.margin);
            let alpha_bg = display_config.alpha_background;

            Ok(Self {
                factory,
                render_target,
                layout,
                cached_bitmap: None,
                prescaled: None,
                alpha_bg,
                checker_brush: None,
                draw_offset_x: 0.0,
                last_draw_rect: None,
            })
        }
    }

    unsafe fn create_render_target(
        factory: &ID2D1Factory,
        hwnd: HWND,
    ) -> Result<ID2D1HwndRenderTarget> {
        unsafe {
            let mut rc = std::mem::zeroed();
            GetClientRect(hwnd, std::ptr::from_mut(&mut rc))?;

            let size = D2D_SIZE_U {
                width: (rc.right - rc.left) as u32,
                height: (rc.bottom - rc.top) as u32,
            };

            let rt_props = D2D1_RENDER_TARGET_PROPERTIES::default();
            let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: size,
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };

            factory
                .CreateHwndRenderTarget(
                    std::ptr::from_ref(&rt_props),
                    std::ptr::from_ref(&hwnd_props),
                )
                .context("HwndRenderTarget作成失敗")
        }
    }

    /// ウィンドウリサイズ時に呼ぶ
    pub fn resize(&mut self, width: u32, height: u32) {
        let size = D2D_SIZE_U { width, height };
        unsafe {
            let _ = self.render_target.Resize(std::ptr::from_ref(&size));
        }
    }

    /// 画像を描画する。imageがNoneなら背景のみ。
    /// `selection_rect`: 選択矩形がある場合はオーバーレイ描画する
    pub fn draw(&mut self, image: Option<&DecodedImage>, selection_rect: Option<&PixelRect>) {
        unsafe {
            self.render_target.BeginDraw();

            let size = self.render_target.GetSize();

            // ファイルリストパネルの右側のみに描画を制限
            // Clear()前にクリップを設定し、パネル領域を塗りつぶさないようにする
            let has_clip = self.draw_offset_x > 0.0;
            if has_clip {
                let clip = D2D_RECT_F {
                    left: self.draw_offset_x,
                    top: 0.0,
                    right: size.width,
                    bottom: size.height,
                };
                self.render_target.PushAxisAlignedClip(
                    std::ptr::from_ref(&clip),
                    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                );
            }

            // クリップ設定後にクリア（パネル領域は保護される）
            self.render_target.Clear(Some(&BG_COLOR));

            if let Some(img) = image {
                // パネル幅を差し引いた描画領域でレイアウト計算
                let avail_width = size.width - self.draw_offset_x;
                let mut draw_rect =
                    self.layout
                        .calculate(img.width, img.height, avail_width, size.height);
                // オフセットを加算して実際の描画位置に変換
                draw_rect.x += self.draw_offset_x;

                // last_draw_rectを更新（選択の座標変換用）
                self.last_draw_rect = Some(draw_rect);

                // αチャネル背景を画像領域に描画
                self.draw_alpha_background(&draw_rect);

                // CPU Lanczos3プリスケーリングで高品質描画
                let target_w = (draw_rect.width as u32).max(1);
                let target_h = (draw_rect.height as u32).max(1);
                if let Ok(bitmap) = self.get_prescaled_bitmap(img, target_w, target_h) {
                    self.draw_bitmap(&bitmap, &draw_rect);
                }

                // ピクセルグリッド（8倍以上で表示）
                let scale = draw_rect.width / img.width as f32;
                if scale >= 8.0 {
                    self.draw_pixel_grid(&draw_rect, img.width, img.height, scale);
                }

                // 選択矩形オーバーレイ
                if let Some(sel_rect) = selection_rect {
                    self.draw_selection_overlay(sel_rect, &draw_rect, img.width, img.height);
                }
            } else {
                self.last_draw_rect = None;
            }

            if has_clip {
                self.render_target.PopAxisAlignedClip();
            }

            // EndDrawのエラーはリカバリ不要（次フレームで再試行される）
            let _ = self.render_target.EndDraw(None, None);
        }
    }

    /// DecodedImageからD2Dビットマップを取得（キャッシュ付き）
    unsafe fn get_or_create_bitmap(&mut self, image: &DecodedImage) -> Result<ID2D1Bitmap> {
        // ソースデータのポインタで同一性を判定
        let key = image.data.as_ptr() as usize;
        if let Some((cached_key, ref bitmap)) = self.cached_bitmap
            && cached_key == key
        {
            return Ok(bitmap.clone());
        }

        let bitmap = unsafe { self.create_bitmap_from_image(image)? };
        self.cached_bitmap = Some((key, bitmap.clone()));
        Ok(bitmap)
    }

    /// 表示サイズにプリスケールしたD2Dビットマップを取得（キャッシュ付き）
    /// 原寸表示の場合はプリスケールせず原画像のビットマップを返す
    unsafe fn get_prescaled_bitmap(
        &mut self,
        image: &DecodedImage,
        target_width: u32,
        target_height: u32,
    ) -> Result<ID2D1Bitmap> {
        let key = image.data.as_ptr() as usize;

        // キャッシュヒット: ソース・サイズ両方一致
        if let Some(ref entry) = self.prescaled
            && entry.source_key == key
            && entry.width == target_width
            && entry.height == target_height
        {
            return Ok(entry.bitmap.clone());
        }

        // 原寸表示ならプリスケール不要
        if target_width == image.width && target_height == image.height {
            return unsafe { self.get_or_create_bitmap(image) };
        }

        // 縮小時はSuperSampling（モアレ・リンギング抑制）、拡大時はLanczos3
        let is_shrink = target_width <= image.width && target_height <= image.height;
        let resize_alg = if is_shrink {
            fr::ResizeAlg::SuperSampling(fr::FilterType::Lanczos3, 2)
        } else {
            fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3)
        };
        let resized_data = fir_resize(
            &image.data,
            image.width,
            image.height,
            target_width,
            target_height,
            resize_alg,
        )?;

        let prescaled = DecodedImage {
            data: resized_data,
            width: target_width,
            height: target_height,
        };

        let bitmap = unsafe { self.create_bitmap_from_image(&prescaled)? };
        self.prescaled = Some(PrescaledEntry {
            source_key: key,
            width: target_width,
            height: target_height,
            bitmap: bitmap.clone(),
        });
        Ok(bitmap)
    }

    /// RGBA画像データからD2Dビットマップを作成
    /// image crateはRGBA、D2DはBGRA前提なのでチャネル入れ替え + premultiplied alpha変換が必要
    unsafe fn create_bitmap_from_image(&self, image: &DecodedImage) -> Result<ID2D1Bitmap> {
        let bgra_data = rgba_to_premultiplied_bgra(&image.data);

        let size = D2D_SIZE_U {
            width: image.width,
            height: image.height,
        };
        let pixel_format = D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        };
        let bitmap_props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: pixel_format,
            dpiX: 96.0,
            dpiY: 96.0,
        };
        let pitch = image.width * 4;

        unsafe {
            self.render_target
                .CreateBitmap(
                    size,
                    Some(bgra_data.as_ptr().cast::<core::ffi::c_void>()),
                    pitch,
                    std::ptr::from_ref(&bitmap_props),
                )
                .context("D2Dビットマップ作成失敗")
        }
    }

    /// αチャネル背景を描画（画像領域のみ）
    unsafe fn draw_alpha_background(&mut self, rect: &DrawRect) {
        let dest = D2D_RECT_F {
            left: rect.x,
            top: rect.y,
            right: rect.x + rect.width,
            bottom: rect.y + rect.height,
        };

        unsafe {
            match self.alpha_bg {
                AlphaBackground::White => {
                    if let Ok(brush) = self.render_target.CreateSolidColorBrush(
                        &D2D1_COLOR_F {
                            r: 1.0,
                            g: 1.0,
                            b: 1.0,
                            a: 1.0,
                        },
                        None,
                    ) {
                        self.render_target
                            .FillRectangle(std::ptr::from_ref(&dest), &brush);
                    }
                }
                AlphaBackground::Black => {
                    if let Ok(brush) = self.render_target.CreateSolidColorBrush(
                        &D2D1_COLOR_F {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                        None,
                    ) {
                        self.render_target
                            .FillRectangle(std::ptr::from_ref(&dest), &brush);
                    }
                }
                AlphaBackground::Checker => {
                    self.ensure_checker_brush();
                    if let Some(ref brush) = self.checker_brush {
                        self.render_target
                            .FillRectangle(std::ptr::from_ref(&dest), brush);
                    }
                }
            }
        }
    }

    /// チェッカーパターンブラシを遅延作成
    fn ensure_checker_brush(&mut self) {
        if self.checker_brush.is_some() {
            return;
        }
        self.checker_brush = unsafe { self.create_checker_brush().ok() };
    }

    /// 16x16の2色チェッカーパターンブラシを作成
    unsafe fn create_checker_brush(&self) -> Result<ID2D1BitmapBrush> {
        // 16x16ピクセル、8x8ブロックのチェッカーパターン (#CCCCCC + #FFFFFF)
        const TILE_SIZE: u32 = 16;
        const BLOCK_SIZE: u32 = 8;
        let mut pixels = vec![0u8; (TILE_SIZE * TILE_SIZE * 4) as usize];
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                let offset = ((y * TILE_SIZE + x) * 4) as usize;
                let is_light = ((x / BLOCK_SIZE) + (y / BLOCK_SIZE)).is_multiple_of(2);
                let color: u8 = if is_light { 0xFF } else { 0xCC };
                pixels[offset] = color; // B
                pixels[offset + 1] = color; // G
                pixels[offset + 2] = color; // R
                pixels[offset + 3] = 0xFF; // A
            }
        }

        let size = D2D_SIZE_U {
            width: TILE_SIZE,
            height: TILE_SIZE,
        };
        let pixel_format = D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        };
        let bitmap_props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: pixel_format,
            dpiX: 96.0,
            dpiY: 96.0,
        };

        unsafe {
            let tile_bitmap = self
                .render_target
                .CreateBitmap(
                    size,
                    Some(pixels.as_ptr().cast::<core::ffi::c_void>()),
                    TILE_SIZE * 4,
                    std::ptr::from_ref(&bitmap_props),
                )
                .context("チェッカータイルビットマップ作成失敗")?;

            let brush_props = D2D1_BITMAP_BRUSH_PROPERTIES {
                extendModeX: D2D1_EXTEND_MODE_WRAP,
                extendModeY: D2D1_EXTEND_MODE_WRAP,
                interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
            };
            self.render_target
                .CreateBitmapBrush(&tile_bitmap, Some(std::ptr::from_ref(&brush_props)), None)
                .context("チェッカーブラシ作成失敗")
        }
    }

    unsafe fn draw_bitmap(&self, bitmap: &ID2D1Bitmap, rect: &DrawRect) {
        unsafe {
            let dest = D2D_RECT_F {
                left: rect.x,
                top: rect.y,
                right: rect.x + rect.width,
                bottom: rect.y + rect.height,
            };
            self.render_target.DrawBitmap(
                bitmap,
                Some(std::ptr::from_ref(&dest)),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }

    /// αチャネル背景を巡回切替 (White → Black → Checker → White)
    pub fn cycle_alpha_background(&mut self) {
        self.alpha_bg = match self.alpha_bg {
            AlphaBackground::White => AlphaBackground::Black,
            AlphaBackground::Black => AlphaBackground::Checker,
            AlphaBackground::Checker => AlphaBackground::White,
        };
    }

    /// 描画領域の左オフセットを設定（ファイルリストパネル幅）
    pub fn set_draw_offset(&mut self, offset_x: f32) {
        self.draw_offset_x = offset_x;
    }

    /// 最後に描画した画像の描画矩形を返す（選択の座標変換用）
    pub fn last_draw_rect(&self) -> Option<&DrawRect> {
        self.last_draw_rect.as_ref()
    }

    /// ピクセルグリッドを描画する（拡大時のピクセル境界表示）
    unsafe fn draw_pixel_grid(
        &self,
        draw_rect: &DrawRect,
        img_width: u32,
        img_height: u32,
        scale: f32,
    ) {
        unsafe {
            let Ok(brush) = self.render_target.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.5,
                    g: 0.5,
                    b: 0.5,
                    a: 0.3,
                },
                None,
            ) else {
                return;
            };

            let half = 0.25;

            // 垂直線（細い矩形で描画）
            for x in 0..=img_width {
                let sx = draw_rect.x + x as f32 * scale;
                let rect = D2D_RECT_F {
                    left: sx - half,
                    top: draw_rect.y,
                    right: sx + half,
                    bottom: draw_rect.y + draw_rect.height,
                };
                self.render_target
                    .FillRectangle(std::ptr::from_ref(&rect), &brush);
            }

            // 水平線（細い矩形で描画）
            for y in 0..=img_height {
                let sy = draw_rect.y + y as f32 * scale;
                let rect = D2D_RECT_F {
                    left: draw_rect.x,
                    top: sy - half,
                    right: draw_rect.x + draw_rect.width,
                    bottom: sy + half,
                };
                self.render_target
                    .FillRectangle(std::ptr::from_ref(&rect), &brush);
            }
        }
    }

    /// 選択矩形のオーバーレイを描画する（白+黒の2本線 + ハンドル）
    unsafe fn draw_selection_overlay(
        &self,
        sel_rect: &PixelRect,
        draw_rect: &DrawRect,
        img_width: u32,
        img_height: u32,
    ) {
        unsafe {
            // 選択矩形のスクリーン座標を計算
            let (left, top) = selection::image_to_screen(
                sel_rect.x, sel_rect.y, draw_rect, img_width, img_height,
            );
            let (right, bottom) = selection::image_to_screen(
                sel_rect.right(),
                sel_rect.bottom(),
                draw_rect,
                img_width,
                img_height,
            );

            let rect_outer = D2D_RECT_F {
                left,
                top,
                right,
                bottom,
            };

            // 黒線（外側）
            if let Ok(black_brush) = self.render_target.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.8,
                },
                None,
            ) {
                self.render_target.DrawRectangle(
                    std::ptr::from_ref(&rect_outer),
                    &black_brush,
                    2.0,
                    None,
                );
            }

            // 白線（内側、破線風に見える）
            if let Ok(white_brush) = self.render_target.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.9,
                },
                None,
            ) {
                self.render_target.DrawRectangle(
                    std::ptr::from_ref(&rect_outer),
                    &white_brush,
                    1.0,
                    None,
                );

                // 8箇所のリサイズハンドルを描画
                let handles = selection::handle_positions(sel_rect);
                let hs = HANDLE_DRAW_SIZE;
                for (_kind, hx, hy) in &handles {
                    let (sx, sy) =
                        selection::image_to_screen(*hx, *hy, draw_rect, img_width, img_height);
                    let handle_rect = D2D_RECT_F {
                        left: sx - hs,
                        top: sy - hs,
                        right: sx + hs,
                        bottom: sy + hs,
                    };
                    self.render_target
                        .FillRectangle(std::ptr::from_ref(&handle_rect), &white_brush);
                }
            }
        }
    }
}

/// RGBA → premultiplied BGRA変換（rayon並列化）
fn rgba_to_premultiplied_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut bgra = rgba.to_vec();
    let row_bytes = 4 * 256; // 256ピクセル単位でチャンク分割
    bgra.par_chunks_mut(row_bytes).for_each(|chunk| {
        for pixel in chunk.chunks_exact_mut(4) {
            let r = pixel[0];
            let g = pixel[1];
            let b = pixel[2];
            let a = pixel[3] as u16;
            // premultiplied alpha: 各チャネルにalpha/255を乗算
            pixel[0] = ((b as u16 * a) / 255) as u8; // B
            pixel[1] = ((g as u16 * a) / 255) as u8; // G
            pixel[2] = ((r as u16 * a) / 255) as u8; // R
            // pixel[3] = a (そのまま)
        }
    });
    bgra
}

/// fast_image_resizeによるリサイズ（SIMD加速）
fn fir_resize(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    resize_alg: fr::ResizeAlg,
) -> Result<Vec<u8>> {
    let mut src_buf = src.to_vec();
    let src_image =
        fr::images::Image::from_slice_u8(src_w, src_h, &mut src_buf, fr::PixelType::U8x4)
            .context("リサイズ用ソース画像作成失敗")?;
    let mut dst_image = fr::images::Image::new(dst_w, dst_h, fr::PixelType::U8x4);
    let options = fr::ResizeOptions::new().resize_alg(resize_alg);
    let mut resizer = fr::Resizer::new();
    resizer
        .resize(&src_image, &mut dst_image, &options)
        .context("画像リサイズ失敗")?;
    Ok(dst_image.into_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fir_resize_shrink_output_size() {
        // 4x4 → 2x2: 出力バッファサイズの検証
        let src: Vec<u8> = [100, 150, 200, 255].repeat(16);
        let result = fir_resize(
            &src,
            4,
            4,
            2,
            2,
            fr::ResizeAlg::SuperSampling(fr::FilterType::Lanczos3, 2),
        )
        .unwrap();
        assert_eq!(result.len(), 2 * 2 * 4);
    }

    #[test]
    fn fir_resize_enlarge_output_size() {
        // 2x2 → 4x4: 出力バッファサイズの検証
        let src: Vec<u8> = [100, 150, 200, 255].repeat(4);
        let result = fir_resize(
            &src,
            2,
            2,
            4,
            4,
            fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3),
        )
        .unwrap();
        assert_eq!(result.len(), 4 * 4 * 4);
    }

    #[test]
    fn fir_resize_uniform_shrink() {
        // 均一色画像の縮小 → 結果も同じ色（alpha=255なので誤差なし）
        let src: Vec<u8> = [100, 150, 200, 255].repeat(16);
        let result = fir_resize(
            &src,
            4,
            4,
            2,
            2,
            fr::ResizeAlg::SuperSampling(fr::FilterType::Lanczos3, 2),
        )
        .unwrap();
        for pixel in result.chunks_exact(4) {
            assert_eq!(pixel, [100, 150, 200, 255]);
        }
    }

    #[test]
    fn fir_resize_alpha_no_halo() {
        // 半透明境界のリサイズでハロー（白フリンジ）が出ないことを検証
        // 上半分: 赤(alpha=255)、下半分: 赤(alpha=0)
        let mut src = Vec::with_capacity(4 * 4 * 4);
        for y in 0..4u32 {
            for _ in 0..4u32 {
                if y < 2 {
                    src.extend_from_slice(&[255, 0, 0, 255]); // 不透明な赤
                } else {
                    src.extend_from_slice(&[0, 0, 0, 0]); // 完全透明
                }
            }
        }
        let result = fir_resize(
            &src,
            4,
            4,
            2,
            2,
            fr::ResizeAlg::SuperSampling(fr::FilterType::Lanczos3, 2),
        )
        .unwrap();
        // alpha境界付近のピクセルで、RGB値が不自然に白くならないことを確認
        for pixel in result.chunks_exact(4) {
            let a = pixel[3] as u16;
            if a > 0 {
                // alpha > 0 のピクセルは、G/Bチャネルが不自然に高くならない
                assert!(
                    pixel[1] < 30 && pixel[2] < 30,
                    "ハロー検出: pixel={pixel:?}"
                );
            }
        }
    }
}
