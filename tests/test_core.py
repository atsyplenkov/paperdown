from __future__ import annotations

import argparse
import tempfile
import unittest
from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path
from unittest import mock

from paper_to_md.cli import main, positive_int
from paper_to_md.core import (
    LEGACY_MARKER_FILENAME,
    MARKER_FILENAME,
    OCRClientError,
    append_log,
    call_layout_parsing,
    download_figure,
    load_api_key,
    localize_figures,
    prepare_output_dir,
    replace_image_urls,
    validate_layout_response,
)


class PrepareOutputDirTests(unittest.TestCase):
    def test_creates_marker_file_for_new_tool_owned_directory(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            output_root = Path(tmpdir) / "md"

            created = prepare_output_dir(output_root, "paper")

            self.assertEqual(created, output_root / "paper")
            self.assertEqual(
                (created / MARKER_FILENAME).read_text(encoding="utf-8"),
                "generated-by=paper_to_md\n",
            )

    def test_replaces_existing_tool_owned_directory(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            output_root = Path(tmpdir) / "md"
            created = prepare_output_dir(output_root, "paper")
            stale_file = created / "old.txt"
            stale_file.write_text("stale", encoding="utf-8")

            replaced = prepare_output_dir(output_root, "paper")

            self.assertEqual(replaced, output_root / "paper")
            self.assertTrue((replaced / MARKER_FILENAME).is_file())
            self.assertFalse(stale_file.exists())

    def test_refuses_to_replace_unmarked_directory(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            output_dir = Path(tmpdir) / "md" / "paper"
            output_dir.mkdir(parents=True)

            with self.assertRaises(OCRClientError):
                prepare_output_dir(Path(tmpdir) / "md", "paper")

    def test_replaces_existing_legacy_tool_owned_directory(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            output_dir = Path(tmpdir) / "md" / "paper"
            output_dir.mkdir(parents=True)
            (output_dir / LEGACY_MARKER_FILENAME).write_text(
                "generated-by=pdf_ocr\n", encoding="utf-8"
            )
            stale_file = output_dir / "old.txt"
            stale_file.write_text("stale", encoding="utf-8")

            replaced = prepare_output_dir(Path(tmpdir) / "md", "paper")

            self.assertEqual(replaced, output_dir)
            self.assertTrue((replaced / MARKER_FILENAME).is_file())
            self.assertFalse(stale_file.exists())


class MarkdownRewriteTests(unittest.TestCase):
    def test_replace_image_urls_uses_exact_matches(self) -> None:
        markdown = "before ![](https://example.com/figure.png) after"
        updated = replace_image_urls(
            markdown,
            {"https://example.com/figure.png": "figures/fig-001-001.png"},
        )

        self.assertEqual(updated, "before ![](figures/fig-001-001.png) after")

    def test_replace_image_urls_leaves_similar_urls_unchanged(self) -> None:
        markdown = (
            "exact ![](https://example.com/figure.png) "
            "query ![](https://example.com/figure.png?download=1)"
        )

        updated = replace_image_urls(
            markdown,
            {"https://example.com/figure.png": "figures/fig-001-001.png"},
        )

        self.assertEqual(
            updated,
            "exact ![](figures/fig-001-001.png) "
            "query ![](https://example.com/figure.png?download=1)",
        )

    def test_replace_image_urls_leaves_unknown_remote_urls_unchanged(self) -> None:
        markdown = (
            "known ![](https://example.com/figure.png) "
            "unknown ![](https://example.com/other.png)"
        )

        updated = replace_image_urls(
            markdown,
            {"https://example.com/figure.png": "figures/fig-001-001.png"},
        )

        self.assertEqual(
            updated,
            "known ![](figures/fig-001-001.png) "
            "unknown ![](https://example.com/other.png)",
        )

    def test_replace_image_urls_rewrites_html_img_src(self) -> None:
        markdown = "<img src='https://example.com/figure.png' alt='x'/>"
        updated = replace_image_urls(
            markdown,
            {"https://example.com/figure.png": "figures/fig-001-001.png"},
        )
        self.assertEqual(updated, "<img src='figures/fig-001-001.png' alt='x'/>")

    def test_localize_figures_keeps_remote_url_when_download_fails(self) -> None:
        markdown = "![Figure](https://example.com/figure.png)"
        layout_details = [
            [{"label": "image", "content": "https://example.com/figure.png"}]
        ]

        with tempfile.TemporaryDirectory() as tmpdir:
            figures_dir = Path(tmpdir) / "figures"
            figures_dir.mkdir()
            with mock.patch("paper_to_md.core.download_figure", return_value=None):
                updated, downloaded, remote_links, image_blocks = localize_figures(
                    markdown=markdown,
                    layout_details=layout_details,
                    figures_dir=figures_dir,
                    timeout=10,
                    max_download_bytes=1024,
                )

        self.assertEqual(updated, markdown)
        self.assertEqual(downloaded, 0)
        self.assertEqual(remote_links, 1)
        self.assertEqual(image_blocks, 1)

    def test_localize_figures_rewrites_to_local_path_when_download_succeeds(
        self,
    ) -> None:
        markdown = "![Figure](https://example.com/figure.png)"
        layout_details = [
            [{"label": "image", "content": "https://example.com/figure.png"}]
        ]

        with tempfile.TemporaryDirectory() as tmpdir:
            figures_dir = Path(tmpdir) / "figures"
            figures_dir.mkdir()
            with mock.patch(
                "paper_to_md.core.download_figure",
                return_value=figures_dir / "fig-001-001.png",
            ):
                updated, downloaded, remote_links, image_blocks = localize_figures(
                    markdown=markdown,
                    layout_details=layout_details,
                    figures_dir=figures_dir,
                    timeout=10,
                    max_download_bytes=1024,
                )

        self.assertEqual(updated, "![Figure](figures/fig-001-001.png)")
        self.assertEqual(downloaded, 1)
        self.assertEqual(remote_links, 1)
        self.assertEqual(image_blocks, 1)


class SchemaValidationTests(unittest.TestCase):
    def test_validate_layout_response_requires_markdown(self) -> None:
        with self.assertRaises(OCRClientError):
            validate_layout_response({"layout_details": []})

    def test_validate_layout_response_requires_layout_details(self) -> None:
        with self.assertRaises(OCRClientError):
            validate_layout_response({"md_results": "# hello"})


class ApiKeyLoadingTests(unittest.TestCase):
    def test_load_api_key_reads_env_file_when_variable_is_missing(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            env_file = Path(tmpdir) / ".env"
            env_file.write_text("ZAI_API_KEY=test-key\n", encoding="utf-8")

            with mock.patch.dict("os.environ", {}, clear=True):
                self.assertEqual(load_api_key(env_file), "test-key")


class NetworkBoundaryTests(unittest.TestCase):
    def test_call_layout_parsing_rejects_non_http_api_url(self) -> None:
        with mock.patch("paper_to_md.core.API_URL", "file:///tmp/mock.json"):
            with self.assertRaises(OCRClientError):
                call_layout_parsing("key", {"model": "glm-ocr"}, timeout=1)

    def test_download_figure_rejects_non_http_url_scheme(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            local = download_figure(
                remote_url="file:///tmp/pic.png",
                figures_dir=Path(tmpdir),
                base_name="fig-001-001",
                timeout=1,
                max_download_bytes=1024,
            )
        self.assertIsNone(local)


class CliTests(unittest.TestCase):
    def test_positive_int_rejects_non_numeric_value_with_argparse_error(self) -> None:
        with self.assertRaises(argparse.ArgumentTypeError):
            positive_int("abc")

    def test_cli_returns_clean_error_message(self) -> None:
        stderr = StringIO()
        with (
            mock.patch(
                "paper_to_md.cli.process_pdf", side_effect=OCRClientError("boom")
            ),
            redirect_stderr(stderr),
        ):
            exit_code = main(["paper.pdf"])

        self.assertEqual(exit_code, 1)
        self.assertEqual(stderr.getvalue().strip(), "error: boom")


class RunLogTests(unittest.TestCase):
    def test_append_log_writes_jsonl_entries(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            log = Path(tmpdir) / "md" / "log.jsonl"
            append_log(log, {"a": 1})
            append_log(log, {"b": 2})
            lines = log.read_text(encoding="utf-8").strip().splitlines()
            self.assertEqual(lines[0], '{"a": 1}')
            self.assertEqual(lines[1], '{"b": 2}')


if __name__ == "__main__":
    unittest.main()
