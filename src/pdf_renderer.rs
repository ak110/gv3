//! PDFページレンダリング
//!
//! Windows.Data.Pdf API を使ってPDFの各ページを画像にレンダリングする。
//! WinRT APIはスレッドセーフではないため、呼び出しごとにPdfDocumentを新規オープンする。

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use windows::Data::Pdf::PdfDocument;
use windows::Storage::StorageFile;
use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use crate::image::DecodedImage;

/// レンダリング解像度スケール（150 DPI / 96 DPI ≈ 1.5625）
const DPI_SCALE: f64 = 1.5625;

/// PDFのページ数を取得する（高速: ページデータはロードしない）
pub fn get_pdf_page_count(pdf_path: &Path) -> Result<u32> {
    let clean_path = crate::util::strip_extended_length_prefix(pdf_path);
    let path_str = clean_path.to_str().unwrap_or("");
    if path_str.is_empty() {
        anyhow::bail!("無効なパス: {}", pdf_path.display());
    }

    let hstring = windows::core::HSTRING::from(path_str);
    let file = StorageFile::GetFileFromPathAsync(&hstring)
        .context("StorageFile取得失敗")?
        .join()
        .context("StorageFile非同期取得失敗")?;

    let doc = PdfDocument::LoadFromFileAsync(&file)
        .context("PdfDocument読み込み失敗")?
        .join()
        .context("PdfDocument非同期読み込み失敗")?;

    doc.PageCount().context("ページ数取得失敗")
}

/// 単一PDFページをDecodedImageにレンダリングする（150 DPI）
pub fn render_pdf_page(pdf_path: &Path, page_index: u32) -> Result<DecodedImage> {
    let clean_path = crate::util::strip_extended_length_prefix(pdf_path);
    let path_str = clean_path.to_str().unwrap_or("");
    if path_str.is_empty() {
        anyhow::bail!("無効なパス: {}", pdf_path.display());
    }

    let hstring = windows::core::HSTRING::from(path_str);
    let file = StorageFile::GetFileFromPathAsync(&hstring)
        .context("StorageFile取得失敗")?
        .join()
        .context("StorageFile非同期取得失敗")?;

    let doc = PdfDocument::LoadFromFileAsync(&file)
        .context("PdfDocument読み込み失敗")?
        .join()
        .context("PdfDocument非同期読み込み失敗")?;

    let page_count = doc.PageCount().context("ページ数取得失敗")?;
    if page_index >= page_count {
        bail!("ページインデックスが範囲外: {page_index} (総ページ数: {page_count})");
    }

    let page = doc.GetPage(page_index).context("ページ取得失敗")?;
    let size = page.Size().context("ページサイズ取得失敗")?;

    // 150 DPIにスケール
    let render_width = (size.Width as f64 * DPI_SCALE) as u32;
    let render_height = (size.Height as f64 * DPI_SCALE) as u32;

    // レンダリングオプション設定
    let options =
        windows::Data::Pdf::PdfPageRenderOptions::new().context("PdfPageRenderOptions作成失敗")?;
    options
        .SetDestinationWidth(render_width)
        .context("幅設定失敗")?;
    options
        .SetDestinationHeight(render_height)
        .context("高さ設定失敗")?;

    // メモリストリームにレンダリング（PNG形式で出力される）
    let stream = InMemoryRandomAccessStream::new().context("ストリーム作成失敗")?;
    page.RenderWithOptionsToStreamAsync(&stream, &options)
        .context("レンダリング開始失敗")?
        .join()
        .context("レンダリング失敗")?;

    // ストリームからバイト列を読み出す
    stream.Seek(0).context("ストリームSeek失敗")?;
    let size = stream.Size().context("ストリームサイズ取得失敗")? as u32;
    let reader = DataReader::CreateDataReader(&stream).context("DataReader作成失敗")?;
    reader
        .LoadAsync(size)
        .context("データ読み込み開始失敗")?
        .join()
        .context("データ読み込み失敗")?;

    let mut png_data = vec![0u8; size as usize];
    reader
        .ReadBytes(&mut png_data)
        .context("バイト読み出し失敗")?;

    // PNG → RGBA デコード（image crateを使用）
    let img = image::load_from_memory_with_format(&png_data, image::ImageFormat::Png)
        .context("PNGデコード失敗")?;
    let rgba = img.into_rgba8();

    Ok(DecodedImage {
        width: rgba.width(),
        height: rgba.height(),
        data: rgba.into_raw(),
    })
}

/// STAメインスレッドから安全にPDFページ数を取得する
/// WinRT非同期APIはSTAでブロッキング待ちするとデッドロックするため、
/// MTAスレッドに委譲して実行する。
pub fn get_pdf_page_count_safe(pdf_path: &Path) -> Result<u32> {
    let path = pdf_path.to_path_buf();
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let result = get_pdf_page_count(&path);
        unsafe {
            CoUninitialize();
        }
        result
    })
    .join()
    .map_err(|_| anyhow::anyhow!("PDFスレッドがパニック"))?
}

/// STAメインスレッドから安全にPDFページをレンダリングする
pub fn render_pdf_page_safe(pdf_path: &Path, page_index: u32) -> Result<DecodedImage> {
    let path = pdf_path.to_path_buf();
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let result = render_pdf_page(&path, page_index);
        unsafe {
            CoUninitialize();
        }
        result
    })
    .join()
    .map_err(|_| anyhow::anyhow!("PDFスレッドがパニック"))?
}
