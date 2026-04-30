//! Windows エクスプローラー互換の自然順比較。
//!
//! `shlwapi.dll` の `StrCmpLogicalW` を呼び出し、エクスプローラー表示と事実上同じ並びを得る。
//! クロスプラットフォーム実装 (例: `natord`) は先頭ゼロ付き数値の扱いがエクスプローラーと
//! 乖離するため使わない。例えば `018, 19, 020` の並びは、`natord` では `018, 020, 19` と
//! なるが、`StrCmpLogicalW` では `018, 19, 020` となりエクスプローラーと一致する。
//!
//! `StrCmpLogicalW` は仕様上ケースインセンシティブのため、大小文字無視の自然順比較として
//! そのまま利用できる。

use std::cmp::Ordering;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::Win32::UI::Shell::StrCmpLogicalW;
use windows::core::PCWSTR;

/// エクスプローラー互換の自然順比較を行う。
///
/// 文字列をヌル終端 UTF-16 列に変換してから `StrCmpLogicalW` を呼ぶ。
pub(super) fn compare_explorer(a: &str, b: &str) -> Ordering {
    let wa = to_wide_null(a);
    let wb = to_wide_null(b);
    // SAFETY: `wa` / `wb` はいずれも末尾 `0` 付きの有効な UTF-16 バッファで、
    // 関数呼び出し中は所有権を保持しているため、`PCWSTR` の指す先は live である。
    // `StrCmpLogicalW` は読み取りのみで副作用を持たない。
    let result = unsafe { StrCmpLogicalW(PCWSTR(wa.as_ptr()), PCWSTR(wb.as_ptr())) };
    result.cmp(&0)
}

/// `&str` を末尾ヌル付きの UTF-16 バッファに変換する。
fn to_wide_null(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(input: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = input.iter().map(|s| (*s).to_string()).collect();
        v.sort_by(|a, b| compare_explorer(a, b));
        v
    }

    #[test]
    fn leading_zero_user_scenario() {
        // ユーザー提示シナリオ: 19 が 100 の方に飛ばず、018 と 020 の間に来る
        let result = sorted(&["018.jpg", "19.jpg", "020.jpg"]);
        assert_eq!(result, vec!["018.jpg", "19.jpg", "020.jpg"]);
    }

    #[test]
    fn leading_zero_with_three_digit_neighbor() {
        // 旧 natord では 19 が 100 の手前に来るが、エクスプローラーでは 100 より前に来る
        let result = sorted(&["019.jpg", "020.jpg", "099.jpg", "19.jpg", "100.jpg"]);
        assert_eq!(
            result,
            vec!["019.jpg", "19.jpg", "020.jpg", "099.jpg", "100.jpg"]
        );
    }

    #[test]
    fn case_insensitive_mixed() {
        let result = sorted(&["IMG1.png", "img2.png", "Img10.png"]);
        assert_eq!(result, vec!["IMG1.png", "img2.png", "Img10.png"]);
    }

    #[test]
    fn empty_string_compares_less() {
        assert_eq!(compare_explorer("", "a"), Ordering::Less);
        assert_eq!(compare_explorer("a", ""), Ordering::Greater);
        assert_eq!(compare_explorer("", ""), Ordering::Equal);
    }

    #[test]
    fn alphanumeric_mixed() {
        let result = sorted(&["a10", "a2", "a1"]);
        assert_eq!(result, vec!["a1", "a2", "a10"]);
    }

    #[test]
    fn path_separator_mixed() {
        let result = sorted(&["foo/bar10", "foo/bar2", "foo/bar1"]);
        assert_eq!(result, vec!["foo/bar1", "foo/bar2", "foo/bar10"]);
    }

    #[test]
    fn pure_numeric_simple_order() {
        let result = sorted(&["10", "2", "1"]);
        assert_eq!(result, vec!["1", "2", "10"]);
    }
}
