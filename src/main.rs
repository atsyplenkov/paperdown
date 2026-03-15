mod cli;
mod core;

use anyhow::Result;
use clap::Parser;
use core::{collect_pdfs, PdfSummary, ProgressCallback, ProgressEvent};
use futures::stream::{self, StreamExt};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use serde::Serialize;
use std::io::IsTerminal;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

#[derive(Debug, Serialize)]
struct BatchFailure {
    pdf: String,
    error: String,
}

#[derive(Debug, Serialize)]
struct BatchReport {
    processed: usize,
    failed: usize,
    results: Vec<PdfSummary>,
    errors: Vec<BatchFailure>,
}

#[tokio::main]
async fn main() {
    let code = match run().await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err}");
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
        if args.verbose {
            eprintln!("Processing 1 PDF: {}", pdfs[0].display());
        }
        let summary = core::process_pdf(
            &pdfs[0],
            &args.output,
            &args.env_file,
            Duration::from_secs(args.timeout),
            args.max_download_bytes,
            args.overwrite,
            progress_callback(&pdfs[0], progress.clone()),
        )
        .await?;
        print_json_stdout(&summary)?;
        return Ok(0);
    }

    let workers = args.workers.min(pdfs.len()).max(1);
    eprintln!("Processing {} PDFs with {} workers...", pdfs.len(), workers);

    let semaphore = Arc::new(Semaphore::new(workers));
    let results = stream::iter(pdfs.into_iter().map(|pdf| {
        let permit_pool = semaphore.clone();
        let output = args.output.clone();
        let env_file = args.env_file.clone();
        let timeout = Duration::from_secs(args.timeout);
        let max_download_bytes = args.max_download_bytes;
        let overwrite = args.overwrite;
        let progress = progress.clone();
        async move {
            let _permit = permit_pool.acquire_owned().await.expect("semaphore");
            let res = core::process_pdf(
                &pdf,
                &output,
                &env_file,
                timeout,
                max_download_bytes,
                overwrite,
                progress_callback(&pdf, progress),
            )
            .await;
            (pdf, res)
        }
    }))
    .buffer_unordered(workers)
    .collect::<Vec<_>>()
    .await;

    let mut success = Vec::new();
    let mut errors = Vec::new();
    for (pdf, result) in results {
        match result {
            Ok(summary) => {
                if args.verbose {
                    eprintln!("  done: {}", pdf.display());
                }
                success.push(summary);
            }
            Err(err) => {
                eprintln!("  failed: {}: {err}", pdf.display());
                errors.push(BatchFailure {
                    pdf: pdf.display().to_string(),
                    error: err.to_string(),
                });
            }
        }
    }

    let report = BatchReport {
        processed: success.len(),
        failed: errors.len(),
        results: success,
        errors,
    };
    print_json_stdout(&report)?;
    Ok(if report.failed > 0 { 1 } else { 0 })
}

fn print_json_stdout<T: Serialize>(value: &T) -> Result<()> {
    let mut out = std::io::stdout();
    write_json(&mut out, value)
}

fn write_json<T: Serialize, W: Write>(out: &mut W, value: &T) -> Result<()> {
    let rendered = serde_json::to_string_pretty(value)?;
    writeln!(out, "{rendered}")?;
    Ok(())
}

fn stderr_is_tty() -> bool {
    std::io::stderr().is_terminal()
}

fn progress_callback(pdf: &PathBuf, multi: Option<Arc<MultiProgress>>) -> Option<ProgressCallback> {
    let multi = multi?;
    let label = pdf.display().to_string();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_json_produces_parsable_payload_with_expected_keys() {
        let value = BatchReport {
            processed: 1,
            failed: 0,
            results: Vec::new(),
            errors: Vec::new(),
        };

        let mut out = Vec::<u8>::new();
        write_json(&mut out, &value).expect("write json");
        let rendered = String::from_utf8(out).expect("utf8");
        let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("valid json");

        assert_eq!(parsed["processed"], 1);
        assert_eq!(parsed["failed"], 0);
        assert!(parsed.get("results").is_some());
        assert!(parsed.get("errors").is_some());
    }
}
