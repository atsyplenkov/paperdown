use anyhow::Result;
use futures::stream::{self, StreamExt};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

const HTML_FRAGMENT_CONCURRENCY: usize = 16;

const HTML_ALLOWLIST_TAGS: &[&str] = &[
    "img",
    "a",
    "p",
    "div",
    "span",
    "br",
    "hr",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "ul",
    "ol",
    "li",
    "blockquote",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "th",
    "td",
    "strong",
    "b",
    "em",
    "i",
    "u",
    "s",
    "del",
    "pre",
    "code",
];

const HTML_EXCLUDED_TAGS: &[&str] = &["math", "sub", "sup"];

const HTML_VOID_TAGS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
    "source", "track", "wbr",
];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HtmlTagKind {
    Opening,
    Closing,
    Special,
}

#[derive(Debug, Clone, Copy)]
struct ParsedHtmlTag<'a> {
    name: &'a str,
    kind: HtmlTagKind,
    end: usize,
    self_closing: bool,
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
    let tag = parse_html_tag(text, start)?;
    if tag.kind != HtmlTagKind::Opening {
        return None;
    }

    if !is_html_allowlisted(tag.name) || is_html_excluded(tag.name) {
        return None;
    }

    let end = if tag.self_closing || is_html_void(tag.name) {
        tag.end
    } else {
        find_html_region_end(text, start, tag.name)?
    };
    let fragment = text[start..end].to_string();
    if contains_tex_delimiters(&fragment) || contains_excluded_math_tags(&fragment) {
        return None;
    }

    Some((end, fragment))
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

fn parse_html_tag(text: &str, start: usize) -> Option<ParsedHtmlTag<'_>> {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&b'<') {
        return None;
    }

    if matches!(bytes.get(start + 1), Some(b'!') | Some(b'?')) {
        let end = find_html_tag_end(text, start)?;
        return Some(ParsedHtmlTag {
            name: "",
            kind: HtmlTagKind::Special,
            end,
            self_closing: false,
        });
    }

    if bytes.get(start + 1) == Some(&b'/') {
        let name_start = start + 2;
        let name_end = name_end(text, name_start)?;
        let end = find_html_tag_end(text, start)?;
        return Some(ParsedHtmlTag {
            name: &text[name_start..name_end],
            kind: HtmlTagKind::Closing,
            end,
            self_closing: false,
        });
    }

    let name_start = start + 1;
    let name_end = name_end(text, name_start)?;
    let end = find_html_tag_end(text, start)?;
    let self_closing = is_self_closing_tag(text, start, end);

    Some(ParsedHtmlTag {
        name: &text[name_start..name_end],
        kind: HtmlTagKind::Opening,
        end,
        self_closing,
    })
}

fn name_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = start;

    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b':' | b'_') {
            i += 1;
            continue;
        }
        break;
    }

    (i > start).then_some(i)
}

fn is_self_closing_tag(text: &str, start: usize, end: usize) -> bool {
    let bytes = text.as_bytes();
    let mut i = end.saturating_sub(1);
    while i > start && bytes[i - 1].is_ascii_whitespace() {
        i -= 1;
    }

    bytes.get(i - 1) == Some(&b'/')
}

fn find_html_region_end(text: &str, start: usize, root_tag: &str) -> Option<usize> {
    let root = parse_html_tag(text, start)?;
    if root.kind != HtmlTagKind::Opening {
        return None;
    }

    let mut stack = vec![root_tag.to_ascii_lowercase()];
    let mut i = root.end;

    while i < text.len() {
        if text.as_bytes().get(i) == Some(&b'<') && let Some(tag) = parse_html_tag(text, i) {
            match tag.kind {
                HtmlTagKind::Special => {
                    i = tag.end;
                    continue;
                }
                HtmlTagKind::Closing => {
                    let Some(current) = stack.last() else {
                        return None;
                    };
                    if !current.eq_ignore_ascii_case(tag.name) {
                        return None;
                    }
                    stack.pop();
                    i = tag.end;
                    if stack.is_empty() {
                        return Some(i);
                    }
                    continue;
                }
                HtmlTagKind::Opening => {
                    if !(tag.self_closing || is_html_void(tag.name)) {
                        stack.push(tag.name.to_ascii_lowercase());
                    }
                    i = tag.end;
                    continue;
                }
            }
        }

        let ch = text[i..].chars().next()?;
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

fn is_html_allowlisted(tag: &str) -> bool {
    HTML_ALLOWLIST_TAGS.iter().any(|candidate| candidate.eq_ignore_ascii_case(tag))
}

fn is_html_excluded(tag: &str) -> bool {
    HTML_EXCLUDED_TAGS.iter().any(|candidate| candidate.eq_ignore_ascii_case(tag))
}

fn is_html_void(tag: &str) -> bool {
    HTML_VOID_TAGS.iter().any(|candidate| candidate.eq_ignore_ascii_case(tag))
}

fn contains_excluded_math_tags(fragment: &str) -> bool {
    let mut i = 0usize;

    while i < fragment.len() {
        if let Some(run_len) = backtick_run_len(fragment, i)
            && let Some(end) = find_matching_backtick_run(fragment, i + run_len, run_len)
        {
            i = end + run_len;
            continue;
        }

        if let Some(tag) = parse_html_tag(fragment, i)
            && matches!(tag.kind, HtmlTagKind::Opening | HtmlTagKind::Closing)
            && is_html_excluded(tag.name)
        {
            return true;
        }

        let ch = fragment[i..].chars().next().expect("valid char boundary");
        i += ch.len_utf8();
    }

    false
}

fn contains_tex_delimiters(fragment: &str) -> bool {
    let bytes = fragment.as_bytes();
    let mut i = 0usize;
    let mut saw_dollar = false;

    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i = i.saturating_add(2);
            continue;
        }

        if bytes[i] == b'$' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'$' {
                return true;
            }

            if saw_dollar {
                return true;
            }
            saw_dollar = true;
        }

        i += 1;
    }

    false
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
