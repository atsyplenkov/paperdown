<h1 align=center><code>paper_to_md</code></h1>

A CLI tool that converts academic PDFs into Markdown using the Z.AI OCR with figures saved locally.

If you work with papers, you already know the annoying part is not the OCR itself. It is the cleanup. Tables go missing, table structure gets mixed up, and formulas sometimes come back as plain text noise. You end up spending more time fixing the output than reading the paper.

This project exists because Docling and Marker are both good tools, but in practice they can still skip tables or mix table structure in ways that need manual repair. Docling can also struggle with formula parsing in some papers. I wanted a simple, repeatable pipeline that produces one Markdown file, a local `figures/` folder, and a possibility to batch process all my library.

`paper_to_md` uses [Z.AI GLM-OCR](https://docs.z.ai/guides/vlm/glm-ocr) via the `layout_parsing` endpoint and then rewrites image links so the Markdown points to local figure files. You can try the OCR quality by uploading your pdf to their website https://ocr.z.ai/ for free, it processes the file on a bit longer side and doesn't allow user to download figures but still gives you the impression of what it looks like.

## What you get

For an input PDF called `SomePaper.pdf`, the tool writes into `md/SomePaper/`. The main output is `md/SomePaper/index.md`. Any figures that the OCR response exposes as URLs are downloaded into `md/SomePaper/figures/`, and the Markdown image links are rewritten to point at those local paths. Each run appends one JSON line into `md/SomePaper/log.jsonl` with basic counters and the token usage reported by the API.

## Requirements

You need Python 3.12 or newer and `uv`. You also need `ZAI_API_KEY` available via environment variables or a local `.env` file in the project root. Refer to [z.ai docs](https://z.ai/model-api) about how get an API key.

## Quick start

This converts a local PDF and writes results under `md/`.

```bash
uv sync
uv run python -m paper_to_md "pdf/your-paper.pdf"
```

## CLI

Run the help to see the available flags, including output location, timeouts, and download limits for remote figure files.

```bash
uv run python -m paper_to_md --help
```

## Testing

The project uses the standard library `unittest` runner.

```bash
uv run python -m unittest discover -s tests -v
```

## Cost example

The tool records token usage in `log.jsonl` under the `usage` field. With pricing at `$0.03` per `1,000,000` tokens, the Batista et al. (2022) paper run with `total_tokens = 79,080` costs `$0.0023724`. That is roughly `0.24` cents.

The cost calculation is `79,080 / 1,000,000 * 0.03`.

## Notes

The tool replaces an output directory only if it contains a tool ownership marker, so it does not accidentally delete unrelated folders. It also accepts the legacy marker `.pdf_ocr_output` for backward compatibility.

## TODO
* add verbose mode
* control `--input` and `--output` flags
* add parallel execution
* add batch processsing
* remove the `.pdf_ocr_output`
* add examples
