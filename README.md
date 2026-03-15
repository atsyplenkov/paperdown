# paper_to_md

Convert academic PDFs into Markdown that you can actually edit, cite, and diff, with figures saved locally.

If you work with papers, you already know the annoying part is not the OCR itself. It is the cleanup. Tables go missing, table structure gets mixed up, and formulas sometimes come back as plain text noise. You end up spending more time fixing the output than reading the paper.

This project exists because Docling and Marker are both good tools, but in practice they can still skip tables or mix table structure in ways that need manual repair. Docling can also struggle with formula parsing in some papers. I wanted a simple, repeatable pipeline that produces one Markdown file, a local `figures/` folder, and a log you can grep later.

`paper_to_md` uses Z.AI GLM-OCR via the `layout_parsing` endpoint and then rewrites image links so the Markdown points to local figure files. The official Z.AI OCR docs are here: https://docs.z.ai/guides/vlm/glm-ocr#python

## What you get

For an input PDF called `SomePaper.pdf`, the tool writes into `md/SomePaper/`. The main output is `md/SomePaper/index.md`. Any figures that the OCR response exposes as URLs are downloaded into `md/SomePaper/figures/`, and the Markdown image links are rewritten to point at those local paths. Each run appends one JSON line into `md/SomePaper/log.jsonl` with basic counters and the token usage reported by the API.

## Requirements

You need Python 3.12 or newer and `uv`. You also need `ZAI_API_KEY` available via environment variables or a local `.env` file in the project root.

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

The tool records token usage in `log.jsonl` under the `usage` field. With pricing at `$0.03` per `1,000,000` tokens, the Batista et al paper run with `total_tokens = 79,080` costs `$0.0023724`. That is about `$0.00237`, or roughly `0.24` cents.

The cost calculation is `79,080 / 1,000,000 * 0.03`.

## Notes

The tool replaces an output directory only if it contains a tool ownership marker, so it does not accidentally delete unrelated folders. It also accepts the legacy marker `.pdf_ocr_output` for backward compatibility.
