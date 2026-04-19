use std::path::{Path, PathBuf};

/// &str を null終端UTF-16ワイド文字列に変換する (Win32 API用)
pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `\\?\` プレフィックスを除去する (Shell API/WinRT APIが非対応のため)
/// UNCパスの場合: `\\?\UNC\server\share` → `\\server\share`
/// 通常パスの場合: `\\?\C:\path` → `C:\path`
pub fn strip_extended_length_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
        // UNCパス: \\?\UNC\server\share → \\server\share
        PathBuf::from(format!(r"\\{unc}"))
    } else if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path.to_path_buf()
    }
}

/// Win32 MessageBoxW のラッパー
///
/// # Safety
/// hwnd は有効なウィンドウハンドル、または `HWND::default()`（NULLに相当）であること。
pub unsafe fn show_message_box(
    hwnd: windows::Win32::Foundation::HWND,
    title: &str,
    message: &str,
    flags: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE,
) -> windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_RESULT {
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
            Some(hwnd),
            &windows::core::HSTRING::from(message),
            &windows::core::HSTRING::from(title),
            flags,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string() {
        let result = to_wide("");
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn ascii_string() {
        let result = to_wide("hello");
        let expected: Vec<u16> = "hello".encode_utf16().chain(std::iter::once(0)).collect();
        assert_eq!(result, expected);
    }

    #[test]
    fn japanese_string() {
        let result = to_wide("画像ビューア");
        let expected: Vec<u16> = "画像ビューア"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        assert_eq!(result, expected);
        // null終端確認
        assert_eq!(*result.last().unwrap(), 0);
    }

    #[test]
    fn strip_prefix_local_path() {
        let path = Path::new(r"\\?\C:\Users\test\image.png");
        assert_eq!(
            strip_extended_length_prefix(path),
            PathBuf::from(r"C:\Users\test\image.png")
        );
    }

    #[test]
    fn strip_prefix_unc_path() {
        let path = Path::new(r"\\?\UNC\server\share\image.png");
        assert_eq!(
            strip_extended_length_prefix(path),
            PathBuf::from(r"\\server\share\image.png")
        );
    }

    #[test]
    fn strip_prefix_no_prefix() {
        let path = Path::new(r"C:\Users\test\image.png");
        assert_eq!(
            strip_extended_length_prefix(path),
            PathBuf::from(r"C:\Users\test\image.png")
        );
    }

    #[test]
    fn strip_prefix_plain_unc() {
        let path = Path::new(r"\\server\share\image.png");
        assert_eq!(
            strip_extended_length_prefix(path),
            PathBuf::from(r"\\server\share\image.png")
        );
    }
}
