#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod archive;
mod config;
mod document;
mod extension_registry;
mod file_info;
mod file_list;
mod image;
mod prefetch;
mod render;
mod shell;
mod susie;
mod ui;

use std::path::PathBuf;

use anyhow::Result;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

fn main() -> Result<()> {
    // DPI awareness設定（COM初期化より先に呼ぶ必要がある）
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    // COM初期化
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }

    // CLI分岐（--register / --unregister）
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "--register" => return shell::register_all(),
            "--unregister" => return shell::unregister_all(),
            _ => {}
        }
    }

    // 設定ファイル読み込み
    let config = config::Config::load();

    // コマンドライン引数から画像ファイルパスを取得
    let initial_file: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);

    // メインウィンドウ作成
    // _appはメッセージループ中に生存する必要がある（Box<AppWindow>のドロップ防止）
    let _app = app::AppWindow::create(config, initial_file.as_deref())?;

    // メッセージループ
    let exit_code = ui::window::run_message_loop();
    std::process::exit(exit_code);
}
