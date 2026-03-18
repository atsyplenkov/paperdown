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

pub(crate) use input::collect_pdfs;

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
#[cfg(test)]
mod tests {
    use super::assets::{content_type_to_suffix, extract_image_url, is_http_url, url_suffix};
    #[cfg(feature = "net-tests")]
    use super::assets::{download_figure, localize_figures};
    use super::input::{collect_pdfs, load_api_key};
    use super::markdown::{replace_image_urls, strip_html_img_alt_attributes};
    use super::ocr::{build_payload, validate_layout_response};
    use super::output::{append_log, atomic_write_text, prepare_output_paths};
    use super::{ProgressCallback, ProgressEvent, fire, process_pdf, round3};
    #[cfg(feature = "net-tests")]
    use httpmock::prelude::*;
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;
    use tempfile::TempDir;
    #[cfg(feature = "net-tests")]
    use tokio::io::AsyncReadExt;
    #[cfg(feature = "net-tests")]
    use tokio::net::TcpListener;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn collect_pdfs_rejects_non_pdf_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("notes.txt");
        std::fs::write(&file, b"plain-text").unwrap();
        let err = collect_pdfs(&file).unwrap_err().to_string();
        assert!(err.contains("Input must be a PDF"));
    }

    #[test]
    fn collect_pdfs_rejects_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let err = collect_pdfs(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("No PDF files found"));
    }

    #[test]
    fn collect_pdfs_rejects_missing_path() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("missing.pdf");
        let err = collect_pdfs(&missing).unwrap_err().to_string();
        assert!(err.contains("Input path does not exist"));
    }

    #[test]
    fn load_api_key_blank_env_falls_back_to_file() {
        let _guard = env_lock().lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let env_file = tmp.path().join(".env");
        std::fs::write(&env_file, "ZAI_API_KEY=file-key\n").unwrap();
        unsafe {
            std::env::set_var("ZAI_API_KEY", "   ");
        }
        let key = load_api_key(&env_file).unwrap();
        unsafe {
            std::env::remove_var("ZAI_API_KEY");
        }
        assert_eq!(key, "file-key");
    }

    #[test]
    fn load_api_key_missing_file_is_error() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::remove_var("ZAI_API_KEY");
        }
        let tmp = TempDir::new().unwrap();
        let err = load_api_key(&tmp.path().join("missing.env"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("env file was not found"));
    }

    #[test]
    fn load_api_key_missing_key_in_file_is_error() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::remove_var("ZAI_API_KEY");
        }
        let tmp = TempDir::new().unwrap();
        let env_file = tmp.path().join(".env");
        std::fs::write(&env_file, "OTHER_KEY=x\n# ZAI_API_KEY=hidden\n").unwrap();
        let err = load_api_key(&env_file).unwrap_err().to_string();
        assert!(err.contains("ZAI_API_KEY was not found"));
    }

    #[test]
    fn load_api_key_missing_file_falls_back_to_environment() {
        let _guard = env_lock().lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("missing.env");
        unsafe {
            std::env::set_var("ZAI_API_KEY", "env-fallback-key");
        }
        let key = load_api_key(&missing).unwrap();
        unsafe {
            std::env::remove_var("ZAI_API_KEY");
        }
        assert_eq!(key, "env-fallback-key");
    }

    #[test]
    fn load_api_key_blank_file_value_falls_back_to_environment() {
        let _guard = env_lock().lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let env_file = tmp.path().join(".env");
        std::fs::write(&env_file, "ZAI_API_KEY=   \n").unwrap();
        unsafe {
            std::env::set_var("ZAI_API_KEY", "env-fallback-key");
        }
        let key = load_api_key(&env_file).unwrap();
        unsafe {
            std::env::remove_var("ZAI_API_KEY");
        }
        assert_eq!(key, "env-fallback-key");
    }

    #[test]
    fn load_api_key_duplicate_entries_last_wins() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::remove_var("ZAI_API_KEY");
        }
        let tmp = TempDir::new().unwrap();
        let env_file = tmp.path().join(".env");
        std::fs::write(&env_file, "ZAI_API_KEY=first\nZAI_API_KEY=second\n").unwrap();
        let key = load_api_key(&env_file).unwrap();
        assert_eq!(key, "second");
    }

    #[test]
    fn load_api_key_export_statement_is_parsed() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::remove_var("ZAI_API_KEY");
        }
        let tmp = TempDir::new().unwrap();
        let env_file = tmp.path().join(".env");
        std::fs::write(&env_file, "export ZAI_API_KEY=from-export\n").unwrap();
        let key = load_api_key(&env_file).unwrap();
        assert_eq!(key, "from-export");
    }

    #[test]
    fn load_api_key_interpolation_follows_dotenvy_behavior() {
        let _guard = env_lock().lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let env_file = tmp.path().join(".env");
        unsafe {
            std::env::set_var("BASE_KEY", "root");
            std::env::remove_var("ZAI_API_KEY");
        }
        std::fs::write(&env_file, "ZAI_API_KEY=${BASE_KEY}-suffix\n").unwrap();
        let key = load_api_key(&env_file).unwrap();
        unsafe {
            std::env::remove_var("BASE_KEY");
        }
        assert_eq!(key, "root-suffix");
    }

    #[test]
    fn load_api_key_invalid_file_is_hard_error() {
        let _guard = env_lock().lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let env_file = tmp.path().join(".env");
        unsafe {
            std::env::set_var("ZAI_API_KEY", "env-fallback-key");
        }
        std::fs::write(&env_file, "ZAI_API_KEY='unterminated\n").unwrap();
        let err = load_api_key(&env_file).unwrap_err().to_string();
        unsafe {
            std::env::remove_var("ZAI_API_KEY");
        }
        assert!(err.contains("Failed to read or parse env file"));
    }

    #[cfg(unix)]
    #[test]
    fn load_api_key_unreadable_file_does_not_fall_back_to_environment() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock().lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let locked_dir = tmp.path().join("locked");
        std::fs::create_dir_all(&locked_dir).unwrap();
        let env_file = locked_dir.join(".env");
        std::fs::write(&env_file, "ZAI_API_KEY=file-key\n").unwrap();
        unsafe {
            std::env::set_var("ZAI_API_KEY", "env-fallback-key");
        }
        std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        let err = load_api_key(&env_file).unwrap_err().to_string();
        std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        unsafe {
            std::env::remove_var("ZAI_API_KEY");
        }
        assert!(err.contains("Failed to access env file metadata"));
    }

    #[test]
    fn validate_layout_response_success_with_usage() {
        let (md, details, usage) = validate_layout_response(json!({
            "md_results": "# Title",
            "layout_details": [{"label": "image"}],
            "usage": {"tokens": 10}
        }))
        .unwrap();
        assert_eq!(md, "# Title");
        assert_eq!(details.len(), 1);
        assert_eq!(usage.unwrap()["tokens"], 10);
    }

    #[test]
    fn validate_layout_response_ignores_non_object_usage() {
        let (_, _, usage) = validate_layout_response(json!({
            "md_results": "# Title",
            "layout_details": [],
            "usage": 123
        }))
        .unwrap();
        assert!(usage.is_none());
    }

    #[test]
    fn build_payload_contains_pdf_data_uri() {
        let tmp = TempDir::new().unwrap();
        let pdf = tmp.path().join("paper.pdf");
        let bytes = b"%PDF-1.7\n";
        std::fs::write(&pdf, bytes).unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let payload = rt.block_on(build_payload(&pdf)).unwrap();

        assert_eq!(payload["model"], "glm-ocr");
        let encoded = payload["file"].as_str().unwrap();
        assert!(encoded.starts_with("data:application/pdf;base64,"));
        let raw = encoded.trim_start_matches("data:application/pdf;base64,");
        let decoded = {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(raw)
                .unwrap()
        };
        assert_eq!(decoded, bytes);
    }

    #[test]
    fn content_type_to_suffix_maps_known_values() {
        assert_eq!(
            content_type_to_suffix(Some("image/jpeg; charset=utf-8")),
            Some(".jpg".to_string())
        );
        assert_eq!(
            content_type_to_suffix(Some("IMAGE/PNG")),
            Some(".png".to_string())
        );
        assert_eq!(content_type_to_suffix(Some("text/plain")), None);
        assert_eq!(content_type_to_suffix(None), None);
    }

    #[test]
    fn url_suffix_handles_extensions() {
        assert_eq!(
            url_suffix("https://x/y/fig.png?v=1"),
            Some(".png".to_string())
        );
        assert_eq!(url_suffix("https://x/y/noext"), None);
        assert_eq!(url_suffix("not-a-url"), None);
    }

    #[test]
    fn append_log_appends_json_lines() {
        let tmp = TempDir::new().unwrap();
        let log = tmp.path().join("nested").join("log.jsonl");
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(append_log(&log, json!({"a": 1}))).unwrap();
        rt.block_on(append_log(&log, json!({"b": 2}))).unwrap();

        let content = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(serde_json::from_str::<Value>(lines[0]).unwrap()["a"], 1);
        assert_eq!(serde_json::from_str::<Value>(lines[1]).unwrap()["b"], 2);
    }

    #[test]
    fn atomic_write_text_creates_parent_and_overwrites() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("deep").join("file.txt");
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(atomic_write_text(&out, "first")).unwrap();
        rt.block_on(atomic_write_text(&out, "second")).unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "second");
    }

    #[test]
    fn fire_invokes_callback_when_present() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls2 = calls.clone();
        let cb: ProgressCallback = Arc::new(move |_| {
            calls2.fetch_add(1, Ordering::SeqCst);
        });
        fire(&Some(cb), ProgressEvent::OcrStarted);
        fire(&None, ProgressEvent::OcrFinished);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn round3_rounds_millis() {
        assert_eq!(round3(Duration::from_millis(1234)), 1.234);
    }

    #[cfg(feature = "net-tests")]
    async fn start_chunked_image_server(
        chunks: Vec<Vec<u8>>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::AsyncWriteExt as _;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut request_buf = [0u8; 1024];
            let _ = socket.read(&mut request_buf).await;
            let _ = socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .await;
            for chunk in chunks {
                let _ = socket
                    .write_all(format!("{:X}\r\n", chunk.len()).as_bytes())
                    .await;
                let _ = socket.write_all(&chunk).await;
                let _ = socket.write_all(b"\r\n").await;
            }
            let _ = socket.write_all(b"0\r\n\r\n").await;
        });
        (format!("http://{addr}"), handle)
    }

    #[cfg(feature = "net-tests")]
    #[test]
    fn download_figure_accepts_image_from_mock_server() {
        let server = MockServer::start();
        let image = vec![137, 80, 78, 71];
        let img_mock = server.mock(|when, then| {
            when.method(GET).path("/img");
            then.status(200)
                .header("content-type", "image/png")
                .body(image.clone());
        });

        let tmp = TempDir::new().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let name = rt.block_on(download_figure(
            &client,
            &server.url("/img"),
            tmp.path(),
            "fig-001-001",
            1024,
        ));

        img_mock.assert_hits(1);
        let name = name.unwrap();
        assert_eq!(name, "fig-001-001.png");
        assert!(tmp.path().join(name).exists());
    }

    #[cfg(feature = "net-tests")]
    #[test]
    fn download_figure_rejects_non_image_content_type() {
        let server = MockServer::start();
        let text_mock = server.mock(|when, then| {
            when.method(GET).path("/txt");
            then.status(200)
                .header("content-type", "text/plain")
                .body("nope");
        });

        let tmp = TempDir::new().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let result = rt.block_on(download_figure(
            &client,
            &server.url("/txt"),
            tmp.path(),
            "fig-001-001",
            1024,
        ));

        text_mock.assert_hits(1);
        assert!(result.is_none());
    }

    #[cfg(feature = "net-tests")]
    #[test]
    fn download_figure_streams_chunked_response_and_cleans_up_on_limit() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (url, server_task) = rt.block_on(start_chunked_image_server(vec![
            vec![1, 2, 3, 4],
            vec![5, 6, 7, 8],
        ]));

        let tmp = TempDir::new().unwrap();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let result = rt.block_on(download_figure(&client, &url, tmp.path(), "fig-001-001", 3));

        assert!(result.is_none());
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
        rt.block_on(server_task).unwrap();
    }

    #[cfg(feature = "net-tests")]
    #[test]
    fn download_figure_streams_chunked_response_successfully() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (url, server_task) =
            rt.block_on(start_chunked_image_server(vec![vec![1, 2], vec![3, 4]]));

        let tmp = TempDir::new().unwrap();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let name = rt.block_on(download_figure(
            &client,
            &url,
            tmp.path(),
            "fig-001-001",
            1024,
        ));

        assert_eq!(name.as_deref(), Some("fig-001-001.png"));
        let file = tmp.path().join(name.unwrap());
        assert_eq!(std::fs::read(&file).unwrap(), vec![1, 2, 3, 4]);
        rt.block_on(server_task).unwrap();
    }

    #[cfg(feature = "net-tests")]
    #[test]
    fn localize_figures_rewrites_markdown_and_tracks_progress() {
        let server = MockServer::start();
        let png_mock = server.mock(|when, then| {
            when.method(GET).path("/a");
            then.status(200)
                .header("content-type", "image/png")
                .body(vec![137, 80, 78, 71]);
        });
        let jpg_mock = server.mock(|when, then| {
            when.method(GET).path("/b");
            then.status(200)
                .header("content-type", "image/jpeg")
                .body(vec![255, 216, 255]);
        });

        let markdown = format!(
            "A ![]({}) B <img src='{}'/> A2 ![]({})",
            server.url("/a"),
            server.url("/b"),
            server.url("/a")
        );
        let layout_details = vec![
            json!([
                {"label": "image", "image_url": server.url("/a")},
                {"label": "text", "content": "ignore"},
                {"label": "image", "image_url": server.url("/a")}
            ]),
            json!([{"label": "image", "content": {"url": server.url("/b")}}]),
        ];

        let progress_events = Arc::new(Mutex::new(Vec::<ProgressEvent>::new()));
        let progress_ref = progress_events.clone();
        let progress: ProgressCallback = Arc::new(move |event| {
            progress_ref.lock().unwrap().push(event);
        });

        let tmp = TempDir::new().unwrap();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (rewritten, downloaded, remote_links, image_blocks) = rt
            .block_on(localize_figures(
                markdown,
                &layout_details,
                &client,
                tmp.path(),
                2048,
                Some(progress),
            ))
            .unwrap();

        png_mock.assert_hits(1);
        jpg_mock.assert_hits(1);
        assert_eq!(downloaded, 2);
        assert_eq!(remote_links, 3);
        assert_eq!(image_blocks, 3);
        assert!(rewritten.contains("figures/fig-001-001.png"));
        assert!(rewritten.contains("figures/fig-002-001.jpg"));
        assert!(tmp.path().join("fig-001-001.png").exists());
        assert!(tmp.path().join("fig-002-001.jpg").exists());

        let events = progress_events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProgressEvent::FigureScanStarted { total: 2 }))
        );
        let downloaded_events = events
            .iter()
            .filter(|e| matches!(e, ProgressEvent::FigureDownloadFinished))
            .count();
        assert_eq!(downloaded_events, 2);
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
        let updated = replace_image_urls(markdown, &replacements);
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
        let updated = replace_image_urls(markdown, &replacements);
        assert_eq!(updated, "<img src='figures/f1.png' alt='x'/>");
    }

    #[test]
    fn replace_image_urls_mixed_markdown_and_html() {
        let markdown = "start ![](https://x/a.png) mid <img src=\"https://x/b.jpg\" alt='x'/> end ![](https://x/c.png?query=1)";
        let mut replacements = HashMap::new();
        replacements.insert("https://x/a.png".to_string(), "figures/a.png".to_string());
        replacements.insert("https://x/b.jpg".to_string(), "figures/b.jpg".to_string());

        let updated = replace_image_urls(markdown, &replacements);
        assert_eq!(
            updated,
            "start ![](figures/a.png) mid <img src=\"figures/b.jpg\" alt='x'/> end ![](https://x/c.png?query=1)"
        );
    }

    #[test]
    fn replace_image_urls_no_replacements_passthrough() {
        let markdown = "plain text ![](https://x/a.png) <img src='https://x/b.jpg' alt='x'/>";
        let replacements = HashMap::new();

        let updated = replace_image_urls(markdown, &replacements);
        assert_eq!(updated, markdown);
    }

    #[test]
    fn strip_html_img_alt_attributes_removes_alt_and_preserves_other_attrs() {
        let markdown = "before <img src='x.png' alt='OCR图片' data-id='1'/> after";

        let updated = strip_html_img_alt_attributes(markdown);

        assert_eq!(updated, "before <img src='x.png' data-id='1'/> after");
    }

    #[test]
    fn strip_html_img_alt_attributes_handles_case_and_spacing() {
        let markdown = "<IMG\n  SRC=\"x.png\"\n  ALT=\"OCR图片\"\n  title='keep me'>";

        let updated = strip_html_img_alt_attributes(markdown);

        assert_eq!(updated, "<IMG\n  SRC=\"x.png\"\n  title='keep me'>");
    }

    #[test]
    fn strip_html_img_alt_attributes_leaves_markdown_and_code_unchanged() {
        let markdown = "![OCR图片](https://x/a.png)\n```html\n<img src='x.png' alt='OCR图片'/>\n```\n`<img src='y.png' alt='OCR图片'/>`";

        let updated = strip_html_img_alt_attributes(markdown);

        assert_eq!(
            updated,
            "![OCR图片](https://x/a.png)\n```html\n<img src='x.png' alt='OCR图片'/>\n```\n`<img src='y.png' alt='OCR图片'/>`"
        );
    }

    #[test]
    fn strip_html_img_alt_attributes_removes_multiple_alt_attributes() {
        let markdown = "<img alt='one' src='x.png' ALT=\"two\"/>";

        let updated = strip_html_img_alt_attributes(markdown);

        assert_eq!(updated, "<img src='x.png'/>");
    }

    #[test]
    fn strip_html_img_alt_attributes_removes_boolean_and_unquoted_alt() {
        let markdown = "<img alt src='x.png' alt=x data-id='1'>";

        let updated = strip_html_img_alt_attributes(markdown);

        assert_eq!(updated, "<img src='x.png' data-id='1'>");
    }

    #[test]
    fn strip_html_img_alt_attributes_keeps_localized_image_urls() {
        let markdown = "<img src='https://x/a.png' alt='OCR图片'/>";
        let mut replacements = HashMap::new();
        replacements.insert("https://x/a.png".to_string(), "figures/a.png".to_string());

        let localized = replace_image_urls(markdown, &replacements);
        let updated = strip_html_img_alt_attributes(&localized);

        assert_eq!(updated, "<img src='figures/a.png'/>");
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
    fn load_api_key_prefers_env_file_over_environment_variable() {
        let _guard = env_lock().lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let env_file = tmp.path().join(".env");
        std::fs::write(&env_file, "ZAI_API_KEY=file-key\n").unwrap();

        unsafe {
            std::env::set_var("ZAI_API_KEY", "env-key");
        }
        let key = load_api_key(&env_file).unwrap();
        unsafe {
            std::env::remove_var("ZAI_API_KEY");
        }

        assert_eq!(key, "file-key");
    }

    #[test]
    fn load_api_key_parses_quoted_value() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::remove_var("ZAI_API_KEY");
        }
        let tmp = TempDir::new().unwrap();
        let env_file = tmp.path().join(".env");
        std::fs::write(&env_file, "ZAI_API_KEY=\"quoted-key\"\n").unwrap();

        let key = load_api_key(&env_file).unwrap();
        assert_eq!(key, "quoted-key");
    }

    #[test]
    fn process_pdf_checks_output_conflict_before_env_lookup() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::remove_var("ZAI_API_KEY");
        }

        let tmp = TempDir::new().unwrap();
        let pdf = tmp.path().join("paper.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();

        let output_root = tmp.path().join("out");
        let output_dir = output_root.join("paper");
        std::fs::create_dir_all(&output_dir).unwrap();
        std::fs::write(output_dir.join("index.md"), b"existing").unwrap();

        let missing_env = tmp.path().join("missing.env");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(process_pdf(
                &pdf,
                &output_root,
                &missing_env,
                Duration::from_secs(1),
                1024,
                false,
                None,
            ))
            .unwrap_err()
            .to_string();

        assert!(err.contains("Re-run with --overwrite"));
        assert!(!err.contains("ZAI_API_KEY"));
    }
}
