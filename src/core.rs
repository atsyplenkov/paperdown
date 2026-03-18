use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

mod assets;
mod input;
mod markdown;
mod ocr;
mod output;

pub fn collect_pdfs(input_path: &Path) -> Result<Vec<std::path::PathBuf>> {
    input::collect_pdfs(input_path)
}

#[derive(Clone, Debug)]
pub enum ProgressEvent {
    OcrStarted,
    OcrFinished,
    MarkdownWriteStarted { bytes: usize },
    MarkdownWriteFinished,
    FigureScanStarted { total: usize },
    FigureDownloadFinished,
}

pub type ProgressCallback = Arc<dyn Fn(ProgressEvent) + Send + Sync>;

#[derive(Debug, Serialize, Clone)]
pub struct PdfSummary {
    pub pdf: String,
    pub output_dir: String,
    pub markdown_path: String,
    pub downloaded_figures: usize,
    pub remote_figure_links: usize,
    pub image_blocks: usize,
    pub usage: Option<Value>,
    pub log_path: String,
}

pub async fn process_pdf(
    pdf_path: &Path,
    output_root: &Path,
    env_file: &Path,
    timeout: Duration,
    max_download_bytes: u64,
    overwrite: bool,
    progress: Option<ProgressCallback>,
) -> Result<PdfSummary> {
    let run_started = Instant::now();
    let pdf_path = pdf_path
        .canonicalize()
        .with_context(|| format!("PDF not found: {}", pdf_path.display()))?;
    if !pdf_path.is_file() || !input::is_pdf_path(&pdf_path) {
        return Err(anyhow!("Input must be a PDF: {}", pdf_path.display()));
    }
    let prepared = output::prepare_output_paths(output_root, &pdf_path, overwrite)?;
    let client = reqwest::Client::builder().timeout(timeout).build()?;

    let api_key = input::load_api_key(env_file)?;
    let payload = ocr::build_payload(&pdf_path).await?;
    fire(&progress, ProgressEvent::OcrStarted);
    let ocr_started = Instant::now();
    let response = ocr::call_layout_parsing(&client, &api_key, payload).await?;
    let ocr_seconds = ocr_started.elapsed();
    fire(&progress, ProgressEvent::OcrFinished);

    let (markdown, layout_details, usage) = ocr::validate_layout_response(response)?;

    let figure_started = Instant::now();
    let (markdown, downloaded_figures, remote_figure_links, image_blocks) =
        assets::localize_figures(
            markdown,
            &layout_details,
            &client,
            &prepared.figures_dir,
            max_download_bytes,
            progress.clone(),
        )
        .await?;
    let figure_seconds = figure_started.elapsed();
    let markdown = markdown::strip_html_img_alt_attributes(&markdown);

    fire(
        &progress,
        ProgressEvent::MarkdownWriteStarted {
            bytes: markdown.len(),
        },
    );
    let write_started = Instant::now();
    output::atomic_write_text(&prepared.markdown_path, &markdown).await?;
    fire(&progress, ProgressEvent::MarkdownWriteFinished);

    output::append_log(
        &prepared.log_path,
        json!({
            "timestamp_utc": OffsetDateTime::now_utc().format(&Rfc3339)?,
            "pdf_path": pdf_path.display().to_string(),
            "output_dir": prepared.output_dir.display().to_string(),
            "markdown_path": prepared.markdown_path.display().to_string(),
            "downloaded_figures": downloaded_figures,
            "remote_figure_links": remote_figure_links,
            "image_blocks": image_blocks,
            "usage": usage,
            "timing": {
                "ocr_call_s": round3(ocr_seconds),
                "figure_processing_s": round3(figure_seconds),
                "write_and_log_s": round3(write_started.elapsed()),
                "total_s": round3(run_started.elapsed()),
            }
        }),
    )
    .await?;

    Ok(PdfSummary {
        pdf: pdf_path.display().to_string(),
        output_dir: prepared.output_dir.display().to_string(),
        markdown_path: prepared.markdown_path.display().to_string(),
        downloaded_figures,
        remote_figure_links,
        image_blocks,
        usage,
        log_path: prepared.log_path.display().to_string(),
    })
}

fn fire(progress: &Option<ProgressCallback>, event: ProgressEvent) {
    if let Some(cb) = progress {
        cb(event);
    }
}

fn round3(duration: Duration) -> f64 {
    ((duration.as_secs_f64() * 1000.0).round()) / 1000.0
}

#[cfg(feature = "internal-testing")]
#[doc(hidden)]
pub mod testing {
    pub use super::ProgressCallback;
    pub use super::ProgressEvent;
    pub use super::process_pdf;
    use anyhow::Result;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::path::Path;
    use std::time::Duration;

    #[derive(Debug)]
    pub struct PreparedOutputPaths {
        pub output_dir: std::path::PathBuf,
        pub figures_dir: std::path::PathBuf,
        pub markdown_path: std::path::PathBuf,
        pub log_path: std::path::PathBuf,
    }

    pub fn load_api_key(env_file: &Path) -> Result<String> {
        super::input::load_api_key(env_file)
    }

    pub async fn build_payload(pdf_path: &Path) -> Result<Value> {
        super::ocr::build_payload(pdf_path).await
    }

    pub fn validate_layout_response(data: Value) -> Result<(String, Vec<Value>, Option<Value>)> {
        super::ocr::validate_layout_response(data)
    }

    pub fn content_type_to_suffix(content_type: Option<&str>) -> Option<String> {
        super::assets::content_type_to_suffix(content_type)
    }

    pub fn url_suffix(url: &str) -> Option<String> {
        super::assets::url_suffix(url)
    }

    pub fn extract_image_url(block: &Value) -> Option<String> {
        super::assets::extract_image_url(block)
    }

    pub fn is_http_url(value: &str) -> bool {
        super::assets::is_http_url(value)
    }

    #[cfg(feature = "net-tests")]
    pub async fn download_figure(
        client: &reqwest::Client,
        url: &str,
        figures_dir: &Path,
        base: &str,
        max_download_bytes: u64,
    ) -> Option<String> {
        super::assets::download_figure(client, url, figures_dir, base, max_download_bytes).await
    }

    #[cfg(feature = "net-tests")]
    pub async fn localize_figures(
        markdown: String,
        layout_details: &[Value],
        client: &reqwest::Client,
        figures_dir: &Path,
        max_download_bytes: u64,
        progress: Option<ProgressCallback>,
    ) -> Result<(String, usize, usize, usize)> {
        super::assets::localize_figures(
            markdown,
            layout_details,
            client,
            figures_dir,
            max_download_bytes,
            progress,
        )
        .await
    }

    pub fn replace_image_urls(markdown: &str, replacements: &HashMap<String, String>) -> String {
        super::markdown::replace_image_urls(markdown, replacements)
    }

    pub fn strip_html_img_alt_attributes(markdown: &str) -> String {
        super::markdown::strip_html_img_alt_attributes(markdown)
    }

    pub fn prepare_output_paths(
        output_root: &Path,
        pdf_path: &Path,
        overwrite: bool,
    ) -> Result<PreparedOutputPaths> {
        let prepared = super::output::prepare_output_paths(output_root, pdf_path, overwrite)?;
        Ok(PreparedOutputPaths {
            output_dir: prepared.output_dir,
            figures_dir: prepared.figures_dir,
            markdown_path: prepared.markdown_path,
            log_path: prepared.log_path,
        })
    }

    pub async fn append_log(log_path: &Path, entry: Value) -> Result<()> {
        super::output::append_log(log_path, entry).await
    }

    pub async fn atomic_write_text(path: &Path, content: &str) -> Result<()> {
        super::output::atomic_write_text(path, content).await
    }

    pub fn fire_for_test(progress: &Option<ProgressCallback>, event: ProgressEvent) {
        super::fire(progress, event);
    }

    pub fn round3_for_test(duration: Duration) -> f64 {
        super::round3(duration)
    }
}
