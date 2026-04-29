//! Document モジュールのユーティリティ関数
//! パス操作、フォルダ走査、スレッド優先度制御など、補助的な関数を集約する。

use std::path::Path;

/// パス列をファイル名の自然順 (大小文字無視) で並べ替える。
/// D&D や CLI 引数の選択順を、ユーザー期待のエクスプローラー表示順へ寄せるために使う。
pub(super) fn sort_paths_natural(paths: &mut [std::path::PathBuf]) {
    paths.sort_by(|a, b| {
        let ak = a.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let bk = b.file_name().and_then(|s| s.to_str()).unwrap_or("");
        natord::compare_ignore_case(ak, bk)
    });
}

/// Windows のリパースポイント (シンボリックリンク・ジャンクション等) か判定する。
/// `FileType::is_symlink()` はジャンクションを確実に識別できないため、
/// `MetadataExt::file_attributes()` で `FILE_ATTRIBUTE_REPARSE_POINT` ビットを直接検査する。
pub(super) fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// フォルダ配下を深さ優先で再帰走査し、走査対象ファイルパスをフラット列で返す。
/// 各階層はファイル→サブディレクトリの順に並べ、階層内は `sort_paths_natural` で整える。
/// この並びにより FileList 側の `group_key` によるサブフォルダ単位グループ化と
/// 「親→子」のグループ出現順がそのまま機能する。
/// シンボリックリンク・ジャンクション等のリパースポイントは追跡しない。
pub(super) fn collect_folder_files_recursive(folder: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    walk_folder_recursive(folder, &mut out);
    out
}

fn walk_folder_recursive(folder: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut subdirs: Vec<std::path::PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if is_reparse_point(&meta) {
            continue;
        }
        if meta.is_file() {
            files.push(path);
        } else if meta.is_dir() {
            subdirs.push(path);
        }
    }
    sort_paths_natural(&mut files);
    sort_paths_natural(&mut subdirs);
    out.extend(files);
    for subdir in subdirs {
        walk_folder_recursive(&subdir, out);
    }
}

/// 現在のスレッドの I/O 優先度を `THREAD_MODE_BACKGROUND_BEGIN` に落とす。
///
/// バックグラウンド展開用の rayon ワーカースレッドから呼び出す。成功すると、
/// そのスレッドが発行する全ての I/O リクエストは Windows I/O スケジューラで
/// Low 優先度 (ページ優先度も低下) に扱われ、同じディスクに対するメイン
/// スレッド・先読みワーカーの I/O が優先される。HDD ではこれにより先読みが
/// 間に合わず待たされる現象が緩和される。
pub(super) fn set_current_thread_background_io_priority() -> windows::core::Result<()> {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
    };
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) }
}

/// バックグラウンド展開用の rayon プールを構築する。
///
/// - 並列度は 1 に固定する。HDD ではシーク競合を避けるため 1 並列が最適で、
///   SSD でも ZIP のセントラルディレクトリ読み出しに並列は不要である。
/// - ワーカースレッド起動時に `set_current_thread_background_io_priority` を
///   呼び、I/O 優先度を Low に落とす。rayon の `start_handler` は戻り値を
///   持たないため、失敗時は `eprintln!` で記録してプール構築自体は成功させる。
pub(super) fn build_expansion_pool() -> anyhow::Result<rayon::ThreadPool> {
    use anyhow::Context;
    rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .thread_name(|_| "bg-expansion".to_string())
        .start_handler(|_idx| {
            if let Err(e) = set_current_thread_background_io_priority() {
                eprintln!("背景展開スレッドの I/O 優先度設定に失敗: {e}");
            }
        })
        .build()
        .context("背景展開用 rayon プールの構築に失敗")
}
