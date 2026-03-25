use clap::{ArgAction, Parser};
use std::path::PathBuf;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "paperdown",
    version,
    about = "Convert academic PDF files into markdown with local figures via Z.AI OCR.",
    long_about = "paperdown converts one PDF or a directory of PDFs into markdown output folders.\n\n\
For each PDF, it creates:\n\
- <output>/<pdf_stem>/index.md\n\
- <output>/<pdf_stem>/figures/\n\
- <output>/<pdf_stem>/tables/ (when --normalize-tables is enabled)\n\
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
Without --overwrite, existing index.md or figures/ causes a failure.\n  \
When --normalize-tables is enabled, existing tables/ also causes a failure.\n  \
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
        default_value = ".env",
        help = "Path to .env file checked first for ZAI_API_KEY, before environment fallback."
    )]
    pub env_file: PathBuf,

    #[arg(
        long,
        default_value_t = 180u64,
        value_parser = clap::value_parser!(u64).range(1..),
        help = "HTTP timeout in seconds for OCR requests and figure downloads."
    )]
    pub timeout: u64,

    #[arg(
        long = "max-download-bytes",
        default_value_t = 20_971_520u64,
        value_parser = clap::value_parser!(u64).range(1..),
        help = "Maximum allowed size (bytes) for each downloaded figure file."
    )]
    pub max_download_bytes: u64,

    #[arg(
        long,
        default_value_t = default_workers(),
        value_parser = parse_positive_usize,
        help = "Maximum number of PDFs processed concurrently in batch mode."
    )]
    pub workers: usize,

    #[arg(
        long = "ocr-workers",
        default_value_t = 2usize,
        value_parser = parse_positive_usize,
        help = "Maximum number of concurrent OCR API calls in batch mode; effective OCR concurrency is min(--workers, --ocr-workers)."
    )]
    pub ocr_workers: usize,

    #[arg(
        short = 'v',
        long,
        action = ArgAction::SetTrue,
        help = "Enable verbose progress messages on stderr."
    )]
    pub verbose: bool,

    #[arg(
        long,
        action = ArgAction::SetTrue,
        help = "Replace existing managed output artifacts (index.md and figures/)."
    )]
    pub overwrite: bool,

    #[arg(
        long = "normalize-tables",
        action = ArgAction::SetTrue,
        help = "Normalize OCR HTML tables into Markdown and store raw HTML under tables/."
    )]
    pub normalize_tables: bool,
}

pub fn default_workers() -> usize {
    32
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
    fn default_workers_is_32() {
        assert_eq!(default_workers(), 32);
    }

    #[test]
    fn parses_defaults() {
        let cli = Cli::parse_from(["paperdown", "--input", "in.pdf"]);
        assert_eq!(cli.input, PathBuf::from("in.pdf"));
        assert_eq!(cli.output, PathBuf::from("md"));
        assert_eq!(cli.env_file, PathBuf::from(".env"));
        assert_eq!(cli.timeout, 180);
        assert_eq!(cli.max_download_bytes, 20_971_520);
        assert_eq!(cli.workers, default_workers());
        assert_eq!(cli.ocr_workers, 2);
        assert!(!cli.verbose);
        assert!(!cli.overwrite);
        assert!(!cli.normalize_tables);
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
        let file_first = help.find("1) ZAI_API_KEY from --env-file");
        let env_second = help.find("2) ZAI_API_KEY from environment");
        assert!(file_first.is_some());
        assert!(env_second.is_some());
        assert!(file_first.unwrap() < env_second.unwrap());
        assert!(help.contains("single .pdf file or a directory"));
        assert!(help.contains("min(--workers, --ocr-workers)"));
    }
}
