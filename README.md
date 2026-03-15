# `paperdown`

`paperdown` converts academic PDFs into Markdown using Z.AI OCR and downloads referenced figure assets locally.

## Features

- Async OCR requests and batch PDF processing
- Concurrent figure downloads per PDF
- Safe overwrite behavior with explicit `--overwrite`
- Pretty JSON output on `stdout` for scripting
- Progress display on `stderr` (TTY only)

## Usage

```bash
cargo run -- --input path/to/paper.pdf
```

Batch directory mode:

```bash
cargo run -- --input pdf/ --output md/ --workers 4 --overwrite
```

## CLI

```text
--input <PATH>                 Required: PDF file or directory containing PDFs
--output <PATH>                Output root directory (default: md)
--env-file <PATH>              Env file fallback for ZAI_API_KEY (default: .env)
--timeout <SECONDS>            Request timeout in seconds (default: 180)
--max-download-bytes <BYTES>   Max bytes per downloaded figure (default: 20971520)
--workers <N>                  Max concurrent PDFs (default: min(32, max(4, cpu*4)))
-v, --verbose                  Verbose stderr logs
--overwrite                    Replace existing managed artifacts in target output
```

## API key

`paperdown` reads `ZAI_API_KEY` from environment first. If not found, it reads the value from `--env-file`.
