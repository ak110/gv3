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

        // 画像(&I)
        let image_menu = create_popup(&[
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
        ]);
        append_popup(menu_bar, view_menu, "表示(&V)");

        // ヘルプ(&H)
        let help_menu = create_popup(&[
            Some((Action::ShowHelp, "ヘルプ\tF1")),
            Some((Action::CheckUpdate, "アップデートを確認...")),
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
fn action_to_menu_id(action: Action) -> u16 {
    WM_COMMAND_BASE + action as u16
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

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// インデックスからActionを復元する（Action enumの discriminant を利用）
fn action_from_index(index: u16) -> Option<Action> {
    // Action enumの全バリアントを順番に対応付ける
    let actions = ALL_ACTIONS;
    actions.get(index as usize).copied()
}

/// 全Actionを列挙順に並べた配列（Action enumのdiscriminantと一致させる）
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
    Action::Exit,
];
