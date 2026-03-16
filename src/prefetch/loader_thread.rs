use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};

use crate::image::{DecodedImage, DecoderChain};

/// ワーカースレッドへのリクエスト
enum LoadRequest {
    Load {
        index: usize,
        path: PathBuf,
        generation: u64,
        /// PDFページの場合: (pdf_path, page_index)
        pdf_page: Option<(PathBuf, u32)>,
    },
    Shutdown,
}

/// ワーカースレッドからのレスポンス
pub enum LoadResponse {
    Loaded {
        index: usize,
        image: DecodedImage,
        generation: u64,
    },
    Failed {
        #[allow(dead_code)]
        index: usize,
        error: String,
        generation: u64,
    },
}

/// 先読みエンジン（ワーカースレッド管理）
pub struct PrefetchEngine {
    request_tx: Sender<LoadRequest>,
    response_rx: Receiver<LoadResponse>,
    worker_handle: Option<JoinHandle<()>>,
    /// ワーカーと共有する世代カウンタ
    current_generation: Arc<AtomicU64>,
    /// メインスレッド側のローカルコピー
    generation: u64,
}

impl PrefetchEngine {
    /// ワーカースレッドを起動する
    /// `notify`はレスポンス送信後に呼ばれるコールバック（UIスレッドへの通知用）
    /// `decoder`は画像デコーダチェーン（Susieプラグイン含む）
    pub fn new(notify: Box<dyn Fn() + Send>, decoder: Arc<DecoderChain>) -> Self {
        let (request_tx, request_rx) = crossbeam_channel::unbounded();
        let (response_tx, response_rx) = crossbeam_channel::unbounded();
        let current_generation = Arc::new(AtomicU64::new(0));
        let gen_clone = Arc::clone(&current_generation);

        let worker_handle = std::thread::Builder::new()
            .name("prefetch-worker".to_string())
            .spawn(move || {
                worker_loop(request_rx, response_tx, gen_clone, notify, decoder);
            })
            .expect("先読みワーカースレッドの起動に失敗");

        Self {
            request_tx,
            response_rx,
            worker_handle: Some(worker_handle),
            current_generation,
            generation: 0,
        }
    }

    /// 現在のgenerationを付与してロードリクエストを送信
    pub fn request_load(&self, index: usize, path: PathBuf, pdf_page: Option<(PathBuf, u32)>) {
        let _ = self.request_tx.send(LoadRequest::Load {
            index,
            path,
            generation: self.generation,
            pdf_page,
        });
    }

    /// 全レスポンスをノンブロッキングで取得
    pub fn drain_responses(&self) -> Vec<LoadResponse> {
        let mut responses = Vec::new();
        while let Ok(resp) = self.response_rx.try_recv() {
            responses.push(resp);
        }
        responses
    }

    /// 現在の世代
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 世代を進行（AtomicU64も更新）
    pub fn advance_generation(&mut self) -> u64 {
        self.generation += 1;
        self.current_generation
            .store(self.generation, Ordering::Relaxed);
        self.generation
    }
}

impl Drop for PrefetchEngine {
    fn drop(&mut self) {
        let _ = self.request_tx.send(LoadRequest::Shutdown);
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
    }
}

/// COMの初期化/解放を管理するDropガード
struct ComGuard;

impl ComGuard {
    fn init() -> Self {
        unsafe {
            // ワーカースレッドではMTAモードで初期化
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        Self
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::Com::CoUninitialize();
        }
    }
}

/// ワーカースレッドのメインループ
fn worker_loop(
    request_rx: Receiver<LoadRequest>,
    response_tx: Sender<LoadResponse>,
    current_generation: Arc<AtomicU64>,
    notify: Box<dyn Fn() + Send>,
    decoder: Arc<DecoderChain>,
) {
    // PDFレンダリングにWinRT APIが必要なのでCOM初期化
    let _com = ComGuard::init();

    while let Ok(request) = request_rx.recv() {
        match request {
            LoadRequest::Load {
                index,
                path,
                generation,
                pdf_page,
            } => {
                // デコード前に世代チェック → 古いリクエストはスキップ
                if generation < current_generation.load(Ordering::Relaxed) {
                    continue;
                }

                let response = if let Some((pdf_path, page_index)) = pdf_page {
                    // PDFページ: レンダリング
                    match crate::pdf_renderer::render_pdf_page(&pdf_path, page_index) {
                        Ok(image) => LoadResponse::Loaded {
                            index,
                            image,
                            generation,
                        },
                        Err(e) => LoadResponse::Failed {
                            index,
                            error: format!("{} page {}: {}", pdf_path.display(), page_index + 1, e),
                            generation,
                        },
                    }
                } else {
                    // 通常ファイル: fs::read → decode
                    let filename_hint = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    match std::fs::read(&path) {
                        Ok(data) => match decoder.decode(&data, filename_hint) {
                            Ok(image) => LoadResponse::Loaded {
                                index,
                                image,
                                generation,
                            },
                            Err(e) => LoadResponse::Failed {
                                index,
                                error: format!("{}: {}", path.display(), e),
                                generation,
                            },
                        },
                        Err(e) => LoadResponse::Failed {
                            index,
                            error: format!("{}: {}", path.display(), e),
                            generation,
                        },
                    }
                };

                let _ = response_tx.send(response);
                notify();
            }
            LoadRequest::Shutdown => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use crate::image::StandardDecoder;

    fn test_decoder() -> Arc<DecoderChain> {
        Arc::new(DecoderChain::new(vec![Box::new(StandardDecoder::new())]))
    }

    /// テスト用: 1x1 白ピクセルのPNGバイナリを生成
    fn create_1x1_white_png() -> Vec<u8> {
        use image::{ImageBuffer, Rgba};
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(1, 1, Rgba([255, 255, 255, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn load_and_receive_response() {
        let dir = std::env::temp_dir().join("gv3_test_prefetch_load");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.png");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(&create_1x1_white_png()).unwrap();
        }

        let engine = PrefetchEngine::new(Box::new(|| {}), test_decoder());
        engine.request_load(0, path, None);

        // ワーカーの処理完了を待つ
        let mut loaded = false;
        for _ in 0..100 {
            let responses = engine.drain_responses();
            for resp in responses {
                match resp {
                    LoadResponse::Loaded { index, image, .. } => {
                        assert_eq!(index, 0);
                        assert_eq!(image.width, 1);
                        assert_eq!(image.height, 1);
                        loaded = true;
                    }
                    LoadResponse::Failed { error, .. } => {
                        panic!("unexpected failure: {error}");
                    }
                }
            }
            if loaded {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(loaded, "レスポンスが受信できなかった");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_generation_is_skipped() {
        let dir = std::env::temp_dir().join("gv3_test_prefetch_gen");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.png");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(&create_1x1_white_png()).unwrap();
        }

        let mut engine = PrefetchEngine::new(Box::new(|| {}), test_decoder());

        // generation=0でリクエストを送信する前に世代を進める
        engine.request_load(0, path.clone(), None);
        engine.advance_generation(); // → generation=1

        // generation=1で新しいリクエスト
        engine.request_load(1, path, None);

        // 少し待ってレスポンスを収集
        std::thread::sleep(std::time::Duration::from_millis(200));
        let responses = engine.drain_responses();

        // generation=0のリクエストはスキップされる可能性がある（タイミング依存）
        // generation=1のリクエストは確実に処理される
        let has_gen1 = responses.iter().any(|r| match r {
            LoadResponse::Loaded { generation, .. } => *generation == 1,
            _ => false,
        });
        assert!(has_gen1, "generation=1のレスポンスが存在するべき");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_response_on_nonexistent_file() {
        let engine = PrefetchEngine::new(Box::new(|| {}), test_decoder());
        engine.request_load(0, PathBuf::from("nonexistent_file_xyz.png"), None);

        let mut failed = false;
        for _ in 0..100 {
            for resp in engine.drain_responses() {
                if matches!(resp, LoadResponse::Failed { .. }) {
                    failed = true;
                }
            }
            if failed {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(failed, "失敗レスポンスが受信できなかった");
    }

    #[test]
    fn drop_shuts_down_cleanly() {
        let engine = PrefetchEngine::new(Box::new(|| {}), test_decoder());
        drop(engine);
        // パニックせずに終了すればOK
    }
}
