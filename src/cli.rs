use crate::config;
use clap::{ArgAction, Parser};
use std::path::PathBuf;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "paperdown",
    version,
    about = "Convert academic PDF files into markdown with local figures via Z.AI OCR.",
    long_about = "paperdown converts one PDF or a directory of PDFs into markdown output folders.\n\n\
For each processed PDF, it writes:\n\
- <output>/<pdf_stem>/index.md\n\
- <output>/<pdf_stem>/figures/ only when at least one figure is downloaded\n\
- <output>/<pdf_stem>/tables/ only when --normalize-tables writes raw OCR tables\n\
- <output>/<pdf_stem>/log.jsonl\n\n\
API key lookup order:\n\
1) ZAI_API_KEY from --env-file\n\
2) ZAI_API_KEY from environment",
    after_help = "Examples:\n  \
paperdown --input pdf/paper.pdf\n  \
paperdown --input pdf/ --output md/ --workers 4\n  \
paperdown --input pdf/ --output md/ --overwrite\n  \
paperdown --input pdf/ --output md/ --normalize-tables\n\n\
Notes:\n  \
Without --overwrite, an existing <output>/<pdf_stem>/log.jsonl marker skips the PDF.\n  \
If the log marker is missing, paperdown treats the PDF as unprocessed and refreshes managed artifacts (index.md, figures/, and tables/ when enabled).\n  \
With --overwrite, the whole <output>/<pdf_stem>/ folder is replaced.\n  \
Progress bars are shown on stderr only when running in a TTY."
)]
pub struct Cli {
    #[arg(
        long,
        value_name = "PATH",
        required = true,
        help = "Input path: a single .pdf file or a directory containing .pdf files."
    )]
    pub input: PathBuf,

    #[arg(
        long,
        default_value = "md",
        help = "Output root directory for generated markdown folders."
    )]
    pub output: PathBuf,

    #[arg(
        long = "env-file",
        value_name = "ENV_FILE",
        help = "Path to .env file checked first for ZAI_API_KEY, before environment fallback."
    )]
    pub env_file: Option<PathBuf>,

    #[arg(
        long,
        value_name = "PATH",
        help = "Path to a paperdown.toml config file; when set, automatic global/local discovery is disabled."
    )]
    pub config: Option<PathBuf>,

    #[arg(
        long,
        value_parser = clap::value_parser!(u64).range(1..),
        help = "HTTP timeout in seconds for OCR requests and figure downloads."
    )]
    pub timeout: Option<u64>,

    #[arg(
        long = "max-download-bytes",
        value_parser = clap::value_parser!(u64).range(1..),
        help = "Maximum allowed size (bytes) for each downloaded figure file."
    )]
    pub max_download_bytes: Option<u64>,

    #[arg(
        long,
        value_parser = parse_positive_usize,
        help = "Maximum number of PDFs processed concurrently in batch mode."
    )]
    pub workers: Option<usize>,

    #[arg(
        long = "ocr-workers",
        value_parser = parse_positive_usize,
        help = "Maximum number of concurrent OCR API calls in batch mode; effective OCR concurrency is min(--workers, --ocr-workers)."
    )]
    pub ocr_workers: Option<usize>,

    #[arg(
        short = 'v',
        long,
        action = ArgAction::SetTrue,
        conflicts_with = "quiet",
        help = "Enable verbose progress messages on stderr."
    )]
    pub verbose: bool,

    #[arg(
        long,
        action = ArgAction::SetTrue,
        help = "Disable verbose progress messages from config."
    )]
    pub quiet: bool,

    #[arg(
        long,
        action = ArgAction::SetTrue,
        conflicts_with = "no_overwrite",
        help = "Replace the whole <output>/<pdf_stem>/ folder before processing."
    )]
    pub overwrite: bool,

    #[arg(
        long = "no-overwrite",
        action = ArgAction::SetTrue,
        help = "Disable overwrite when enabled by config."
    )]
    pub no_overwrite: bool,

    #[arg(
        long = "normalize-tables",
        action = ArgAction::SetTrue,
        conflicts_with = "no_normalize_tables",
        help = "Normalize OCR HTML tables into Markdown and store raw HTML under tables/."
    )]
    pub normalize_tables: bool,

    #[arg(
        long = "no-normalize-tables",
        action = ArgAction::SetTrue,
        help = "Disable table normalization when enabled by config."
    )]
    pub no_normalize_tables: bool,
}

impl Cli {
    pub fn config_overrides(&self) -> config::ConfigOverrides {
        config::ConfigOverrides {
            env_file: self.env_file.clone(),
            timeout: self.timeout,
            max_download_bytes: self.max_download_bytes,
            workers: self.workers,
            ocr_workers: self.ocr_workers,
            verbose: bool_override(self.verbose, self.quiet),
            overwrite: bool_override(self.overwrite, self.no_overwrite),
            normalize_tables: bool_override(self.normalize_tables, self.no_normalize_tables),
        }
    }
}

fn bool_override(enable: bool, disable: bool) -> Option<bool> {
    match (enable, disable) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

fn parse_positive_usize(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("invalid integer: {value}"))?;
    if parsed == 0 {
        return Err("must be greater than 0".to_string());
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn config_overrides_capture_only_explicit_cli_values() {
        let cli = Cli::parse_from([
            "paperdown",
            "--input",
            "in.pdf",
            "--env-file",
            "custom.env",
            "--timeout",
            "9",
            "--max-download-bytes",
            "99",
            "--workers",
            "3",
            "--ocr-workers",
            "2",
            "--verbose",
            "--overwrite",
            "--normalize-tables",
        ]);

        assert_eq!(
            cli.config_overrides(),
            config::ConfigOverrides {
                env_file: Some(PathBuf::from("custom.env")),
                timeout: Some(9),
                max_download_bytes: Some(99),
                workers: Some(3),
                ocr_workers: Some(2),
                verbose: Some(true),
                overwrite: Some(true),
                normalize_tables: Some(true),
            }
        );
    }

    #[test]
    fn disabling_flags_produce_false_config_overrides() {
        let cli = Cli::parse_from([
            "paperdown",
            "--input",
            "in.pdf",
            "--quiet",
            "--no-overwrite",
            "--no-normalize-tables",
        ]);

        assert_eq!(
            cli.config_overrides(),
            config::ConfigOverrides {
                verbose: Some(false),
                overwrite: Some(false),
                normalize_tables: Some(false),
                ..config::ConfigOverrides::default()
            }
        );
    }

    #[test]
    fn rejects_zero_positive_fields() {
        assert!(Cli::try_parse_from(["paperdown", "--input", "in.pdf", "--timeout", "0"]).is_err());
        assert!(
            Cli::try_parse_from([
                "paperdown",
                "--input",
                "in.pdf",
                "--max-download-bytes",
                "0"
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["paperdown", "--input", "in.pdf", "--workers", "0"]).is_err());
        assert!(
            Cli::try_parse_from(["paperdown", "--input", "in.pdf", "--ocr-workers", "0"]).is_err()
        );
    }

    #[test]
    fn help_text_contains_examples_and_key_guidance() {
        let mut cmd = Cli::command();
        let help = cmd.render_long_help().to_string();
        assert!(help.contains("Examples:"));
        assert!(help.contains("--overwrite"));
        assert!(help.contains("--normalize-tables"));
        assert!(help.contains("--config"));
        assert!(help.contains(
            "Without --overwrite, an existing <output>/<pdf_stem>/log.jsonl marker skips the PDF."
        ));
        assert!(help.contains(
            "If the log marker is missing, paperdown treats the PDF as unprocessed and refreshes managed artifacts (index.md, figures/, and tables/ when enabled)."
        ));
        assert!(
            help.contains("With --overwrite, the whole <output>/<pdf_stem>/ folder is replaced.")
        );
        let file_first = help.find("1) ZAI_API_KEY from --env-file");
        let env_second = help.find("2) ZAI_API_KEY from environment");
        assert!(file_first.is_some());
        assert!(env_second.is_some());
        assert!(file_first.unwrap() < env_second.unwrap());
        assert!(help.contains("single .pdf file or a directory"));
        assert!(help.contains("min(--workers, --ocr-workers)"));
    }
}
