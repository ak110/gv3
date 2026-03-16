#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod archive;
mod bookmark;
mod clipboard;
mod config;
mod document;
mod extension_registry;
mod file_info;
mod file_list;
mod file_ops;
mod image;
mod pdf_renderer;
mod prefetch;
mod render;
mod shell;
mod susie;
mod temp_cleanup;
mod ui;
mod updater;

use std::path::PathBuf;

use anyhow::Result;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

fn main() -> Result<()> {
    // CLIフラグの分岐（DPI/COM初期化前に処理: 副作用不要なもの）
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            _ => {}
        }
    }

    // DPI awareness設定（COM初期化より先に呼ぶ必要がある）
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    // COM初期化
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }

    // 起動時クリーンアップ
    temp_cleanup::cleanup_orphaned_temp_dirs();
    updater::cleanup_old_exe();

    // CLI分岐（--register / --unregister: COM初期化後に実行）
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "--register" => return shell::register_all(),
            "--unregister" => return shell::unregister_all(),
            _ => {}
        }
    }

    // 設定ファイル読み込み
    let config = config::Config::load();

    // コマンドライン引数からファイルパスを収集（--で始まるフラグは除外）
    let initial_files: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .filter(|arg| !arg.to_str().is_some_and(|s| s.starts_with("--")))
        .map(PathBuf::from)
        .collect();

    // メインウィンドウ作成
    // _appはメッセージループ中に生存する必要がある（Box<AppWindow>のドロップ防止）
    let _app = app::AppWindow::create(config, &initial_files)?;

    // メッセージループ
    let exit_code = ui::window::run_message_loop();
    std::process::exit(exit_code);
}

fn print_help() {
    let version = env!("CARGO_PKG_VERSION");
    println!(
        "\
ぐらびゅ3 v{version} - Windows用画像ビューアー

使い方:
  gv3.exe [オプション] [ファイルパス]

オプション:
  --help, -h        このヘルプを表示
  --register        ファイル関連付け・コンテキストメニュー・送るを一括登録
  --unregister      一括解除

対応フォーマット:
  画像:     JPEG, PNG, GIF, BMP, WebP
  ドキュメント: PDF
  アーカイブ: ZIP/cbz, RAR/cbr, 7z
  ※ 64bit Susieプラグイン (.sph/.spi) で拡張可能

主要キーバインド:
  ← / →              前後の画像に移動
  ホイール上/下       前後の画像に移動
  PageUp / PageDown   5ページ移動
  Ctrl+Home / End     最初 / 最後へ
  Ctrl+ホイール       拡大 / 縮小
  Alt+Enter           フルスクリーン
  Esc                 メニューバー表示/非表示
  F4                  ファイルリスト表示/非表示
  F1                  ヘルプ表示

詳細は docs/keybindings.md を参照してください。"
    );
}
