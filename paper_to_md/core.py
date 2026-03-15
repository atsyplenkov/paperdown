from __future__ import annotations

import base64
import json
import mimetypes
import os
import re
import shutil
import tempfile
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from time import perf_counter
from typing import Any
from urllib import error, parse, request

API_URL = "https://api.z.ai/api/paas/v4/layout_parsing"
MARKER_FILENAME = ".paper_to_md_output"
ALLOWED_URL_SCHEMES = {"http", "https"}


class OCRClientError(RuntimeError):
    pass


@dataclass(frozen=True)
class ProcessResult:
    output_dir: Path
    markdown_path: Path
    downloaded_figures: int
    remote_figure_links: int
    image_blocks: int
    usage: dict[str, Any] | None
    log_path: Path


def atomic_write_text(path: Path, content: str, encoding: str = "utf-8") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding=encoding,
        dir=path.parent,
        delete=False,
    ) as handle:
        handle.write(content)
        temp_path = Path(handle.name)
    temp_path.replace(path)


def atomic_write_bytes(path: Path, content: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        mode="wb", dir=path.parent, delete=False
    ) as handle:
        handle.write(content)
        temp_path = Path(handle.name)
    temp_path.replace(path)


def is_allowed_url_scheme(url: str) -> bool:
    scheme = parse.urlparse(url).scheme.lower()
    return scheme in ALLOWED_URL_SCHEMES


def open_request(req: request.Request, timeout: int) -> Any:
    opener = request.build_opener()
    return opener.open(req, timeout=timeout)


def process_pdf(
    pdf_path: Path,
    output_root: Path,
    env_file: Path,
    timeout: int,
    max_download_bytes: int,
) -> ProcessResult:
    run_started = perf_counter()
    pdf_path = pdf_path.resolve()
    if not pdf_path.is_file():
        raise OCRClientError(f"PDF not found: {pdf_path}")
    if pdf_path.suffix.lower() != ".pdf":
        raise OCRClientError(f"Input must be a PDF: {pdf_path}")

    api_key = load_api_key(env_file)
    payload = build_payload(pdf_path)
    ocr_started = perf_counter()
    response = call_layout_parsing(api_key, payload, timeout=timeout)
    ocr_seconds = perf_counter() - ocr_started
    markdown, layout_details = validate_layout_response(response)

    output_dir = prepare_output_dir(output_root.resolve(), pdf_path.stem)
    figures_dir = output_dir / "figures"
    figures_dir.mkdir(exist_ok=True)

    figure_processing_started = perf_counter()
    markdown, downloaded_figures, remote_figure_links, image_blocks = localize_figures(
        markdown=markdown,
        layout_details=layout_details,
        figures_dir=figures_dir,
        timeout=timeout,
        max_download_bytes=max_download_bytes,
    )
    figure_processing_seconds = perf_counter() - figure_processing_started

    markdown_path = output_dir / "index.md"
    write_started = perf_counter()
    atomic_write_text(markdown_path, markdown)
    usage = extract_usage(response)
    log_path = output_dir / "log.jsonl"
    write_seconds = perf_counter() - write_started
    total_seconds = perf_counter() - run_started
    append_log(
        log_path=log_path,
        entry={
            "timestamp_utc": datetime.now(UTC).isoformat(),
            "pdf_path": str(pdf_path),
            "output_dir": str(output_dir),
            "markdown_path": str(markdown_path),
            "downloaded_figures": downloaded_figures,
            "remote_figure_links": remote_figure_links,
            "image_blocks": image_blocks,
            "usage": usage,
            "timing": {
                "ocr_call_s": round(ocr_seconds, 3),
                "figure_processing_s": round(figure_processing_seconds, 3),
                "write_and_log_s": round(write_seconds, 3),
                "total_s": round(total_seconds, 3),
            },
        },
    )

    return ProcessResult(
        output_dir=output_dir,
        markdown_path=markdown_path,
        downloaded_figures=downloaded_figures,
        remote_figure_links=remote_figure_links,
        image_blocks=image_blocks,
        usage=usage,
        log_path=log_path,
    )


def load_api_key(env_file: Path) -> str:
    api_key = os.environ.get("ZAI_API_KEY")
    if api_key:
        return api_key

    if not env_file.is_file():
        raise OCRClientError(
            f"ZAI_API_KEY is not set and env file was not found: {env_file}"
        )

    for line in env_file.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or "=" not in stripped:
            continue
        key, value = stripped.split("=", 1)
        if key.strip() != "ZAI_API_KEY":
            continue
        value = value.strip().strip("'").strip('"')
        if value:
            return value

    raise OCRClientError(f"ZAI_API_KEY was not found in {env_file}")


def build_payload(pdf_path: Path) -> dict[str, Any]:
    encoded_pdf = base64.b64encode(pdf_path.read_bytes()).decode("ascii")
    return {
        "model": "glm-ocr",
        "file": f"data:application/pdf;base64,{encoded_pdf}",
        "return_crop_images": True,
    }


def call_layout_parsing(
    api_key: str, payload: dict[str, Any], timeout: int
) -> dict[str, Any]:
    if not is_allowed_url_scheme(API_URL):
        raise OCRClientError(f"Unsupported API URL scheme: {API_URL}")
    body = json.dumps(payload).encode("utf-8")
    req = request.Request(
        API_URL,
        data=body,
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
        method="POST",
    )

    try:
        with open_request(req, timeout=timeout) as response:
            raw = response.read().decode("utf-8")
    except error.HTTPError as exc:
        details = exc.read().decode("utf-8", errors="replace")
        raise OCRClientError(
            f"Z.AI OCR request failed with HTTP {exc.code}: {details}"
        ) from exc
    except error.URLError as exc:
        raise OCRClientError(f"Could not reach Z.AI OCR API: {exc.reason}") from exc

    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise OCRClientError("Z.AI OCR API returned invalid JSON") from exc

    if not isinstance(parsed, dict):
        raise OCRClientError("Z.AI OCR API returned an unexpected response type")
    return parsed


def validate_layout_response(data: dict[str, Any]) -> tuple[str, list[Any]]:
    markdown = data.get("md_results")
    if not isinstance(markdown, str):
        raise OCRClientError("Z.AI OCR response is missing string field 'md_results'")

    layout_details = data.get("layout_details")
    if not isinstance(layout_details, list):
        raise OCRClientError("Z.AI OCR response is missing list field 'layout_details'")

    return markdown, layout_details


def extract_usage(data: dict[str, Any]) -> dict[str, Any] | None:
    usage = data.get("usage")
    if not isinstance(usage, dict):
        return None
    return usage


def append_log(log_path: Path, entry: dict[str, Any]) -> None:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(entry, ensure_ascii=False))
        handle.write("\n")


def prepare_output_dir(output_root: Path, pdf_name: str) -> Path:
    output_root.mkdir(parents=True, exist_ok=True)
    output_dir = output_root / pdf_name
    marker_path = output_dir / MARKER_FILENAME

    if output_dir.exists():
        if not output_dir.is_dir():
            raise OCRClientError(
                f"Output path exists and is not a directory: {output_dir}"
            )
        if not marker_path.is_file():
            raise OCRClientError(
                f"Refusing to replace non-tool output directory: {output_dir}"
            )
        shutil.rmtree(output_dir)

    output_dir.mkdir(parents=True, exist_ok=False)
    atomic_write_text(marker_path, "generated-by=paper_to_md\n")
    return output_dir


def localize_figures(
    markdown: str,
    layout_details: list[Any],
    figures_dir: Path,
    timeout: int,
    max_download_bytes: int,
) -> tuple[str, int, int, int]:
    downloaded_figures = 0
    remote_figure_links = 0
    image_blocks = 0
    replacements: dict[str, str] = {}

    for page_number, page_blocks in enumerate(layout_details, start=1):
        if not isinstance(page_blocks, list):
            continue
        for block_number, block in enumerate(page_blocks, start=1):
            if not isinstance(block, dict) or block.get("label") != "image":
                continue
            image_blocks += 1
            remote_url = extract_image_url(block)
            if not remote_url:
                continue
            remote_figure_links += 1
            if remote_url in replacements:
                continue

            local_name = f"fig-{page_number:03d}-{block_number:03d}"
            local_path = download_figure(
                remote_url=remote_url,
                figures_dir=figures_dir,
                base_name=local_name,
                timeout=timeout,
                max_download_bytes=max_download_bytes,
            )
            if local_path is None:
                continue

            downloaded_figures += 1
            replacements[remote_url] = f"figures/{local_path.name}"

    updated_markdown = replace_image_urls(markdown, replacements)
    return updated_markdown, downloaded_figures, remote_figure_links, image_blocks


def extract_image_url(block: dict[str, Any]) -> str | None:
    for key in ("content", "image_url", "crop_image_url", "url", "file_url"):
        found = find_http_url(block.get(key))
        if found is not None:
            return found
    return None


def find_http_url(value: Any) -> str | None:
    if isinstance(value, str):
        return value if is_http_url(value) else None
    if isinstance(value, list):
        for item in value:
            found = find_http_url(item)
            if found is not None:
                return found
    if isinstance(value, dict):
        for item in value.values():
            found = find_http_url(item)
            if found is not None:
                return found
    return None


def is_http_url(value: str) -> bool:
    return value.startswith("http://") or value.startswith("https://")


def download_figure(
    remote_url: str,
    figures_dir: Path,
    base_name: str,
    timeout: int,
    max_download_bytes: int,
) -> Path | None:
    if not is_allowed_url_scheme(remote_url):
        return None
    req = request.Request(
        remote_url,
        headers={"User-Agent": "paper_to_md/0.1.0"},
        method="GET",
    )

    try:
        with open_request(req, timeout=timeout) as response:
            content_type = response.headers.get_content_type()
            content_length = response.headers.get("Content-Length")
            if content_length and int(content_length) > max_download_bytes:
                return None
            data = response.read(max_download_bytes + 1)
    except (error.HTTPError, error.URLError, ValueError):
        return None

    if len(data) > max_download_bytes:
        return None

    if content_type and not content_type.startswith("image/"):
        return None

    suffix = content_type_to_suffix(content_type) or url_suffix(remote_url) or ".img"
    local_path = figures_dir / f"{base_name}{suffix}"
    atomic_write_bytes(local_path, data)
    return local_path


def content_type_to_suffix(content_type: str | None) -> str | None:
    if not content_type:
        return None
    content_type = content_type.split(";", 1)[0].strip().lower()
    suffix = mimetypes.guess_extension(content_type)
    if suffix == ".jpe":
        return ".jpg"
    return suffix


def url_suffix(url: str) -> str | None:
    path = parse.urlparse(url).path
    suffix = Path(path).suffix
    return suffix if suffix else None


def replace_image_urls(markdown: str, replacements: dict[str, str]) -> str:
    markdown_pattern = re.compile(r"\((https?://[^)\s]+)\)")
    html_pattern = re.compile(r"""(src\s*=\s*)(['"])(https?://[^'"]+)(\2)""")

    def markdown_replacement(match: re.Match[str]) -> str:
        remote_url = match.group(1)
        return f"({replacements.get(remote_url, remote_url)})"

    def html_replacement(match: re.Match[str]) -> str:
        remote_url = match.group(3)
        return f"{match.group(1)}{match.group(2)}{replacements.get(remote_url, remote_url)}{match.group(4)}"

    updated = markdown_pattern.sub(markdown_replacement, markdown)
    return html_pattern.sub(html_replacement, updated)
