//! ファイルリストパネル（左側 ListBox）
//!
//! メインウィンドウ左側にファイル一覧を表示する。
//! キー入力は親ウィンドウへ転送し、マウスクリックによる項目選択はListBox標準動作を使う。

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::file_list::FileList;

/// ファイルリストパネルのコントロールID
pub const FILE_LIST_CONTROL_ID: u16 = 0x100;

/// デフォルトのパネル幅（px）
const DEFAULT_WIDTH: i32 = 250;

/// ListBox のサブクラスID（SetWindowSubclass用）
const SUBCLASS_ID: usize = 1;

/// ファイルリストパネル
pub struct FileListPanel {
    listbox: HWND,
    parent: HWND,
    visible: bool,
    width: i32,
}

impl FileListPanel {
    /// ListBox を子ウィンドウとして作成する
    pub fn create(parent: HWND) -> Self {
        let listbox = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                windows::core::w!("LISTBOX"),
                None,
                // WS_CHILD | WS_VSCROLL | WS_BORDER | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT
                WINDOW_STYLE(
                    WS_CHILD.0
                        | WS_VSCROLL.0
                        | WS_BORDER.0
                        | LBS_NOTIFY as u32
                        | LBS_NOINTEGRALHEIGHT as u32,
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

        // ListBoxをサブクラス化してキー入力を親に転送
        unsafe {
            let _ = SetWindowSubclass(
                listbox,
                Some(listbox_subclass_proc),
                SUBCLASS_ID,
                parent.0 as usize,
            );
        }

        Self {
            listbox,
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
            let _ = ShowWindow(self.listbox, cmd);
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
                let _ = MoveWindow(self.listbox, 0, 0, self.width, parent_height, true);
            }
        }
    }

    /// ファイルリストの内容で ListBox を更新
    /// `is_cached`: 各インデックスのキャッシュ状態を返すクロージャ
    pub fn update(&self, file_list: &FileList, is_cached: impl Fn(usize) -> bool) {
        unsafe {
            SendMessageW(self.listbox, LB_RESETCONTENT, None, None);

            for (i, info) in file_list.files().iter().enumerate() {
                // マーク: ★、キャッシュ: ● (済) / ○ (未)
                let mark = if info.marked { "\u{2605}" } else { "\u{3000}" }; // ★ or 全角スペース
                let cache = if is_cached(i) { "\u{25CF}" } else { "\u{25CB}" }; // ● or ○
                let label = format!("{mark}{cache} {}\0", info.file_name);
                let wide: Vec<u16> = label.encode_utf16().collect();
                SendMessageW(
                    self.listbox,
                    LB_ADDSTRING,
                    None,
                    Some(LPARAM(wide.as_ptr() as isize)),
                );
            }
        }
    }

    /// 現在位置をハイライト
    pub fn set_selection(&self, index: usize) {
        unsafe {
            SendMessageW(self.listbox, LB_SETCURSEL, Some(WPARAM(index)), None);
        }
    }

    /// ListBox の HWND を返す（WM_COMMAND 判定用）
    pub fn listbox_hwnd(&self) -> HWND {
        self.listbox
    }

    /// 表示中かどうか
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// 表示状態を設定
    #[allow(dead_code)]
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        let cmd = if visible { SW_SHOW } else { SW_HIDE };
        unsafe {
            let _ = ShowWindow(self.listbox, cmd);
        }
    }

    /// 非表示にするがフラグは保持する（フルスクリーン開始時用）
    pub fn hide_preserve_state(&self) {
        unsafe {
            let _ = ShowWindow(self.listbox, SW_HIDE);
        }
    }

    /// 表示する（フルスクリーン解除時用、visibleフラグがtrueの場合のみ呼ぶ）
    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.listbox, SW_SHOW);
        }
    }
}

/// ListBoxサブクラスプロシージャ: キー入力を親ウィンドウへ転送
unsafe extern "system" fn listbox_subclass_proc(
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
            // キー入力は親ウィンドウへ転送
            unsafe {
                let _ = PostMessageW(Some(parent), msg, wparam, lparam);
            }
            return LRESULT(0);
        }
        WM_CHAR => {
            // ListBoxのデフォルト文字検索を無効化
            return LRESULT(0);
        }
        _ => {}
    }

    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}
