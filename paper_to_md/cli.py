from __future__ import annotations

import argparse
import json
import os
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

from .core import OCRClientError, process_pdf

DEFAULT_MAX_DOWNLOAD_BYTES = 20 * 1024 * 1024
DEFAULT_MAX_WORKERS = min(32, max(4, (os.cpu_count() or 1) * 4))


def positive_int(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("value must be a positive integer") from exc
    if parsed <= 0:
        raise argparse.ArgumentTypeError("value must be a positive integer")
    return parsed


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="paper_to_md",
        description="Convert academic PDFs to markdown with Z.AI GLM-OCR.",
    )
    parser.add_argument(
        "--input",
        type=Path,
        required=True,
        help="Path to a single PDF or a directory of PDFs.",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=Path("md"),
        help="Output directory for generated markdown.",
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
        default=DEFAULT_MAX_DOWNLOAD_BYTES,
        help="Maximum bytes to download for any single remote figure.",
    )
    parser.add_argument(
        "--workers",
        type=positive_int,
        default=DEFAULT_MAX_WORKERS,
        help="Maximum parallel workers for batch processing.",
    )
    parser.add_argument(
        "-v",
        "--verbose",
        action="store_true",
        help="Print progress details, including paths and queued files.",
    )
    return parser


def collect_pdfs(input_path: Path) -> list[Path]:
    input_path = input_path.resolve()
    if input_path.is_file():
        if input_path.suffix.lower() != ".pdf":
            raise OCRClientError(f"Input must be a PDF: {input_path}")
        return [input_path]
    if input_path.is_dir():
        pdfs = sorted(input_path.glob("*.pdf"))
        if not pdfs:
            raise OCRClientError(f"No PDF files found in: {input_path}")
        return pdfs
    raise OCRClientError(f"Input path does not exist: {input_path}")


def process_single(
    pdf_path: Path,
    output: Path,
    env_file: Path,
    timeout: int,
    max_download_bytes: int,
) -> dict:
    result = process_pdf(
        pdf_path=pdf_path,
        output_root=output,
        env_file=env_file,
        timeout=timeout,
        max_download_bytes=max_download_bytes,
    )
    return {
        "pdf": str(pdf_path),
        "output_dir": str(result.output_dir),
        "markdown_path": str(result.markdown_path),
        "downloaded_figures": result.downloaded_figures,
        "remote_figure_links": result.remote_figure_links,
        "image_blocks": result.image_blocks,
        "usage": result.usage,
        "log_path": str(result.log_path),
    }


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    try:
        pdfs = collect_pdfs(args.input)
    except OCRClientError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    if len(pdfs) == 1:
        if args.verbose:
            print(f"Processing 1 PDF: {pdfs[0].name}", file=sys.stderr)
        try:
            summary = process_single(
                pdfs[0],
                args.output,
                args.env_file,
                args.timeout,
                args.max_download_bytes,
            )
        except OCRClientError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1
        if args.verbose:
            print(f"  done: {pdfs[0].name}", file=sys.stderr)
        print(json.dumps(summary, indent=2))
        return 0

    results: list[dict] = []
    errors: list[dict] = []
    workers = min(args.workers, len(pdfs))
    print(f"Processing {len(pdfs)} PDFs with {workers} workers...", file=sys.stderr)
    if args.verbose:
        print(f"Input path: {args.input.resolve()}", file=sys.stderr)
        print(f"Output path: {args.output.resolve()}", file=sys.stderr)
        for pdf in pdfs:
            print(f"  queued: {pdf.name}", file=sys.stderr)

    with ThreadPoolExecutor(max_workers=workers) as pool:
        futures = {
            pool.submit(
                process_single,
                pdf,
                args.output,
                args.env_file,
                args.timeout,
                args.max_download_bytes,
            ): pdf
            for pdf in pdfs
        }
        for future in as_completed(futures):
            pdf = futures[future]
            try:
                results.append(future.result())
                print(f"  done: {pdf.name}", file=sys.stderr)
            except OCRClientError as exc:
                errors.append({"pdf": str(pdf), "error": str(exc)})
                print(f"  failed: {pdf.name}: {exc}", file=sys.stderr)

    print(
        json.dumps(
            {
                "processed": len(results),
                "failed": len(errors),
                "results": results,
                "errors": errors,
            },
            indent=2,
        )
    )
    return 1 if errors else 0
