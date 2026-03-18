mod rar;
mod sevenz;
pub mod susie;
pub mod zip;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, bail};

use crate::extension_registry::ExtensionRegistry;

/// アーカイブ展開結果: (展開先tempパス, アーカイブ内エントリパス)
pub type ExtractedEntry = (PathBuf, String);

/// オンデマンド読み出し用のアーカイブ内画像エントリ情報
pub struct ArchiveImageEntry {
    /// アーカイブ内パス（例: "subfolder/image.png"）
    pub entry_name: String,
    /// フラット化したファイル名（ソート用）
    pub file_name: String,
    /// 非圧縮サイズ
    pub file_size: u64,
}

/// アーカイブハンドラのトレイト
/// 各フォーマットのハンドラが実装する
pub trait ArchiveHandler: Send + Sync {
    fn supported_extensions(&self) -> Vec<String>;

    /// アーカイブ内の画像ファイルをtarget_dirに展開する
    /// 戻り値: (展開先tempパス, アーカイブ内エントリパス) のペア一覧
    fn extract_images(&self, archive_path: &Path, target_dir: &Path)
    -> Result<Vec<ExtractedEntry>>;

    /// オンデマンド読み出しに対応しているかどうか
    fn supports_on_demand(&self) -> bool {
        false
    }

    /// アーカイブ内の画像エントリ一覧を取得する（データ読み込みなし）
    #[allow(dead_code)]
    fn list_images(&self, _archive_path: &Path) -> Result<Vec<ArchiveImageEntry>> {
        bail!("オンデマンド読み出し未対応")
    }

    /// アーカイブから指定エントリのデータを読み出す
    fn read_entry(&self, _archive_path: &Path, _entry_name: &str) -> Result<Vec<u8>> {
        bail!("オンデマンド読み出し未対応")
    }
}

/// アーカイブハンドラのディスパッチャ
pub struct ArchiveManager {
    handlers: Vec<Box<dyn ArchiveHandler>>,
    registry: Arc<ExtensionRegistry>,
}

impl ArchiveManager {
    pub fn new(registry: Arc<ExtensionRegistry>) -> Self {
        Self {
            handlers: vec![
                Box::new(zip::ZipHandler::new(Arc::clone(&registry))),
                Box::new(rar::RarHandler::new(Arc::clone(&registry))),
                Box::new(sevenz::SevenZHandler::new(Arc::clone(&registry))),
            ],
            registry,
        }
    }

    /// ハンドラを追加する（Susieプラグイン等の動的追加用）
    pub fn add_handler(&mut self, handler: Box<dyn ArchiveHandler>) {
        self.handlers.push(handler);
    }

    /// パスの拡張子を正規化する（例: "test.ZIP" → ".zip"）
    fn normalized_extension(path: &Path) -> String {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_lowercase()))
            .unwrap_or_default()
    }

    /// パスの拡張子に対応するハンドラを返す
    fn find_handler(&self, archive_path: &Path) -> Result<&dyn ArchiveHandler> {
        let ext = Self::normalized_extension(archive_path);
        self.handlers
            .iter()
            .find(|h| h.supported_extensions().contains(&ext))
            .map(|h| h.as_ref())
            .ok_or_else(|| anyhow::anyhow!("未対応のアーカイブ形式: {}", archive_path.display()))
    }

    /// パスがアーカイブファイルか拡張子で判定する
    pub fn is_archive(&self, path: &Path) -> bool {
        path.file_name()
            .and_then(|n| n.to_str())
            .map(|name| self.registry.is_archive_extension(name))
            .unwrap_or(false)
    }

    /// 対応ハンドラでアーカイブ内の画像を一括展開する
    pub fn extract_images(
        &self,
        archive_path: &Path,
        target_dir: &Path,
    ) -> Result<Vec<ExtractedEntry>> {
        self.find_handler(archive_path)?
            .extract_images(archive_path, target_dir)
    }

    /// パスに対応するハンドラがオンデマンド読み出しに対応しているか判定する
    pub fn supports_on_demand(&self, archive_path: &Path) -> bool {
        self.find_handler(archive_path)
            .is_ok_and(|h| h.supports_on_demand())
    }

    /// アーカイブ内の画像エントリ一覧を取得する（オンデマンド用）
    #[allow(dead_code)]
    pub fn list_images(&self, archive_path: &Path) -> Result<Vec<ArchiveImageEntry>> {
        self.find_handler(archive_path)?.list_images(archive_path)
    }

    /// アーカイブから指定エントリのデータを読み出す（オンデマンド用）
    pub fn read_entry(&self, archive_path: &Path, entry_name: &str) -> Result<Vec<u8>> {
        self.find_handler(archive_path)?
            .read_entry(archive_path, entry_name)
    }

    /// インメモリバッファからエントリ一覧を取得する（ZIPキャッシュ用）
    pub fn list_images_from_buffer(
        &self,
        buffer: &[u8],
        archive_path: &Path,
    ) -> Result<Vec<ArchiveImageEntry>> {
        let ext = Self::normalized_extension(archive_path);
        if ext == ".zip" || ext == ".cbz" {
            return zip::ZipHandler::list_images_from_buffer(buffer, &self.registry);
        }
        bail!("バッファベース読み出し未対応: {}", archive_path.display());
    }

    /// インメモリバッファからエントリを読み出す（ZIPキャッシュ用、Stored最適化付き）
    #[allow(dead_code)]
    pub fn read_entry_from_buffer(
        &self,
        buffer: &[u8],
        entry_name: &str,
        archive_path: &Path,
    ) -> Result<Vec<u8>> {
        let ext = Self::normalized_extension(archive_path);
        if ext == ".zip" || ext == ".cbz" {
            return zip::ZipHandler::read_entry_from_buffer(buffer, entry_name);
        }
        bail!("バッファベース読み出し未対応: {}", archive_path.display());
    }
}

/// アーカイブ展開時のファイル名重複を解決する
/// target_dirに同名ファイルが既に存在する場合、"_2", "_3"...のサフィックスを付ける
pub fn resolve_filename(target_dir: &Path, original_name: &str) -> std::path::PathBuf {
    let path = target_dir.join(original_name);
    if !path.exists() {
        return path;
    }

    let stem = Path::new(original_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(original_name);
    let ext = Path::new(original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    for i in 2..=9999 {
        let new_name = if ext.is_empty() {
            format!("{stem}_{i}")
        } else {
            format!("{stem}_{i}.{ext}")
        };
        let new_path = target_dir.join(&new_name);
        if !new_path.exists() {
            return new_path;
        }
    }

    // 万が一9999まで使い切った場合（実質ありえない）
    target_dir.join(format!("{original_name}_overflow"))
}

/// アーカイブエントリのパスからファイル名部分のみを抽出する
/// アーカイブ内のサブフォルダ構造はフラット化する
pub fn extract_filename(entry_path: &str) -> &str {
    // パス区切りは '/' と '\' の両方を考慮
    entry_path.rsplit(['/', '\\']).next().unwrap_or(entry_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_archive_recognizes_extensions() {
        let reg = Arc::new(ExtensionRegistry::new());
        let mgr = ArchiveManager::new(reg);
        assert!(mgr.is_archive(Path::new("test.zip")));
        assert!(mgr.is_archive(Path::new("test.ZIP")));
        assert!(mgr.is_archive(Path::new("test.cbz")));
        assert!(mgr.is_archive(Path::new("test.rar")));
        assert!(mgr.is_archive(Path::new("test.cbr")));
        assert!(mgr.is_archive(Path::new("test.7z")));
        assert!(!mgr.is_archive(Path::new("test.jpg")));
        assert!(!mgr.is_archive(Path::new("test.txt")));
        assert!(!mgr.is_archive(Path::new("test")));
    }

    #[test]
    fn extract_filename_flattens_paths() {
        assert_eq!(extract_filename("folder/subfolder/image.jpg"), "image.jpg");
        assert_eq!(extract_filename("folder\\image.png"), "image.png");
        assert_eq!(extract_filename("image.bmp"), "image.bmp");
        assert_eq!(extract_filename(""), "");
    }

    #[test]
    fn resolve_filename_handles_duplicates() {
        let dir = std::env::temp_dir().join("gv3_test_resolve_fn");
        let _ = std::fs::create_dir_all(&dir);

        // 重複なし
        let path = resolve_filename(&dir, "unique.jpg");
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "unique.jpg");

        // 重複あり
        std::fs::write(dir.join("dup.jpg"), b"").unwrap();
        let path = resolve_filename(&dir, "dup.jpg");
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "dup_2.jpg");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
