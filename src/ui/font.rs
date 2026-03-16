//! 等幅フォント管理
//!
//! ダイアログ・ファイルリスト等で使用する等幅フォントを作成・管理する。

use windows::Win32::Graphics::Gdi::{
    CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, DeleteObject, FF_MODERN, FIXED_PITCH, HFONT,
    OUT_DEFAULT_PRECIS, PROOF_QUALITY,
};

/// 等幅フォント（RAII管理）
pub struct MonospaceFont {
    hfont: HFONT,
}

impl MonospaceFont {
    /// 指定サイズで等幅フォントを作成する
    /// Consolasを優先し、存在しなければOSがFF_MODERN | FIXED_PITCHでフォールバックする
    pub fn new(size: i32) -> Self {
        let hfont = unsafe {
            CreateFontW(
                size,
                0,
                0,
                0,
                400, // FW_NORMAL
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                PROOF_QUALITY,
                (FF_MODERN.0 | FIXED_PITCH.0) as u32,
                windows::core::w!("Consolas"),
            )
        };
        Self { hfont }
    }

    pub fn hfont(&self) -> HFONT {
        self.hfont
    }
}

impl Drop for MonospaceFont {
    fn drop(&mut self) {
        if !self.hfont.is_invalid() {
            unsafe {
                let _ = DeleteObject(self.hfont.into());
            }
        }
    }
}
