#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod archive;
mod bookmark;
mod clipboard;
mod config;
mod document;
mod editing;
mod extension_registry;
mod file_info;
mod file_list;
mod file_ops;
mod filter;
mod image;
mod pdf_renderer;
mod persistent_filter;
mod prefetch;
mod render;
mod selection;
mod shell;
mod susie;
mod temp_cleanup;
mod ui;
mod updater;
mod util;

#[cfg(test)]
mod test_helpers;

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

    // 起動時マイグレーション・クリーンアップ
    migrate_old_filenames();
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

/// 旧ファイル名(gv3.*)を新ファイル名(ぐらびゅ.*)にリネームする
fn migrate_old_filenames() {
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(std::path::Path::to_path_buf))
    else {
        return;
    };
    let migrations = [
        ("gv3.toml", "ぐらびゅ.toml"),
        ("gv3.keys.toml", "ぐらびゅ.keys.toml"),
        ("gv3.default.toml", "ぐらびゅ.default.toml"),
        ("gv3.keys.default.toml", "ぐらびゅ.keys.default.toml"),
    ];
    for (old, new) in &migrations {
        let old_path = dir.join(old);
        let new_path = dir.join(new);
        if old_path.exists() && !new_path.exists() {
            let _ = std::fs::rename(&old_path, &new_path);
        }
    }
}

fn print_help() {
    let version = env!("CARGO_PKG_VERSION");
    println!(
        "\
ぐらびゅ v{version} - Windows用画像ビューアー

使い方:
  ぐらびゅ.exe [オプション] [ファイルパス]

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

詳細は ぐらびゅ.keys.default.toml を参照してください。"
    );
}
