use std::collections::HashMap;
use std::path::Path;

use anyhow::{Result, bail};

/// 配布版のデフォルトキーバインドTOML。
/// ビルド時に取り込み、`with_defaults()` のソースとして使う。
/// パース不能になる事故はビルド時テスト `default_toml_parses_and_resolves` で防止する。
const DEFAULT_KEYS_TOML: &str = include_str!("../../ぐらびゅ.keys.default.toml");

// --- 入力表現 ---

/// 修飾キーの組み合わせ
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// ホイール方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WheelDirection {
    Up,
    Down,
}

/// マウスボタン操作
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    LeftDoubleClick,
    MiddleClick,
}

/// 入力イベント (キー/ホイール/マウスを統一的に扱う)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputChord {
    Key {
        vk: u16,
        modifiers: Modifiers,
    },
    Wheel {
        direction: WheelDirection,
        modifiers: Modifiers,
    },
    Mouse {
        button: MouseButton,
    },
}

// --- Action enum ---

/// 全操作を列挙するenum
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::EnumIter)]
pub enum Action {
    // --- ナビゲーション ---
    NavigateBack,
    NavigateForward,
    Navigate5Back,
    Navigate5Forward,
    Navigate50Back,
    Navigate50Forward,
    NavigateFirst,
    NavigateLast,
    NavigatePrevFolder,
    NavigateNextFolder,
    NavigatePrevMark,
    NavigateNextMark,
    NavigateToPage,
    SortNavigateBack,
    SortNavigateForward,
    ShuffleAll,
    ShuffleGroups,

    // --- 表示モード ---
    DisplayAutoShrink,
    DisplayAutoFit,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ToggleMargin,
    CycleAlphaBackground,

    // --- ウィンドウ ---
    ToggleFullscreen,
    Minimize,
    ToggleMaximize,
    ToggleAlwaysOnTop,
    ToggleCursorHide,
    ToggleMenuBar,

    // --- ファイル操作 ---
    NewWindow,
    OpenFile,
    OpenFolder,
    CloseAll,
    Reload,
    RemoveFromList,
    DeleteFile,
    MoveFile,
    CopyFile,
    OpenContainingFolder,
    CopyFileName,
    CopyImage,
    PasteImage,
    ExportJpg,
    ExportBmp,
    ExportPng,
    ShowImageInfo,

    // --- マーク操作 ---
    MarkSet,
    MarkUnset,
    MarkInvertAll,
    MarkInvertToHere,
    MarkedRemoveFromList,
    MarkedDelete,
    MarkedMove,
    MarkedCopy,
    MarkedCopyNames,

    // --- 編集 ---
    DeselectSelection,
    Crop,
    FlipHorizontal,
    FlipVertical,
    Rotate180,
    Rotate90CW,
    Rotate90CCW,
    RotateArbitrary,
    Resize,

    // --- フィルタ (画像メニュー) ---
    Fill,
    Levels,
    Gamma,
    BrightnessContrast,
    Mosaic,
    GaussianBlur,
    UnsharpMask,
    InvertColors,
    GrayscaleSimple,
    GrayscaleStrict,
    ApplyAlpha,
    Blur,
    BlurStrong,
    Sharpen,
    SharpenStrong,
    MedianFilter,

    // --- 永続フィルタ ---
    PFilterToggle,
    PFilterFlipH,
    PFilterFlipV,
    PFilterRotate180,
    PFilterRotate90CW,
    PFilterRotate90CCW,
    PFilterLevels,
    PFilterGamma,
    PFilterBrightnessContrast,
    PFilterGrayscaleSimple,
    PFilterGrayscaleStrict,
    PFilterBlur,
    PFilterBlurStrong,
    PFilterSharpen,
    PFilterSharpenStrong,
    PFilterGaussianBlur,
    PFilterUnsharpMask,
    PFilterMedianFilter,
    PFilterInvertColors,
    PFilterApplyAlpha,

    // --- ブックマーク ---
    BookmarkSave,
    BookmarkLoad,

    // --- ファイルリスト ---
    ToggleFileList,

    // --- ユーティリティ ---
    OpenExeFolder,
    OpenBookmarkFolder,
    OpenSpiFolder,
    OpenTempFolder,
    ShowHelp,
    CheckUpdate,
    OpenHomepage,
    RegisterShell,
    UnregisterShell,
    Exit,

    // --- スライドショー ---
    SlideshowToggle,
    SlideshowFaster,
    SlideshowSlower,
}

// --- KeyConfig ---

/// キーバインド設定
pub struct KeyConfig {
    bindings: HashMap<InputChord, Action>,
}

impl KeyConfig {
    /// 設定ファイルからロード。ファイルがなければデフォルトを使用。
    pub fn load(path: Option<&Path>) -> Self {
        if let Some(p) = path
            && let Ok(content) = std::fs::read_to_string(p)
        {
            match Self::parse_toml(&content) {
                Ok(config) => return config,
                Err(e) => {
                    eprintln!("キーバインド設定のパースに失敗 ({e})。デフォルトを使用する。");
                }
            }
        }
        Self::with_defaults()
    }

    /// 配布TOMLから取得したデフォルトバインディングで初期化。
    /// 配布TOMLのパース失敗はビルド時の単体テストで防止しているため、
    /// ここでは必ず成功する前提で `expect` する。
    pub fn with_defaults() -> Self {
        Self::parse_toml(DEFAULT_KEYS_TOML)
            .expect("ぐらびゅ.keys.default.tomlのパースに失敗 (配布物の不整合)")
    }

    /// 入力からアクションを検索
    pub fn lookup(&self, chord: InputChord) -> Option<Action> {
        self.bindings.get(&chord).copied()
    }

    /// TOMLテキストからパース。
    /// セクション名を識別し、`[persistent_filter]` 配下のフィールドを `PFilter*` アクションへ解決する。
    /// 同名フィールド (`levels` 等) が `[filter]` と `[persistent_filter]` で別アクションへ
    /// 振り分けられるよう、セクション認識を必須とする。
    fn parse_toml(content: &str) -> Result<Self> {
        let table: toml::Table = content.parse()?;
        let mut bindings = HashMap::new();

        for (section, value) in &table {
            let Some(section_table) = value.as_table() else {
                continue;
            };
            for (field, val) in section_table {
                let Some(action) = resolve_action(section, field) else {
                    continue;
                };
                let Some(key_str) = val.as_str() else {
                    continue;
                };
                // カンマ区切りで複数キーを登録
                for part in key_str.split(',') {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    match parse_chord(part) {
                        Ok(chord) => {
                            bindings.insert(chord, action);
                        }
                        Err(e) => {
                            eprintln!(
                                "キーバインドパースエラー ([{section}].{field} = {part:?}): {e}"
                            );
                        }
                    }
                }
            }
        }

        Ok(Self { bindings })
    }
}

/// (セクション, フィールド) からアクションを解決する。
/// `[persistent_filter]` セクションのフィールド名は永続フィルタ系アクションへ解決し、
/// 他セクションでは `field_to_action` の標準マッピングへ委ねる。
fn resolve_action(section: &str, field: &str) -> Option<Action> {
    if section == "persistent_filter" {
        // `[persistent_filter].levels` → `Action::PFilterLevels` のように
        // `pfilter_` プレフィックスを補ってから標準マッピングを引く。
        // すでに `pfilter_` プレフィックス付きのフィールド名が書かれている場合は二重付与を避ける。
        let canonical = if field.starts_with("pfilter_") {
            field.to_string()
        } else {
            format!("pfilter_{field}")
        };
        return field_to_action(&canonical);
    }
    field_to_action(field)
}

// --- キー名パーサー ---

/// キー文字列を解析してInputChordに変換
pub fn parse_chord(s: &str) -> Result<InputChord> {
    let s = s.trim();

    // マウス操作
    match s {
        "LeftDoubleClick" => {
            return Ok(InputChord::Mouse {
                button: MouseButton::LeftDoubleClick,
            });
        }
        "MiddleClick" => {
            return Ok(InputChord::Mouse {
                button: MouseButton::MiddleClick,
            });
        }
        _ => {}
    }

    // 修飾子 + キーを分離
    let mut modifiers = Modifiers::default();
    let mut remaining = s;

    // "Ctrl+Shift+←" → 最後の要素がキー名
    loop {
        if let Some(rest) = remaining.strip_prefix("Ctrl+") {
            modifiers.ctrl = true;
            remaining = rest;
        } else if let Some(rest) = remaining.strip_prefix("Shift+") {
            modifiers.shift = true;
            remaining = rest;
        } else if let Some(rest) = remaining.strip_prefix("Alt+") {
            modifiers.alt = true;
            remaining = rest;
        } else {
            break;
        }
    }

    // ホイール操作
    match remaining {
        "WheelUp" | "ホイール(上)" => {
            return Ok(InputChord::Wheel {
                direction: WheelDirection::Up,
                modifiers,
            });
        }
        "WheelDown" | "ホイール(下)" => {
            return Ok(InputChord::Wheel {
                direction: WheelDirection::Down,
                modifiers,
            });
        }
        _ => {}
    }

    // キー名 → 仮想キーコード
    let vk = key_name_to_vk(remaining)?;
    Ok(InputChord::Key { vk, modifiers })
}

/// キー名から仮想キーコードを返す
fn key_name_to_vk(name: &str) -> Result<u16> {
    // 単一の英字・数字
    if name.len() == 1 {
        // バイト長1の文字列なので chars().next() は必ず Some を返す。
        let ch = name
            .chars()
            .next()
            .expect("name.len() == 1 なら chars() は1要素を返す");
        if ch.is_ascii_uppercase() || ch.is_ascii_digit() {
            return Ok(ch as u16);
        }
    }

    // テーブルから検索
    for &(key_name, vk) in KEY_NAMES {
        if key_name.eq_ignore_ascii_case(name) {
            return Ok(vk);
        }
    }

    bail!("不明なキー名: {name:?}");
}

/// キー名 → VKコード マッピングテーブル
const KEY_NAMES: &[(&str, u16)] = &[
    // 矢印キー
    ("←", 0x25),
    ("→", 0x27),
    ("↑", 0x26),
    ("↓", 0x28),
    ("Left", 0x25),
    ("Right", 0x27),
    ("Up", 0x26),
    ("Down", 0x28),
    // ページ
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("Home", 0x24),
    ("End", 0x23),
    // 特殊キー
    ("Enter", 0x0D),
    ("Return", 0x0D),
    ("Space", 0x20),
    ("Tab", 0x09),
    ("Esc", 0x1B),
    ("Escape", 0x1B),
    ("BackSpace", 0x08),
    ("Delete", 0x2E),
    // ファンクションキー
    ("F1", 0x70),
    ("F2", 0x71),
    ("F3", 0x72),
    ("F4", 0x73),
    ("F5", 0x74),
    ("F6", 0x75),
    ("F7", 0x76),
    ("F8", 0x77),
    ("F9", 0x78),
    ("F10", 0x79),
    ("F11", 0x7A),
    ("F12", 0x7B),
    // テンキー
    ("Num0", 0x60),
    ("Num1", 0x61),
    ("Num2", 0x62),
    ("Num3", 0x63),
    ("Num4", 0x64),
    ("Num5", 0x65),
    ("Num6", 0x66),
    ("Num7", 0x67),
    ("Num8", 0x68),
    ("Num9", 0x69),
    ("Num +", 0x6B),
    ("Num -", 0x6D),
    ("Num *", 0x6A),
    ("Num /", 0x6F),
];

/// TOMLフィールド名 → Action のマッピング
fn field_to_action(field: &str) -> Option<Action> {
    Some(match field {
        // ナビゲーション
        // "page_back"/"navigate_1_back" 等は既存設定ファイルとの互換性のためNavigateBackにマップ
        "back" | "navigate_back" | "page_back" | "navigate_1_back" => Action::NavigateBack,
        "forward" | "navigate_forward" | "page_forward" | "navigate_1_forward" => {
            Action::NavigateForward
        }
        "navigate_5_back" => Action::Navigate5Back,
        "navigate_5_forward" => Action::Navigate5Forward,
        "navigate_50_back" => Action::Navigate50Back,
        "navigate_50_forward" => Action::Navigate50Forward,
        "navigate_first" => Action::NavigateFirst,
        "navigate_last" => Action::NavigateLast,
        "navigate_prev_folder" => Action::NavigatePrevFolder,
        "navigate_next_folder" => Action::NavigateNextFolder,
        "navigate_prev_mark" => Action::NavigatePrevMark,
        "navigate_next_mark" => Action::NavigateNextMark,
        "navigate_to_page" => Action::NavigateToPage,
        "sort_navigate_back" => Action::SortNavigateBack,
        "sort_navigate_forward" => Action::SortNavigateForward,
        "shuffle_all" => Action::ShuffleAll,
        "shuffle_groups" => Action::ShuffleGroups,

        // 表示
        "auto_shrink" => Action::DisplayAutoShrink,
        "auto_fit" => Action::DisplayAutoFit,
        "zoom_in" => Action::ZoomIn,
        "zoom_out" => Action::ZoomOut,
        "zoom_reset" => Action::ZoomReset,
        "toggle_margin" => Action::ToggleMargin,
        "cycle_alpha_background" => Action::CycleAlphaBackground,

        // ウィンドウ
        "toggle_fullscreen" => Action::ToggleFullscreen,
        "minimize" => Action::Minimize,
        "toggle_maximize" => Action::ToggleMaximize,
        "toggle_always_on_top" => Action::ToggleAlwaysOnTop,
        "toggle_cursor_hide" => Action::ToggleCursorHide,
        "toggle_menu_bar" => Action::ToggleMenuBar,

        // ファイル操作
        "new_window" => Action::NewWindow,
        "open_file" => Action::OpenFile,
        "open_folder" => Action::OpenFolder,
        "close_all" => Action::CloseAll,
        "reload" => Action::Reload,
        "remove_from_list" => Action::RemoveFromList,
        "delete_file" => Action::DeleteFile,
        "move_file" => Action::MoveFile,
        "copy_file" => Action::CopyFile,
        "open_containing_folder" => Action::OpenContainingFolder,
        "copy_filename" => Action::CopyFileName,
        "copy_image" => Action::CopyImage,
        "paste_image" => Action::PasteImage,
        "export_jpg" => Action::ExportJpg,
        "export_bmp" => Action::ExportBmp,
        "export_png" => Action::ExportPng,
        "show_image_info" => Action::ShowImageInfo,

        // マーク
        "mark_set" | "set" => Action::MarkSet,
        "mark_unset" | "unset" => Action::MarkUnset,
        "mark_invert_all" | "invert_all" => Action::MarkInvertAll,
        "mark_invert_to_here" | "invert_to_here" => Action::MarkInvertToHere,
        "marked_remove_from_list" => Action::MarkedRemoveFromList,
        "marked_delete" => Action::MarkedDelete,
        "marked_move" => Action::MarkedMove,
        "marked_copy" => Action::MarkedCopy,
        "marked_copy_names" => Action::MarkedCopyNames,

        // 編集
        "deselect_selection" => Action::DeselectSelection,
        "crop" => Action::Crop,
        "flip_horizontal" => Action::FlipHorizontal,
        "flip_vertical" => Action::FlipVertical,
        "rotate_180" => Action::Rotate180,
        "rotate_90_cw" => Action::Rotate90CW,
        "rotate_90_ccw" => Action::Rotate90CCW,
        "rotate_arbitrary" => Action::RotateArbitrary,
        "resize" => Action::Resize,

        // フィルタ
        "fill" => Action::Fill,
        "levels" => Action::Levels,
        "gamma" => Action::Gamma,
        "brightness_contrast" => Action::BrightnessContrast,
        "mosaic" => Action::Mosaic,
        "gaussian_blur" => Action::GaussianBlur,
        "unsharp_mask" => Action::UnsharpMask,
        "invert_colors" => Action::InvertColors,
        "grayscale_simple" => Action::GrayscaleSimple,
        "grayscale_strict" => Action::GrayscaleStrict,
        "apply_alpha" => Action::ApplyAlpha,
        "blur" => Action::Blur,
        "blur_strong" => Action::BlurStrong,
        "sharpen" => Action::Sharpen,
        "sharpen_strong" => Action::SharpenStrong,
        "median_filter" => Action::MedianFilter,

        // 永続フィルタ
        "pfilter_toggle" => Action::PFilterToggle,
        "pfilter_flip_h" => Action::PFilterFlipH,
        "pfilter_flip_v" => Action::PFilterFlipV,
        "pfilter_rotate_180" => Action::PFilterRotate180,
        "pfilter_rotate_90_cw" => Action::PFilterRotate90CW,
        "pfilter_rotate_90_ccw" => Action::PFilterRotate90CCW,
        "pfilter_levels" => Action::PFilterLevels,
        "pfilter_gamma" => Action::PFilterGamma,
        "pfilter_brightness_contrast" => Action::PFilterBrightnessContrast,
        "pfilter_grayscale_simple" => Action::PFilterGrayscaleSimple,
        "pfilter_grayscale_strict" => Action::PFilterGrayscaleStrict,
        "pfilter_blur" => Action::PFilterBlur,
        "pfilter_blur_strong" => Action::PFilterBlurStrong,
        "pfilter_sharpen" => Action::PFilterSharpen,
        "pfilter_sharpen_strong" => Action::PFilterSharpenStrong,
        "pfilter_gaussian_blur" => Action::PFilterGaussianBlur,
        "pfilter_unsharp_mask" => Action::PFilterUnsharpMask,
        "pfilter_median_filter" => Action::PFilterMedianFilter,
        "pfilter_invert_colors" => Action::PFilterInvertColors,
        "pfilter_apply_alpha" => Action::PFilterApplyAlpha,

        // ブックマーク
        "bookmark_save" => Action::BookmarkSave,
        "bookmark_load" => Action::BookmarkLoad,

        // ファイルリスト
        "toggle_file_list" => Action::ToggleFileList,

        // ユーティリティ
        "open_exe_folder" => Action::OpenExeFolder,
        "open_bookmark_folder" => Action::OpenBookmarkFolder,
        "open_spi_folder" => Action::OpenSpiFolder,
        "open_temp_folder" => Action::OpenTempFolder,
        "show_help" => Action::ShowHelp,
        "check_update" => Action::CheckUpdate,
        "open_homepage" => Action::OpenHomepage,
        "register_shell" => Action::RegisterShell,
        "unregister_shell" => Action::UnregisterShell,
        "exit" => Action::Exit,

        // スライドショー
        "slideshow_toggle" => Action::SlideshowToggle,
        "slideshow_faster" => Action::SlideshowFaster,
        "slideshow_slower" => Action::SlideshowSlower,

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_key() {
        let chord = parse_chord("←").unwrap();
        assert_eq!(
            chord,
            InputChord::Key {
                vk: 0x25,
                modifiers: Modifiers::default()
            }
        );
    }

    #[test]
    fn parse_ctrl_modifier() {
        let chord = parse_chord("Ctrl+Home").unwrap();
        assert_eq!(
            chord,
            InputChord::Key {
                vk: 0x24,
                modifiers: Modifiers {
                    ctrl: true,
                    shift: false,
                    alt: false
                }
            }
        );
    }

    #[test]
    fn parse_ctrl_shift() {
        let chord = parse_chord("Ctrl+Shift+←").unwrap();
        assert_eq!(
            chord,
            InputChord::Key {
                vk: 0x25,
                modifiers: Modifiers {
                    ctrl: true,
                    shift: true,
                    alt: false
                }
            }
        );
    }

    #[test]
    fn parse_alt_enter() {
        let chord = parse_chord("Alt+Enter").unwrap();
        assert_eq!(
            chord,
            InputChord::Key {
                vk: 0x0D,
                modifiers: Modifiers {
                    ctrl: false,
                    shift: false,
                    alt: true
                }
            }
        );
    }

    #[test]
    fn parse_function_keys() {
        let chord = parse_chord("F5").unwrap();
        assert_eq!(
            chord,
            InputChord::Key {
                vk: 0x74,
                modifiers: Modifiers::default()
            }
        );
    }

    #[test]
    fn parse_numpad_keys() {
        let chord = parse_chord("Num0").unwrap();
        assert_eq!(
            chord,
            InputChord::Key {
                vk: 0x60,
                modifiers: Modifiers::default()
            }
        );

        let chord = parse_chord("Num +").unwrap();
        assert_eq!(
            chord,
            InputChord::Key {
                vk: 0x6B,
                modifiers: Modifiers::default()
            }
        );
    }

    #[test]
    fn parse_alpha_key() {
        let chord = parse_chord("A").unwrap();
        assert_eq!(
            chord,
            InputChord::Key {
                vk: 0x41,
                modifiers: Modifiers::default()
            }
        );
    }

    #[test]
    fn parse_wheel() {
        let chord = parse_chord("WheelUp").unwrap();
        assert_eq!(
            chord,
            InputChord::Wheel {
                direction: WheelDirection::Up,
                modifiers: Modifiers::default()
            }
        );

        let chord = parse_chord("Ctrl+WheelDown").unwrap();
        assert_eq!(
            chord,
            InputChord::Wheel {
                direction: WheelDirection::Down,
                modifiers: Modifiers {
                    ctrl: true,
                    shift: false,
                    alt: false
                }
            }
        );
    }

    #[test]
    fn parse_mouse() {
        let chord = parse_chord("LeftDoubleClick").unwrap();
        assert_eq!(
            chord,
            InputChord::Mouse {
                button: MouseButton::LeftDoubleClick
            }
        );

        let chord = parse_chord("MiddleClick").unwrap();
        assert_eq!(
            chord,
            InputChord::Mouse {
                button: MouseButton::MiddleClick
            }
        );
    }

    #[test]
    fn parse_unknown_key_returns_error() {
        assert!(parse_chord("UnknownKey").is_err());
    }

    #[test]
    fn default_bindings_complete() {
        let config = KeyConfig::with_defaults();

        // ナビゲーション基本
        assert_eq!(
            config.lookup(parse_chord("←").unwrap()),
            Some(Action::NavigateBack)
        );
        assert_eq!(
            config.lookup(parse_chord("→").unwrap()),
            Some(Action::NavigateForward)
        );
        assert_eq!(
            config.lookup(parse_chord("PageUp").unwrap()),
            Some(Action::Navigate5Back)
        );
        assert_eq!(
            config.lookup(parse_chord("Ctrl+Home").unwrap()),
            Some(Action::NavigateFirst)
        );

        // 表示モード
        assert_eq!(
            config.lookup(parse_chord("Num /").unwrap()),
            Some(Action::DisplayAutoShrink)
        );
        assert_eq!(
            config.lookup(parse_chord("A").unwrap()),
            Some(Action::CycleAlphaBackground)
        );

        // ウィンドウ
        assert_eq!(
            config.lookup(parse_chord("Alt+Enter").unwrap()),
            Some(Action::ToggleFullscreen)
        );

        // ファイル操作
        assert_eq!(
            config.lookup(parse_chord("Ctrl+O").unwrap()),
            Some(Action::OpenFile)
        );

        // マーク
        assert_eq!(
            config.lookup(parse_chord("Delete").unwrap()),
            Some(Action::MarkSet)
        );

        // マウス
        assert_eq!(
            config.lookup(parse_chord("LeftDoubleClick").unwrap()),
            Some(Action::ToggleMaximize)
        );
        assert_eq!(
            config.lookup(parse_chord("MiddleClick").unwrap()),
            Some(Action::ShowImageInfo)
        );
    }

    #[test]
    fn toml_parse() {
        let toml = r#"
[navigation]
back = "A"
forward = "Ctrl+B"

[display]
zoom_in = "Ctrl+Num +, Ctrl+WheelDown"
"#;
        let config = KeyConfig::parse_toml(toml).unwrap();
        assert_eq!(
            config.lookup(parse_chord("A").unwrap()),
            Some(Action::NavigateBack)
        );
        assert_eq!(
            config.lookup(parse_chord("Ctrl+B").unwrap()),
            Some(Action::NavigateForward)
        );
        // カンマ区切り
        assert_eq!(
            config.lookup(parse_chord("Ctrl+Num +").unwrap()),
            Some(Action::ZoomIn)
        );
        assert_eq!(
            config.lookup(parse_chord("Ctrl+WheelDown").unwrap()),
            Some(Action::ZoomIn)
        );
    }

    #[test]
    fn field_to_action_coverage() {
        // 主要なフィールド名がマッピングされていることを確認
        assert_eq!(field_to_action("back"), Some(Action::NavigateBack));
        assert_eq!(field_to_action("forward"), Some(Action::NavigateForward));
        assert_eq!(
            field_to_action("toggle_fullscreen"),
            Some(Action::ToggleFullscreen)
        );
        assert_eq!(field_to_action("mark_set"), Some(Action::MarkSet));
        assert_eq!(field_to_action("set"), Some(Action::MarkSet)); // エイリアス
        assert_eq!(field_to_action("unknown_field"), None);
    }

    /// 配布TOMLが正しくパースでき、主要アクションへ解決されることを保証する。
    /// `with_defaults()` のフォールバック経路はビルド時にこのテストで担保する。
    #[test]
    fn default_toml_parses_and_resolves() {
        let config = KeyConfig::parse_toml(DEFAULT_KEYS_TOML)
            .expect("配布版ぐらびゅ.keys.default.tomlのパースに失敗");

        // ナビゲーション
        assert_eq!(
            config.lookup(parse_chord("←").unwrap()),
            Some(Action::NavigateBack)
        );
        assert_eq!(
            config.lookup(parse_chord("Tab").unwrap()),
            Some(Action::SortNavigateForward)
        );
        // 編集
        assert_eq!(
            config.lookup(parse_chord("Ctrl+→").unwrap()),
            Some(Action::Rotate90CW)
        );
        // フィルタ
        assert_eq!(
            config.lookup(parse_chord("Ctrl+L").unwrap()),
            Some(Action::Levels)
        );
        assert_eq!(
            config.lookup(parse_chord("Ctrl+G").unwrap()),
            Some(Action::GrayscaleStrict)
        );
        // 永続フィルタ (parse_toml がセクションを認識して PFilter* に割り当てる箇所)
        assert_eq!(
            config.lookup(parse_chord("Ctrl+Shift+L").unwrap()),
            Some(Action::PFilterLevels)
        );
        assert_eq!(
            config.lookup(parse_chord("Ctrl+Shift+G").unwrap()),
            Some(Action::PFilterGrayscaleSimple)
        );
        // ブックマーク・スライドショー
        assert_eq!(
            config.lookup(parse_chord("F9").unwrap()),
            Some(Action::BookmarkSave)
        );
        assert_eq!(
            config.lookup(parse_chord("Shift+Space").unwrap()),
            Some(Action::SlideshowToggle)
        );
    }

    /// `[persistent_filter]` セクションのフィールドが `PFilter*` に解決されることを単独で確認する。
    /// 旧バグ「セクション無視で `levels` が常に `Action::Levels` へ解決された」の再発防止。
    #[test]
    fn persistent_filter_section_resolves_to_pfilter_actions() {
        let toml = r#"
[filter]
levels = "Ctrl+L"
grayscale_simple = "Ctrl+Alt+L"

[persistent_filter]
levels = "Ctrl+Shift+L"
grayscale_simple = "Ctrl+Shift+G"
"#;
        let config = KeyConfig::parse_toml(toml).unwrap();
        // [filter] 配下は通常アクション
        assert_eq!(
            config.lookup(parse_chord("Ctrl+L").unwrap()),
            Some(Action::Levels)
        );
        assert_eq!(
            config.lookup(parse_chord("Ctrl+Alt+L").unwrap()),
            Some(Action::GrayscaleSimple)
        );
        // [persistent_filter] 配下は PFilter* アクション
        assert_eq!(
            config.lookup(parse_chord("Ctrl+Shift+L").unwrap()),
            Some(Action::PFilterLevels)
        );
        assert_eq!(
            config.lookup(parse_chord("Ctrl+Shift+G").unwrap()),
            Some(Action::PFilterGrayscaleSimple)
        );
    }

    /// `[persistent_filter]` 配下にすでに `pfilter_` プレフィックス付きのフィールド名を書いた場合も
    /// 二重付与せず正しく解決することを確認する。
    #[test]
    fn persistent_filter_section_accepts_explicit_prefix() {
        let toml = r#"
[persistent_filter]
pfilter_levels = "Ctrl+Shift+L"
"#;
        let config = KeyConfig::parse_toml(toml).unwrap();
        assert_eq!(
            config.lookup(parse_chord("Ctrl+Shift+L").unwrap()),
            Some(Action::PFilterLevels)
        );
    }
}
