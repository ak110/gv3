//! メニューバー構築
//!
//! Win32 HMENU を構築し、メニューIDからActionへの変換を提供する。

use windows::Win32::UI::WindowsAndMessaging::*;

use super::key_config::Action;

/// メニューID の基底値
const WM_COMMAND_BASE: u16 = 0x1000;

/// メニューバーを構築して返す
pub fn build_menu_bar() -> HMENU {
    unsafe {
        let menu_bar = CreateMenu().unwrap_or_default();

        // ファイル(&F)
        let file_menu = create_popup(&[
            Some((Action::NewWindow, "新規ウィンドウ(&N)\tCtrl+N")),
            Some((Action::OpenFile, "ファイルを開く(&O)\tCtrl+O")),
            Some((Action::OpenFolder, "フォルダを開く(&F)\tCtrl+Shift+O")),
            None,
            Some((Action::CopyFile, "ファイルを複製(&S)\tCtrl+S")),
            Some((Action::MoveFile, "ファイルを移動(&R)\tCtrl+R")),
            Some((Action::DeleteFile, "ファイルを削除\tShift+Delete")),
            Some((Action::RemoveFromList, "リストから削除\tBackSpace")),
            None,
            Some((Action::Reload, "再読み込み(&L)\tF5")),
            Some((Action::CloseAll, "全て閉じる(&W)\tCtrl+W")),
            None,
            Some((Action::Exit, "終了(&X)")),
        ]);
        append_popup(menu_bar, file_menu, "ファイル(&F)");

        // マーク(&M)
        let mark_menu = create_popup(&[
            Some((Action::MarkSet, "マークを設定\tDelete")),
            Some((Action::MarkUnset, "マークを解除\tCtrl+Delete")),
            Some((Action::MarkInvertAll, "全てのマークを反転\tCtrl+Shift+I")),
            Some((Action::MarkInvertToHere, "ここまでのマークを反転")),
            None,
            Some((Action::NavigatePrevMark, "前のマーク画像へ\tCtrl+Shift+←")),
            Some((Action::NavigateNextMark, "次のマーク画像へ\tCtrl+Shift+→")),
            None,
            Some((
                Action::MarkedRemoveFromList,
                "マークをリストから削除\tCtrl+BackSpace",
            )),
            Some((
                Action::MarkedDelete,
                "マークを完全に削除\tCtrl+Shift+Delete",
            )),
            Some((Action::MarkedMove, "マークを移動\tCtrl+Shift+M")),
            Some((Action::MarkedCopy, "マークを複製\tCtrl+Shift+C")),
            Some((Action::MarkedCopyNames, "マークのファイル名をコピー")),
        ]);
        append_popup(menu_bar, mark_menu, "マーク(&M)");

        // 編集(&E)
        let edit_menu = create_popup(&[
            Some((Action::FlipHorizontal, "左右反転")),
            Some((Action::FlipVertical, "上下反転")),
            None,
            Some((Action::Rotate180, "180度回転\tCtrl+↑")),
            Some((Action::Rotate90CW, "時計回りに90度回転\tCtrl+→")),
            Some((Action::Rotate90CCW, "反時計回りに90度回転\tCtrl+←")),
            Some((Action::RotateArbitrary, "角度指定回転\tCtrl+↓")),
            None,
            Some((Action::Resize, "解像度の変更\tCtrl+Shift+R")),
        ]);
        append_popup(menu_bar, edit_menu, "編集(&E)");

        // 画像(&I)
        let image_menu = create_popup(&[
            Some((Action::DeselectSelection, "選択範囲を取り消し\tEnter")),
            Some((Action::Crop, "画像の切り抜き\tCtrl+Shift+X")),
            None,
            Some((Action::Fill, "塗り潰す\tCtrl+Shift+F")),
            Some((Action::Levels, "レベル補正\tCtrl+L")),
            Some((Action::Gamma, "ガンマ補正")),
            Some((Action::BrightnessContrast, "明るさとコントラスト")),
            Some((Action::Mosaic, "モザイク\tCtrl+M")),
            None,
            Some((Action::Blur, "ぼかし")),
            Some((Action::BlurStrong, "ぼかし (強)")),
            Some((Action::Sharpen, "シャープ")),
            Some((Action::SharpenStrong, "シャープ (強)")),
            Some((Action::GaussianBlur, "ガウスぼかし")),
            Some((Action::UnsharpMask, "アンシャープマスク")),
            Some((Action::MedianFilter, "メディアンフィルタ")),
            None,
            Some((Action::InvertColors, "色を反転\tCtrl+I")),
            Some((Action::GrayscaleSimple, "簡易グレースケール化")),
            Some((Action::GrayscaleStrict, "厳密グレースケール化\tCtrl+G")),
            Some((Action::ApplyAlpha, "αチャンネルの反映")),
            None,
            Some((Action::CopyImage, "画像をコピー\tCtrl+C")),
            Some((Action::PasteImage, "クリップボードから貼り付け\tCtrl+V")),
            Some((Action::CopyFileName, "ファイル名をコピー\tCtrl+F")),
            None,
            Some((Action::ExportJpg, "JPGとして書き出す\tCtrl+J")),
            Some((Action::ExportBmp, "BMPとして書き出す\tCtrl+B")),
            Some((Action::ExportPng, "PNGとして書き出す\tCtrl+P")),
            None,
            Some((Action::ShowImageInfo, "画像情報\tホイールクリック")),
        ]);
        append_popup(menu_bar, image_menu, "画像(&I)");

        // フィルタ(&T)
        let filter_menu = create_popup(&[
            Some((Action::PFilterToggle, "フィルタを有効にする")),
            None,
            Some((Action::PFilterFlipH, "左右反転")),
            Some((Action::PFilterFlipV, "上下反転")),
            Some((Action::PFilterRotate180, "180度回転")),
            Some((Action::PFilterRotate90CW, "時計回りに90度回転")),
            Some((Action::PFilterRotate90CCW, "反時計回りに90度回転")),
            None,
            Some((Action::PFilterLevels, "レベル補正\tCtrl+Shift+L")),
            Some((Action::PFilterGamma, "ガンマ補正")),
            Some((Action::PFilterBrightnessContrast, "明るさとコントラスト")),
            Some((
                Action::PFilterGrayscaleSimple,
                "簡易グレースケール化\tCtrl+Shift+G",
            )),
            Some((Action::PFilterGrayscaleStrict, "厳密グレースケール化")),
            None,
            Some((Action::PFilterBlur, "ぼかし")),
            Some((Action::PFilterBlurStrong, "ぼかし (強)")),
            Some((Action::PFilterSharpen, "シャープ")),
            Some((Action::PFilterSharpenStrong, "シャープ (強)")),
            Some((Action::PFilterGaussianBlur, "ガウスぼかし")),
            Some((Action::PFilterUnsharpMask, "アンシャープマスク")),
            Some((Action::PFilterMedianFilter, "メディアンフィルタ")),
            None,
            Some((Action::PFilterInvertColors, "色の反転")),
            Some((Action::PFilterApplyAlpha, "αチャンネルの反映")),
        ]);
        append_popup(menu_bar, filter_menu, "フィルタ(&T)");

        // リスト(&L)
        let list_menu = create_popup(&[
            Some((Action::NavigateFirst, "最初へ\tCtrl+Home")),
            Some((Action::NavigateLast, "最後へ\tCtrl+End")),
            Some((Action::NavigateToPage, "ページ指定\tCtrl+Space")),
            None,
            Some((Action::NavigatePrevFolder, "前のフォルダ\tShift+PageUp")),
            Some((Action::NavigateNextFolder, "次のフォルダ\tShift+PageDown")),
            Some((Action::SortNavigateBack, "ソート順で前へ\tShift+Tab")),
            Some((Action::SortNavigateForward, "ソート順で次へ\tTab")),
            None,
            Some((Action::ShuffleAll, "全体をシャッフル")),
            Some((Action::ShuffleGroups, "グループ順をシャッフル")),
            None,
            Some((Action::ToggleFileList, "ファイルリスト\tF4")),
            None,
            Some((Action::BookmarkSave, "ブックマーク保存\tF9")),
            Some((Action::BookmarkLoad, "ブックマーク読み込み\tF12")),
        ]);
        append_popup(menu_bar, list_menu, "リスト(&L)");

        // 表示(&V)
        let view_menu = create_popup(&[
            Some((Action::DisplayAutoShrink, "自動縮小表示\tNum /")),
            Some((Action::DisplayAutoFit, "自動縮小・拡大表示\tNum *")),
            None,
            Some((Action::ZoomIn, "拡大\tCtrl+Num +")),
            Some((Action::ZoomOut, "縮小\tCtrl+Num -")),
            Some((Action::ZoomReset, "等倍\tCtrl+Num0")),
            None,
            Some((Action::ToggleMargin, "余白\tNum0")),
            Some((Action::CycleAlphaBackground, "α背景切替\tA")),
            None,
            Some((Action::ToggleFullscreen, "全画面表示\tAlt+Enter")),
            Some((Action::ToggleAlwaysOnTop, "常に手前に表示\tT")),
            Some((Action::ToggleCursorHide, "カーソル自動非表示\tNum -")),
            None,
            Some((Action::SlideshowToggle, "スライドショー\tShift+Space")),
            Some((Action::SlideshowFaster, "スライドショー加速\tShift+↑")),
            Some((Action::SlideshowSlower, "スライドショー減速\tShift+↓")),
        ]);
        append_popup(menu_bar, view_menu, "表示(&V)");

        // ヘルプ(&H)
        let help_menu = create_popup(&[
            Some((Action::ShowHelp, "ヘルプ\tF1")),
            Some((Action::CheckUpdate, "アップデートを確認...")),
            Some((Action::OpenHomepage, "ホームページを開く...")),
            None,
            Some((Action::RegisterShell, "シェル統合を登録...")),
            Some((Action::UnregisterShell, "シェル統合を解除...")),
            None,
            Some((Action::OpenExeFolder, "実行ファイルのフォルダ\tShift+M")),
            Some((
                Action::OpenBookmarkFolder,
                "ブックマークのフォルダ\tShift+B",
            )),
            Some((Action::OpenSpiFolder, "SPIのフォルダ\tShift+S")),
            Some((Action::OpenTempFolder, "一時フォルダ\tShift+T")),
            None,
            Some((Action::OpenContainingFolder, "画像のフォルダを開く\tCtrl+D")),
        ]);
        append_popup(menu_bar, help_menu, "ヘルプ(&H)");

        menu_bar
    }
}

/// Action からメニューIDを計算
pub fn action_to_menu_id(action: Action) -> u16 {
    WM_COMMAND_BASE + action as u16
}

/// メニュー項目の有効/無効を更新
pub fn update_menu_enabled(menu: HMENU, action: Action, enabled: bool) {
    unsafe {
        let flag = if enabled { MF_ENABLED } else { MF_GRAYED };
        let _ = EnableMenuItem(menu, action_to_menu_id(action) as u32, MF_BYCOMMAND | flag);
    }
}

/// メニュー項目のチェック状態を更新
pub fn update_menu_check(menu: HMENU, action: Action, checked: bool) {
    unsafe {
        let flag = if checked { MF_CHECKED } else { MF_UNCHECKED };
        let _ = CheckMenuItem(
            menu,
            action_to_menu_id(action) as u32,
            (MF_BYCOMMAND | flag).0,
        );
    }
}

/// メニューIDからActionに変換
pub fn menu_id_to_action(id: u16) -> Option<Action> {
    if id < WM_COMMAND_BASE {
        return None;
    }
    let index = id - WM_COMMAND_BASE;
    action_from_index(index)
}

/// ポップアップメニューを作成してアイテムを追加
unsafe fn create_popup(items: &[Option<(Action, &str)>]) -> HMENU {
    unsafe {
        let popup = CreatePopupMenu().unwrap_or_default();
        for item in items {
            match item {
                Some((action, label)) => {
                    let wide_label = to_wide(label);
                    let _ = AppendMenuW(
                        popup,
                        MF_STRING,
                        action_to_menu_id(*action) as usize,
                        windows::core::PCWSTR(wide_label.as_ptr()),
                    );
                }
                None => {
                    let _ = AppendMenuW(popup, MF_SEPARATOR, 0, None);
                }
            }
        }
        popup
    }
}

/// ポップアップをメニューバーに追加
unsafe fn append_popup(menu_bar: HMENU, popup: HMENU, label: &str) {
    unsafe {
        let wide_label = to_wide(label);
        let _ = AppendMenuW(
            menu_bar,
            MF_POPUP,
            popup.0 as usize,
            windows::core::PCWSTR(wide_label.as_ptr()),
        );
    }
}

use crate::util::to_wide;

/// インデックスからActionを復元する (Action enumの discriminant を利用)
fn action_from_index(index: u16) -> Option<Action> {
    // Action enumの全バリアントを順番に対応付ける
    let actions = ALL_ACTIONS;
    actions.get(index as usize).copied()
}

/// 全Actionを列挙順に並べた配列 (Action enumのdiscriminantと一致させる)
const ALL_ACTIONS: &[Action] = &[
    // ナビゲーション
    Action::NavigateBack,
    Action::NavigateForward,
    Action::Navigate1Back,
    Action::Navigate1Forward,
    Action::Navigate5Back,
    Action::Navigate5Forward,
    Action::Navigate50Back,
    Action::Navigate50Forward,
    Action::NavigateFirst,
    Action::NavigateLast,
    Action::NavigatePrevFolder,
    Action::NavigateNextFolder,
    Action::NavigatePrevMark,
    Action::NavigateNextMark,
    Action::NavigateToPage,
    Action::SortNavigateBack,
    Action::SortNavigateForward,
    Action::ShuffleAll,
    Action::ShuffleGroups,
    // 表示モード
    Action::DisplayAutoShrink,
    Action::DisplayAutoFit,
    Action::ZoomIn,
    Action::ZoomOut,
    Action::ZoomReset,
    Action::ToggleMargin,
    Action::CycleAlphaBackground,
    // ウィンドウ
    Action::ToggleFullscreen,
    Action::Minimize,
    Action::ToggleMaximize,
    Action::ToggleAlwaysOnTop,
    Action::ToggleCursorHide,
    Action::ToggleMenuBar,
    // ファイル操作
    Action::NewWindow,
    Action::OpenFile,
    Action::OpenFolder,
    Action::CloseAll,
    Action::Reload,
    Action::RemoveFromList,
    Action::DeleteFile,
    Action::MoveFile,
    Action::CopyFile,
    Action::OpenContainingFolder,
    Action::CopyFileName,
    Action::CopyImage,
    Action::PasteImage,
    Action::ExportJpg,
    Action::ExportBmp,
    Action::ExportPng,
    Action::ShowImageInfo,
    // マーク操作
    Action::MarkSet,
    Action::MarkUnset,
    Action::MarkInvertAll,
    Action::MarkInvertToHere,
    Action::MarkedRemoveFromList,
    Action::MarkedDelete,
    Action::MarkedMove,
    Action::MarkedCopy,
    Action::MarkedCopyNames,
    // 編集
    Action::DeselectSelection,
    Action::Crop,
    Action::FlipHorizontal,
    Action::FlipVertical,
    Action::Rotate180,
    Action::Rotate90CW,
    Action::Rotate90CCW,
    Action::RotateArbitrary,
    Action::Resize,
    // フィルタ (画像メニュー)
    Action::Fill,
    Action::Levels,
    Action::Gamma,
    Action::BrightnessContrast,
    Action::Mosaic,
    Action::GaussianBlur,
    Action::UnsharpMask,
    Action::InvertColors,
    Action::GrayscaleSimple,
    Action::GrayscaleStrict,
    Action::ApplyAlpha,
    Action::Blur,
    Action::BlurStrong,
    Action::Sharpen,
    Action::SharpenStrong,
    Action::MedianFilter,
    // 永続フィルタ
    Action::PFilterToggle,
    Action::PFilterFlipH,
    Action::PFilterFlipV,
    Action::PFilterRotate180,
    Action::PFilterRotate90CW,
    Action::PFilterRotate90CCW,
    Action::PFilterLevels,
    Action::PFilterGamma,
    Action::PFilterBrightnessContrast,
    Action::PFilterGrayscaleSimple,
    Action::PFilterGrayscaleStrict,
    Action::PFilterBlur,
    Action::PFilterBlurStrong,
    Action::PFilterSharpen,
    Action::PFilterSharpenStrong,
    Action::PFilterGaussianBlur,
    Action::PFilterUnsharpMask,
    Action::PFilterMedianFilter,
    Action::PFilterInvertColors,
    Action::PFilterApplyAlpha,
    // ブックマーク
    Action::BookmarkSave,
    Action::BookmarkLoad,
    // ファイルリスト
    Action::ToggleFileList,
    // ユーティリティ
    Action::OpenExeFolder,
    Action::OpenBookmarkFolder,
    Action::OpenSpiFolder,
    Action::OpenTempFolder,
    Action::ShowHelp,
    Action::CheckUpdate,
    Action::OpenHomepage,
    Action::RegisterShell,
    Action::UnregisterShell,
    Action::Exit,
    // スライドショー
    Action::SlideshowToggle,
    Action::SlideshowFaster,
    Action::SlideshowSlower,
];
