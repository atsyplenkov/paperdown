use assert_cmd::Command;
use std::fs;

#[test]
fn okf_skip_path_regenerates_root_index_from_existing_paper_dirs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_root = temp.path().join("config");
    let pdf_dir = temp.path().join("pdf");
    let out_dir = temp.path().join("out");
    fs::create_dir_all(&config_root).expect("config dir");
    fs::create_dir_all(&pdf_dir).expect("pdf dir");
    fs::create_dir_all(&out_dir).expect("out dir");

    let pdf_a = pdf_dir.join("a.pdf");
    let pdf_b = pdf_dir.join("b.pdf");
    fs::write(&pdf_a, b"%PDF").expect("pdf a");
    fs::write(&pdf_b, b"%PDF").expect("pdf b");

    // Two existing OKF paper bundles. Each carries the renderer's own frontmatter,
    // a manuscript, and a log.jsonl skip marker so nothing reaches OCR/the network.
    for (stem, title, description) in [
        ("a", "Paper Alpha", "First study."),
        ("b", "Paper Beta", "Second study."),
    ] {
        let dir = out_dir.join(stem);
        fs::create_dir_all(&dir).expect("paper dir");
        fs::write(
            dir.join("index.md"),
            format!(
                "---\n\
                 type: Article\n\
                 title: \"{title}\"\n\
                 description: \"{description}\"\n\
                 source: \"{stem}.pdf\"\n\
                 timestamp: 2026-01-01T00:00:00Z\n\
                 ---\n\n\
                 # Contents\n\n\
                 * [Manuscript](manuscript.md)\n"
            ),
        )
        .expect("paper index");
        fs::write(dir.join("manuscript.md"), b"# Body\n").expect("manuscript");
        fs::write(dir.join("log.jsonl"), b"{}\n").expect("skip marker");
    }

    let missing_env = temp.path().join("missing.env");

    let output = Command::cargo_bin("paperdown")
        .expect("binary")
        .args([
            "--input",
            pdf_dir.to_str().expect("pdf path"),
            "--output",
            out_dir.to_str().expect("out path"),
            "--okf",
            "--env",
            missing_env.to_str().expect("env path"),
        ])
        .env("PAPERDOWN_CONFIG_DIR", &config_root)
        .env_remove("ZAI_API_KEY")
        .output()
        .expect("run");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Batch Complete processed: 0 skipped: 2 failed: 0 figures: 0"));

    // The all-skipped path still regenerates the authoritative bundle root index.
    let root_index = fs::read_to_string(out_dir.join("index.md")).expect("root index");
    assert!(root_index.contains("okf_version: \"0.1\""));
    assert!(root_index.contains("* [Paper Alpha](a/index.md) - First study."));
    assert!(root_index.contains("* [Paper Beta](b/index.md) - Second study."));
    // Titles are drawn from frontmatter and sorted by directory name.
    assert!(root_index.find("Paper Alpha").unwrap() < root_index.find("Paper Beta").unwrap());
}
