//! Document モジュールの型定義
//! ZIPバッファ、イベント、コンテナ結果など、Document全体で使う各種型を集約する。

use std::path::PathBuf;

use crate::archive::ArchiveImageEntry;

/// ZIPファイルのバッファ (mmapまたはメモリ読み込み)
#[derive(Debug)]
pub enum ZipBuffer {
    /// メモリマップドファイル (OSがページフォルト駆動で必要部分のみロード)
    Mmap(memmap2::Mmap),
    /// ヒープ上のバイト列 (mmapフォールバック用)
    Memory(Vec<u8>),
}

impl AsRef<[u8]> for ZipBuffer {
    fn as_ref(&self) -> &[u8] {
        match self {
            ZipBuffer::Mmap(m) => m,
            ZipBuffer::Memory(v) => v,
        }
    }
}

/// DocumentからUIへの通知イベント (loader_threadから構築され、app.rsで受信される)
#[derive(Debug)]
pub enum DocumentEvent {
    /// 画像のデコード完了、再描画可能
    ImageReady,
    /// ファイルリスト変更
    FileListChanged,
    /// 表示位置変更
    NavigationChanged { index: usize },
    /// エラー通知
    Error(String),
}

/// コンテナ (ZIP/PDF/RAR/7z) の並列読み込み結果
pub(super) enum ContainerResult {
    Pdf {
        path: PathBuf,
        page_count: u32,
    },
    Zip {
        path: PathBuf,
        buffer: ZipBuffer,
        entries: Vec<ArchiveImageEntry>,
    },
    TempExtracted {
        path: PathBuf,
        temp_dir: PathBuf,
        entries: Vec<(PathBuf, String)>,
    },
    Error {
        path: PathBuf,
        error: String,
    },
}

/// コンテナの展開状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContainerState {
    /// 未展開 (プレースホルダとしてリストに存在、バックグラウンドキュー未投入)
    Pending,
    /// バックグラウンド展開中
    InFlight,
    /// 展開完了
    Expanded,
}

/// バックグラウンド展開スレッドからの通知イベント
pub(super) enum ContainerExpandEvent {
    /// 展開成功
    Expanded {
        container_path: PathBuf,
        result: ContainerResult,
        generation: u64,
    },
    /// 全コンテナの展開完了
    AllDone { generation: u64 },
}
