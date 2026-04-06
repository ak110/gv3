//! ファイルリストパネル（左側 ListView 仮想モード）
//!
//! メインウィンドウ左側にファイル一覧を表示する。
//! `SysListView32` の仮想モード (`LVS_OWNERDATA`) を使うことで、ファイル数が
//! 数十万件になっても項目セット (`LVM_SETITEMCOUNTEX`) を O(1) で行える。
//! 表示行のテキストは親ウィンドウが `LVN_GETDISPINFO` で要求されたタイミングで
//! 提供する (file_list_panel 自体は内容を保持しない)。
//! キー入力はサブクラス化して親ウィンドウへ転送し、ListView 標準のキー処理は
//! 抑止する (二重発火防止)。

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::file_info::FileInfo;

/// ファイルリストパネルのコントロールID
pub const FILE_LIST_CONTROL_ID: u16 = 0x100;

/// デフォルトのパネル幅（px）
const DEFAULT_WIDTH: i32 = 250;

/// ListView のサブクラスID（SetWindowSubclass用）
const SUBCLASS_ID: usize = 1;

/// ファイルリストパネル
pub struct FileListPanel {
    listview: HWND,
    parent: HWND,
    visible: bool,
    width: i32,
}

impl FileListPanel {
    /// ListView を子ウィンドウとして作成する
    pub fn create(parent: HWND) -> Self {
        // Common Controls の ListView クラスを初期化
        let icc = INITCOMMONCONTROLSEX {
            dwSize: u32::try_from(std::mem::size_of::<INITCOMMONCONTROLSEX>()).unwrap_or(0),
            dwICC: ICC_LISTVIEW_CLASSES,
        };
        unsafe {
            let _ = InitCommonControlsEx(&raw const icc);
        }

        let listview = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                windows::core::w!("SysListView32"),
                None,
                // WS_CHILD | WS_VSCROLL | WS_BORDER
                // | LVS_REPORT | LVS_OWNERDATA | LVS_SINGLESEL
                // | LVS_NOCOLUMNHEADER | LVS_SHOWSELALWAYS
                WINDOW_STYLE(
                    WS_CHILD.0
                        | WS_VSCROLL.0
                        | WS_BORDER.0
                        | LVS_REPORT
                        | LVS_OWNERDATA
                        | LVS_SINGLESEL
                        | LVS_NOCOLUMNHEADER
                        | LVS_SHOWSELALWAYS,
                ),
                0,
                0,
                DEFAULT_WIDTH,
                600, // 初期高さ（WM_SIZEで上書きされる）
                Some(parent),
                Some(HMENU(FILE_LIST_CONTROL_ID as *mut _)),
                None,
                None,
            )
            .unwrap_or_default()
        };

        // 拡張スタイル: ダブルバッファ + 行全体選択
        let ex_style: u32 = LVS_EX_DOUBLEBUFFER | LVS_EX_FULLROWSELECT;
        unsafe {
            SendMessageW(
                listview,
                LVM_SETEXTENDEDLISTVIEWSTYLE,
                Some(WPARAM(ex_style as usize)),
                Some(LPARAM(ex_style as isize)),
            );
        }

        // 1カラムを追加 (ヘッダ非表示なので幅だけ設定)
        let mut col = LVCOLUMNW {
            mask: LVCOLUMNW_MASK(LVCF_WIDTH.0 | LVCF_FMT.0),
            fmt: LVCOLUMNW_FORMAT(LVCFMT_LEFT.0),
            cx: DEFAULT_WIDTH - 24, // スクロールバー幅+α分を引く
            ..Default::default()
        };
        unsafe {
            SendMessageW(
                listview,
                LVM_INSERTCOLUMNW,
                Some(WPARAM(0)),
                Some(LPARAM(std::ptr::from_mut(&mut col) as isize)),
            );
        }

        // ListView をサブクラス化してキー入力を親に転送
        unsafe {
            let _ = SetWindowSubclass(
                listview,
                Some(listview_subclass_proc),
                SUBCLASS_ID,
                parent.0 as usize,
            );
        }

        Self {
            listview,
            parent,
            visible: false,
            width: DEFAULT_WIDTH,
        }
    }

    /// 表示/非表示をトグル
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        let cmd = if self.visible { SW_SHOW } else { SW_HIDE };
        unsafe {
            let _ = ShowWindow(self.listview, cmd);
            // 親のレイアウト再計算をトリガー
            let _ = SendMessageW(self.parent, WM_SIZE, None, None);
        }
    }

    /// visible なら panel_width、非表示なら 0
    pub fn panel_width(&self) -> i32 {
        if self.visible { self.width } else { 0 }
    }

    /// 親ウィンドウのリサイズに追従
    pub fn resize(&self, parent_height: i32) {
        if self.visible {
            unsafe {
                let _ = MoveWindow(self.listview, 0, 0, self.width, parent_height, true);
            }
        }
    }

    /// ファイルリストの項目数で ListView を更新する
    /// 仮想モードのため `LVM_SETITEMCOUNTEX` で項目数だけセットする (O(1))。
    /// 実際の表示テキストは親ウィンドウの `LVN_GETDISPINFO` ハンドラから返す。
    pub fn update(&self, count: usize) {
        unsafe {
            // LVSICF_NOINVALIDATEALL: 可視範囲外は再描画しない
            // LVSICF_NOSCROLL: スクロール位置を維持
            // LVSICF_NOINVALIDATEALL=0x1, LVSICF_NOSCROLL=0x2
            const FLAGS: isize = 0x1 | 0x2;
            SendMessageW(
                self.listview,
                LVM_SETITEMCOUNT,
                Some(WPARAM(count)),
                Some(LPARAM(FLAGS)),
            );
            let _ = InvalidateRect(Some(self.listview), None, true);
        }
    }

    /// 単一項目を再描画要求する（マーク・キャッシュ状態の変更用）
    /// 仮想モードでは項目データを LV が保持しないので、行のみ無効化すれば
    /// 次回描画時に親ウィンドウへ再度 LVN_GETDISPINFO で問い合わせが行く。
    pub fn update_item(&self, index: usize) {
        let i = i32::try_from(index).unwrap_or(i32::MAX);
        unsafe {
            SendMessageW(
                self.listview,
                LVM_REDRAWITEMS,
                Some(WPARAM(i as usize)),
                Some(LPARAM(i as isize)),
            );
        }
    }

    /// ListView項目のラベル文字列を生成 (LVN_GETDISPINFO ハンドラから呼ばれる)
    pub fn format_label(info: &FileInfo, is_cached: bool) -> String {
        let mark = if info.marked { "\u{2605}" } else { "\u{3000}" }; // ★ or 全角スペース
        let cache = if is_cached { "\u{25CF}" } else { "\u{25CB}" }; // ● or ○
        format!("{mark}{cache} {}", info.file_name)
    }

    /// 現在位置をハイライトしてスクロール
    pub fn set_selection(&self, index: usize) {
        let i = i32::try_from(index).unwrap_or(i32::MAX);
        let sel_focus = LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
        unsafe {
            // 既存の選択を解除 (state=0 で SELECTED|FOCUSED ビットをクリア)
            let mut clear = LVITEMW {
                stateMask: sel_focus,
                state: LIST_VIEW_ITEM_STATE_FLAGS(0),
                ..Default::default()
            };
            SendMessageW(
                self.listview,
                LVM_SETITEMSTATE,
                Some(WPARAM(usize::MAX)), // 全項目対象 (-1)
                Some(LPARAM(std::ptr::from_mut(&mut clear) as isize)),
            );

            // 新しい選択をセット
            let mut item = LVITEMW {
                stateMask: sel_focus,
                state: sel_focus,
                ..Default::default()
            };
            SendMessageW(
                self.listview,
                LVM_SETITEMSTATE,
                Some(WPARAM(i as usize)),
                Some(LPARAM(std::ptr::from_mut(&mut item) as isize)),
            );
            SendMessageW(
                self.listview,
                LVM_ENSUREVISIBLE,
                Some(WPARAM(i as usize)),
                Some(LPARAM(0)),
            );
        }
    }

    /// ListView の HWND を返す（WM_NOTIFY 判定用）
    pub fn listview_hwnd(&self) -> HWND {
        self.listview
    }

    /// 後方互換: 旧 API 名 (app.rs から参照)
    pub fn listbox_hwnd(&self) -> HWND {
        self.listview
    }

    /// 表示中かどうか
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// 非表示にするがフラグは保持する（フルスクリーン開始時用）
    pub fn hide_preserve_state(&self) {
        unsafe {
            let _ = ShowWindow(self.listview, SW_HIDE);
        }
    }

    /// 表示する（フルスクリーン解除時用、visibleフラグがtrueの場合のみ呼ぶ）
    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.listview, SW_SHOW);
        }
    }
}

/// ListView サブクラスプロシージャ: キー入力を親ウィンドウへ転送し、
/// ListView 標準のキー処理 (矢印キーでの選択移動など) を抑止する。
unsafe extern "system" fn listview_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uid: usize,
    parent_ptr: usize,
) -> LRESULT {
    let parent = HWND(parent_ptr as *mut _);

    match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            // キー入力は親ウィンドウへ転送し、ListView の標準処理は止める
            unsafe {
                let _ = PostMessageW(Some(parent), msg, wparam, lparam);
            }
            return LRESULT(0);
        }
        WM_CHAR => {
            // ListView のデフォルト文字検索を無効化
            return LRESULT(0);
        }
        _ => {}
    }

    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}
