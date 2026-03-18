use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};

pub(crate) fn collect_pdfs(input_path: &Path) -> Result<Vec<PathBuf>> {
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

pub(crate) fn load_api_key(env_file: &Path) -> Result<String> {
    let env_file_exists = env_file
        .try_exists()
        .with_context(|| format!("Failed to access env file metadata: {}", env_file.display()))?;

    if env_file_exists {
        let entries = dotenvy::from_path_iter(env_file)
            .with_context(|| format!("Failed to read or parse env file: {}", env_file.display()))?;
        let mut file_key = None;
        for entry in entries {
            let (key, value) = entry.with_context(|| {
                format!("Failed to read or parse env file: {}", env_file.display())
            })?;
            if key == "ZAI_API_KEY" {
                if value.trim().is_empty() {
                    file_key = None;
                } else {
                    file_key = Some(value);
                }
            }
        }
        if let Some(key) = file_key {
            return Ok(key);
        }
    }

    if let Ok(api_key) = std::env::var("ZAI_API_KEY")
        && !api_key.trim().is_empty()
    {
        return Ok(api_key);
    }

    if !env_file_exists {
        return Err(anyhow!(
            "ZAI_API_KEY is not set and env file was not found: {}",
            env_file.display()
        ));
    }

    Err(anyhow!(
        "ZAI_API_KEY was not found in {}",
        env_file.display()
    ))
}

pub(crate) fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}
