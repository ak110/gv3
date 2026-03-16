//! PDFページレンダリング
//!
//! Windows.Data.Pdf API を使ってPDFの各ページを画像にレンダリングする。
//! WinRT APIはスレッドセーフではないため、呼び出しごとにPdfDocumentを新規オープンする。

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use windows::Data::Pdf::PdfDocument;
use windows::Storage::StorageFile;
use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};

use crate::image::DecodedImage;

/// レンダリング解像度スケール（150 DPI / 96 DPI ≈ 1.5625）
const DPI_SCALE: f64 = 1.5625;

/// PDFのページ数を取得する（高速: ページデータはロードしない）
pub fn get_pdf_page_count(pdf_path: &Path) -> Result<u32> {
    let path_str = pdf_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("無効なパス: {}", pdf_path.display()))?;

    let hstring = windows::core::HSTRING::from(path_str);
    let file = StorageFile::GetFileFromPathAsync(&hstring)
        .context("StorageFile取得失敗")?
        .get()
        .context("StorageFile非同期取得失敗")?;

    let doc = PdfDocument::LoadFromFileAsync(&file)
        .context("PdfDocument読み込み失敗")?
        .get()
        .context("PdfDocument非同期読み込み失敗")?;

    doc.PageCount().context("ページ数取得失敗")
}

/// 単一PDFページをDecodedImageにレンダリングする（150 DPI）
pub fn render_pdf_page(pdf_path: &Path, page_index: u32) -> Result<DecodedImage> {
    let path_str = pdf_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("無効なパス: {}", pdf_path.display()))?;

    let hstring = windows::core::HSTRING::from(path_str);
    let file = StorageFile::GetFileFromPathAsync(&hstring)
        .context("StorageFile取得失敗")?
        .get()
        .context("StorageFile非同期取得失敗")?;

    let doc = PdfDocument::LoadFromFileAsync(&file)
        .context("PdfDocument読み込み失敗")?
        .get()
        .context("PdfDocument非同期読み込み失敗")?;

    let page_count = doc.PageCount().context("ページ数取得失敗")?;
    if page_index >= page_count {
        bail!(
            "ページインデックスが範囲外: {} (総ページ数: {})",
            page_index,
            page_count
        );
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
        .get()
        .context("レンダリング失敗")?;

    // ストリームからバイト列を読み出す
    stream.Seek(0).context("ストリームSeek失敗")?;
    let size = stream.Size().context("ストリームサイズ取得失敗")? as u32;
    let reader = DataReader::CreateDataReader(&stream).context("DataReader作成失敗")?;
    reader
        .LoadAsync(size)
        .context("データ読み込み開始失敗")?
        .get()
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
