//! Document モジュールのコンテナ展開機能
//! ZIP/PDF/RAR/7z などのアーカイブ形式の読み込みと展開を担当する。

use std::fs::File;
use std::path::Path;

use crate::archive::ArchiveManager;

use super::types::{ContainerResult, ZipBuffer};

/// 単一コンテナを処理する (rayon workerから呼ばれる)
///
/// Document::is_pdfはこのモジュール外で定義されているため、
/// superのprivateメソッドへのアクセスが必要。
/// ここでは型参照のみに限定し、実装の呼び出しは親で仲介する。
pub(super) fn process_single_container(
    path: &Path,
    archive_manager: &ArchiveManager,
    is_pdf: fn(&Path) -> bool,
) -> ContainerResult {
    if is_pdf(path) {
        match crate::pdf_renderer::get_pdf_page_count_safe(path) {
            Ok(page_count) => ContainerResult::Pdf {
                path: path.to_path_buf(),
                page_count,
            },
            Err(e) => ContainerResult::Error {
                path: path.to_path_buf(),
                error: format!("{e:#}"),
            },
        }
    } else if archive_manager.supports_on_demand(path) {
        // ZIP: mmapで読み込み
        // SAFETY:
        // - memmap2::Mmap::map は対象ファイルが mmap 中に外部から書き換えられないことを
        //   呼び出し側が保証する必要がある (Rust 的には UB の可能性)。本アプリでは
        //   閲覧専用かつ ZipBuffer の生存期間中は書き換え操作を行わない設計のため許容する。
        // - 失敗時は通常の fs::read にフォールバックするので致命的にはならない。
        let buffer =
            if let Ok(mmap) = File::open(path).and_then(|f| unsafe { memmap2::Mmap::map(&f) }) {
                ZipBuffer::Mmap(mmap)
            } else {
                match std::fs::read(path) {
                    Ok(data) => ZipBuffer::Memory(data),
                    Err(e) => {
                        return ContainerResult::Error {
                            path: path.to_path_buf(),
                            error: format!("アーカイブ読み込み失敗: {e}"),
                        };
                    }
                }
            };
        match archive_manager.list_images_from_buffer(buffer.as_ref(), path) {
            Ok(entries) => ContainerResult::Zip {
                path: path.to_path_buf(),
                buffer,
                entries,
            },
            Err(e) => ContainerResult::Error {
                path: path.to_path_buf(),
                error: format!("{e:#}"),
            },
        }
    } else {
        // RAR/7z/Susie: temp展開
        // システムクロックが UNIX epoch より前にずれていても処理を継続するため、
        // duration_since のエラーは ZERO にフォールバック (一意性は process_id + path が担保)
        let temp_dir = std::env::temp_dir().join(format!(
            "gv_archive_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or(std::time::Duration::ZERO)
                .as_millis()
        ));
        if let Err(e) = std::fs::create_dir_all(&temp_dir) {
            return ContainerResult::Error {
                path: path.to_path_buf(),
                error: format!("一時ディレクトリ作成失敗: {e}"),
            };
        }
        match archive_manager.extract_images(path, &temp_dir) {
            Ok(entries) => {
                if entries.is_empty() {
                    let _ = std::fs::remove_dir_all(&temp_dir);
                }
                ContainerResult::TempExtracted {
                    path: path.to_path_buf(),
                    temp_dir,
                    entries,
                }
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&temp_dir);
                ContainerResult::Error {
                    path: path.to_path_buf(),
                    error: format!("{e:#}"),
                }
            }
        }
    }
}
