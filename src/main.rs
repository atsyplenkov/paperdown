mod cli;

use anyhow::Result;
use clap::Parser;
use futures::stream::{self, StreamExt};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use paperdown::core::{
    self, PdfSummary, ProcessPdfOptions, ProgressCallback, ProgressEvent, collect_pdfs,
};
use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

#[tokio::main]
async fn main() {
    let code = match run().await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {}", format_error_for_stderr(&err.to_string()));
            1
        }
    };
    std::process::exit(code);
}

async fn run() -> Result<i32> {
    let args = cli::Cli::parse();
    let pdfs = collect_pdfs(&args.input)?;
    let progress = if stderr_is_tty() {
        Some(Arc::new(MultiProgress::with_draw_target(
            ProgressDrawTarget::stderr(),
        )))
    } else {
        None
    };

    if pdfs.len() == 1 {
        if !args.overwrite && has_existing_log_marker(&args.output, &pdfs[0]) {
            print_single_skip_summary_stdout(&pdfs[0]);
            return Ok(0);
        }
        if args.verbose {
            eprintln!("Processing 1 PDF: {}", pdfs[0].display());
        }
        let summary = core::process_pdf(
            &pdfs[0],
            &args.output,
            &args.env_file,
            ProcessPdfOptions {
                timeout: Duration::from_secs(args.timeout),
                max_download_bytes: args.max_download_bytes,
                overwrite: args.overwrite,
                normalize_tables: args.normalize_tables,
                progress: progress_callback(&pdfs[0], progress.clone()),
            },
        )
        .await?;
        print_single_summary_stdout(&summary);
        return Ok(0);
    }

    let total_inputs = pdfs.len();
    let mut skipped_count = 0usize;
    let mut process_pdfs = Vec::new();
    for pdf in pdfs {
        if !args.overwrite && has_existing_log_marker(&args.output, &pdf) {
            skipped_count += 1;
        } else {
            process_pdfs.push(pdf);
        }
    }

    if process_pdfs.is_empty() {
        let counts = batch_accounting(total_inputs, 0, skipped_count, 0, 0);
        print_batch_summary_stdout(
            counts.processed,
            counts.skipped,
            counts.failed,
            counts.figures,
        );
        return Ok(0);
    }

    let workers = args.workers.min(process_pdfs.len()).max(1);
    let ocr_workers = effective_ocr_workers(workers, args.ocr_workers);
    eprintln!(
        "Processing {} PDFs with {} workers (OCR concurrency: {})...",
        process_pdfs.len(),
        workers,
        ocr_workers
    );

    let semaphore = Arc::new(Semaphore::new(workers));
    let ocr_semaphore = Arc::new(Semaphore::new(ocr_workers));
    let results = stream::iter(process_pdfs.into_iter().map(|pdf| {
        let permit_pool = semaphore.clone();
        let ocr_limiter = ocr_semaphore.clone();
        let output = args.output.clone();
        let env_file = args.env_file.clone();
        let progress = progress.clone();
        let options = ProcessPdfOptions {
            timeout: Duration::from_secs(args.timeout),
            max_download_bytes: args.max_download_bytes,
            overwrite: args.overwrite,
            normalize_tables: args.normalize_tables,
            progress: progress_callback(&pdf, progress),
        };
        async move {
            let _permit = permit_pool.acquire_owned().await.expect("semaphore");
            let res = core::process_pdf_with_ocr_limiter(
                &pdf,
                &output,
                &env_file,
                options,
                Some(ocr_limiter),
            )
            .await;
            (pdf, res)
        }
    }))
    .buffer_unordered(workers)
    .collect::<Vec<_>>()
    .await;

    let mut failed_count = 0usize;
    let mut success_count = 0usize;
    let mut downloaded_figures = 0usize;
    for (pdf, result) in results {
        match result {
            Ok(summary) => {
                success_count += 1;
                downloaded_figures += summary.downloaded_figures;
                if args.verbose {
                    eprintln!("  done: {}", pdf.display());
                }
            }
            Err(err) => {
                let rendered = format_error_for_stderr(&err.to_string());
                eprintln!("  failed: {}: {rendered}", pdf.display());
                failed_count += 1;
            }
        }
    }

    let counts = batch_accounting(
        total_inputs,
        success_count,
        skipped_count,
        failed_count,
        downloaded_figures,
    );
    print_batch_summary_stdout(
        counts.processed,
        counts.skipped,
        counts.failed,
        counts.figures,
    );
    Ok(if counts.failed > 0 { 1 } else { 0 })
}

fn stderr_is_tty() -> bool {
    std::io::stderr().is_terminal()
}

fn effective_ocr_workers(workers: usize, ocr_workers: usize) -> usize {
    workers.min(ocr_workers).max(1)
}

fn format_error_for_stderr(message: &str) -> String {
    if stderr_is_tty() {
        return message.replace("--overwrite", "\x1b[1;33m--overwrite\x1b[0m");
    }
    message.to_string()
}

fn stdout_is_tty() -> bool {
    std::io::stdout().is_terminal()
}

fn has_existing_log_marker(output_root: &Path, pdf: &Path) -> bool {
    let Some(stem) = pdf.file_stem() else {
        return false;
    };
    let log_path = output_root.join(stem).join("log.jsonl");
    if !log_path.is_file() {
        return false;
    }

    let Ok(contents) = std::fs::read_to_string(&log_path) else {
        return true;
    };
    let Some(last_line) = contents.lines().rev().find(|line| !line.trim().is_empty()) else {
        return true;
    };
    let Ok(entry) = serde_json::from_str::<serde_json::Value>(last_line) else {
        return true;
    };
    let Some(pdf_path) = entry.get("pdf_path").and_then(|value| value.as_str()) else {
        return true;
    };

    pdf_path == pdf.display().to_string()
}

fn print_single_skip_summary_stdout(pdf: &Path) {
    if stdout_is_tty() {
        println!("\x1b[1;33mSkipped\x1b[0m {}", display_path(pdf));
    } else {
        println!("Skipped {}", display_path(pdf));
    }
}

fn print_single_summary_stdout(summary: &PdfSummary) {
    if stdout_is_tty() {
        println!(
            "\x1b[1;32mDone\x1b[0m {}",
            display_path(Path::new(&summary.pdf))
        );
        println!(
            "\x1b[36m->\x1b[0m markdown: {}",
            display_path(Path::new(&summary.markdown_path))
        );
        println!(
            "\x1b[36m->\x1b[0m downloaded figures: \x1b[1m{}\x1b[0m",
            summary.downloaded_figures
        );
    } else {
        println!(
            "Done {} | markdown: {} | downloaded figures: {}",
            display_path(Path::new(&summary.pdf)),
            display_path(Path::new(&summary.markdown_path)),
            summary.downloaded_figures
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BatchAccounting {
    processed: usize,
    skipped: usize,
    failed: usize,
    figures: usize,
}

fn batch_accounting(
    total_inputs: usize,
    processed: usize,
    skipped: usize,
    failed: usize,
    figures: usize,
) -> BatchAccounting {
    assert_eq!(
        processed + skipped + failed,
        total_inputs,
        "batch accounting invariant violated"
    );
    BatchAccounting {
        processed,
        skipped,
        failed,
        figures,
    }
}

fn print_batch_summary_stdout(processed: usize, skipped: usize, failed: usize, figures: usize) {
    if stdout_is_tty() {
        let color = if failed == 0 {
            "\x1b[1;32m"
        } else {
            "\x1b[1;33m"
        };
        println!(
            "{color}Batch Complete\x1b[0m processed: \x1b[1m{processed}\x1b[0m skipped: \x1b[1m{skipped}\x1b[0m failed: \x1b[1m{failed}\x1b[0m figures: \x1b[1m{figures}\x1b[0m"
        );
    } else {
        println!(
            "Batch Complete processed: {processed} skipped: {skipped} failed: {failed} figures: {figures}"
        );
    }
}

fn progress_callback(pdf: &Path, multi: Option<Arc<MultiProgress>>) -> Option<ProgressCallback> {
    let multi = multi?;
    let label = display_path(pdf);
    let spinner = multi.add(ProgressBar::new_spinner());
    spinner.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg}")
            .expect("spinner template")
            .tick_chars("-\\|/ "),
    );
    spinner.set_message(format!("{label} OCR"));
    spinner.enable_steady_tick(Duration::from_millis(90));

    let markdown_pb = multi.add(ProgressBar::new(1));
    markdown_pb.set_style(
        ProgressStyle::with_template("{bar:20.cyan/blue} {bytes}/{total_bytes} {msg}")
            .expect("markdown template"),
    );
    markdown_pb.set_message(format!("{label} markdown"));
    markdown_pb.finish_and_clear();

    let figures_pb = multi.add(ProgressBar::new(1));
    figures_pb.set_style(
        ProgressStyle::with_template("{bar:20.green/blue} {pos}/{len} {msg}")
            .expect("figure template"),
    );
    figures_pb.set_message(format!("{label} figures"));
    figures_pb.finish_and_clear();

    let cb = move |event: ProgressEvent| match event {
        ProgressEvent::OcrStarted => {
            spinner.enable_steady_tick(Duration::from_millis(90));
        }
        ProgressEvent::OcrFinished => {
            spinner.finish_with_message(format!("{label} OCR done"));
        }
        ProgressEvent::MarkdownWriteStarted { bytes } => {
            markdown_pb.reset();
            markdown_pb.set_length(bytes as u64);
            markdown_pb.set_position(0);
        }
        ProgressEvent::MarkdownWriteFinished => {
            let len = markdown_pb.length().unwrap_or(1);
            markdown_pb.set_position(len);
            markdown_pb.finish_with_message(format!("{label} markdown written"));
        }
        ProgressEvent::FigureScanStarted { total } => {
            figures_pb.reset();
            figures_pb.set_length(total as u64);
            figures_pb.set_position(0);
        }
        ProgressEvent::FigureDownloadFinished => {
            figures_pb.inc(1);
            if figures_pb.position() >= figures_pb.length().unwrap_or(0) {
                figures_pb.finish_with_message(format!("{label} figures downloaded"));
            }
        }
    };

    Some(Arc::new(cb))
}

fn display_path(path: &Path) -> String {
    if let Ok(cwd) = std::env::current_dir()
        && let Ok(rel) = path.strip_prefix(cwd)
    {
        return rel.display().to_string();
    }
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn display_path_uses_relative_path_when_possible() {
        let cwd = std::env::current_dir().expect("cwd");
        let abs = cwd.join("pdf").join("paper.pdf");
        assert_eq!(
            display_path(&abs),
            PathBuf::from("pdf").join("paper.pdf").display().to_string()
        );
    }

    #[test]
    fn display_path_falls_back_to_file_name() {
        let path = PathBuf::from("/tmp/example.pdf");
        assert_eq!(display_path(&path), "example.pdf");
    }

    #[test]
    fn format_error_for_stderr_rewrites_overwrite_token_when_tty() {
        let message = "Re-run with --overwrite";
        let rendered = format_error_for_stderr(message);
        if stderr_is_tty() {
            assert!(rendered.contains("\x1b[1;33m--overwrite\x1b[0m"));
        } else {
            assert_eq!(rendered, message);
        }
    }

    #[test]
    fn print_helpers_execute_without_panic() {
        let summary = PdfSummary {
            pdf: "/tmp/paper.pdf".to_string(),
            output_dir: "/tmp/out/paper".to_string(),
            markdown_path: "/tmp/out/paper/index.md".to_string(),
            downloaded_figures: 2,
            remote_figure_links: 3,
            image_blocks: 3,
            usage: None,
            log_path: "/tmp/out/paper/log.jsonl".to_string(),
        };
        print_single_summary_stdout(&summary);
        print_single_skip_summary_stdout(Path::new(&summary.pdf));
        print_batch_summary_stdout(2, 1, 1, 4);
    }

    #[test]
    fn progress_callback_returns_none_without_multi_progress() {
        assert!(progress_callback(Path::new("paper.pdf"), None).is_none());
    }

    #[test]
    fn progress_callback_handles_all_events() {
        let multi = Arc::new(MultiProgress::with_draw_target(ProgressDrawTarget::hidden()));
        let callback = progress_callback(Path::new("paper.pdf"), Some(multi)).unwrap();
        callback(ProgressEvent::OcrStarted);
        callback(ProgressEvent::OcrFinished);
        callback(ProgressEvent::MarkdownWriteStarted { bytes: 16 });
        callback(ProgressEvent::MarkdownWriteFinished);
        callback(ProgressEvent::FigureScanStarted { total: 2 });
        callback(ProgressEvent::FigureDownloadFinished);
        callback(ProgressEvent::FigureDownloadFinished);
    }

    #[test]
    fn effective_ocr_workers_caps_to_total_workers() {
        assert_eq!(effective_ocr_workers(32, 2), 2);
        assert_eq!(effective_ocr_workers(8, 32), 8);
        assert_eq!(effective_ocr_workers(1, 2), 1);
    }

    #[test]
    fn has_existing_log_marker_returns_false_for_pdf_path_mismatch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let output_root = temp.path();
        let pdf = Path::new("/input/current/paper.pdf");
        let log_path = output_root.join("paper").join("log.jsonl");
        std::fs::create_dir_all(log_path.parent().expect("log parent")).expect("create log dir");
        std::fs::write(
            &log_path,
            "{\"pdf_path\":\"/input/current/paper.pdf\"}\n\n{\"pdf_path\":\"/input/other/paper.pdf\"}\n",
        )
        .expect("write log marker");

        assert!(!has_existing_log_marker(output_root, pdf));
    }

    mod main {
        use super::*;

        mod tests {
            use super::*;

            #[test]
            fn batch_accounting_mixed_outcomes_is_consistent() {
                let counts = batch_accounting(5, 2, 1, 2, 7);
                assert_eq!(
                    counts,
                    BatchAccounting {
                        processed: 2,
                        skipped: 1,
                        failed: 2,
                        figures: 7
                    }
                );
            }
        }
    }
}
