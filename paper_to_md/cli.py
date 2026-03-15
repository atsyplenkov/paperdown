from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from .core import OCRClientError, process_pdf


def positive_int(value: str) -> int:
    parsed = int(value)
    if parsed <= 0:
        raise argparse.ArgumentTypeError("value must be a positive integer")
    return parsed


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="paper_to_md",
        description="Convert a local PDF to markdown with Z.AI GLM-OCR.",
    )
    parser.add_argument("pdf_path", type=Path, help="Path to the input PDF.")
    parser.add_argument(
        "--output-root",
        type=Path,
        default=Path("md"),
        help="Root directory for generated markdown output.",
    )
    parser.add_argument(
        "--env-file",
        type=Path,
        default=Path(".env"),
        help="Path to the .env file containing ZAI_API_KEY.",
    )
    parser.add_argument(
        "--timeout",
        type=positive_int,
        default=180,
        help="HTTP timeout in seconds for OCR and asset downloads.",
    )
    parser.add_argument(
        "--max-download-bytes",
        type=positive_int,
        default=20 * 1024 * 1024,
        help="Maximum bytes to download for any single remote figure.",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    try:
        result = process_pdf(
            pdf_path=args.pdf_path,
            output_root=args.output_root,
            env_file=args.env_file,
            timeout=args.timeout,
            max_download_bytes=args.max_download_bytes,
        )
    except OCRClientError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    print(
        json.dumps(
            {
                "output_dir": str(result.output_dir),
                "markdown_path": str(result.markdown_path),
                "downloaded_figures": result.downloaded_figures,
                "remote_figure_links": result.remote_figure_links,
                "image_blocks": result.image_blocks,
                "usage": result.usage,
                "run_log_path": str(result.run_log_path),
            },
            indent=2,
        )
    )
    return 0
