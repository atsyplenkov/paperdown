use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::TempDir;

const DEFAULT_CONFIG_TEMPLATE: &str = "env-file = \".env\"\ntimeout = 180\nmax-download-bytes = 20971520\nworkers = 32\nocr-workers = 2\nverbose = false\noverwrite = false\nnormalize-tables = false\n";
const TEST_CONFIG_DIR_ENV: &str = "PAPERDOWN_CONFIG_DIR";

fn test_config_root(tmp: &TempDir) -> PathBuf {
    let root = tmp.path().join("config-dir");
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn default_config_path(config_root: &Path) -> PathBuf {
    config_root.join("paperdown.toml")
}

fn default_env_file_line(stdout: &str) -> &str {
    let line = stdout
        .lines()
        .find(|line| line.starts_with("env file: "))
        .expect("doctor output includes env file line");
    assert!(
        line.ends_with(".env"),
        "default env file should end with .env, got {line:?}"
    );
    line
}

#[test]
fn cli_reports_missing_input_path() {
    let tmp = TempDir::new().unwrap();
    let config_root = test_config_root(&tmp);

    let mut cmd = Command::cargo_bin("paperdown").unwrap();
    let output = cmd
        .args(["--input", "/definitely/missing/path.pdf"])
        .env(TEST_CONFIG_DIR_ENV, &config_root)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Input path does not exist"));
}

#[test]
fn cli_batch_reports_failed_count() {
    let tmp = TempDir::new().unwrap();
    let config_root = test_config_root(&tmp);
    let input_dir = tmp.path().join("pdf");
    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::write(input_dir.join("a.pdf"), b"%PDF-1.7\n").unwrap();
    std::fs::write(input_dir.join("b.pdf"), b"%PDF-1.7\n").unwrap();
    let env_file = tmp.path().join("missing.env");

    let mut cmd = Command::cargo_bin("paperdown").unwrap();
    let output = cmd
        .current_dir(tmp.path())
        .args([
            "--input",
            input_dir.to_str().unwrap(),
            "--env",
            env_file.to_str().unwrap(),
            "--workers",
            "1",
            "--ocr-workers",
            "5",
        ])
        .env(TEST_CONFIG_DIR_ENV, &config_root)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("Batch Complete processed: 0 skipped: 0 failed: 2 figures: 0"));
    assert!(stderr.contains("failed:"));
    assert!(stderr.contains("OCR concurrency: 1"));
}

#[test]
fn cli_single_pdf_skips_when_log_exists_and_env_missing() {
    let tmp = TempDir::new().unwrap();
    let config_root = test_config_root(&tmp);
    let pdf = tmp.path().join("paper.pdf");
    std::fs::write(&pdf, b"%PDF-1.7\n").unwrap();

    let output_dir = tmp.path().join("output");
    let paper_dir = output_dir.join("paper");
    std::fs::create_dir_all(&paper_dir).unwrap();
    std::fs::write(paper_dir.join("log.jsonl"), b"{}\n").unwrap();

    let env_file = tmp.path().join("missing.env");

    let mut cmd = Command::cargo_bin("paperdown").unwrap();
    let output = cmd
        .current_dir(tmp.path())
        .args([
            "--input",
            pdf.to_str().unwrap(),
            "--output",
            output_dir.to_str().unwrap(),
            "--env",
            env_file.to_str().unwrap(),
        ])
        .env_remove("ZAI_API_KEY")
        .env(TEST_CONFIG_DIR_ENV, &config_root)
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("Skipped"));
    assert!(stdout.contains("paper.pdf"));
    assert!(!stderr.contains("ZAI_API_KEY"));
}

#[test]
fn invalid_config_reports_parse_error() {
    let tmp = TempDir::new().unwrap();
    let pdf = tmp.path().join("paper.pdf");
    std::fs::write(&pdf, b"%PDF-1.7\n").unwrap();
    std::fs::write(tmp.path().join("paperdown.toml"), "timeout =").unwrap();
    let config_root = test_config_root(&tmp);

    let mut cmd = Command::cargo_bin("paperdown").unwrap();
    let output = cmd
        .current_dir(tmp.path())
        .args(["--input", pdf.to_str().unwrap()])
        .env(TEST_CONFIG_DIR_ENV, &config_root)
        .env_remove("ZAI_API_KEY")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Failed to parse config file"));
}

#[test]
fn config_init_writes_default_config() {
    let tmp = TempDir::new().unwrap();
    let config_root = test_config_root(&tmp);

    let mut cmd = Command::cargo_bin("paperdown").unwrap();
    let output = cmd
        .args(["config", "init"])
        .env(TEST_CONFIG_DIR_ENV, &config_root)
        .output()
        .unwrap();

    assert!(output.status.success());
    let config_path = default_config_path(&config_root);
    assert_eq!(
        std::fs::read_to_string(config_path).unwrap(),
        DEFAULT_CONFIG_TEMPLATE
    );
}

#[test]
fn config_init_refuses_existing_without_force() {
    let tmp = TempDir::new().unwrap();
    let config_root = test_config_root(&tmp);
    let config_dir = config_root.clone();
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("paperdown.toml"), "timeout = 9\n").unwrap();

    let mut cmd = Command::cargo_bin("paperdown").unwrap();
    let output = cmd
        .args(["config", "init"])
        .env(TEST_CONFIG_DIR_ENV, &config_root)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("already exists"));
}

#[test]
fn config_init_force_overwrites_existing() {
    let tmp = TempDir::new().unwrap();
    let config_root = test_config_root(&tmp);
    let config_dir = config_root.clone();
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("paperdown.toml");
    std::fs::write(&config_path, "timeout = 0\n").unwrap();

    let mut cmd = Command::cargo_bin("paperdown").unwrap();
    let output = cmd
        .args(["config", "init", "--force"])
        .env(TEST_CONFIG_DIR_ENV, &config_root)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        std::fs::read_to_string(config_path).unwrap(),
        DEFAULT_CONFIG_TEMPLATE
    );
}

#[test]
fn config_check_accepts_valid_explicit_config() {
    let tmp = TempDir::new().unwrap();
    let config_root = test_config_root(&tmp);
    let config = tmp.path().join("paperdown.toml");
    std::fs::write(&config, "timeout = 9\n").unwrap();

    let mut cmd = Command::cargo_bin("paperdown").unwrap();
    let output = cmd
        .args(["config", "check", "--config", config.to_str().unwrap()])
        .env(TEST_CONFIG_DIR_ENV, &config_root)
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Config OK:"));
}

#[test]
fn config_check_rejects_missing_default_config() {
    let tmp = TempDir::new().unwrap();
    let config_root = test_config_root(&tmp);

    let mut cmd = Command::cargo_bin("paperdown").unwrap();
    let output = cmd
        .args(["config", "check"])
        .env(TEST_CONFIG_DIR_ENV, &config_root)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Failed to read config file"));
}

#[test]
fn doctor_reports_missing_auth_without_input() {
    let tmp = TempDir::new().unwrap();
    let config_root = test_config_root(&tmp);

    let mut cmd = Command::cargo_bin("paperdown").unwrap();
    let output = cmd
        .current_dir(tmp.path())
        .args(["doctor"])
        .env(TEST_CONFIG_DIR_ENV, &config_root)
        .env_remove("ZAI_API_KEY")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("config: ok"));
    assert!(stdout.contains("config files:\n  none"));
    let env_file_marker = default_env_file_line(&stdout);
    assert!(stdout.contains("auth: error:"));
    assert!(stdout.find("auth: error:").unwrap() < stdout.find(env_file_marker).unwrap());
}

#[test]
fn doctor_accepts_environment_auth_without_input() {
    let tmp = TempDir::new().unwrap();
    let config_root = test_config_root(&tmp);

    let mut cmd = Command::cargo_bin("paperdown").unwrap();
    let output = cmd
        .current_dir(tmp.path())
        .args(["doctor"])
        .env(TEST_CONFIG_DIR_ENV, &config_root)
        .env("ZAI_API_KEY", "test-key")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("config: ok"));
    assert!(stdout.contains("config files:\n  none"));
    let env_file_marker = default_env_file_line(&stdout);
    assert!(stdout.contains("auth: ok"));
    assert!(stdout.find("auth: ok").unwrap() < stdout.find(env_file_marker).unwrap());
}

#[test]
fn doctor_reports_explicit_config_and_rebased_env_paths() {
    let tmp = TempDir::new().unwrap();
    let config_root = test_config_root(&tmp);
    let config_dir = tmp.path().join("configs");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = config_dir.join("paperdown.toml");
    let env_file = config_dir.join("custom.env");
    std::fs::write(&config, "env-file = \"custom.env\"\n").unwrap();

    let mut cmd = Command::cargo_bin("paperdown").unwrap();
    let output = cmd
        .current_dir(tmp.path())
        .args(["--config", config.to_str().unwrap(), "doctor"])
        .env(TEST_CONFIG_DIR_ENV, &config_root)
        .env("ZAI_API_KEY", "test-key")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("config: ok"));
    assert!(stdout.contains(&format!("  {}", config.display())));
    let env_line = format!("env file: {}", env_file.display());
    assert!(stdout.contains(&env_line));
    assert!(stdout.contains("auth: ok"));
    assert!(stdout.find("auth: ok").unwrap() < stdout.find(&env_line).unwrap());
}
