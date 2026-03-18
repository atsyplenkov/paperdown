use anyhow::Result;
use futures::stream::{self, StreamExt};
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

use super::markdown::replace_image_urls;
use super::{ProgressCallback, ProgressEvent, fire};

const INTERNAL_FIGURE_CONCURRENCY: usize = 16;

pub(crate) async fn localize_figures(
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

    let rewritten = replace_image_urls(&markdown, &replacements);
    Ok((
        rewritten,
        downloaded_figures,
        remote_figure_links,
        image_blocks,
    ))
}

pub(crate) fn extract_image_url(block: &Value) -> Option<String> {
    for key in ["content", "image_url", "crop_image_url", "url", "file_url"] {
        if let Some(value) = block.get(key)
            && let Some(found) = find_http_url(value)
        {
            return Some(found);
        }
    }
    None
}

pub(crate) fn find_http_url(value: &Value) -> Option<String> {
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

pub(crate) fn is_http_url(value: &str) -> bool {
    value.strip_prefix("http://").is_some() || value.strip_prefix("https://").is_some()
}

pub(crate) async fn download_figure(
    client: &reqwest::Client,
    url: &str,
    figures_dir: &Path,
    base: &str,
    max_download_bytes: u64,
) -> Option<String> {
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }

    if let Some(length) = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        && length > max_download_bytes
    {
        return None;
    }

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    if let Some(ctype) = content_type
        && !ctype.to_lowercase().starts_with("image/")
    {
        return None;
    }

    let suffix = content_type_to_suffix(content_type)
        .or_else(|| url_suffix(url))
        .unwrap_or_else(|| ".img".to_string());

    let filename = format!("{base}{suffix}");
    let output = figures_dir.join(&filename);
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        let next_len = bytes.len() as u64 + chunk.len() as u64;
        if next_len > max_download_bytes {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }

    tokio::fs::create_dir_all(figures_dir).await.ok()?;
    if tokio::fs::write(&output, &bytes).await.is_err() {
        return None;
    }

    Some(filename)
}

pub(crate) fn content_type_to_suffix(content_type: Option<&str>) -> Option<String> {
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

pub(crate) fn url_suffix(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let path = parsed.path();
    let ext = Path::new(path).extension()?.to_str()?;
    if ext.is_empty() {
        return None;
    }
    Some(format!(".{ext}"))
}
