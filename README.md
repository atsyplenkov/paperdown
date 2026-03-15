<h1 align=center><code>paperdown</code></h1>

<p align="center">
    <a href="https://github.com/atsyplenkov/paperdown/releases">
        <img src="https://img.shields.io/github/v/release/atsyplenkov/paperdown?style=flat&labelColor=1C2C2E&color=dea584&logo=GitHub&logoColor=white"></a>
    <a href="https://crates.io/crates/paperdown/">
        <img src="https://img.shields.io/crates/v/paperdown?style=flat&labelColor=1C2C2E&color=dea584&logo=Rust&logoColor=white"></a>
    <a href="https://codecov.io/gh/atsyplenkov/paperdown">
        <img src="https://img.shields.io/codecov/c/gh/atsyplenkov/paperdown?style=flat&labelColor=1C2C2E&color=dea584&logo=Codecov&logoColor=white"></a>
    <br>
    <a href="https://github.com/atsyplenkov/paperdown/actions/workflows/rust-ci.yml">
        <img src="https://img.shields.io/github/actions/workflow/status/atsyplenkov/paperdown/rust-ci.yml?style=flat&labelColor=1C2C2E&color=dea584&logo=GitHub%20Actions&logoColor=white"></a>
    <a href="https://github.com/atsyplenkov/paperdown/actions/workflows/rust-cd.yml">
        <img src="https://img.shields.io/github/actions/workflow/status/atsyplenkov/paperdown/rust-cd.yml?style=flat&labelColor=1C2C2E&color=dea584&logo=GitHub%20Actions&logoColor=white&label=deploy"></a>
    <a href="https://docs.rs/paperdown/">
        <img src="https://img.shields.io/docsrs/paperdown?style=flat&labelColor=1C2C2E&color=dea584&logo=Rust&logoColor=white"></a>
    <br>
</p>

`paperdown` converts paper PDFs into Markdown using Z.AI OCR and downloads referenced figure assets locally.

If you work with papers, you already know the annoying part is not the OCR itself. It is the cleanup. Tables go missing, table structure gets mixed up, and formulas sometimes come back as plain text noise. You end up spending more time fixing the output than reading the paper.

This project exists because Docling and Marker are both good tools, but in practice they can still skip tables or mix table structure in ways that need manual repair. Docling can also struggle with formula parsing in some papers. I wanted a simple, repeatable pipeline that produces one Markdown file, a local `figures/` folder, and a possibility to batch process all my library.

## Features

- Async OCR requests and batch PDF processing
- Concurrent figure downloads per PDF
- Safe overwrite behavior with explicit `--overwrite`
- Progress display on `stderr` (TTY only)

## Usage

```bash
paperdown --input path/to/paper.pdf
```

Batch directory mode:

```bash
paperdown --input pdf/ --output md/ --workers 4 --overwrite
```

## Install

Install from crates.io:

```bash
cargo install paperdown
```

Install from source (this repository):

```bash
cargo install --path .
```

## CLI

```text
$ paperdown --help
paperdown converts one PDF or a directory of PDFs into markdown output folders.

For each PDF, it creates:
- <output>/<pdf_stem>/index.md
- <output>/<pdf_stem>/figures/
- <output>/<pdf_stem>/log.jsonl

API key lookup order:
1) ZAI_API_KEY from environment
2) ZAI_API_KEY from --env-file

Usage: paperdown [OPTIONS] --input <PATH>

Options:
      --input <PATH>                             Input path: a single .pdf file or a directory containing .pdf files.
      --output <OUTPUT>                          Output root directory for generated markdown folders. [default: md]
      --env-file <ENV_FILE>                      Path to .env file used only if ZAI_API_KEY is not already set. [default: .env]
      --timeout <TIMEOUT>                        HTTP timeout in seconds for OCR requests and figure downloads. [default: 180]
      --max-download-bytes <MAX_DOWNLOAD_BYTES>  Maximum allowed size (bytes) for each downloaded figure file. [default: 20971520]
      --workers <WORKERS>                        Maximum number of PDFs processed concurrently in batch mode. [default: 32]
  -v, --verbose                                  Enable verbose progress messages on stderr.
      --overwrite                                Replace existing managed output artifacts (index.md and figures/).
  -h, --help                                     Print help (see a summary with '-h')
  -V, --version                                  Print version
```

## API key

`paperdown` reads `ZAI_API_KEY` from environment first. If not found, it reads the value from `--env-file`.

## Cost example

The tool records token usage in `log.jsonl` under the `usage` field. With pricing at `$0.03` per `1,000,000` tokens, the Batista et al. (2022) paper run with `total_tokens = 79,080` costs `$0.0023724`. That is roughly `0.24` cents per paper.

The cost calculation is `79,080 / 1,000,000 * 0.03`.
