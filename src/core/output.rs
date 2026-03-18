use anyhow::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::io::AsyncWriteExt;

#[derive(Debug)]
pub(crate) struct PreparedOutput {
    pub(crate) output_dir: PathBuf,
    pub(crate) figures_dir: PathBuf,
    pub(crate) markdown_path: PathBuf,
    pub(crate) log_path: PathBuf,
}

pub(crate) fn prepare_output_paths(
    output_root: &Path,
    pdf_path: &Path,
    overwrite: bool,
) -> Result<PreparedOutput> {
    let stem = pdf_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("Invalid PDF filename: {}", pdf_path.display()))?;

    let output_dir = output_root.join(stem);
    std::fs::create_dir_all(&output_dir)?;

    let markdown_path = output_dir.join("index.md");
    let figures_dir = output_dir.join("figures");
    let log_path = output_dir.join("log.jsonl");

    if !overwrite {
        if markdown_path.exists() {
            return Err(anyhow::anyhow!(
                "Output already exists: {}. Re-run with --overwrite",
                markdown_path.display()
            ));
        }
        if figures_dir.exists() {
            return Err(anyhow::anyhow!(
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

pub(crate) async fn append_log(log_path: &Path, entry: Value) -> Result<()> {
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

pub(crate) async fn atomic_write_text(path: &Path, content: &str) -> Result<()> {
    atomic_write_bytes(path, content.as_bytes()).await
}

pub(crate) async fn atomic_write_bytes(path: &Path, content: &[u8]) -> Result<()> {
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
