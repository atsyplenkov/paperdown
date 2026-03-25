use assert_cmd::Command;
use tempfile::TempDir;

#[test]
fn cli_reports_missing_input_path() {
    let mut cmd = Command::cargo_bin("paperdown").unwrap();
    let output = cmd
        .args(["--input", "/definitely/missing/path.pdf"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Input path does not exist"));
}

#[test]
fn cli_single_pdf_fails_before_network_when_env_missing() {
    let tmp = TempDir::new().unwrap();
    let pdf = tmp.path().join("paper.pdf");
    std::fs::write(&pdf, b"%PDF-1.7\n").unwrap();
    let env_file = tmp.path().join("missing.env");

    let mut cmd = Command::cargo_bin("paperdown").unwrap();
    let output = cmd
        .current_dir(tmp.path())
        .args([
            "--input",
            pdf.to_str().unwrap(),
            "--env-file",
            env_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ZAI_API_KEY is not set and env file was not found"));
}

#[test]
fn cli_batch_reports_failed_count() {
    let tmp = TempDir::new().unwrap();
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
            "--env-file",
            env_file.to_str().unwrap(),
            "--workers",
            "1",
            "--ocr-workers",
            "5",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("Batch Complete processed: 0 failed: 2 figures: 0"));
    assert!(stderr.contains("failed:"));
    assert!(stderr.contains("OCR concurrency: 1"));
}
