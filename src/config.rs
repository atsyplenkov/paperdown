use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "paperdown";
pub const CONFIG_FILE_NAME: &str = "paperdown.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfig {
    pub env_file: PathBuf,
    pub timeout: u64,
    pub max_download_bytes: u64,
    pub workers: usize,
    pub ocr_workers: usize,
    pub verbose: bool,
    pub overwrite: bool,
    pub normalize_tables: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveConfig {
    pub settings: ResolvedConfig,
    pub config_files: Vec<PathBuf>,
}

impl Default for ResolvedConfig {
    fn default() -> Self {
        Self {
            env_file: PathBuf::from(DEFAULT_ENV_FILE),
            timeout: DEFAULT_TIMEOUT,
            max_download_bytes: DEFAULT_MAX_DOWNLOAD_BYTES,
            workers: DEFAULT_WORKERS,
            ocr_workers: DEFAULT_OCR_WORKERS,
            verbose: false,
            overwrite: false,
            normalize_tables: false,
        }
    }
}

impl ResolvedConfig {
    fn apply(mut self, overrides: ConfigOverrides) -> Self {
        if let Some(value) = overrides.env_file {
            self.env_file = value;
        }
        if let Some(value) = overrides.timeout {
            self.timeout = value;
        }
        if let Some(value) = overrides.max_download_bytes {
            self.max_download_bytes = value;
        }
        if let Some(value) = overrides.workers {
            self.workers = value;
        }
        if let Some(value) = overrides.ocr_workers {
            self.ocr_workers = value;
        }
        if let Some(value) = overrides.verbose {
            self.verbose = value;
        }
        if let Some(value) = overrides.overwrite {
            self.overwrite = value;
        }
        if let Some(value) = overrides.normalize_tables {
            self.normalize_tables = value;
        }
        self
    }
}

pub const DEFAULT_ENV_FILE: &str = ".env";
pub const DEFAULT_TIMEOUT: u64 = 180;
pub const DEFAULT_MAX_DOWNLOAD_BYTES: u64 = 20_971_520;
pub const DEFAULT_WORKERS: usize = 32;
pub const DEFAULT_OCR_WORKERS: usize = 2;

pub const DEFAULT_CONFIG_TEMPLATE: &str = "env-file = \".env\"\ntimeout = 180\nmax-download-bytes = 20971520\nworkers = 32\nocr-workers = 2\nverbose = false\noverwrite = false\nnormalize-tables = false\n";

#[derive(Debug, Clone, Default, serde::Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct ConfigOverrides {
    pub env_file: Option<PathBuf>,
    pub timeout: Option<u64>,
    pub max_download_bytes: Option<u64>,
    pub workers: Option<usize>,
    pub ocr_workers: Option<usize>,
    pub verbose: Option<bool>,
    pub overwrite: Option<bool>,
    pub normalize_tables: Option<bool>,
}

impl ConfigOverrides {
    pub fn merge(self, higher: ConfigOverrides) -> ConfigOverrides {
        ConfigOverrides {
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
        if self.timeout == Some(0) {
            return Err(ConfigLoadError::InvalidPositive {
                field: "timeout",
                path: path.to_path_buf(),
            });
        }
        if self.max_download_bytes == Some(0) {
            return Err(ConfigLoadError::InvalidPositive {
                field: "max-download-bytes",
                path: path.to_path_buf(),
            });
        }
        if self.workers == Some(0) {
            return Err(ConfigLoadError::InvalidPositive {
                field: "workers",
                path: path.to_path_buf(),
            });
        }
        if self.ocr_workers == Some(0) {
            return Err(ConfigLoadError::InvalidPositive {
                field: "ocr-workers",
                path: path.to_path_buf(),
            });
        }
        Ok(())
    }

    fn rebase_env_file(mut self, path: &Path) -> ConfigOverrides {
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
    InvalidPositive {
        field: &'static str,
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
            ConfigLoadError::InvalidPositive { field, path } => write!(
                f,
                "config field `{field}` in {} must be greater than 0",
                path.display()
            ),
        }
    }
}

impl Error for ConfigLoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ConfigLoadError::Read { source, .. } => Some(source),
            ConfigLoadError::Parse { source, .. } => Some(source),
            ConfigLoadError::InvalidPositive { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum ConfigPathError {
    Unavailable,
}

impl fmt::Display for ConfigPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigPathError::Unavailable => write!(f, "could not determine XDG config directory"),
        }
    }
}

impl Error for ConfigPathError {}

#[derive(Debug)]
pub enum ConfigInitError {
    Path(ConfigPathError),
    Exists {
        path: PathBuf,
    },
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for ConfigInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigInitError::Path(source) => write!(f, "{source}"),
            ConfigInitError::Exists { path } => {
                write!(f, "config file already exists: {}", path.display())
            }
            ConfigInitError::CreateDir { path, source } => {
                write!(
                    f,
                    "failed to create config directory {}: {source}",
                    path.display()
                )
            }
            ConfigInitError::Write { path, source } => {
                write!(
                    f,
                    "failed to write config file {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl Error for ConfigInitError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ConfigInitError::Path(source) => Some(source),
            ConfigInitError::Exists { .. } => None,
            ConfigInitError::CreateDir { source, .. } => Some(source),
            ConfigInitError::Write { source, .. } => Some(source),
        }
    }
}

impl From<ConfigPathError> for ConfigInitError {
    fn from(value: ConfigPathError) -> Self {
        ConfigInitError::Path(value)
    }
}

#[derive(Debug)]
pub enum ConfigCheckError {
    Path(ConfigPathError),
    Load(ConfigLoadError),
}

impl fmt::Display for ConfigCheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigCheckError::Path(source) => write!(f, "{source}"),
            ConfigCheckError::Load(source) => write!(f, "{source}"),
        }
    }
}

impl Error for ConfigCheckError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ConfigCheckError::Path(source) => Some(source),
            ConfigCheckError::Load(source) => Some(source),
        }
    }
}

impl From<ConfigPathError> for ConfigCheckError {
    fn from(value: ConfigPathError) -> Self {
        ConfigCheckError::Path(value)
    }
}

impl From<ConfigLoadError> for ConfigCheckError {
    fn from(value: ConfigLoadError) -> Self {
        ConfigCheckError::Load(value)
    }
}

fn config_file_path(config_dir: PathBuf) -> PathBuf {
    config_dir.join(APP_NAME).join(CONFIG_FILE_NAME)
}

pub fn global_config_file_path() -> Option<PathBuf> {
    dirs::config_dir().map(config_file_path)
fn configured_config_dir() -> Option<PathBuf> {
    test_config_dir_override().or_else(dirs::config_dir)
}

#[cfg(debug_assertions)]
fn test_config_dir_override() -> Option<PathBuf> {
    std::env::var_os("PAPERDOWN_TEST_CONFIG_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

#[cfg(not(debug_assertions))]
fn test_config_dir_override() -> Option<PathBuf> {
    None
}

pub fn global_config_file_path() -> Option<PathBuf> {
    configured_config_dir().map(config_file_path)
}

pub fn default_config_path() -> Result<PathBuf, ConfigPathError> {
    global_config_file_path().ok_or(ConfigPathError::Unavailable)
}

fn init_config_at(path: PathBuf, force: bool) -> Result<PathBuf, ConfigInitError> {
    let parent = path.parent().ok_or(ConfigPathError::Unavailable)?;
    std::fs::create_dir_all(parent).map_err(|source| ConfigInitError::CreateDir {
        path: parent.to_path_buf(),
        source,
    })?;

    if force {
        std::fs::write(&path, DEFAULT_CONFIG_TEMPLATE).map_err(|source| {
            ConfigInitError::Write {
                path: path.clone(),
                source,
            }
        })?;
        return Ok(path);
    }

    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            use std::io::Write;
            file.write_all(DEFAULT_CONFIG_TEMPLATE.as_bytes())
                .map_err(|source| ConfigInitError::Write {
                    path: path.clone(),
                    source,
                })?;
            Ok(path)
        }
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(ConfigInitError::Exists { path })
        }
        Err(source) => Err(ConfigInitError::Write { path, source }),
    }
}

pub fn init_default_config(force: bool) -> Result<PathBuf, ConfigInitError> {
    init_config_at(default_config_path()?, force)
}

pub fn check_config_file(explicit: Option<&Path>) -> Result<PathBuf, ConfigCheckError> {
    let path = match explicit {
        Some(path) => path.to_path_buf(),
        None => default_config_path()?,
    };
    load_config_from_path(&path)?;
    Ok(path)
}

pub fn find_local_config(start_dir: &Path) -> Option<PathBuf> {
    start_dir
        .ancestors()
        .map(|dir| dir.join(CONFIG_FILE_NAME))
        .find(|path| path.exists())
}

pub fn load_config_from_path(path: &Path) -> Result<ConfigOverrides, ConfigLoadError> {
    let content = std::fs::read_to_string(path).map_err(|source| ConfigLoadError::Read {
        source,
        path: path.to_path_buf(),
    })?;
    let config =
        toml::from_str::<ConfigOverrides>(&content).map_err(|source| ConfigLoadError::Parse {
            source,
            path: path.to_path_buf(),
        })?;
    config.validate(path)?;
    Ok(config.rebase_env_file(path))
}

fn load_discovered_configs(
    global: Option<&Path>,
    local: Option<&Path>,
) -> Result<ConfigOverrides, ConfigLoadError> {
    let mut config = ConfigOverrides::default();
    if let Some(path) = global {
        config = config.merge(load_config_from_path(path)?);
    }
    if let Some(path) = local {
        config = config.merge(load_config_from_path(path)?);
    }
    Ok(config)
}

fn load_file_config(
    explicit: Option<&Path>,
    cwd: &Path,
) -> Result<ConfigOverrides, ConfigLoadError> {
    load_file_config_with_sources(explicit, cwd).map(|(config, _sources)| config)
}

fn load_file_config_with_sources_from_config_dir(
    explicit: Option<&Path>,
    cwd: &Path,
    config_dir: Option<PathBuf>,
) -> Result<(ConfigOverrides, Vec<PathBuf>), ConfigLoadError> {
    if let Some(path) = explicit {
        return Ok((load_config_from_path(path)?, vec![path.to_path_buf()]));
    }

    let global = config_dir
        .map(config_file_path)
        .filter(|path| path.exists());
    let local = find_local_config(cwd);
    let mut sources = Vec::new();
    if let Some(path) = global.as_ref() {
        sources.push(path.clone());
    }
    if let Some(path) = local.as_ref() {
        sources.push(path.clone());
    }
    let config = load_discovered_configs(global.as_deref(), local.as_deref())?;
    Ok((config, sources))
}

fn load_file_config_with_sources(
    explicit: Option<&Path>,
    cwd: &Path,
) -> Result<(ConfigOverrides, Vec<PathBuf>), ConfigLoadError> {
    load_file_config_with_sources_from_config_dir(explicit, cwd, dirs::config_dir())
    load_file_config_with_sources_from_config_dir(explicit, cwd, configured_config_dir())
}

pub fn load_effective_config(
    explicit: Option<&Path>,
    cwd: &Path,
    cli: ConfigOverrides,
) -> Result<ResolvedConfig, ConfigLoadError> {
    let file_config = load_file_config(explicit, cwd)?;
    Ok(ResolvedConfig::default().apply(file_config).apply(cli))
}

pub fn load_effective_config_with_sources(
    explicit: Option<&Path>,
    cwd: &Path,
    cli: ConfigOverrides,
) -> Result<EffectiveConfig, ConfigLoadError> {
    let (file_config, config_files) = load_file_config_with_sources(explicit, cwd)?;
    let settings = ResolvedConfig::default().apply(file_config).apply(cli);
    Ok(EffectiveConfig {
        settings,
        config_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_template_is_valid() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(CONFIG_FILE_NAME);
        std::fs::write(&path, DEFAULT_CONFIG_TEMPLATE).expect("write default config");

        let config = load_config_from_path(&path).expect("load default config");

        assert_eq!(
            config,
            ConfigOverrides {
                env_file: Some(temp.path().join(".env")),
                timeout: Some(DEFAULT_TIMEOUT),
                max_download_bytes: Some(DEFAULT_MAX_DOWNLOAD_BYTES),
                workers: Some(DEFAULT_WORKERS),
                ocr_workers: Some(DEFAULT_OCR_WORKERS),
                verbose: Some(false),
                overwrite: Some(false),
                normalize_tables: Some(false),
            }
        );
    }

    #[test]
    fn init_default_config_refuses_existing_without_force() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_root = temp.path().join("config");
        let path = config_file_path(config_root);
        std::fs::create_dir_all(path.parent().expect("config parent")).expect("create config dir");
        std::fs::write(&path, "timeout = 9\n").expect("write existing config");

        let err = init_config_at(path.clone(), false).expect_err("existing config is refused");
        match err {
            ConfigInitError::Exists { path: err_path } => assert_eq!(err_path, path),
            other => panic!("expected exists error, got {other:?}"),
        }
    }

    #[test]
    fn init_default_config_overwrites_with_force() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_root = temp.path().join("config");
        let path = config_file_path(config_root);
        std::fs::create_dir_all(path.parent().expect("config parent")).expect("create config dir");
        std::fs::write(&path, "timeout = 0\n").expect("write invalid config");

        let created = init_config_at(path.clone(), true).expect("force overwrite config");
        assert_eq!(created, path);

        assert_eq!(
            std::fs::read_to_string(&path).expect("read overwritten config"),
            DEFAULT_CONFIG_TEMPLATE
        );
    }

    #[test]
    fn check_config_file_validates_explicit_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let valid = temp.path().join("valid.toml");
        let invalid = temp.path().join("invalid.toml");
        std::fs::write(&valid, "timeout = 9\n").expect("write valid config");
        std::fs::write(&invalid, "timeout = 0\n").expect("write invalid config");

        assert_eq!(
            check_config_file(Some(&valid)).expect("valid config"),
            valid
        );
        let err = check_config_file(Some(&invalid)).expect_err("invalid config rejected");
        assert!(matches!(err, ConfigCheckError::Load(_)));
    }

    #[test]
    fn parses_partial_config_with_kebab_case_keys() {
        let config: ConfigOverrides = toml::from_str(
            r#"
timeout = 9
max-download-bytes = 99
normalize-tables = true
"#,
        )
        .expect("parse partial config");

        assert_eq!(
            config,
            ConfigOverrides {
                timeout: Some(9),
                max_download_bytes: Some(99),
                normalize_tables: Some(true),
                ..ConfigOverrides::default()
            }
        );
    }

    #[test]
    fn merge_preserves_explicit_false_override() {
        let lower = ConfigOverrides {
            verbose: Some(true),
            overwrite: Some(true),
            normalize_tables: Some(true),
            ..ConfigOverrides::default()
        };
        let higher = ConfigOverrides {
            verbose: Some(false),
            overwrite: Some(false),
            normalize_tables: Some(false),
            ..ConfigOverrides::default()
        };

        let merged = lower.merge(higher);

        assert_eq!(merged.verbose, Some(false));
        assert_eq!(merged.overwrite, Some(false));
        assert_eq!(merged.normalize_tables, Some(false));
    }

    #[test]
    fn rejects_unknown_input_output_fields() {
        for field in ["input", "output"] {
            let err = toml::from_str::<ConfigOverrides>(&format!(r#"{field} = "paper.pdf""#))
                .expect_err("input and output are CLI-only");

            assert!(
                err.to_string().contains("unknown field"),
                "unexpected parse error for {field}: {err}"
            );
        }
    }

    #[test]
    fn rejects_zero_numeric_values_with_field_and_path() {
        let temp = tempfile::tempdir().expect("tempdir");

        for field in ["timeout", "max-download-bytes", "workers", "ocr-workers"] {
            let path = temp.path().join(format!("{field}.toml"));
            std::fs::write(&path, format!("{field} = 0\n")).expect("write config");

            let err = load_config_from_path(&path).expect_err("zero numeric field is invalid");

            match err {
                ConfigLoadError::InvalidPositive {
                    field: err_field,
                    path: err_path,
                } => {
                    assert_eq!(err_field, field);
                    assert_eq!(err_path, path);
                }
                other => panic!("expected invalid-positive error for {field}, got {other:?}"),
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
    fn load_effective_config_merges_global_then_local_then_cli() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_root = temp.path().join("config");
        let global_dir = config_root.join(APP_NAME);
        std::fs::create_dir_all(&global_dir).expect("create global config dir");
        std::fs::write(
            global_dir.join(CONFIG_FILE_NAME),
            r#"
env-file = "global.env"
timeout = 9
max-download-bytes = 900
workers = 4
ocr-workers = 5
verbose = true
overwrite = true
normalize-tables = true
"#,
        )
        .expect("write global config");

        let project = temp.path().join("project");
        let cwd = project.join("nested");
        std::fs::create_dir_all(&cwd).expect("create project dirs");
        std::fs::write(
            project.join(CONFIG_FILE_NAME),
            r#"
env-file = "local.env"
workers = 2
ocr-workers = 3
overwrite = false
"#,
        )
        .expect("write local config");

        let cli = ConfigOverrides {
            timeout: Some(11),
            max_download_bytes: Some(1_100),
            verbose: Some(false),
            normalize_tables: Some(false),
            ..ConfigOverrides::default()
        };

        let (file_config, _) =
            load_file_config_with_sources_from_config_dir(None, &cwd, Some(config_root))
                .expect("load file config");
        let config = ResolvedConfig::default().apply(file_config).apply(cli);

        assert_eq!(
            config,
            ResolvedConfig {
                env_file: project.join("local.env"),
                timeout: 11,
                max_download_bytes: 1_100,
                workers: 2,
                ocr_workers: 3,
                verbose: false,
                overwrite: false,
                normalize_tables: false,
            }
        );
    }

    #[test]
    fn load_effective_config_with_sources_reports_loaded_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_root = temp.path().join("config");
        let global_dir = config_root.join(APP_NAME);
        std::fs::create_dir_all(&global_dir).expect("create global config dir");
        let global = global_dir.join(CONFIG_FILE_NAME);
        std::fs::write(&global, "timeout = 9\n").expect("write global config");

        let project = temp.path().join("project");
        let cwd = project.join("nested");
        std::fs::create_dir_all(&cwd).expect("create project dirs");
        let local = project.join(CONFIG_FILE_NAME);
        std::fs::write(&local, "workers = 2\n").expect("write local config");

        let (file_config, config_files) =
            load_file_config_with_sources_from_config_dir(None, &cwd, Some(config_root))
                .expect("load file config");
        let effective = EffectiveConfig {
            settings: ResolvedConfig::default().apply(file_config),
            config_files,
        };

        assert_eq!(effective.config_files, vec![global, local]);
        assert_eq!(effective.settings.timeout, 9);
        assert_eq!(effective.settings.workers, 2);
    }

    #[test]
    fn explicit_config_disables_local_discovery_and_still_yields_to_cli() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("project");
        let cwd = project.join("nested");
        std::fs::create_dir_all(&cwd).expect("create project dirs");
        std::fs::write(project.join(CONFIG_FILE_NAME), "workers = 0\n")
            .expect("write invalid local config");

        let explicit_dir = temp.path().join("configs");
        std::fs::create_dir(&explicit_dir).expect("create explicit config dir");
        let explicit = explicit_dir.join("chosen.toml");
        std::fs::write(
            &explicit,
            r#"
env-file = "explicit.env"
timeout = 7
max-download-bytes = 700
workers = 6
ocr-workers = 4
verbose = true
overwrite = true
normalize-tables = true
"#,
        )
        .expect("write explicit config");

        let cli = ConfigOverrides {
            max_download_bytes: Some(99),
            verbose: Some(false),
            overwrite: Some(false),
            ..ConfigOverrides::default()
        };

        let config =
            load_effective_config(Some(&explicit), &cwd, cli).expect("load explicit config");

        assert_eq!(
            config,
            ResolvedConfig {
                env_file: explicit_dir.join("explicit.env"),
                timeout: 7,
                max_download_bytes: 99,
                workers: 6,
                ocr_workers: 4,
                verbose: false,
                overwrite: false,
                normalize_tables: true,
            }
        );
    }
}
