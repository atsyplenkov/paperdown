# paper_to_md

Convert academic papers in PDF into Markdown with figure assets using Z.AI GLM-OCR.

## Features

- OCR a local PDF through Z.AI `layout_parsing` (`glm-ocr`)
- Save markdown into `md/<PDF_NAME>/index.md`
- Save figure files into `md/<PDF_NAME>/figures/`
- Rewrite markdown and HTML `<img src=...>` links to local figure paths
- Persist run metadata and token usage into `md/<PDF_NAME>/log.jsonl`

## Requirements

- Python 3.12+
- `uv`
- `ZAI_API_KEY` in `.env` or environment

## Quick start

```bash
uv sync
uv run python -m paper_to_md "pdf/your-paper.pdf"
```

## CLI

```bash
uv run python -m paper_to_md --help
```

## Testing

```bash
uv run python -m unittest discover -s tests -v
```

## Notes

- Output directories are replaced only if they contain a tool ownership marker.
- Legacy marker `.pdf_ocr_output` is still accepted for backward compatibility.
