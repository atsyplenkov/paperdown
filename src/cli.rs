use clap::{ArgAction, Parser};
use std::path::PathBuf;

#[derive(Debug, Clone, Parser)]
#[command(name = "paperdown", version, about = "Convert PDF inputs to markdown")]
pub struct Cli {
    #[arg(long, value_name = "PATH", required = true)]
    pub input: PathBuf,

    #[arg(long, default_value = "md")]
    pub output: PathBuf,

    #[arg(long = "env-file", default_value = ".env")]
    pub env_file: PathBuf,

    #[arg(long, default_value_t = 180u64, value_parser = clap::value_parser!(u64).range(1..))]
    pub timeout: u64,

    #[arg(
        long = "max-download-bytes",
        default_value_t = 20_971_520u64,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub max_download_bytes: u64,

    #[arg(
        long,
        default_value_t = default_workers(),
        value_parser = parse_positive_usize
    )]
    pub workers: usize,

    #[arg(short = 'v', long, action = ArgAction::SetTrue)]
    pub verbose: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub overwrite: bool,
}

pub fn default_workers() -> usize {
    let cpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    (cpu * 4).clamp(4, 32)
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
    use clap::Parser;

    #[test]
    fn default_workers_formula_bounds() {
        let workers = default_workers();
        assert!((4..=32).contains(&workers));
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
        assert!(!cli.verbose);
        assert!(!cli.overwrite);
    }

    #[test]
    fn rejects_zero_positive_fields() {
        assert!(Cli::try_parse_from(["paperdown", "--input", "in.pdf", "--timeout", "0"]).is_err());
        assert!(Cli::try_parse_from([
            "paperdown",
            "--input",
            "in.pdf",
            "--max-download-bytes",
            "0"
        ])
        .is_err());
        assert!(Cli::try_parse_from(["paperdown", "--input", "in.pdf", "--workers", "0"]).is_err());
    }
}
