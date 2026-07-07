use anyhow::Result;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use tokio::fs;

use super::output;

#[derive(Debug, Clone, Default)]
pub(crate) struct PaperMetadata {
    pub(crate) title: Option<String>,
    pub(crate) abstract_text: Option<String>,
    pub(crate) keywords: Vec<String>,
}

pub(crate) struct RootLogEntry {
    pub(crate) stem: String,
    pub(crate) title: String,
}

pub(crate) fn extract_metadata(markdown: &str) -> PaperMetadata {
    PaperMetadata {
        title: extract_title(markdown),
        abstract_text: extract_abstract(markdown),
        keywords: extract_keywords(markdown),
    }
}

pub(crate) fn render_paper_index(
    meta: &PaperMetadata,
    stem: &str,
    source_rel: &str,
    timestamp_rfc3339: &str,
    figures_count: usize,
    tables_count: usize,
) -> String {
    let title = meta.title.as_deref().unwrap_or(stem);
    let mut out = String::from("---\n");
    out.push_str("type: Article\n");
    out.push_str("title: ");
    out.push_str(&yaml_string(title));
    out.push('\n');

    if let Some(abstract_text) = meta.abstract_text.as_deref() {
        out.push_str("description: ");
        out.push_str(&yaml_string(&description_from_abstract(abstract_text)));
        out.push('\n');
        out.push_str("abstract: ");
        out.push_str(&yaml_string(abstract_text));
        out.push('\n');
    }

    if !meta.keywords.is_empty() {
        out.push_str("keywords: [");
        for (index, keyword) in meta.keywords.iter().enumerate() {
            if index > 0 {
                out.push_str(", ");
            }
            out.push_str(&render_keyword(keyword));
        }
        out.push_str("]\n");
    }

    out.push_str("source: ");
    out.push_str(&yaml_string(source_rel));
    out.push('\n');
    out.push_str("timestamp: ");
    out.push_str(timestamp_rfc3339);
    out.push_str("\n---\n\n# Contents\n\n");
    out.push_str("* [Manuscript](manuscript.md) - Full parsed text of the manuscript.\n");
    out.push_str(&format!(
        "* [figures/](figures/) - {figures_count} downloaded figure file(s).\n"
    ));
    out.push_str(&format!(
        "* [tables/](tables/) - {tables_count} extracted table artifact(s).\n"
    ));
    out
}

pub(crate) async fn regenerate_root_index(output_root: &Path) -> Result<()> {
    let mut papers = Vec::new();
    let mut entries = match fs::read_dir(output_root).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            output::atomic_write_text(&output_root.join("index.md"), &render_root_index(&papers))
                .await?;
            return Ok(());
        }
        Err(err) => return Err(err.into()),
    };

    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if !file_type.is_dir() {
            continue;
        }

        let dirname = entry.file_name().to_string_lossy().into_owned();
        if dirname.starts_with('.') {
            continue;
        }

        let dir = entry.path();
        if !dir.join("index.md").is_file() || !dir.join("manuscript.md").is_file() {
            continue;
        }

        let frontmatter = fs::read_to_string(dir.join("index.md"))
            .await
            .ok()
            .and_then(|content| parse_frontmatter_summary(&content));
        let (title, description) = frontmatter.unwrap_or_else(|| (dirname.clone(), String::new()));
        papers.push(PaperIndexEntry {
            dirname,
            title,
            description,
        });
    }

    papers.sort_by(|left, right| left.dirname.cmp(&right.dirname));
    output::atomic_write_text(&output_root.join("index.md"), &render_root_index(&papers)).await
}

pub(crate) async fn append_root_log(output_root: &Path, entries: &[RootLogEntry]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }

    let log_path = output_root.join("log.md");
    let mut content = match fs::read_to_string(&log_path).await {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::from("# Update Log\n"),
        Err(err) => return Err(err.into()),
    };
    if content.is_empty() {
        content.push_str("# Update Log\n");
    }
    if !content.ends_with('\n') {
        content.push('\n');
    }

    let today = OffsetDateTime::now_utc().date().to_string();
    let heading = format!("## {today}");
    let bullets = entries
        .iter()
        .map(|entry| {
            format!(
                "* **Creation**: Parsed [{}](/{}/index.md).",
                entry.title, entry.stem
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    if let Some(heading_start) = find_date_heading(&content, &heading) {
        let insert_at = content[heading_start..]
            .find('\n')
            .map(|offset| heading_start + offset + 1)
            .unwrap_or(content.len());
        content.insert_str(insert_at, &format!("{bullets}\n"));
    } else if let Some(title_end) = content.find('\n') {
        let insert_at = title_end + 1;
        content.insert_str(insert_at, &format!("\n{heading}\n{bullets}\n"));
    } else {
        content.push_str(&format!("\n{heading}\n{bullets}\n"));
    }

    output::atomic_write_text(&log_path, &content).await
}

fn extract_title(markdown: &str) -> Option<String> {
    for line in markdown.lines() {
        if let Some(title) = line.strip_prefix("# ") {
            let title = title.trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }

    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("![")
            || trimmed.starts_with('<')
            || trimmed.starts_with('|')
        {
            continue;
        }
        return Some(truncate_chars(trimmed, 300));
    }

    None
}

fn extract_abstract(markdown: &str) -> Option<String> {
    let lines = markdown.lines().collect::<Vec<_>>();

    for (index, line) in lines.iter().enumerate() {
        let Some(heading_label) = heading_text(line) else {
            continue;
        };
        if !is_abstract_label(heading_label) {
            continue;
        }

        let mut collected = Vec::new();
        for next in lines.iter().skip(index + 1) {
            if heading_text(next).is_some() || keyword_remainder(next).is_some() {
                break;
            }
            let trimmed = next.trim();
            if !trimmed.is_empty() {
                collected.push(trimmed);
            }
        }
        let abstract_text = collapse_ws(&collected.join(" "));
        if !abstract_text.is_empty() {
            return Some(abstract_text);
        }
    }

    for (index, line) in lines.iter().enumerate() {
        let Some(remainder) = abstract_line_remainder(line) else {
            continue;
        };
        let mut collected = Vec::new();
        if !remainder.trim().is_empty() {
            collected.push(remainder.trim());
        }
        for next in lines.iter().skip(index + 1) {
            let trimmed = next.trim();
            if trimmed.is_empty() {
                break;
            }
            collected.push(trimmed);
        }
        let abstract_text = collapse_ws(&collected.join(" "));
        if !abstract_text.is_empty() {
            return Some(abstract_text);
        }
    }

    None
}

fn extract_keywords(markdown: &str) -> Vec<String> {
    for line in markdown.lines() {
        let Some(remainder) = keyword_remainder(line) else {
            continue;
        };
        return remainder
            .split([',', ';'])
            .map(|keyword| keyword.trim().trim_end_matches('.').trim())
            .filter(|keyword| !keyword.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    Vec::new()
}

fn heading_text(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if hashes == 0 {
        return None;
    }
    let after_hashes = trimmed.get(hashes..)?;
    if !after_hashes.starts_with(' ') {
        return None;
    }
    Some(after_hashes.trim())
}

fn is_abstract_label(value: &str) -> bool {
    let cleaned = value
        .trim()
        .trim_matches(|ch| matches!(ch, '*' | '_' | ':' | ' '))
        .to_ascii_lowercase();
    cleaned == "abstract" || cleaned.starts_with("abstract")
}

fn abstract_line_remainder(line: &str) -> Option<&str> {
    let trimmed = strip_leading_emphasis(line.trim_start());
    let lower = trimmed.to_ascii_lowercase();
    for prefix in ["abstract:", "abstract."] {
        if lower.starts_with(prefix) {
            return trimmed.get(prefix.len()..).map(strip_remainder_decoration);
        }
    }
    None
}

fn keyword_remainder(line: &str) -> Option<&str> {
    let trimmed = strip_leading_emphasis(line.trim_start());
    let lower = trimmed.to_ascii_lowercase();
    for prefix in ["keywords:", "key words:"] {
        if lower.starts_with(prefix) {
            return trimmed.get(prefix.len()..).map(strip_remainder_decoration);
        }
    }
    None
}

fn strip_leading_emphasis(value: &str) -> &str {
    value.trim_start_matches(['*', '_']).trim_start()
}

fn strip_remainder_decoration(value: &str) -> &str {
    value.trim_start_matches(['*', '_', ':', ' '])
}

fn collapse_ws(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn description_from_abstract(abstract_text: &str) -> String {
    let collapsed = collapse_ws(abstract_text);
    let sentence_end = collapsed
        .match_indices('.')
        .find_map(|(index, _)| {
            let after = collapsed.get(index + 1..)?;
            (after.is_empty() || after.starts_with(' ')).then_some(index + 1)
        })
        .unwrap_or(collapsed.len());
    truncate_chars(&collapsed[..sentence_end], 200)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn yaml_string(value: &str) -> String {
    let escaped = collapse_ws(value)
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn render_keyword(keyword: &str) -> String {
    if keyword
        .chars()
        .any(|ch| matches!(ch, ',' | ':' | '"' | '[' | ']'))
    {
        yaml_string(keyword)
    } else {
        keyword.to_string()
    }
}

#[derive(Debug)]
struct PaperIndexEntry {
    dirname: String,
    title: String,
    description: String,
}

fn render_root_index(papers: &[PaperIndexEntry]) -> String {
    let mut out = String::from("---\nokf_version: \"0.1\"\n---\n\n# Papers\n");
    if !papers.is_empty() {
        out.push('\n');
    }
    for paper in papers {
        out.push_str(&format!("* [{}]({}/index.md)", paper.title, paper.dirname));
        if !paper.description.is_empty() {
            out.push_str(&format!(" - {}", paper.description));
        }
        out.push('\n');
    }
    out
}

fn parse_frontmatter_summary(content: &str) -> Option<(String, String)> {
    let mut lines = content.lines();
    if lines.next()? != "---" {
        return None;
    }

    let mut title = None;
    let mut description = None;
    for line in lines {
        if line == "---" {
            break;
        }
        if let Some(value) = line.strip_prefix("title:") {
            title = Some(parse_frontmatter_value(value.trim()));
        } else if let Some(value) = line.strip_prefix("description:") {
            description = Some(parse_frontmatter_value(value.trim()));
        }
    }

    title.map(|title| (title, description.unwrap_or_default()))
}

fn parse_frontmatter_value(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        unescape_yaml_string(&trimmed[1..trimmed.len() - 1])
    } else {
        trimmed.to_string()
    }
}

fn unescape_yaml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(next) => {
                    out.push('\\');
                    out.push(next);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn find_date_heading(content: &str, heading: &str) -> Option<usize> {
    let mut offset = 0usize;
    for line in content.split_inclusive('\n') {
        if line.trim_end_matches('\n') == heading {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

pub(crate) fn source_relative_to_output(pdf_path: &Path, output_root: &Path) -> String {
    let canonical_output =
        std::fs::canonicalize(output_root).unwrap_or_else(|_| output_root.to_path_buf());
    pdf_path
        .strip_prefix(canonical_output)
        .map(PathBuf::from)
        .unwrap_or_else(|_| pdf_path.to_path_buf())
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ---- title extraction ----

    #[test]
    fn extract_title_from_h1_heading() {
        let md = "# The Real Title\n\nSome body text.\n";
        assert_eq!(extract_title(md), Some("The Real Title".to_string()));
    }

    #[test]
    fn extract_title_falls_back_to_first_plain_line() {
        let md = "![an image](x.png)\n<table>\n</table>\n| col |\n| --- |\nA Plaintext Lead Line\n";
        assert_eq!(extract_title(md), Some("A Plaintext Lead Line".to_string()));
    }

    #[test]
    fn extract_title_truncates_overlong_fallback_to_300_chars() {
        let long = "x".repeat(500);
        let got = extract_title(&long).expect("fallback title");
        assert_eq!(got.chars().count(), 300);
        assert!(got.chars().all(|c| c == 'x'));
    }

    #[test]
    fn extract_title_none_when_only_images_html_and_tables() {
        let md = "![img](a.png)\n<table>\n</table>\n| a | b |\n| --- | --- |\n";
        assert_eq!(extract_title(md), None);
    }

    // ---- abstract extraction ----

    #[test]
    fn extract_abstract_from_heading_section() {
        let md = "# Paper\n\n## Abstract\n\nThis is the abstract.\nIt spans two lines.\n\n## Introduction\n\nBody.\n";
        assert_eq!(
            extract_abstract(md),
            Some("This is the abstract. It spans two lines.".to_string())
        );
    }

    #[test]
    fn extract_abstract_from_emphasized_inline_label() {
        let md = "# Paper\n\n**Abstract:** This is the abstract text.\n\nBody.\n";
        assert_eq!(
            extract_abstract(md),
            Some("This is the abstract text.".to_string())
        );
    }

    #[test]
    fn extract_abstract_none_when_absent() {
        let md = "# Paper\n\nNo abstract here.\n\n## Introduction\n\nBody.\n";
        assert_eq!(extract_abstract(md), None);
    }

    // ---- keywords extraction ----

    #[test]
    fn extract_keywords_from_plain_label() {
        let md = "Keywords: remote sensing, GIS; hydrology.\n";
        assert_eq!(
            extract_keywords(md),
            vec![
                "remote sensing".to_string(),
                "GIS".to_string(),
                "hydrology".to_string(),
            ]
        );
    }

    #[test]
    fn extract_keywords_from_emphasized_label() {
        let md = "# Paper\n\n**Keywords:** glaciers, permafrost\n";
        assert_eq!(
            extract_keywords(md),
            vec!["glaciers".to_string(), "permafrost".to_string()]
        );
    }

    #[test]
    fn extract_keywords_empty_when_absent() {
        let md = "# Paper\n\nNo keywords here.\n";
        assert!(extract_keywords(md).is_empty());
    }

    // ---- extract_metadata integration ----

    #[test]
    fn extract_metadata_pulls_title_abstract_and_keywords() {
        let md = "# My Paper Title\n\n## Abstract\n\nFirst sentence. Second sentence.\n\n**Keywords:** alpha, beta\n";
        let meta = extract_metadata(md);
        assert_eq!(meta.title.as_deref(), Some("My Paper Title"));
        assert_eq!(
            meta.abstract_text.as_deref(),
            Some("First sentence. Second sentence.")
        );
        assert_eq!(meta.keywords, vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn extract_metadata_empty_input_has_no_fields() {
        let meta = extract_metadata("");
        assert!(meta.title.is_none());
        assert!(meta.abstract_text.is_none());
        assert!(meta.keywords.is_empty());
    }

    // ---- render_paper_index ----

    #[test]
    fn render_paper_index_with_full_metadata() {
        let meta = PaperMetadata {
            title: Some("My Title".to_string()),
            abstract_text: Some("First sentence. Second sentence.".to_string()),
            keywords: vec!["alpha".to_string(), "beta".to_string()],
        };
        let rendered = render_paper_index(
            &meta,
            "stem",
            "papers/foo.pdf",
            "2024-01-01T00:00:00Z",
            3,
            1,
        );
        let expected = "\
---
type: Article
title: \"My Title\"
description: \"First sentence.\"
abstract: \"First sentence. Second sentence.\"
keywords: [alpha, beta]
source: \"papers/foo.pdf\"
timestamp: 2024-01-01T00:00:00Z
---

# Contents

* [Manuscript](manuscript.md) - Full parsed text of the manuscript.
* [figures/](figures/) - 3 downloaded figure file(s).
* [tables/](tables/) - 1 extracted table artifact(s).
";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn render_paper_index_with_empty_metadata_falls_back_to_stem() {
        let meta = PaperMetadata::default();
        let rendered = render_paper_index(
            &meta,
            "fallback-stem",
            "/abs/papers/foo.pdf",
            "2024-01-01T00:00:00Z",
            0,
            0,
        );
        let expected = "\
---
type: Article
title: \"fallback-stem\"
source: \"/abs/papers/foo.pdf\"
timestamp: 2024-01-01T00:00:00Z
---

# Contents

* [Manuscript](manuscript.md) - Full parsed text of the manuscript.
* [figures/](figures/) - 0 downloaded figure file(s).
* [tables/](tables/) - 0 extracted table artifact(s).
";
        assert_eq!(rendered, expected);
    }

    // ---- frontmatter round-trip via the module parser ----

    #[test]
    fn parse_frontmatter_round_trips_escaped_title_and_description() {
        let meta = PaperMetadata {
            title: Some("Title with \"quotes\" and \\ backslash".to_string()),
            abstract_text: Some("First sentence here. More text follows.".to_string()),
            keywords: Vec::new(),
        };
        let rendered = render_paper_index(
            &meta,
            "stem",
            "papers/foo.pdf",
            "2024-01-01T00:00:00Z",
            0,
            0,
        );
        let parsed = parse_frontmatter_summary(&rendered).expect("frontmatter parses");
        assert_eq!(parsed.0, "Title with \"quotes\" and \\ backslash");
        assert_eq!(parsed.1, "First sentence here.");
    }

    #[test]
    fn parse_frontmatter_summary_none_without_marker() {
        assert!(parse_frontmatter_summary("no frontmatter here").is_none());
    }

    // ---- append_root_log date-merge behavior ----

    fn today_heading() -> String {
        format!("## {}", OffsetDateTime::now_utc().date())
    }

    #[test]
    fn append_root_log_merges_into_existing_today_section() {
        let dir = TempDir::new().expect("tempdir");
        let log_path = dir.path().join("log.md");
        let today = today_heading();
        let seed =
            format!("# Update Log\n\n{today}\n\n* **Creation**: Parsed [Old](/old/index.md).\n");
        std::fs::write(&log_path, seed).expect("seed log");

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime
            .block_on(append_root_log(
                dir.path(),
                &[RootLogEntry {
                    stem: "new".to_string(),
                    title: "New".to_string(),
                }],
            ))
            .expect("append ok");

        let content = std::fs::read_to_string(&log_path).expect("read log");
        assert_eq!(
            content.matches(&today).count(),
            1,
            "today heading must not be duplicated"
        );
        let heading_pos = content.find(&today).expect("heading present");
        let new_pos = content
            .find("* **Creation**: Parsed [New](/new/index.md).")
            .expect("new bullet");
        let old_pos = content
            .find("* **Creation**: Parsed [Old](/old/index.md).")
            .expect("old bullet");
        assert!(heading_pos < new_pos, "new bullet sits under the heading");
        assert!(
            new_pos < old_pos,
            "new bullet precedes the preserved old bullet"
        );
    }

    #[test]
    fn append_root_log_creates_today_section_when_missing() {
        let dir = TempDir::new().expect("tempdir");
        let log_path = dir.path().join("log.md");
        let today = today_heading();
        let seed =
            "# Update Log\n\n## 1999-01-01\n\n* **Creation**: Parsed [Old](/old/index.md).\n";
        std::fs::write(&log_path, seed).expect("seed log");

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime
            .block_on(append_root_log(
                dir.path(),
                &[RootLogEntry {
                    stem: "new".to_string(),
                    title: "New".to_string(),
                }],
            ))
            .expect("append ok");

        let content = std::fs::read_to_string(&log_path).expect("read log");
        assert_eq!(
            content.matches(&today).count(),
            1,
            "today section created exactly once"
        );
        assert!(content.contains("* **Creation**: Parsed [New](/new/index.md)."));
        assert!(content.contains("## 1999-01-01"));
        assert!(content.contains("* **Creation**: Parsed [Old](/old/index.md)."));
        let today_pos = content.find(&today).expect("today section");
        let old_section_pos = content.find("## 1999-01-01").expect("old section");
        assert!(
            today_pos < old_section_pos,
            "newest date section is inserted before older ones"
        );
    }

    #[test]
    fn append_root_log_empty_entries_is_a_noop() {
        let dir = TempDir::new().expect("tempdir");
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime
            .block_on(append_root_log(dir.path(), &[]))
            .expect("ok");
        assert!(
            !dir.path().join("log.md").exists(),
            "no log written when there are no entries"
        );
    }
}
