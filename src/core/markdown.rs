use anyhow::Result;
use futures::stream::{self, StreamExt};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

const HTML_FRAGMENT_CONCURRENCY: usize = 16;

static MARKDOWN_IMAGE_URL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\((https?://[^)\s]+)\)").expect("valid markdown image URL regex")
});

static HTML_IMAGE_URL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(src\s*=\s*)(['"])(https?://[^'"]+)(['"])"#).expect("valid HTML image URL regex")
});

#[derive(Debug)]
enum Segment {
    Text(String),
    Code(String),
    Html { index: usize, raw: String },
}

pub(crate) fn replace_image_urls(markdown: &str, replacements: &HashMap<String, String>) -> String {
    let updated = MARKDOWN_IMAGE_URL_PATTERN
        .replace_all(markdown, |caps: &regex::Captures<'_>| {
            let remote_url = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            let replacement = replacements
                .get(remote_url)
                .cloned()
                .unwrap_or_else(|| remote_url.to_string());
            format!("({replacement})")
        })
        .into_owned();

    HTML_IMAGE_URL_PATTERN
        .replace_all(&updated, |caps: &regex::Captures<'_>| {
            let prefix = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            let quote = caps.get(2).map(|m| m.as_str()).unwrap_or("\"");
            let remote_url = caps.get(3).map(|m| m.as_str()).unwrap_or_default();
            let suffix = caps.get(4).map(|m| m.as_str()).unwrap_or_default();
            let replacement = replacements
                .get(remote_url)
                .cloned()
                .unwrap_or_else(|| remote_url.to_string());
            format!("{prefix}{quote}{replacement}{suffix}")
        })
        .into_owned()
}

pub(crate) async fn sanitize_html_fragments(markdown: String) -> Result<String> {
    let mut out = String::with_capacity(markdown.len());
    let mut chunk_start = 0usize;
    let mut in_fence = false;
    let mut fence_marker = '\0';
    let mut fence_len = 0usize;
    let mut i = 0usize;

    while i < markdown.len() {
        let line_end = markdown[i..]
            .find('\n')
            .map(|offset| i + offset + 1)
            .unwrap_or(markdown.len());
        let line = &markdown[i..line_end];

        if in_fence {
            out.push_str(line);
            if is_closing_fence_line(line, fence_marker, fence_len) {
                in_fence = false;
                chunk_start = line_end;
            }
            i = line_end;
            continue;
        }

        if let Some((marker, len)) = fence_start(line) {
            out.push_str(&sanitize_non_code_chunk(&markdown[chunk_start..i]).await?);
            out.push_str(line);
            in_fence = true;
            fence_marker = marker;
            fence_len = len;
            chunk_start = line_end;
            i = line_end;
            continue;
        }

        i = line_end;
    }

    if !in_fence {
        out.push_str(&sanitize_non_code_chunk(&markdown[chunk_start..]).await?);
    }

    Ok(out)
}

async fn sanitize_non_code_chunk(chunk: &str) -> Result<String> {
    let mut segments = Vec::new();
    let mut i = 0usize;
    let mut literal_start = 0usize;
    let mut html_count = 0usize;

    while i < chunk.len() {
        if let Some(run_len) = backtick_run_len(chunk, i)
            && let Some(end) = find_matching_backtick_run(chunk, i + run_len, run_len)
        {
            if literal_start < i {
                segments.push(Segment::Text(chunk[literal_start..i].to_string()));
            }
            segments.push(Segment::Code(chunk[i..end + run_len].to_string()));
            i = end + run_len;
            literal_start = i;
            continue;
        }

        if let Some((tag_end, fragment)) = extract_html_fragment(chunk, i) {
            if literal_start < i {
                segments.push(Segment::Text(chunk[literal_start..i].to_string()));
            }
            segments.push(Segment::Html {
                index: html_count,
                raw: fragment,
            });
            html_count += 1;
            i = tag_end;
            literal_start = i;
            continue;
        }

        let ch = chunk[i..].chars().next().expect("valid char boundary");
        i += ch.len_utf8();
    }

    if literal_start < chunk.len() {
        segments.push(Segment::Text(chunk[literal_start..].to_string()));
    }

    if html_count == 0 {
        return Ok(join_segments(&segments, &[]));
    }

    let html_fragments: Vec<String> = segments
        .iter()
        .filter_map(|segment| match segment {
            Segment::Html { raw, .. } => Some(raw.clone()),
            _ => None,
        })
        .collect();
    let converted = convert_html_fragments(html_fragments).await;
    Ok(join_segments(&segments, &converted))
}

fn join_segments(segments: &[Segment], converted: &[Option<String>]) -> String {
    let mut out = String::with_capacity(
        segments
            .iter()
            .map(|segment| match segment {
                Segment::Text(text) | Segment::Code(text) => text.len(),
                Segment::Html { raw, .. } => raw.len(),
            })
            .sum(),
    );

    for segment in segments {
        match segment {
            Segment::Text(text) | Segment::Code(text) => out.push_str(text),
            Segment::Html { index, raw } => {
                if let Some(Some(rewritten)) = converted.get(*index) {
                    out.push_str(rewritten);
                } else {
                    out.push_str(raw);
                }
            }
        }
    }

    out
}

fn extract_html_fragment(text: &str, start: usize) -> Option<(usize, String)> {
    if starts_html_tag(text, start, "table") {
        let end = find_table_fragment_end(text, start)?;
        return Some((end, text[start..end].to_string()));
    }

    if starts_html_tag(text, start, "img") {
        let end = find_html_tag_end(text, start)?;
        return Some((end, text[start..end].to_string()));
    }

    None
}

async fn convert_html_fragments(fragments: Vec<String>) -> Vec<Option<String>> {
    let converted = stream::iter(fragments.into_iter().enumerate().map(
        |(index, fragment)| async move {
            let converted = html2md::rewrite_html_streaming(&fragment, true).await;
            let converted = if converted.trim().is_empty() {
                None
            } else {
                Some(converted)
            };
            (index, converted)
        },
    ))
    .buffer_unordered(HTML_FRAGMENT_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;

    let mut ordered = vec![None; converted.len()];
    for (index, item) in converted {
        ordered[index] = item;
    }
    ordered
}

fn backtick_run_len(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&b'`') {
        return None;
    }

    let mut len = 1usize;
    while start + len < bytes.len() && bytes[start + len] == b'`' {
        len += 1;
    }
    Some(len)
}

fn find_matching_backtick_run(text: &str, start: usize, run_len: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = start;

    while i + run_len <= bytes.len() {
        if bytes[i] == b'`' && bytes[i..i + run_len].iter().all(|b| *b == b'`') {
            return Some(i);
        }
        i += 1;
    }

    None
}

fn starts_html_tag(text: &str, start: usize, tag: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&b'<') {
        return false;
    }

    let tag_start = start + 1;
    let tag_end = tag_start + tag.len();
    let Some(candidate) = text.get(tag_start..tag_end) else {
        return false;
    };
    if !candidate.eq_ignore_ascii_case(tag) {
        return false;
    }

    !matches!(bytes.get(tag_end), Some(b) if b.is_ascii_alphanumeric() || *b == b'-')
}

fn starts_end_html_tag(text: &str, start: usize, tag: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&b'<') || bytes.get(start + 1) != Some(&b'/') {
        return false;
    }

    let tag_start = start + 2;
    let tag_end = tag_start + tag.len();
    let Some(candidate) = text.get(tag_start..tag_end) else {
        return false;
    };
    if !candidate.eq_ignore_ascii_case(tag) {
        return false;
    }

    !matches!(bytes.get(tag_end), Some(b) if b.is_ascii_alphanumeric() || *b == b'-')
}

fn find_table_fragment_end(text: &str, start: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut i = find_html_tag_end(text, start)?;

    while i < text.len() {
        let ch = text[i..].chars().next()?;
        if ch == '<' {
            if starts_html_tag(text, i, "table") {
                let end = find_html_tag_end(text, i)?;
                depth += 1;
                i = end;
                continue;
            }

            if starts_end_html_tag(text, i, "table") {
                let end = find_html_tag_end(text, i)?;
                depth = depth.saturating_sub(1);
                i = end;
                if depth == 0 {
                    return Some(end);
                }
                continue;
            }
        }

        i += ch.len_utf8();
    }

    None
}

fn find_html_tag_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut i = start + 1;

    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'>' if !in_single && !in_double => return Some(i + 1),
            _ => {}
        }
        i += 1;
    }

    None
}

fn fence_start(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    let mut chars = trimmed.chars();
    let marker = chars.next()?;
    if marker != '`' && marker != '~' {
        return None;
    }

    let mut len = 1usize;
    for c in chars {
        if c == marker {
            len += 1;
        } else {
            break;
        }
    }

    if len < 3 {
        return None;
    }

    Some((marker, len))
}

fn is_closing_fence_line(line: &str, marker: char, len: usize) -> bool {
    let trimmed = line.trim_start();
    let mut chars = trimmed.chars();
    let mut count = 0usize;

    while matches!(chars.clone().next(), Some(c) if c == marker) {
        chars.next();
        count += 1;
    }

    count >= len && chars.all(char::is_whitespace)
}
