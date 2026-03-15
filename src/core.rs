use anyhow::{anyhow, Context, Result};
use futures::stream::{self, StreamExt};
use regex::Regex;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, USER_AGENT};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::fs;
use tokio::io::AsyncWriteExt;

const API_URL: &str = "https://api.z.ai/api/paas/v4/layout_parsing";
const INTERNAL_FIGURE_CONCURRENCY: usize = 16;

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

#[derive(Debug)]
struct PreparedOutput {
    output_dir: PathBuf,
    figures_dir: PathBuf,
    markdown_path: PathBuf,
    log_path: PathBuf,
}

pub fn collect_pdfs(input_path: &Path) -> Result<Vec<PathBuf>> {
    let input_path = input_path
        .canonicalize()
        .with_context(|| format!("Input path does not exist: {}", input_path.display()))?;

    if input_path.is_file() {
        if !is_pdf_path(&input_path) {
            return Err(anyhow!("Input must be a PDF: {}", input_path.display()));
        }
        return Ok(vec![input_path]);
    }

    if input_path.is_dir() {
        let mut pdfs = Vec::new();
        for entry in std::fs::read_dir(&input_path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && is_pdf_path(&path) {
                pdfs.push(path);
            }
        }
        pdfs.sort();
        if pdfs.is_empty() {
            return Err(anyhow!("No PDF files found in: {}", input_path.display()));
        }
        return Ok(pdfs);
    }

    Err(anyhow!(
        "Input path does not exist: {}",
        input_path.display()
    ))
}

fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

pub fn load_api_key(env_file: &Path) -> Result<String> {
    if let Ok(api_key) = std::env::var("ZAI_API_KEY") {
        if !api_key.trim().is_empty() {
            return Ok(api_key);
        }
    }

    let content = std::fs::read_to_string(env_file).with_context(|| {
        format!(
            "ZAI_API_KEY is not set and env file was not found: {}",
            env_file.display()
        )
    })?;

    for line in content.lines() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') || !stripped.contains('=') {
            continue;
        }
        let mut split = stripped.splitn(2, '=');
        let key = split.next().unwrap_or_default().trim();
        let value = split
            .next()
            .unwrap_or_default()
            .trim()
            .trim_matches('"')
            .trim_matches('\'');
        if key == "ZAI_API_KEY" && !value.is_empty() {
            return Ok(value.to_string());
        }
    }

    Err(anyhow!(
        "ZAI_API_KEY was not found in {}",
        env_file.display()
    ))
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
    if !pdf_path.is_file() || !is_pdf_path(&pdf_path) {
        return Err(anyhow!("Input must be a PDF: {}", pdf_path.display()));
    }
    let prepared = prepare_output_paths(output_root, &pdf_path, overwrite)?;
    let client = reqwest::Client::builder().timeout(timeout).build()?;

    let api_key = load_api_key(env_file)?;
    let payload = build_payload(&pdf_path).await?;
    fire(&progress, ProgressEvent::OcrStarted);
    let ocr_started = Instant::now();
    let response = call_layout_parsing(&client, &api_key, payload).await?;
    let ocr_seconds = ocr_started.elapsed();
    fire(&progress, ProgressEvent::OcrFinished);

    let (markdown, layout_details, usage) = validate_layout_response(response)?;

    let figure_started = Instant::now();
    let (markdown, downloaded_figures, remote_figure_links, image_blocks) = localize_figures(
        markdown,
        &layout_details,
        &client,
        &prepared.figures_dir,
        max_download_bytes,
        progress.clone(),
    )
    .await?;
    let figure_seconds = figure_started.elapsed();

    fire(
        &progress,
        ProgressEvent::MarkdownWriteStarted {
            bytes: markdown.len(),
        },
    );
    let write_started = Instant::now();
    atomic_write_text(&prepared.markdown_path, &markdown).await?;
    fire(&progress, ProgressEvent::MarkdownWriteFinished);

    append_log(
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

async fn build_payload(pdf_path: &Path) -> Result<Value> {
    let bytes = fs::read(pdf_path).await?;
    let encoded = {
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine;
        STANDARD.encode(bytes)
    };
    Ok(json!({
        "model": "glm-ocr",
        "file": format!("data:application/pdf;base64,{encoded}"),
        "return_crop_images": true
    }))
}

async fn call_layout_parsing(
    client: &reqwest::Client,
    api_key: &str,
    payload: Value,
) -> Result<Value> {
    let response = client
        .post(API_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&payload)
        .send()
        .await
        .context("Could not reach Z.AI OCR API")?;

    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(anyhow!(
            "Z.AI OCR request failed with HTTP {}: {}",
            status.as_u16(),
            text
        ));
    }

    let parsed: Value =
        serde_json::from_str(&text).context("Z.AI OCR API returned invalid JSON")?;
    if !parsed.is_object() {
        return Err(anyhow!("Z.AI OCR API returned an unexpected response type"));
    }
    Ok(parsed)
}

fn validate_layout_response(data: Value) -> Result<(String, Vec<Value>, Option<Value>)> {
    let markdown = data
        .get("md_results")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Z.AI OCR response is missing string field 'md_results'"))?
        .to_string();

    let layout_details = data
        .get("layout_details")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Z.AI OCR response is missing list field 'layout_details'"))?
        .clone();

    let usage = data.get("usage").filter(|v| v.is_object()).cloned();
    Ok((markdown, layout_details, usage))
}

fn prepare_output_paths(
    output_root: &Path,
    pdf_path: &Path,
    overwrite: bool,
) -> Result<PreparedOutput> {
    let stem = pdf_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("Invalid PDF filename: {}", pdf_path.display()))?;

    let output_dir = output_root.join(stem);
    std::fs::create_dir_all(&output_dir)?;

    let markdown_path = output_dir.join("index.md");
    let figures_dir = output_dir.join("figures");
    let log_path = output_dir.join("log.jsonl");

    if !overwrite {
        if markdown_path.exists() {
            return Err(anyhow!(
                "Output already exists: {}. Re-run with --overwrite",
                markdown_path.display()
            ));
        }
        if figures_dir.exists() {
            return Err(anyhow!(
                "Output already exists: {}. Re-run with --overwrite",
                figures_dir.display()
            ));
        }
    } else {
        if markdown_path.exists() {
            std::fs::remove_file(&markdown_path)?;
        }
        if figures_dir.exists() {
            if figures_dir.is_dir() {
                std::fs::remove_dir_all(&figures_dir)?;
            } else {
                std::fs::remove_file(&figures_dir)?;
            }
        }
    }

    std::fs::create_dir_all(&figures_dir)?;

    Ok(PreparedOutput {
        output_dir,
        figures_dir,
        markdown_path,
        log_path,
    })
}

async fn localize_figures(
    markdown: String,
    layout_details: &[Value],
    client: &reqwest::Client,
    figures_dir: &Path,
    max_download_bytes: u64,
    progress: Option<ProgressCallback>,
) -> Result<(String, usize, usize, usize)> {
    let mut remote_figure_links = 0usize;
    let mut image_blocks = 0usize;
    let mut first_url_order: Vec<(String, String)> = Vec::new();
    let mut seen: HashMap<String, String> = HashMap::new();

    for (page_index, page_blocks) in layout_details.iter().enumerate() {
        let Some(blocks) = page_blocks.as_array() else {
            continue;
        };
        for (block_index, block) in blocks.iter().enumerate() {
            if block.get("label").and_then(Value::as_str) != Some("image") {
                continue;
            }
            image_blocks += 1;
            let Some(remote_url) = extract_image_url(block) else {
                continue;
            };
            remote_figure_links += 1;
            if !seen.contains_key(&remote_url) {
                let base = format!("fig-{:03}-{:03}", page_index + 1, block_index + 1);
                seen.insert(remote_url.clone(), base.clone());
                first_url_order.push((remote_url, base));
            }
        }
    }

    fire(
        &progress,
        ProgressEvent::FigureScanStarted {
            total: first_url_order.len(),
        },
    );

    let figure_cap = INTERNAL_FIGURE_CONCURRENCY.max(1);
    let tasks = first_url_order.iter().map(|(url, base)| {
        let url = url.clone();
        let base = base.clone();
        let figures_dir = figures_dir.to_path_buf();
        let client = client.clone();
        let progress = progress.clone();
        async move {
            let downloaded =
                download_figure(&client, &url, &figures_dir, &base, max_download_bytes).await;
            if downloaded.is_some() {
                fire(&progress, ProgressEvent::FigureDownloadFinished);
            }
            (url, downloaded)
        }
    });

    let results = stream::iter(tasks)
        .buffer_unordered(figure_cap)
        .collect::<Vec<_>>()
        .await;

    let mut replacements: HashMap<String, String> = HashMap::new();
    let mut downloaded_figures = 0usize;
    for (url, local) in results {
        if let Some(local_path) = local {
            downloaded_figures += 1;
            replacements.insert(url, format!("figures/{}", local_path));
        }
    }

    let rewritten = replace_image_urls(&markdown, &replacements)?;
    Ok((
        rewritten,
        downloaded_figures,
        remote_figure_links,
        image_blocks,
    ))
}

fn extract_image_url(block: &Value) -> Option<String> {
    for key in ["content", "image_url", "crop_image_url", "url", "file_url"] {
        if let Some(value) = block.get(key) {
            if let Some(found) = find_http_url(value) {
                return Some(found);
            }
        }
    }
    None
}

fn find_http_url(value: &Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        if is_http_url(s) {
            return Some(s.to_string());
        }
        return None;
    }

    if let Some(array) = value.as_array() {
        for item in array {
            if let Some(found) = find_http_url(item) {
                return Some(found);
            }
        }
    }

    if let Some(map) = value.as_object() {
        for item in map.values() {
            if let Some(found) = find_http_url(item) {
                return Some(found);
            }
        }
    }

    None
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

async fn download_figure(
    client: &reqwest::Client,
    remote_url: &str,
    figures_dir: &Path,
    base_name: &str,
    max_download_bytes: u64,
) -> Option<String> {
    let parsed = url::Url::parse(remote_url).ok()?;
    let scheme = parsed.scheme().to_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }

    let response = client
        .get(remote_url)
        .header(USER_AGENT, "paperdown/0.1.0")
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        return None;
    }

    if let Some(length) = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
    {
        if length > max_download_bytes {
            return None;
        }
    }

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());

    if let Some(ref ctype) = content_type {
        if !ctype.to_lowercase().starts_with("image/") {
            return None;
        }
    }

    let mut stream = response.bytes_stream();
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        bytes.extend_from_slice(&chunk);
        if bytes.len() as u64 > max_download_bytes {
            return None;
        }
    }

    let suffix = content_type_to_suffix(content_type.as_deref())
        .or_else(|| url_suffix(remote_url))
        .unwrap_or_else(|| ".img".to_string());

    let filename = format!("{base_name}{suffix}");
    let output = figures_dir.join(&filename);
    if atomic_write_bytes(&output, &bytes).await.is_err() {
        return None;
    }

    Some(filename)
}

fn content_type_to_suffix(content_type: Option<&str>) -> Option<String> {
    let ct = content_type?.split(';').next()?.trim().to_ascii_lowercase();
    let suffix = match ct.as_str() {
        "image/jpeg" => ".jpg",
        "image/jpg" => ".jpg",
        "image/png" => ".png",
        "image/webp" => ".webp",
        "image/gif" => ".gif",
        "image/svg+xml" => ".svg",
        "image/bmp" => ".bmp",
        "image/tiff" => ".tif",
        _ => return None,
    };
    Some(suffix.to_string())
}

fn url_suffix(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let path = parsed.path();
    let ext = Path::new(path).extension()?.to_str()?;
    if ext.is_empty() {
        return None;
    }
    Some(format!(".{ext}"))
}

fn replace_image_urls(markdown: &str, replacements: &HashMap<String, String>) -> Result<String> {
    let markdown_pattern = Regex::new(r"\((https?://[^)\s]+)\)")?;
    let html_pattern = Regex::new(r#"(src\s*=\s*)(['"])(https?://[^'"]+)(['"])"#)?;

    let updated = markdown_pattern
        .replace_all(markdown, |caps: &regex::Captures<'_>| {
            let remote_url = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            let replacement = replacements
                .get(remote_url)
                .cloned()
                .unwrap_or_else(|| remote_url.to_string());
            format!("({replacement})")
        })
        .into_owned();

    Ok(html_pattern
        .replace_all(&updated, |caps: &regex::Captures<'_>| {
            let prefix = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            let quote = caps.get(2).map(|m| m.as_str()).unwrap_or("\"");
            let remote_url = caps.get(3).map(|m| m.as_str()).unwrap_or_default();
            let suffix = caps.get(4).map(|m| m.as_str()).unwrap_or_default();
            let replacement = replacements
                .get(remote_url)
                .cloned()
                .unwrap_or_else(|| remote_url.to_string());
            format!("{prefix}{quote}{replacement}{suffix}")
        })
        .into_owned())
}

async fn append_log(log_path: &Path, entry: Value) -> Result<()> {
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .await?;
    let line = serde_json::to_string(&entry)?;
    file.write_all(line.as_bytes()).await?;
    file.write_all(b"\n").await?;
    Ok(())
}

async fn atomic_write_text(path: &Path, content: &str) -> Result<()> {
    atomic_write_bytes(path, content.as_bytes()).await
}

async fn atomic_write_bytes(path: &Path, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_path = path.with_extension(format!("tmp-{}-{seed}", std::process::id()));
    let mut temp_file = fs::File::create(&temp_path).await?;
    temp_file.write_all(content).await?;
    temp_file.flush().await?;
    drop(temp_file);
    fs::rename(&temp_path, path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn collect_single_pdf() {
        let tmp = TempDir::new().unwrap();
        let pdf = tmp.path().join("a.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();
        let items = collect_pdfs(&pdf).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].file_name().unwrap(), "a.pdf");
    }

    #[test]
    fn collect_sorted_directory_pdfs() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("b.pdf"), b"%PDF").unwrap();
        std::fs::write(tmp.path().join("a.pdf"), b"%PDF").unwrap();
        std::fs::write(tmp.path().join("notes.txt"), b"x").unwrap();
        let items = collect_pdfs(tmp.path()).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].file_name().unwrap(), "a.pdf");
        assert_eq!(items[1].file_name().unwrap(), "b.pdf");
    }

    #[test]
    fn validate_layout_response_requires_fields() {
        assert!(validate_layout_response(json!({"layout_details": []})).is_err());
        assert!(validate_layout_response(json!({"md_results": "# h"})).is_err());
    }

    #[test]
    fn replace_image_urls_exact_matches_only() {
        let markdown = "exact ![](https://x/fig.png) query ![](https://x/fig.png?v=1)";
        let mut replacements = HashMap::new();
        replacements.insert(
            "https://x/fig.png".to_string(),
            "figures/f1.png".to_string(),
        );
        let updated = replace_image_urls(markdown, &replacements).unwrap();
        assert!(updated.contains("![](figures/f1.png)"));
        assert!(updated.contains("![](https://x/fig.png?v=1)"));
    }

    #[test]
    fn replace_image_urls_html_src() {
        let markdown = "<img src='https://x/fig.png' alt='x'/>";
        let mut replacements = HashMap::new();
        replacements.insert(
            "https://x/fig.png".to_string(),
            "figures/f1.png".to_string(),
        );
        let updated = replace_image_urls(markdown, &replacements).unwrap();
        assert_eq!(updated, "<img src='figures/f1.png' alt='x'/>");
    }

    #[test]
    fn prepare_output_without_overwrite_fails_on_existing_managed_artifacts() {
        let tmp = TempDir::new().unwrap();
        let pdf = tmp.path().join("paper.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();
        let target = tmp.path().join("out").join("paper");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("index.md"), b"old").unwrap();

        let err = prepare_output_paths(&tmp.path().join("out"), &pdf, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--overwrite"));
    }

    #[test]
    fn prepare_output_without_overwrite_fails_when_only_figures_exists() {
        let tmp = TempDir::new().unwrap();
        let pdf = tmp.path().join("paper.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();
        let target = tmp.path().join("out").join("paper");
        std::fs::create_dir_all(target.join("figures")).unwrap();

        let err = prepare_output_paths(&tmp.path().join("out"), &pdf, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("figures"));
        assert!(err.contains("--overwrite"));
    }

    #[test]
    fn prepare_output_without_overwrite_fails_when_both_exist() {
        let tmp = TempDir::new().unwrap();
        let pdf = tmp.path().join("paper.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();
        let target = tmp.path().join("out").join("paper");
        std::fs::create_dir_all(target.join("figures")).unwrap();
        std::fs::write(target.join("index.md"), b"old").unwrap();

        let err = prepare_output_paths(&tmp.path().join("out"), &pdf, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("index.md"));
        assert!(err.contains("--overwrite"));
    }

    #[test]
    fn prepare_output_with_overwrite_preserves_unrelated_files() {
        let tmp = TempDir::new().unwrap();
        let pdf = tmp.path().join("paper.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();

        let out = tmp.path().join("out").join("paper");
        let figures = out.join("figures");
        std::fs::create_dir_all(&figures).unwrap();
        std::fs::write(out.join("index.md"), b"old").unwrap();
        std::fs::write(figures.join("stale.png"), b"old").unwrap();
        std::fs::write(out.join("keep.txt"), b"keep").unwrap();

        let prepared = prepare_output_paths(&tmp.path().join("out"), &pdf, true).unwrap();
        assert!(prepared.figures_dir.exists());
        assert!(!prepared.figures_dir.join("stale.png").exists());
        assert!(out.join("keep.txt").exists());
    }

    #[test]
    fn prepare_output_with_overwrite_handles_figures_file() {
        let tmp = TempDir::new().unwrap();
        let pdf = tmp.path().join("paper.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();
        let out = tmp.path().join("out").join("paper");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("figures"), b"stale").unwrap();

        let prepared = prepare_output_paths(&tmp.path().join("out"), &pdf, true).unwrap();
        assert!(prepared.figures_dir.is_dir());
    }

    #[test]
    fn extract_image_url_checks_fallback_keys() {
        let block = json!({
            "label": "image",
            "image_url": "https://example.com/fig.png"
        });
        assert_eq!(
            extract_image_url(&block),
            Some("https://example.com/fig.png".to_string())
        );
    }

    #[test]
    fn non_http_urls_rejected() {
        assert!(url::Url::parse("file:///tmp/a.png").is_ok());
        assert!(!is_http_url("file:///tmp/a.png"));
    }

    #[test]
    fn load_api_key_prefers_environment_variable() {
        let _guard = env_lock().lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let env_file = tmp.path().join(".env");
        std::fs::write(&env_file, "ZAI_API_KEY=file-key\n").unwrap();

        std::env::set_var("ZAI_API_KEY", "env-key");
        let key = load_api_key(&env_file).unwrap();
        std::env::remove_var("ZAI_API_KEY");

        assert_eq!(key, "env-key");
    }

    #[test]
    fn load_api_key_parses_quoted_value() {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("ZAI_API_KEY");
        let tmp = TempDir::new().unwrap();
        let env_file = tmp.path().join(".env");
        std::fs::write(&env_file, "ZAI_API_KEY=\"quoted-key\"\n").unwrap();

        let key = load_api_key(&env_file).unwrap();
        assert_eq!(key, "quoted-key");
    }

    #[tokio::test]
    async fn process_pdf_checks_output_conflict_before_env_lookup() {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("ZAI_API_KEY");

        let tmp = TempDir::new().unwrap();
        let pdf = tmp.path().join("paper.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();

        let output_root = tmp.path().join("out");
        let output_dir = output_root.join("paper");
        std::fs::create_dir_all(&output_dir).unwrap();
        std::fs::write(output_dir.join("index.md"), b"existing").unwrap();

        let missing_env = tmp.path().join("missing.env");
        let err = process_pdf(
            &pdf,
            &output_root,
            &missing_env,
            Duration::from_secs(1),
            1024,
            false,
            None,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("Re-run with --overwrite"));
        assert!(!err.contains("ZAI_API_KEY"));
    }
}
