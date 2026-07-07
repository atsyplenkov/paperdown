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

`paperdown` converts research papers from PDF to Markdown using Z.AI's [GLM-OCR](https://github.com/zai-org/GLM-OCR) model and downloads referenced figure assets locally.

If you work with academic papers, you know that the OCR process itself is not the most difficult part. The real challenge is cleaning up the output. Tables can disappear, their structure can become jumbled, and formulas might be converted into meaningless text. This often means you spend more time correcting the output than working with it.

I used to rely on [`marker`](https://github.com/datalab-to/marker) for PDF parsing and thought it was great. However, after converting the [Batista et al. (2022)](https://hess.copernicus.org/articles/26/3753/2022/) article one day, I discovered that Table 4 was missing, regardless of the settings or LLMs I used (via the `--use-llm` flag). I then switched to [`docling`](https://github.com/docling-project/docling), and Table 4 reappeared, but all the formulas were gone. Furthermore, both tools require a GPU, and even on a Google Colab T4 instance, processing one article takes 4 to 5 minutes.

Therefore, this project was created because, while [`docling`](https://github.com/docling-project/docling) and [`marker`](https://github.com/datalab-to/marker) are both good tools, they can sometimes miss tables or mix up table structures in ways that require manual correction. I wanted a simple, reliable process that produces a Markdown index file I can trust, local `figures/` and optional `tables/` folders, and the ability to process my entire library quickly on my laptop.

## Features

- Async OCR requests and batch PDF processing using the Z.AI API.
- Concurrent figure downloads for each PDF.
- Fast processing with separate controls for total pipeline concurrency and OCR API concurrency.

> [!note]
> This tool was designed to be used with academic papers written in English. Parsing other PDFs, heavy in tables or figures, or in other languages rather than English has not been tested.

## Usage

Start by running:

```bash
paperdown --input path/to/paper.pdf
```

My preferred method is batch directory processing:

```bash
paperdown --input pdf/ --output md/ --workers 32 --ocr-workers 2 --overwrite
```

`--workers` controls how many PDFs are processed concurrently in batch mode. `--ocr-workers` controls concurrent OCR API calls. Effective OCR concurrency is `min(--workers, --ocr-workers)`.

Without `--overwrite`, an existing `<output>/<pdf_stem>/log.jsonl` marker skips the PDF. If the log marker is missing, `paperdown` treats the PDF as unprocessed and refreshes managed artifacts (`index.md`, `figures/`, and `tables/` when `--normalize-tables` is enabled). With `--overwrite`, `paperdown` replaces the whole `<output>/<pdf_stem>/` folder before processing.

OKF output: pass `--okf` to structure each paper directory as an [Open Knowledge Format](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md) bundle. In OKF mode `<output>/<pdf_stem>/manuscript.md` contains the parsed manuscript text, `<output>/<pdf_stem>/index.md` contains metadata frontmatter plus a directory map, `<output>/<pdf_stem>/layout.json` contains OCR layout regions, and `figures/` and `tables/` are always present. The output root also gets a regenerated `index.md` listing all paper bundles and an append-only `log.md` update history.

### Table formats and LLM readability

GLM-OCR returns tables as HTML (`<table>...</table>`), not markdown, and `paperdown` keeps that HTML inline by default. This is deliberate: HTML is the only format of the three discussed here that losslessly preserves merged cells (`rowspan`/`colspan`), which scientific papers use heavily, and cross-format benchmarks ([TabVerse](https://arxiv.org/abs/2606.09578), [Table Meets LLM](https://arxiv.org/abs/2305.13062)) find HTML to be among the most robust text representations for LLM table understanding -- likely because of how much HTML markup LLMs see during pretraining. The cost is tokens: HTML uses roughly 3x the tokens of an equivalent markdown table.

`--normalize-tables` rewrites each inline table into a record-per-row format (`Row: {"column": "value", ...}`) with column names repeated on every row, and stores the untouched OCR HTML under `tables/`. This record style closely matches the key-value formats that score highest in format-comparison benchmarks ([Which Table Format Do LLMs Understand Best?](https://www.improvingagents.com/blog/best-input-data-format-for-llms/)): repeating the keys on every row means an LLM never has to count columns to associate a value with its header, which is where pipe-tables and CSV degrade on wide or long tables. Tables that are too large or contain nested tables are left as a placeholder pointing at the raw HTML artifact instead of being rewritten.

Plain markdown pipe-tables are intentionally not offered: they benchmark no better than HTML for LLM comprehension and cannot represent merged cells at all.

Practical guidance: keep the default (inline HTML) when you want a faithful, lossless transcript of the paper; add `--normalize-tables` when the markdown is destined for LLM consumption (RAG, agents) and per-row lookup accuracy matters more than token count. Both compose with `--okf`; with `--okf` alone, raw HTML artifacts are still extracted to `tables/` while the manuscript keeps the inline HTML unchanged.

### Formulas

GLM-OCR returns formulas as LaTeX. `paperdown` preserves that LaTeX verbatim in `manuscript.md` and in `layout.json` region content; it does not convert formulas to Unicode or plain text. Keeping the original LaTeX leaves mathematical structure recoverable for agents and downstream parsers.

### OCR layout regions (OKF)

With `--okf`, each paper bundle includes `layout.json`, a per-page sidecar with schema `paperdown.layout.v1`. Each region records its OCR label, bounding box, raw content, and an artifact link when one can be resolved. Image regions link to downloaded files under `figures/`; table regions link to `tables/table_NNN.html` when the table-region count matches the number of extracted table artifacts. If those counts disagree, table linking is skipped and `table_artifact_match` is `"none"`.

## Installation

Install from crates.io:

```bash
cargo install paperdown
```

Install from source (this repository):

```bash
cargo install --git https://github.com/atsyplenkov/paperdown.git
```

## CLI Usage

```text
Usage: paperdown [OPTIONS] [COMMAND]

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
```

## Configuration

`paperdown` can read shared defaults from `paperdown.toml`. Runtime settings such as API key file, timeouts, worker counts, verbosity, overwrite behavior, table normalization, and OKF output can be configured. `--input`, `--output`, and `--config` stay CLI-only so each run still names the source PDFs, output root, and config source explicitly.

Default config locations:

- Global: `${PAPERDOWN_CONFIG_DIR}/paperdown.toml` when `PAPERDOWN_CONFIG_DIR` is set to an absolute path; otherwise the CLI-style platform config directory plus `paperdown/paperdown.toml` (`${XDG_CONFIG_HOME:-~/.config}/paperdown/paperdown.toml` on Linux/macOS, `%APPDATA%\\paperdown\\paperdown.toml` on Windows).
- Local: the nearest `paperdown.toml` found by walking upward from the current working directory.

Precedence without `--config`: CLI overrides > nearest local `paperdown.toml` > global `paperdown.toml` > built-in defaults.

Precedence with `--config <PATH>`: CLI overrides > that config file > built-in defaults. Automatic global/local discovery is disabled. Verbose output enabled in config can be disabled per run with `--quiet`.

Example:

```toml
env-file = ".env"
timeout = 180
max-download-bytes = 20971520
workers = 32
ocr-workers = 2
verbose = false
overwrite = false
normalize-tables = false
okf = false
```

Relative `env-file` values in TOML are resolved relative to the TOML file directory. CLI `--env` paths keep normal current-working-directory behavior.

## API Key

`paperdown` first looks for `ZAI_API_KEY` in the `--env` file. If it is not found, it then checks the environment variables. To obtain a key, create an account in the [Z.AI console](https://z.ai/manage-apikey/apikey-list) and generate an API key from your account settings.
### Storing the Key

The easiest method is to set `ZAI_API_KEY` in your shell environment.

```bash
export ZAI_API_KEY="your-api-key"
paperdown --input path/to/paper.pdf
```

If you prefer to use a file, create a `.env` file in the project's root directory.

```dotenv
ZAI_API_KEY=your-api-key
```

Then, run `paperdown` as usual, or specify a different file using `--env`.

## Examples

Another example of a table that was parsed incorrectly from a paper is shown below. The paper by Van Rompaey et al. (2005) was converted to markdown by `marker` incorrectly after about 4 minutes of runtime on T4 GPUs in Google Colab. Using LLM postprocessing in `marker` (with the `--use-llm` flag and the GEMINI model), the model parsed the table correctly. However, the compute time increased to about 5 minutes and the GEMINI API call cost around `$0.02`. The `paperdown` tool parsed the table correctly, returned the files after 24 seconds, and used `46945` tokens, costing approximately `$0.0014`.

![](assets/paperdown_example.png)

## Cost (Rough Estimate)

The tool records token usage in `log.jsonl` under the `usage` field. With pricing at `$0.03` per `1,000,000` tokens (both input and output), processing an average-sized scientific paper like [Batista et al., 2022](https://hess.copernicus.org/articles/26/3753/2022/) with `total_tokens = 79,080` costs approximately `$0.0023724`. This is about **0.24 cents** per article.

## Related Projects

* [`docling`](https://github.com/docling-project/docling) — my preference if you do not need tables, figures, or formulas.
* [`marker`](https://github.com/datalab-to/marker) — good for extracting formulas with LLM post-processing.
* [`opendataloader-pdf`](https://github.com/opendataloader-project/opendataloader-pdf) — I have not tried this yet, but its benchmarks are very good.

## Licence

MIT
