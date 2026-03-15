mod cli;
mod core;

use anyhow::Result;
use clap::Parser;
use core::{collect_pdfs, ProgressCallback, ProgressEvent};
use futures::stream::{self, StreamExt};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
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
        core::process_pdf(
            &pdfs[0],
            &args.output,
            &args.env_file,
            Duration::from_secs(args.timeout),
            args.max_download_bytes,
            args.overwrite,
            progress_callback(&pdfs[0], progress.clone()),
        )
        .await?;
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

    let mut failed_count = 0usize;
    for (pdf, result) in results {
        match result {
            Ok(_) => {
                if args.verbose {
                    eprintln!("  done: {}", pdf.display());
                }
            }
            Err(err) => {
                eprintln!("  failed: {}: {err}", pdf.display());
                failed_count += 1;
            }
        }
    }

    Ok(if failed_count > 0 { 1 } else { 0 })
}

fn stderr_is_tty() -> bool {
    std::io::stderr().is_terminal()
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
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(rel) = path.strip_prefix(cwd) {
            return rel.display().to_string();
        }
    }
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn display_path_uses_relative_path_when_possible() {
        let cwd = std::env::current_dir().expect("cwd");
        let abs = cwd.join("pdf").join("paper.pdf");
        assert_eq!(
            display_path(&abs),
            PathBuf::from("pdf/paper.pdf").display().to_string()
        );
    }

    #[test]
    fn display_path_falls_back_to_file_name() {
        let path = PathBuf::from("/tmp/example.pdf");
        assert_eq!(display_path(&path), "example.pdf");
    }
}
