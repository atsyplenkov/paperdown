use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "paperdown";
pub const CONFIG_FILE_NAME: &str = "paperdown.toml";

#[derive(Debug, Clone, Default, serde::Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct FileConfig {
    pub env_file: Option<PathBuf>,
    pub timeout: Option<u64>,
    pub max_download_bytes: Option<u64>,
    pub workers: Option<usize>,
    pub ocr_workers: Option<usize>,
    pub verbose: Option<bool>,
    pub overwrite: Option<bool>,
    pub normalize_tables: Option<bool>,
}

impl FileConfig {
    pub fn merge(self, higher: FileConfig) -> FileConfig {
        FileConfig {
            env_file: higher.env_file.or(self.env_file),
            timeout: higher.timeout.or(self.timeout),
            max_download_bytes: higher.max_download_bytes.or(self.max_download_bytes),
            workers: higher.workers.or(self.workers),
            ocr_workers: higher.ocr_workers.or(self.ocr_workers),
            verbose: higher.verbose.or(self.verbose),
            overwrite: higher.overwrite.or(self.overwrite),
            normalize_tables: higher.normalize_tables.or(self.normalize_tables),
        }
    }

    fn validate(&self, path: &Path) -> Result<(), ConfigLoadError> {
        validate_positive(self.timeout, "timeout", path)?;
        validate_positive(self.max_download_bytes, "max-download-bytes", path)?;
        validate_positive(self.workers, "workers", path)?;
        validate_positive(self.ocr_workers, "ocr-workers", path)?;
        Ok(())
    }

    fn rebase_env_file(mut self, path: &Path) -> FileConfig {
        if let Some(env_file) = self.env_file.as_ref()
            && env_file.is_relative()
        {
            let parent = path.parent().unwrap_or_else(|| Path::new(""));
            self.env_file = Some(parent.join(env_file));
        }
        self
    }
}

#[derive(Debug)]
pub enum ConfigLoadError {
    Read {
        source: std::io::Error,
        path: PathBuf,
    },
    Parse {
        source: toml::de::Error,
        path: PathBuf,
    },
    Validate {
        message: String,
        path: PathBuf,
    },
}

impl fmt::Display for ConfigLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigLoadError::Read { source, path } => {
                write!(f, "Failed to read config file {}: {source}", path.display())
            }
            ConfigLoadError::Parse { source, path } => {
                write!(
                    f,
                    "Failed to parse config file {}: {source}",
                    path.display()
                )
            }
            ConfigLoadError::Validate { message, path } => {
                let _ = path;
                f.write_str(message)
            }
        }
    }
}

impl Error for ConfigLoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ConfigLoadError::Read { source, .. } => Some(source),
            ConfigLoadError::Parse { source, .. } => Some(source),
            ConfigLoadError::Validate { .. } => None,
        }
    }
}

pub fn global_config_file_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join(APP_NAME).join(CONFIG_FILE_NAME))
}

pub fn find_local_config(start_dir: &Path) -> Option<PathBuf> {
    start_dir
        .ancestors()
        .map(|dir| dir.join(CONFIG_FILE_NAME))
        .find(|path| path.exists())
}

pub fn load_config_from_path(path: &Path) -> Result<FileConfig, ConfigLoadError> {
    let content = std::fs::read_to_string(path).map_err(|source| ConfigLoadError::Read {
        source,
        path: path.to_path_buf(),
    })?;
    let config =
        toml::from_str::<FileConfig>(&content).map_err(|source| ConfigLoadError::Parse {
            source,
            path: path.to_path_buf(),
        })?;
    config.validate(path)?;
    Ok(config.rebase_env_file(path))
}

fn load_discovered_configs(
    global: Option<&Path>,
    local: Option<&Path>,
) -> Result<FileConfig, ConfigLoadError> {
    let mut config = FileConfig::default();
    if let Some(path) = global {
        config = config.merge(load_config_from_path(path)?);
    }
    if let Some(path) = local {
        config = config.merge(load_config_from_path(path)?);
    }
    Ok(config)
}

pub fn load_effective_config(
    explicit: Option<&Path>,
    cwd: &Path,
) -> Result<FileConfig, ConfigLoadError> {
    if let Some(path) = explicit {
        return load_config_from_path(path);
    }

    let global = global_config_file_path().filter(|path| path.exists());
    let local = find_local_config(cwd);
    load_discovered_configs(global.as_deref(), local.as_deref())
}

fn validate_positive<T>(value: Option<T>, field: &str, path: &Path) -> Result<(), ConfigLoadError>
where
    T: PartialEq + From<u8>,
{
    if value == Some(T::from(0)) {
        return Err(ConfigLoadError::Validate {
            message: format!(
                "config field `{field}` in {} must be greater than 0",
                path.display()
            ),
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_partial_config_with_kebab_case_keys() {
        let config: FileConfig = toml::from_str(
            r#"
timeout = 9
max-download-bytes = 99
normalize-tables = true
"#,
        )
        .expect("parse partial config");

        assert_eq!(
            config,
            FileConfig {
                timeout: Some(9),
                max_download_bytes: Some(99),
                normalize_tables: Some(true),
                ..FileConfig::default()
            }
        );
    }

    #[test]
    fn merge_preserves_explicit_false_override() {
        let lower = FileConfig {
            overwrite: Some(true),
            ..FileConfig::default()
        };
        let higher = FileConfig {
            overwrite: Some(false),
            ..FileConfig::default()
        };

        let merged = lower.merge(higher);

        assert_eq!(merged.overwrite, Some(false));
    }

    #[test]
    fn rejects_unknown_input_output_fields() {
        for field in ["input", "output"] {
            let err = toml::from_str::<FileConfig>(&format!(r#"{field} = "paper.pdf""#))
                .expect_err("input and output are CLI-only");

            assert!(
                err.to_string().contains("unknown field"),
                "unexpected parse error for {field}: {err}"
            );
        }
    }

    #[test]
    fn rejects_zero_numeric_values() {
        let temp = tempfile::tempdir().expect("tempdir");

        for field in ["timeout", "max-download-bytes", "workers", "ocr-workers"] {
            let path = temp.path().join(format!("{field}.toml"));
            std::fs::write(&path, format!("{field} = 0\n")).expect("write config");

            let err = load_config_from_path(&path).expect_err("zero numeric field is invalid");

            match err {
                ConfigLoadError::Validate {
                    message,
                    path: err_path,
                } => {
                    assert_eq!(err_path, path);
                    assert!(
                        message.contains(&format!("config field `{field}`")),
                        "validation message should name {field}: {message}"
                    );
                    assert!(
                        message.contains(&path.display().to_string()),
                        "validation message should include config path: {message}"
                    );
                    assert!(
                        message.contains("must be greater than 0"),
                        "validation message should explain the bound: {message}"
                    );
                }
                other => panic!("expected validation error for {field}, got {other:?}"),
            }
        }
    }

    #[test]
    fn rebases_relative_env_file_to_config_parent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_dir = temp.path().join("sub");
        std::fs::create_dir(&config_dir).expect("create config dir");
        let path = config_dir.join(CONFIG_FILE_NAME);
        std::fs::write(&path, r#"env-file = ".env""#).expect("write config");

        let config = load_config_from_path(&path).expect("load config");

        assert_eq!(config.env_file, Some(config_dir.join(".env")));
    }

    #[test]
    fn load_discovered_configs_merges_global_then_local() {
        let temp = tempfile::tempdir().expect("tempdir");
        let global = temp.path().join("global.toml");
        let local = temp.path().join("local.toml");
        std::fs::write(
            &global,
            r#"
workers = 1
overwrite = true
"#,
        )
        .expect("write global config");
        std::fs::write(
            &local,
            r#"
ocr-workers = 2
overwrite = false
"#,
        )
        .expect("write local config");

        let config =
            load_discovered_configs(Some(&global), Some(&local)).expect("load discovered configs");

        assert_eq!(config.workers, Some(1));
        assert_eq!(config.ocr_workers, Some(2));
        assert_eq!(config.overwrite, Some(false));
    }
}
