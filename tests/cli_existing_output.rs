use assert_cmd::Command;
use std::fs;

#[test]
fn batch_without_log_marker_reaches_env_lookup_even_with_stale_outputs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let pdf_dir = temp.path().join("pdf");
    let out_dir = temp.path().join("md");
    fs::create_dir_all(&pdf_dir).expect("pdf dir");

    let pdf_a = pdf_dir.join("a.pdf");
    let pdf_b = pdf_dir.join("b.pdf");
    fs::write(&pdf_a, b"%PDF").expect("pdf a");
    fs::write(&pdf_b, b"%PDF").expect("pdf b");

    fs::create_dir_all(out_dir.join("a")).expect("out a");
    fs::create_dir_all(out_dir.join("b")).expect("out b");
    fs::write(out_dir.join("a/index.md"), b"old").expect("index a");
    fs::write(out_dir.join("b/index.md"), b"old").expect("index b");
    fs::create_dir_all(out_dir.join("a/figures")).expect("figures a");
    fs::create_dir_all(out_dir.join("b/figures")).expect("figures b");
    fs::write(out_dir.join("a/figures/stale.png"), b"old").expect("stale fig a");
    fs::write(out_dir.join("b/figures/stale.png"), b"old").expect("stale fig b");

    let missing_env = temp.path().join("missing.env");

    let output = Command::cargo_bin("paperdown")
        .expect("binary")
        .args([
            "--input",
            pdf_dir.to_str().expect("pdf path"),
            "--output",
            out_dir.to_str().expect("out path"),
            "--workers",
            "4",
            "--env-file",
            missing_env.to_str().expect("env path"),
        ])
        .env_remove("ZAI_API_KEY")
        .output()
        .expect("run");

    assert!(!output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(stdout.contains("Batch Complete processed: 0 skipped: 0 failed: 2 figures: 0"));

    assert!(stderr.contains("failed:"));
    assert!(stderr.contains("a.pdf"));
    assert!(stderr.contains("b.pdf"));
    assert!(stderr.contains("OCR concurrency:"));

    assert!(stderr.contains("ZAI_API_KEY"));
    assert!(!stderr.contains("Re-run with --overwrite"));
    assert!(!stdout.contains("\u{1b}["));
    assert!(!stderr.contains("\u{1b}["));
}

#[test]
fn batch_existing_log_outputs_skip_without_env_or_ocr() {
    let temp = tempfile::tempdir().expect("tempdir");
    let pdf_dir = temp.path().join("pdf");
    let out_dir = temp.path().join("output");
    fs::create_dir_all(&pdf_dir).expect("pdf dir");
    fs::create_dir_all(&out_dir).expect("output dir");

    let pdf_a = pdf_dir.join("a.pdf");
    let pdf_b = pdf_dir.join("b.pdf");
    fs::write(&pdf_a, b"%PDF").expect("pdf a");
    fs::write(&pdf_b, b"%PDF").expect("pdf b");

    fs::create_dir_all(out_dir.join("a")).expect("out a");
    fs::create_dir_all(out_dir.join("b")).expect("out b");
    fs::write(out_dir.join("a/log.jsonl"), b"{}\n").expect("log a");
    fs::write(out_dir.join("b/log.jsonl"), b"{}\n").expect("log b");

    let missing_env = temp.path().join("missing.env");

    let output = Command::cargo_bin("paperdown")
        .expect("binary")
        .args([
            "--input",
            pdf_dir.to_str().expect("pdf path"),
            "--output",
            out_dir.to_str().expect("out path"),
            "--workers",
            "2",
            "--env-file",
            missing_env.to_str().expect("env path"),
        ])
        .env_remove("ZAI_API_KEY")
        .output()
        .expect("run");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(stdout.contains("Batch Complete processed: 0 skipped: 2 failed: 0 figures: 0"));
    assert!(!stderr.contains("ZAI_API_KEY"));
    assert!(!stderr.contains("OCR concurrency:"));
    assert!(!stderr.contains("failed:"));
}
