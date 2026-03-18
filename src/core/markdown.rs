use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

static MARKDOWN_IMAGE_URL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\((https?://[^)\s]+)\)").expect("valid markdown image URL regex")
});

static HTML_IMAGE_URL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(src\s*=\s*)(['"])(https?://[^'"]+)(['"])"#).expect("valid HTML image URL regex")
});

static HTML_IMAGE_ALT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)\s+alt(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s/>]+))?"#)
        .expect("valid HTML image alt regex")
});

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

pub(crate) fn strip_html_img_alt_attributes(markdown: &str) -> String {
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
            out.push_str(&sanitize_non_code_chunk(&markdown[chunk_start..i]));
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
        out.push_str(&sanitize_non_code_chunk(&markdown[chunk_start..]));
    }

    out
}

fn sanitize_non_code_chunk(chunk: &str) -> String {
    let mut out = String::with_capacity(chunk.len());
    let mut i = 0usize;

    while i < chunk.len() {
        if let Some(run_len) = backtick_run_len(chunk, i)
            && let Some(end) = find_matching_backtick_run(chunk, i + run_len, run_len)
        {
            out.push_str(&chunk[i..end + run_len]);
            i = end + run_len;
            continue;
        }

        if starts_html_img_tag(chunk, i)
            && let Some(tag_end) = find_html_tag_end(chunk, i)
        {
            out.push_str(&HTML_IMAGE_ALT_PATTERN.replace_all(&chunk[i..tag_end], ""));
            i = tag_end;
            continue;
        }

        let ch = chunk[i..].chars().next().expect("valid char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }

    out
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

fn starts_html_img_tag(text: &str, start: usize) -> bool {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&b'<') {
        return false;
    }

    let Some(prefix) = text.get(start + 1..start + 4) else {
        return false;
    };
    if !prefix.eq_ignore_ascii_case("img") {
        return false;
    }

    !matches!(bytes.get(start + 4), Some(b) if b.is_ascii_alphanumeric() || *b == b'-')
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
