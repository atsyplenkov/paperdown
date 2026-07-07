use crate::config;
use clap::{ArgAction, Args, Parser, Subcommand};
use std::path::PathBuf;

const TOP_LEVEL_HELP_TEMPLATE: &str = r#"Usage: paperdown [OPTIONS] [COMMAND]

Commands:
  config       Configuration management
  doctor       Diagnose config, auth, and ...
  help         Print this message or the help of the given subcommand(s)

Options:
  -i, --input <INPUT>
          Input path: a single .pdf file or a directory containing .pdf files.

  -o, --output <OUTPUT>
          Output root directory for generated markdown files.

  -c, --config <CONFIG>
          Path to configuration file

  -e, --env <ENV>
          Path to .env file checked first for ZAI_API_KEY, before environment fallback.

  --timeout <TIMEOUT>
          HTTP timeout in seconds for OCR requests and figure downloads.

          [default: 180]

  --max-download-bytes <MAX_DOWNLOAD_BYTES>
          Maximum allowed size (bytes) for each downloaded figure file.

          [default: 20971520]

  --workers <WORKERS>
          Maximum number of PDFs processed concurrently in batch mode.

          [default: 32]

  --ocr-workers <OCR_WORKERS>
          Maximum number of concurrent OCR API calls in batch mode; effective OCR concurrency is min(--workers, --ocr-workers).

          [default: 2]

  -q, --quiet
          Don't print messages

  -v, --verbose
          Enable verbose progress messages on stderr.

  --overwrite
          Replace the whole <output>/<pdf_stem>/ folder before processing.

  -n, --normalize-tables
          Normalize OCR HTML tables into Markdown and store raw HTML under tables/.

  --okf
          Structure output as an Open Knowledge Format (OKF) bundle (manuscript.md, index.md with metadata, root index.md and log.md).

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
"#;

const CONFIG_HELP_TEMPLATE: &str = r#"Configuration management

Usage: paperdown config <COMMAND>

Commands:
  init   Generate a default configuration file
  check  Validate the configuration file
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help
"#;

const CONFIG_INIT_HELP_TEMPLATE: &str = r#"Generate a default configuration file

Usage: paperdown config init [OPTIONS]

Options:
  -f, --force  Overwrite existing configuration file
  -h, --help   Print help
"#;

const CONFIG_CHECK_HELP_TEMPLATE: &str = r#"Validate the configuration file

Check the config file for syntax errors and report any issues. Exit code 0 means valid, non-zero means file not found or has errors.

Usage: paperdown config check [OPTIONS]

Options:
  -c, --config <CONFIG>
          Path to configuration file to check (defaults to XDG config location)

  -h, --help
          Print help (see a summary with '-h')
"#;

const DOCTOR_HELP_TEMPLATE: &str = r#"Check your system for potential problems. Will exit with a non-zero status if
any potential problems are found.

Usage: paperdown doctor <COMMAND>

Commands:
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help
"#;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "paperdown",
    version,
    override_usage = "paperdown [OPTIONS] [COMMAND]",
    help_template = TOP_LEVEL_HELP_TEMPLATE
)]
pub struct Cli {
    #[arg(
        short = 'i',
        long,
        value_name = "INPUT",
        help = "Input path: a single .pdf file or a directory containing .pdf files."
    )]
    pub input: Option<PathBuf>,

    #[arg(
        short = 'o',
        long,
        value_name = "OUTPUT",
        default_value = "md",
        hide_default_value = true,
        help = "Output root directory for generated markdown files."
    )]
    pub output: PathBuf,

    #[arg(
        short = 'c',
        long,
        value_name = "CONFIG",
        help = "Path to configuration file"
    )]
    pub config: Option<PathBuf>,

    #[arg(
        short = 'e',
        long = "env",
        value_name = "ENV",
        help = "Path to .env file checked first for ZAI_API_KEY, before environment fallback."
    )]
    pub env_file: Option<PathBuf>,

    #[arg(
        long,
        value_name = "TIMEOUT",
        value_parser = clap::value_parser!(u64).range(1..),
        long_help = "HTTP timeout in seconds for OCR requests and figure downloads.\n\n[default: 180]"
    )]
    pub timeout: Option<u64>,

    #[arg(
        long = "max-download-bytes",
        value_name = "MAX_DOWNLOAD_BYTES",
        value_parser = clap::value_parser!(u64).range(1..),
        long_help = "Maximum allowed size (bytes) for each downloaded figure file.\n\n[default: 20971520]"
    )]
    pub max_download_bytes: Option<u64>,

    #[arg(
        long,
        value_name = "WORKERS",
        value_parser = parse_positive_usize,
        long_help = "Maximum number of PDFs processed concurrently in batch mode.\n\n[default: 32]"
    )]
    pub workers: Option<usize>,

    #[arg(
        long = "ocr-workers",
        value_name = "OCR_WORKERS",
        value_parser = parse_positive_usize,
        long_help = "Maximum number of concurrent OCR API calls in batch mode; effective OCR concurrency is min(--workers, --ocr-workers).\n\n[default: 2]"
    )]
    pub ocr_workers: Option<usize>,

    #[arg(
        short = 'q',
        long,
        action = ArgAction::SetTrue,
        conflicts_with = "verbose",
        help = "Don't print messages"
    )]
    pub quiet: bool,

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
        help = "Replace the whole <output>/<pdf_stem>/ folder before processing."
    )]
    pub overwrite: bool,

    #[arg(
        short = 'n',
        long = "normalize-tables",
        action = ArgAction::SetTrue,
        help = "Normalize OCR HTML tables into Markdown and store raw HTML under tables/."
    )]
    pub normalize_tables: bool,
    #[arg(
        long,
        action = ArgAction::SetTrue,
        help = "Structure output as an Open Knowledge Format (OKF) bundle (manuscript.md, index.md with metadata, root index.md and log.md)."
    )]
    pub okf: bool,

    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum CliCommand {
    #[command(about = "Configuration management")]
    Config(ConfigArgs),
    #[command(about = "Diagnose config, auth, and ...")]
    Doctor(DoctorArgs),
}

#[derive(Debug, Clone, Args)]
#[command(
    about = "Configuration management",
    override_usage = "paperdown config <COMMAND>",
    help_template = CONFIG_HELP_TEMPLATE
)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ConfigCommand {
    #[command(
        about = "Generate a default configuration file",
        override_usage = "paperdown config init [OPTIONS]",
        help_template = CONFIG_INIT_HELP_TEMPLATE
    )]
    Init(ConfigInitArgs),
    #[command(
        about = "Validate the configuration file",
        long_about = "Validate the configuration file\n\nCheck the config file for syntax errors and report any issues. Exit code 0 means valid, non-zero means file not found or has errors.",
        override_usage = "paperdown config check [OPTIONS]",
        help_template = CONFIG_CHECK_HELP_TEMPLATE
    )]
    Check(ConfigCheckArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ConfigInitArgs {
    #[arg(short = 'f', long, help = "Overwrite existing configuration file")]
    pub force: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ConfigCheckArgs {
    #[arg(
        short = 'c',
        long,
        value_name = "CONFIG",
        help = "Path to configuration file to check (defaults to the global config location)"
    )]
    pub config: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
#[command(
    long_about = "Check your system for potential problems. Will exit with a non-zero status if\nany potential problems are found.",
    override_usage = "paperdown doctor <COMMAND>",
    disable_help_subcommand = true,
    help_template = DOCTOR_HELP_TEMPLATE
)]
pub struct DoctorArgs {
    #[command(subcommand)]
    pub command: Option<DoctorCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum DoctorCommand {
    #[command(about = "Print this message or the help of the given subcommand(s)")]
    Help,
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
            overwrite: bool_override(self.overwrite, false),
            normalize_tables: bool_override(self.normalize_tables, false),
            okf: bool_override(self.okf, false),
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
    use clap::Parser;

    #[test]
    fn config_overrides_capture_only_explicit_cli_values() {
        let cli = Cli::parse_from([
            "paperdown",
            "--input",
            "in.pdf",
            "--env",
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
            "--okf",
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
                okf: Some(true),
            }
        );
    }

    #[test]
    fn quiet_disables_verbose_config_override() {
        let cli = Cli::parse_from(["paperdown", "--input", "in.pdf", "--quiet"]);

        assert_eq!(
            cli.config_overrides(),
            config::ConfigOverrides {
                verbose: Some(false),
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
        assert!(Cli::try_parse_from(["paperdown", "-i", "in.pdf", "-e", ".env", "-n"]).is_ok());
    }

    #[test]
    fn cli_parses_okf_flag() {
        // Parsed on its own (without --normalize-tables) so a cross-wiring bug in
        // config_overrides() -- e.g. okf bound to the normalize_tables field --
        // would be caught here even though the bundled overrides test still passes.
        let cli = Cli::parse_from(["paperdown", "--input", "in.pdf", "--okf"]);

        assert_eq!(cli.config_overrides().okf, Some(true));
    }
}
