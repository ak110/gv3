use anyhow::{Context as _, Result};
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

/// αチャネル背景モード
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlphaBackground {
    White,
    Black,
    Checker,
}

/// Direct2D描画エンジン
pub struct D2DRenderer {
    #[allow(dead_code)]
    factory: ID2D1Factory,
    render_target: ID2D1HwndRenderTarget,
    layout: Layout,
    /// 現在キャッシュ中のD2Dビットマップとそのソースポインタ（同一画像の再描画を高速化）
    cached_bitmap: Option<(usize, ID2D1Bitmap)>,
    /// αチャネル背景モード
    alpha_bg: AlphaBackground,
    /// チェッカーパターンブラシ（遅延初期化）
    checker_brush: Option<ID2D1BitmapBrush>,
    /// 描画領域の左オフセット（ファイルリストパネル分）
    draw_offset_x: f32,
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
            let alpha_bg = display_config.to_alpha_background();

            Ok(Self {
                factory,
                render_target,
                layout,
                cached_bitmap: None,
                alpha_bg,
                checker_brush: None,
                draw_offset_x: 0.0,
            })
        }
    }

    unsafe fn create_render_target(
        factory: &ID2D1Factory,
        hwnd: HWND,
    ) -> Result<ID2D1HwndRenderTarget> {
        unsafe {
            let mut rc = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc)?;

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
                .CreateHwndRenderTarget(&rt_props, &hwnd_props)
                .context("HwndRenderTarget作成失敗")
        }
    }

    /// ウィンドウリサイズ時に呼ぶ
    pub fn resize(&mut self, width: u32, height: u32) {
        let size = D2D_SIZE_U { width, height };
        unsafe {
            let _ = self.render_target.Resize(&size);
        }
    }

    /// 画像を描画する。imageがNoneなら背景のみ。
    pub fn draw(&mut self, image: Option<&DecodedImage>) {
        unsafe {
            self.render_target.BeginDraw();

            // ファイルリストパネルの右側のみに描画を制限
            // （D2DはGDIクリッピングをバイパスするため、パネル上に描画が被るのを防止）
            let size = self.render_target.GetSize();
            let has_clip = self.draw_offset_x > 0.0;
            if has_clip {
                let clip = D2D_RECT_F {
                    left: self.draw_offset_x,
                    top: 0.0,
                    right: size.width,
                    bottom: size.height,
                };
                self.render_target
                    .PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            }

            self.render_target.Clear(Some(&BG_COLOR));

            if let Some(img) = image
                && let Ok(bitmap) = self.get_or_create_bitmap(img)
            {
                // パネル幅を差し引いた描画領域でレイアウト計算
                let avail_width = size.width - self.draw_offset_x;
                let mut draw_rect =
                    self.layout
                        .calculate(img.width, img.height, avail_width, size.height);
                // オフセットを加算して実際の描画位置に変換
                draw_rect.x += self.draw_offset_x;

                // αチャネル背景を画像領域に描画
                self.draw_alpha_background(&draw_rect);
                self.draw_bitmap(&bitmap, &draw_rect);
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

    /// RGBA画像データからD2Dビットマップを作成
    /// image crateはRGBA、D2DはBGRA前提なのでチャネル入れ替え + premultiplied alpha変換が必要
    unsafe fn create_bitmap_from_image(&self, image: &DecodedImage) -> Result<ID2D1Bitmap> {
        // RGBA → premultiplied BGRA変換
        let mut bgra_data = image.data.clone();
        for pixel in bgra_data.chunks_exact_mut(4) {
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
                    Some(bgra_data.as_ptr() as *const _),
                    pitch,
                    &bitmap_props,
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
                        self.render_target.FillRectangle(&dest, &brush);
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
                        self.render_target.FillRectangle(&dest, &brush);
                    }
                }
                AlphaBackground::Checker => {
                    self.ensure_checker_brush();
                    if let Some(ref brush) = self.checker_brush {
                        self.render_target.FillRectangle(&dest, brush);
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
                    Some(pixels.as_ptr() as *const _),
                    TILE_SIZE * 4,
                    &bitmap_props,
                )
                .context("チェッカータイルビットマップ作成失敗")?;

            let brush_props = D2D1_BITMAP_BRUSH_PROPERTIES {
                extendModeX: D2D1_EXTEND_MODE_WRAP,
                extendModeY: D2D1_EXTEND_MODE_WRAP,
                interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
            };
            self.render_target
                .CreateBitmapBrush(&tile_bitmap, Some(&brush_props), None)
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
                Some(&dest),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
        }
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

    /// αチャネル背景を設定
    #[allow(dead_code)]
    pub fn set_alpha_background(&mut self, bg: AlphaBackground) {
        self.alpha_bg = bg;
    }

    /// 描画領域の左オフセットを設定（ファイルリストパネル幅）
    pub fn set_draw_offset(&mut self, offset_x: f32) {
        self.draw_offset_x = offset_x;
    }
}
