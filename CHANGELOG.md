# Changelog

This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Possible sections are:

- `Added` for new features.
- `Changed` for changes in existing functionality.
- `Deprecated` for soon-to-be removed features.
- `Removed` for now removed features.
- `Fixed` for any bug fixes.
- `Security` in case of vulnerabilities.

<!-- next-header -->

## [Unreleased]

### Fixed:
- avoid Z.AI OCR rate-limit failures in large batch runs by introducing OCR-specific concurrency control (`--ocr-workers`) and clearer HTTP 429 guidance ([#7](https://github.com/atsyplenkov/paperdown/issues/7))
- align CLI help and README overwrite guidance with marker-based skip behavior (existing `<output>/<pdf_stem>/log.jsonl` skips by default; skip is not applied when the last marker entry `pdf_path` explicitly points to a different PDF) and conflict recovery instructions ([#11](https://github.com/atsyplenkov/paperdown/issues/11))

## [0.2.0] - 2026-03-18

- published on crates.io

### Changed:
- reduced the binary size
- added in-memory buffering of downloaded figures
- switched to dotenvy as a `.env` reader

### Fixed:
- make `Regex` reusable

## [0.1.0] - 2026-03-16

- initial release, basic coverage of the Z.AI GLM-OCR API
