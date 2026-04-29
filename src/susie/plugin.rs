/// Susieプラグイン DLLラッパー
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, bail};
use libloading::Library;

use super::ffi::*;
use super::util::{
    HLocalGuard, dib_to_rgba, from_ansi, hlocal_from_isize, local_size, to_ansi, with_local_lock,
};
use crate::image::DecodedImage;

/// プラグインの種別
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SusiePluginType {
    /// 画像展開プラグイン ('I'/'i' in type string)
    Image,
    /// アーカイブ展開プラグイン ('A'/'a' in type string)
    Archive,
}

/// Susieプラグインの関数ポインタ群
/// `Library`をDrop順で最後に解放するためフィールド順序に注意
pub struct SusiePlugin {
    pub plugin_type: SusiePluginType,
    /// プラグインDLLのパス (画像情報表示等で使用)
    pub path: std::path::PathBuf,
    get_plugin_info: GetPluginInfoFn,
    is_supported: IsSupportedFn,
    get_picture: Option<GetPictureFn>,
    get_archive_info: Option<GetArchiveInfoFn>,
    get_file: Option<GetFileFn>,
    /// DLLハンドル (最後にDropされる)
    _lib: Library,
}

// SAFETY: SusiePlugin は内部に Library (HMODULE) と関数ポインタを保持する。
// プラグイン側は単一の DLL ロードに対してスレッド安全とは限らないが、本アプリでは
// `Arc<Mutex<SusiePlugin>>` で包んで全呼び出しを直列化する前提のため Send は安全。
// Sync は実装せず、必ず Mutex 経由でアクセスすること。
unsafe impl Send for SusiePlugin {}

impl SusiePlugin {
    /// DLLをロードしてプラグインを初期化する
    pub fn try_load(path: &Path) -> Result<Self> {
        // SAFETY: Library::new はネイティブコード (Susie プラグイン DLL) を読み込む。
        // ロード自体は OS のローダ任せで、副作用 (DllMain 実行) があるが、Susie 規約に
        // 準拠した DLL であることを前提とする。失敗時は Err を返してリソースは即解放される。
        let lib = unsafe {
            Library::new(path).with_context(|| format!("DLLロード失敗: {}", path.display()))?
        };

        // SAFETY (以下の `lib.get::<Fn>` 系すべてに共通):
        // 取得するシンボル名と関数型 (GetPluginInfoFn / IsSupportedFn / GetPictureFn /
        // GetArchiveInfoFn / GetFileFn) のシグネチャは Susie プラグイン仕様
        // (32bit/64bit ABI) に準拠している。シグネチャ不一致の DLL を読み込んだ場合は
        // 呼び出し時に未定義動作になりうるが、これは Susie 規格自体の仮定。
        let get_plugin_info: GetPluginInfoFn = unsafe {
            *lib.get::<GetPluginInfoFn>(b"GetPluginInfo\0")
                .with_context(|| "GetPluginInfoが見つからない")?
        };
        // SAFETY: 上記 GetPluginInfo 取得と同じ前提 (Susie 仕様準拠のシグネチャ)。
        let is_supported: IsSupportedFn = unsafe {
            *lib.get::<IsSupportedFn>(b"IsSupported\0")
                .with_context(|| "IsSupportedが見つからない")?
        };

        // プラグイン種別を判定 (GetPluginInfo(0) → type string)
        let type_str = Self::call_get_plugin_info_static(get_plugin_info, 0);
        let plugin_type = if type_str.starts_with("00IN") || type_str.starts_with("00in") {
            SusiePluginType::Image
        } else if type_str.starts_with("00AM") || type_str.starts_with("00am") {
            SusiePluginType::Archive
        } else {
            bail!("未知のプラグイン種別: {type_str} ({})", path.display());
        };

        // オプションシンボルの解決
        let get_picture: Option<GetPictureFn> = if plugin_type == SusiePluginType::Image {
            // SAFETY: 上記と同じ前提 (Susie 仕様準拠のシグネチャ)。
            Some(unsafe {
                *lib.get::<GetPictureFn>(b"GetPicture\0")
                    .with_context(|| "画像プラグインにGetPictureが存在しない")?
            })
        } else {
            None
        };

        let get_archive_info: Option<GetArchiveInfoFn> = if plugin_type == SusiePluginType::Archive
        {
            // SAFETY: 上記と同じ前提 (Susie 仕様準拠のシグネチャ)。
            Some(unsafe {
                *lib.get::<GetArchiveInfoFn>(b"GetArchiveInfo\0")
                    .with_context(|| "アーカイブプラグインにGetArchiveInfoが存在しない")?
            })
        } else {
            None
        };

        let get_file: Option<GetFileFn> = if plugin_type == SusiePluginType::Archive {
            // SAFETY: 上記と同じ前提 (Susie 仕様準拠のシグネチャ)。
            Some(unsafe {
                *lib.get::<GetFileFn>(b"GetFile\0")
                    .with_context(|| "アーカイブプラグインにGetFileが存在しない")?
            })
        } else {
            None
        };

        Ok(Self {
            plugin_type,
            path: path.to_path_buf(),
            get_plugin_info,
            is_supported,
            get_picture,
            get_archive_info,
            get_file,
            _lib: lib,
        })
    }

    /// GetPluginInfoを呼び出す (staticバージョン、初期化時用)
    fn call_get_plugin_info_static(func: GetPluginInfoFn, info_no: i32) -> String {
        let mut buf = vec![0u8; 256];
        let len = unsafe { func(info_no, buf.as_mut_ptr(), buf.len() as i32) };
        if len > 0 {
            from_ansi(&buf)
        } else {
            String::new()
        }
    }

    /// GetPluginInfoを呼び出す
    fn get_plugin_info_str(&self, info_no: i32) -> String {
        Self::call_get_plugin_info_static(self.get_plugin_info, info_no)
    }

    /// プラグインが対応する拡張子のリストを取得する
    /// GetPluginInfo(2n+2) → フィルタ文字列 (例: "*.jpg;*.jpeg") から拡張子を抽出
    pub fn supported_extensions(&self) -> Vec<String> {
        let mut exts = Vec::new();
        // info_no=2,4,6,...でフィルタを列挙 (空文字が返されたら終了)
        let mut info_no = 2;
        loop {
            let filter = self.get_plugin_info_str(info_no);
            if filter.is_empty() {
                break;
            }
            // "*.jpg;*.jpeg" → [".jpg", ".jpeg"]
            for part in filter.split(';') {
                let part = part.trim();
                if let Some(dot_pos) = part.rfind('.') {
                    let ext = format!(".{}", &part[dot_pos + 1..].to_lowercase());
                    if ext != ".*" && !exts.contains(&ext) {
                        exts.push(ext);
                    }
                }
            }
            info_no += 2;
        }
        exts
    }

    /// IsSupportedを呼び出す
    pub fn is_supported(&self, filename: &str, data_header: &[u8]) -> bool {
        let ansi_filename = to_ansi(filename);
        let result = unsafe { (self.is_supported)(ansi_filename.as_ptr(), data_header.as_ptr()) };
        result != 0
    }

    /// GetPictureを呼び出して画像をデコードする (メモリイメージモード)
    pub fn get_picture(&self, data: &[u8], filename: &str) -> Result<DecodedImage> {
        let get_picture = self
            .get_picture
            .context("このプラグインはGetPictureをサポートしない")?;

        let _ansi_filename = to_ansi(filename);
        let mut h_bm_info: isize = 0;
        let mut h_bm: isize = 0;

        // flag=1: メモリイメージモード (bufはデータポインタ、lenはデータサイズ)
        let result = unsafe {
            get_picture(
                data.as_ptr(),
                data.len() as isize,
                1, // メモリイメージモード
                std::ptr::from_mut(&mut h_bm_info),
                std::ptr::from_mut(&mut h_bm),
                0,
                0,
            )
        };

        if result != SUSIE_SUCCESS {
            bail!("GetPicture失敗 (code={result}): {filename}");
        }

        let h_info = hlocal_from_isize(h_bm_info);
        let h_bits = hlocal_from_isize(h_bm);
        let _guard_info = HLocalGuard(h_info);
        let _guard_bits = HLocalGuard(h_bits);

        // LocalLock → DIB変換 → LocalUnlock
        let image = with_local_lock(h_info, |bmi_ptr| {
            with_local_lock(h_bits, |bits_ptr| {
                dib_to_rgba(bmi_ptr as *const u8, bits_ptr as *const u8)
            })
        })
        .with_context(|| format!("DIB変換失敗: {filename}"))?;

        Ok(image)
    }

    /// GetArchiveInfoを呼び出してアーカイブ内のファイル一覧を取得する
    pub fn get_archive_info(&self, archive_path: &str) -> Result<Vec<ArchiveFileInfo>> {
        let get_archive_info = self
            .get_archive_info
            .context("このプラグインはGetArchiveInfoをサポートしない")?;

        let ansi_path = to_ansi(archive_path);
        let mut h_inf: isize = 0;

        // flag=0: ファイル名モード
        let result = unsafe {
            get_archive_info(
                ansi_path.as_ptr(),
                0, // 未使用
                0, // flag=0: ファイル名
                std::ptr::from_mut(&mut h_inf),
            )
        };

        if result != SUSIE_SUCCESS {
            bail!("GetArchiveInfo失敗 (code={result}): {archive_path}");
        }

        let handle = hlocal_from_isize(h_inf);
        let _guard = HLocalGuard(handle);

        // LocalLock → エントリ配列をパース → LocalUnlock
        let entries = with_local_lock(handle, |ptr| {
            if ptr.is_null() {
                return Vec::new();
            }

            let entry_size = std::mem::size_of::<ArchiveFileInfo>();
            let mut entries = Vec::new();
            let mut offset = 0usize;
            loop {
                // SAFETY: ptr は `LocalLock` で取得した有効なメモリ領域を指す。offset はアライン境界内で
                // entry_size ずつ進むため、キャスト後の ArchiveFileInfo への参照は妥当である。
                // ループ内で offset の上限チェック（10000エントリ）により暴走を防止する。
                let entry_ptr = unsafe { ptr.cast::<u8>().add(offset) };
                let entry = unsafe { &*entry_ptr.cast::<ArchiveFileInfo>() };
                if entry.is_terminator() {
                    break;
                }
                entries.push(entry.clone());
                offset += entry_size;

                // 安全策: 最大10000エントリ
                if entries.len() >= 10000 {
                    break;
                }
            }
            entries
        });

        Ok(entries)
    }

    /// GetFileを呼び出してアーカイブ内のファイルをメモリに展開する
    pub fn get_file_to_memory(&self, archive_path: &str, position: u64) -> Result<Vec<u8>> {
        let get_file = self
            .get_file
            .context("このプラグインはGetFileをサポートしない")?;

        let ansi_path = to_ansi(archive_path);
        // dest: メモリ出力モードではHLOCAL*として扱われる
        let mut h_dest: isize = 0;

        // flag=0x0100(メモリ出力)
        // src=ファイル名, len=position
        let result = unsafe {
            get_file(
                ansi_path.as_ptr(),
                position as isize,
                std::ptr::from_mut(&mut h_dest).cast::<u8>(),
                0x0100, // メモリ出力
                0,
                0,
            )
        };

        if result != SUSIE_SUCCESS {
            bail!("GetFile失敗 (code={result}): {archive_path} position={position}");
        }

        if h_dest == 0 {
            bail!("GetFile: 出力ハンドルがnull");
        }

        let handle = hlocal_from_isize(h_dest);
        let _guard = HLocalGuard(handle);
        let size = local_size(handle);

        let data = with_local_lock(handle, |ptr| {
            if ptr.is_null() || size == 0 {
                Vec::new()
            } else {
                // SAFETY: ptr は `LocalLock` で取得した有効なメモリ領域を指し、size は `LocalSize` で確認した
                // 正確なバイト数である。スライス変換後の参照は with_local_lock のクロージャ内に留まり、
                // LocalUnlock 後に参照が逃げないことが保証される（to_vec で所有権化）。
                unsafe { std::slice::from_raw_parts(ptr as *const u8, size) }.to_vec()
            }
        });

        if data.is_empty() {
            bail!("GetFile: データが空");
        }

        Ok(data)
    }
}

/// スレッド安全なプラグインラッパー
pub type SharedPlugin = Arc<Mutex<SusiePlugin>>;
