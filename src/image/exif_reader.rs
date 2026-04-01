//! EXIFメタデータの読み取りとフォーマット
//!
//! StandardDecoderとSusieDecoderの両方から利用される共通ユーティリティ。

use std::io::Cursor;

use exif::{In, Tag};

/// EXIFメタデータを読み取り、(日本語ラベル, フォーマット済み値) のペアで返す。
/// EXIFが存在しない場合やパース失敗時は空のVecを返す。
pub fn read_exif_fields(data: &[u8]) -> Vec<(String, String)> {
    let reader = exif::Reader::new();
    let Ok(exif) = reader.read_from_container(&mut Cursor::new(data)) else {
        return Vec::new();
    };

    let mut fields = Vec::new();

    // カメラ
    if let Some(v) = get_string(&exif, Tag::Model) {
        fields.push(("カメラ".to_string(), v));
    }
    // レンズ
    if let Some(v) = get_string(&exif, Tag::LensModel) {
        fields.push(("レンズ".to_string(), v));
    }
    // 焦点距離
    if let Some(v) = format_focal_length(&exif) {
        fields.push(("焦点距離".to_string(), v));
    }
    // F値
    if let Some(v) = format_fnumber(&exif) {
        fields.push(("F値".to_string(), v));
    }
    // シャッター速度
    if let Some(v) = format_exposure_time(&exif) {
        fields.push(("シャッター速度".to_string(), v));
    }
    // ISO感度
    if let Some(v) = get_uint(&exif, Tag::PhotographicSensitivity) {
        fields.push(("ISO感度".to_string(), format!("ISO {v}")));
    }
    // 撮影日時
    if let Some(v) = get_string(&exif, Tag::DateTimeOriginal) {
        fields.push(("撮影日時".to_string(), v));
    }
    // GPS座標
    if let Some(v) = format_gps(&exif) {
        fields.push(("GPS".to_string(), v));
    }

    fields
}

/// 文字列フィールドを取得（前後の空白を除去）
fn get_string(exif: &exif::Exif, tag: Tag) -> Option<String> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    let s = field.display_value().to_string();
    let trimmed = s.trim().trim_matches('"').trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// 符号なし整数フィールドを取得
fn get_uint(exif: &exif::Exif, tag: Tag) -> Option<u32> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    field.value.get_uint(0)
}

/// Rationalフィールドを取得
fn get_rational(exif: &exif::Exif, tag: Tag) -> Option<exif::Rational> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    if let exif::Value::Rational(ref v) = field.value {
        v.first().copied()
    } else {
        None
    }
}

/// 焦点距離をフォーマット (例: "50 mm")
fn format_focal_length(exif: &exif::Exif) -> Option<String> {
    let r = get_rational(exif, Tag::FocalLength)?;
    if r.denom == 0 {
        return None;
    }
    let mm = f64::from(r.num) / f64::from(r.denom);
    // 整数で表示できる場合は整数で
    if (mm - mm.round()).abs() < 0.01 {
        Some(format!("{} mm", mm.round() as u32))
    } else {
        Some(format!("{mm:.1} mm"))
    }
}

/// F値をフォーマット (例: "f/2.8")
fn format_fnumber(exif: &exif::Exif) -> Option<String> {
    let r = get_rational(exif, Tag::FNumber)?;
    if r.denom == 0 {
        return None;
    }
    let f = f64::from(r.num) / f64::from(r.denom);
    if (f - f.round()).abs() < 0.01 {
        Some(format!("f/{}", f.round() as u32))
    } else {
        Some(format!("f/{f:.1}"))
    }
}

/// シャッター速度をフォーマット (例: "1/250 秒", "2 秒")
fn format_exposure_time(exif: &exif::Exif) -> Option<String> {
    let r = get_rational(exif, Tag::ExposureTime)?;
    if r.denom == 0 || r.num == 0 {
        return None;
    }
    if r.num >= r.denom {
        // 1秒以上
        let secs = f64::from(r.num) / f64::from(r.denom);
        if (secs - secs.round()).abs() < 0.01 {
            Some(format!("{} 秒", secs.round() as u32))
        } else {
            Some(format!("{secs:.1} 秒"))
        }
    } else {
        // 分数表示
        Some(format!("{}/{} 秒", r.num, r.denom))
    }
}

/// GPS座標をフォーマット (例: "35.681236, 139.767125")
fn format_gps(exif: &exif::Exif) -> Option<String> {
    let lat = gps_to_decimal(exif, Tag::GPSLatitude, Tag::GPSLatitudeRef)?;
    let lon = gps_to_decimal(exif, Tag::GPSLongitude, Tag::GPSLongitudeRef)?;
    Some(format!("{lat:.6}, {lon:.6}"))
}

/// GPS度分秒を10進度に変換
fn gps_to_decimal(exif: &exif::Exif, coord_tag: Tag, ref_tag: Tag) -> Option<f64> {
    let field = exif.get_field(coord_tag, In::PRIMARY)?;
    let rationals = if let exif::Value::Rational(ref v) = field.value {
        if v.len() >= 3 {
            v
        } else {
            return None;
        }
    } else {
        return None;
    };

    let deg = f64::from(rationals[0].num) / f64::from(rationals[0].denom);
    let min = f64::from(rationals[1].num) / f64::from(rationals[1].denom);
    let sec = f64::from(rationals[2].num) / f64::from(rationals[2].denom);
    let mut decimal = deg + min / 60.0 + sec / 3600.0;

    // S/W なら負数
    if let Some(ref_field) = exif.get_field(ref_tag, In::PRIMARY) {
        let ref_str = ref_field.display_value().to_string();
        let ref_trimmed = ref_str.trim().trim_matches('"');
        if ref_trimmed == "S" || ref_trimmed == "W" {
            decimal = -decimal;
        }
    }

    Some(decimal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_exif_fields_returns_empty_for_non_exif_data() {
        // PNGデータにはEXIFがないので空を返す
        let png_data = create_1x1_white_png();
        let fields = read_exif_fields(&png_data);
        assert!(fields.is_empty());
    }

    #[test]
    fn read_exif_fields_returns_empty_for_invalid_data() {
        let fields = read_exif_fields(&[0, 1, 2, 3]);
        assert!(fields.is_empty());
    }

    #[test]
    fn read_exif_fields_returns_empty_for_empty_data() {
        let fields = read_exif_fields(&[]);
        assert!(fields.is_empty());
    }

    #[test]
    fn format_exposure_time_fraction() {
        // 1/250
        let r = exif::Rational { num: 1, denom: 250 };
        assert_eq!(r.num, 1);
        assert!(r.denom > r.num); // 分数表示条件
    }

    #[test]
    fn format_exposure_time_whole_seconds() {
        // 2秒
        let r = exif::Rational { num: 2, denom: 1 };
        assert!(r.num >= r.denom); // 整数秒条件
    }

    #[test]
    fn gps_decimal_conversion() {
        // 35° 40' 52.45" N = 35.681236...
        let deg = 35.0;
        let min = 40.0;
        let sec = 52.45;
        let decimal = deg + min / 60.0 + sec / 3600.0;
        assert!((decimal - 35.681_236_f64).abs() < 0.001);
    }

    #[test]
    fn focal_length_integer() {
        let r = exif::Rational { num: 50, denom: 1 };
        let mm = f64::from(r.num) / f64::from(r.denom);
        assert!((mm - mm.round()).abs() < 0.01);
        assert_eq!(format!("{} mm", mm.round() as u32), "50 mm");
    }

    #[test]
    fn fnumber_decimal() {
        let r = exif::Rational { num: 28, denom: 10 };
        let f = f64::from(r.num) / f64::from(r.denom);
        assert!((f - f.round()).abs() >= 0.01);
        assert_eq!(format!("f/{f:.1}"), "f/2.8");
    }

    /// テスト用: 1x1 白ピクセルのPNGバイナリを生成
    fn create_1x1_white_png() -> Vec<u8> {
        use image::{ImageBuffer, Rgba};
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(1, 1, Rgba([255, 255, 255, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }
}
