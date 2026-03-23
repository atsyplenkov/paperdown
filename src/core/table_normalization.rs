use anyhow::{Result, anyhow};
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use super::output;

const RAW_HTML_LIMIT: usize = 128 * 1024;
const CELL_LIMIT: usize = 2_000;
const NORMALIZED_CHAR_LIMIT: usize = 32 * 1024;
const ROW_GROUP_SIZE: usize = 25;

#[derive(Debug, Default, Clone, Serialize)]
pub struct TableStats {
    pub tables_found: usize,
    pub tables_raw_written: usize,
    pub tables_normalized: usize,
    pub tables_skipped_in_code: usize,
    pub tables_skipped_nested: usize,
    pub tables_skipped_too_large: usize,
    pub tables_failed_extract: usize,
    pub tables_failed_parse: usize,
}

pub(crate) async fn normalize_tables(
    markdown: &str,
    tables_dir: &Path,
) -> Result<(String, TableStats)> {
    let mut out = String::with_capacity(markdown.len());
    let mut stats = TableStats::default();
    let mut table_index = 0usize;
    let mut pos = 0usize;

    while pos < markdown.len() {
        let line_end_pos = line_end(markdown, pos);
        let line = &markdown[pos..line_end_pos];

        if let Some((marker, fence_len)) = fence_start(line) {
            let fence_start_pos = pos;
            let mut fence_end = line_end_pos;
            let mut closed = false;

            while fence_end < markdown.len() {
                let next_end = line_end(markdown, fence_end);
                let next_line = &markdown[fence_end..next_end];
                fence_end = next_end;
                if is_closing_fence_line(next_line, marker, fence_len) {
                    closed = true;
                    break;
                }
            }

            let block = &markdown[fence_start_pos..fence_end];
            stats.tables_skipped_in_code += count_case_insensitive_occurrences(block, "<table");
            out.push_str(block);
            pos = fence_end;

            if !closed {
                break;
            }
            continue;
        }

        let mut chunk_end = line_end_pos;
        while chunk_end < markdown.len() {
            let next_end = line_end(markdown, chunk_end);
            let next_line = &markdown[chunk_end..next_end];
            if fence_start(next_line).is_some() {
                break;
            }
            chunk_end = next_end;
        }

        let chunk = &markdown[pos..chunk_end];
        let rewritten =
            rewrite_non_code_chunk(chunk, tables_dir, &mut table_index, &mut stats).await?;
        out.push_str(&rewritten);
        pos = chunk_end;
    }

    Ok((out, stats))
}

async fn rewrite_non_code_chunk(
    chunk: &str,
    tables_dir: &Path,
    table_index: &mut usize,
    stats: &mut TableStats,
) -> Result<String> {
    let mut out = String::with_capacity(chunk.len());
    let mut i = 0usize;

    while i < chunk.len() {
        if let Some(run_len) = backtick_run_len(chunk, i) {
            let end = find_matching_backtick_run(chunk, i + run_len, run_len)
                .map(|offset| offset + run_len)
                .unwrap_or(chunk.len());
            let code = &chunk[i..end];
            stats.tables_skipped_in_code += count_case_insensitive_occurrences(code, "<table");
            out.push_str(code);
            i = end;
            continue;
        }

        if starts_tag(chunk, i, "table") {
            stats.tables_found += 1;
            *table_index += 1;
            let ordinal = *table_index;
            let artifact_name = format!("table_{ordinal:03}.html");
            let artifact_rel = format!("tables/{artifact_name}");
            let artifact_path = tables_dir.join(&artifact_name);

            match extract_table_span(chunk, i) {
                TableExtraction::Failed => {
                    stats.tables_failed_extract += 1;
                    out.push_str(&chunk[i..]);
                    break;
                }
                TableExtraction::Span { html, end, nested } => {
                    output::atomic_write_text(&artifact_path, &html).await?;
                    stats.tables_raw_written += 1;

                    if nested {
                        stats.tables_skipped_nested += 1;
                        out.push_str(&render_placeholder_block(
                            ordinal,
                            &artifact_rel,
                            "normalization skipped (nested table detected)",
                        ));
                    } else {
                        match render_normalized_table(&html, ordinal, &artifact_rel) {
                            Ok(Some(rendered)) => {
                                stats.tables_normalized += 1;
                                out.push_str(&rendered);
                            }
                            Ok(None) => {
                                stats.tables_skipped_too_large += 1;
                                out.push_str(&render_placeholder_block(
                                    ordinal,
                                    &artifact_rel,
                                    "normalization skipped (table too large)",
                                ));
                            }
                            Err(_) => {
                                stats.tables_failed_parse += 1;
                                out.push_str(&render_placeholder_block(
                                    ordinal,
                                    &artifact_rel,
                                    "normalization skipped (parse failed)",
                                ));
                            }
                        }
                    }

                    i = end;
                    continue;
                }
            }
        }

        let ch = chunk[i..]
            .chars()
            .next()
            .ok_or_else(|| anyhow!("invalid markdown boundary"))?;
        out.push(ch);
        i += ch.len_utf8();
    }

    Ok(out)
}

fn render_normalized_table(
    html: &str,
    ordinal: usize,
    artifact_rel: &str,
) -> Result<Option<String>> {
    if html.len() > RAW_HTML_LIMIT {
        return Ok(None);
    }

    let parsed = parse_table_fragment(html)?;
    if parsed.rows.is_empty() {
        return Err(anyhow!("table has no rows"));
    }

    let cell_count = parsed.columns.len() * parsed.rows.len();
    if cell_count > CELL_LIMIT {
        return Ok(None);
    }

    let mut out = String::new();
    write!(&mut out, "\n\n##### OCR Table {ordinal}\n").unwrap();
    write!(&mut out, "Source (OCR HTML): {artifact_rel}\n").unwrap();
    write!(&mut out, "Columns: {}\n\n", parsed.columns.join(", ")).unwrap();

    for (row_index, row) in parsed.rows.iter().enumerate() {
        if row_index > 0 && row_index % ROW_GROUP_SIZE == 0 {
            out.push('\n');
        }
        write!(
            &mut out,
            "Row: {}\n",
            render_row_json(&parsed.columns, row)?
        )
        .unwrap();
    }
    out.push('\n');

    if out.len() > NORMALIZED_CHAR_LIMIT {
        return Ok(None);
    }

    Ok(Some(out))
}

fn render_placeholder_block(ordinal: usize, artifact_rel: &str, reason: &str) -> String {
    format!(
        "\n\n##### OCR Table {ordinal}\nSource (OCR HTML): {artifact_rel}\nStatus: {reason}\n\n"
    )
}

fn render_row_json(columns: &[String], values: &[String]) -> Result<String> {
    let mut out = String::from("{");
    for (index, (key, value)) in columns.iter().zip(values.iter()).enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(key)?);
        out.push(':');
        out.push_str(&serde_json::to_string(value)?);
    }
    out.push('}');
    Ok(out)
}

fn parse_table_fragment(fragment: &str) -> Result<ParsedTable> {
    let rows = parse_rows(fragment)?;
    if rows.is_empty() {
        return Err(anyhow!("table has no rows"));
    }

    let header_rows = rows.iter().take_while(|row| row.has_th).count();
    let expanded = expand_rows(rows)?;
    let width = expanded.width;
    if width == 0 {
        return Err(anyhow!("table has no columns"));
    }

    let columns = if header_rows == 0 {
        (1..=width).map(|index| format!("col_{index}")).collect()
    } else {
        build_columns(&expanded.grid, header_rows)
    };

    let mut data_rows = Vec::new();
    for row in expanded.grid.into_iter().skip(header_rows) {
        data_rows.push(
            row.into_iter()
                .map(|cell| cell.unwrap_or_default())
                .collect::<Vec<_>>(),
        );
    }

    Ok(ParsedTable {
        columns,
        rows: data_rows,
    })
}

fn build_columns(grid: &[Vec<Option<String>>], header_rows: usize) -> Vec<String> {
    let width = grid.first().map(|row| row.len()).unwrap_or_default();
    let mut raw_keys = Vec::with_capacity(width);

    for col in 0..width {
        let mut parts = Vec::new();
        for row in 0..header_rows.min(grid.len()) {
            let value = grid[row][col]
                .as_ref()
                .map(|value| value.trim())
                .unwrap_or("");
            if value.is_empty() {
                continue;
            }
            if parts.last().is_none_or(|last: &String| last != value) {
                parts.push(value.to_string());
            }
        }
        let joined = parts.join(" / ");
        raw_keys.push(normalize_key(&joined, col + 1));
    }

    disambiguate_keys(raw_keys)
}

fn disambiguate_keys(keys: Vec<String>) -> Vec<String> {
    let mut seen = HashMap::<String, usize>::new();
    let mut out = Vec::with_capacity(keys.len());

    for key in keys {
        let count = seen.entry(key.clone()).or_insert(0);
        *count += 1;
        if *count == 1 {
            out.push(key);
        } else {
            out.push(format!("{key}_{count}"));
        }
    }

    out
}

fn normalize_key(value: &str, index: usize) -> String {
    let mut out = String::new();
    let mut last_was_underscore = false;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_underscore = false;
        } else if !out.is_empty() && !last_was_underscore {
            out.push('_');
            last_was_underscore = true;
        }
    }

    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        format!("col_{index}")
    } else {
        trimmed.to_string()
    }
}

fn expand_rows(rows: Vec<ParsedRow>) -> Result<ExpandedTable> {
    let mut grid: Vec<Vec<Option<String>>> = Vec::new();
    let mut occupied: Vec<Vec<bool>> = Vec::new();
    let mut width = 0usize;

    for (row_index, row) in rows.iter().enumerate() {
        ensure_row(&mut grid, &mut occupied, row_index, width);
        let mut col = 0usize;

        for cell in &row.cells {
            while col < width && occupied[row_index][col] {
                col += 1;
            }
            if col >= width {
                width = width.max(col + cell.colspan);
                resize_width(&mut grid, &mut occupied, width);
            }

            let required_width = col + cell.colspan;
            if required_width > width {
                width = required_width;
                resize_width(&mut grid, &mut occupied, width);
            }

            for row_offset in 0..cell.rowspan {
                let target_row = row_index + row_offset;
                ensure_row(&mut grid, &mut occupied, target_row, width);
                for col_offset in 0..cell.colspan {
                    let target_col = col + col_offset;
                    occupied[target_row][target_col] = true;
                    grid[target_row][target_col] = Some(cell.text.clone());
                }
            }

            col += cell.colspan;
        }
    }

    if width == 0 {
        return Err(anyhow!("table has no columns"));
    }

    for row in &mut grid {
        if row.len() < width {
            row.resize(width, None);
        }
    }

    Ok(ExpandedTable { grid, width })
}

fn ensure_row(
    grid: &mut Vec<Vec<Option<String>>>,
    occupied: &mut Vec<Vec<bool>>,
    row_index: usize,
    width: usize,
) {
    while grid.len() <= row_index {
        grid.push(vec![None; width]);
        occupied.push(vec![false; width]);
    }
}

fn resize_width(grid: &mut Vec<Vec<Option<String>>>, occupied: &mut Vec<Vec<bool>>, width: usize) {
    for row in grid.iter_mut() {
        if row.len() < width {
            row.resize(width, None);
        }
    }
    for row in occupied.iter_mut() {
        if row.len() < width {
            row.resize(width, false);
        }
    }
}

fn parse_rows(fragment: &str) -> Result<Vec<ParsedRow>> {
    let mut rows = Vec::new();
    let mut pos = 0usize;

    while let Some(row_start) = find_tag(fragment, pos, "tr", false) {
        let row_open_end = find_tag_end(fragment, row_start)
            .ok_or_else(|| anyhow!("table row start tag was not closed"))?;
        let row_close_start = find_tag(fragment, row_open_end, "tr", true)
            .ok_or_else(|| anyhow!("table row end tag was not found"))?;
        let row_close_end = find_tag_end(fragment, row_close_start)
            .ok_or_else(|| anyhow!("table row end tag was not closed"))?;
        let row_inner = &fragment[row_open_end..row_close_start];
        rows.push(parse_row(row_inner)?);
        pos = row_close_end;
    }

    Ok(rows)
}

fn parse_row(row: &str) -> Result<ParsedRow> {
    let mut cells = Vec::new();
    let mut pos = 0usize;
    let mut has_th = false;

    while pos < row.len() {
        if let Some(cell_start) = find_tag(row, pos, "th", false) {
            let (cell, cell_end) = parse_cell(row, cell_start, "th")?;
            has_th = true;
            cells.push(cell);
            pos = cell_end;
            continue;
        }
        if let Some(cell_start) = find_tag(row, pos, "td", false) {
            let (cell, cell_end) = parse_cell(row, cell_start, "td")?;
            cells.push(cell);
            pos = cell_end;
            continue;
        }

        let ch = row[pos..]
            .chars()
            .next()
            .ok_or_else(|| anyhow!("invalid table row boundary"))?;
        pos += ch.len_utf8();
    }

    Ok(ParsedRow { cells, has_th })
}

fn parse_cell(row: &str, start: usize, name: &str) -> Result<(ParsedCell, usize)> {
    let open_end =
        find_tag_end(row, start).ok_or_else(|| anyhow!("table cell start tag was not closed"))?;
    let close_start = find_tag(row, open_end, name, true)
        .ok_or_else(|| anyhow!("table cell end tag was not found"))?;
    let close_end = find_tag_end(row, close_start)
        .ok_or_else(|| anyhow!("table cell end tag was not closed"))?;
    let tag = &row[start..open_end];
    let inner = &row[open_end..close_start];
    let rowspan = parse_span_attr(tag, "rowspan");
    let colspan = parse_span_attr(tag, "colspan");

    Ok((
        ParsedCell {
            text: html_fragment_to_text(inner),
            rowspan,
            colspan,
        },
        close_end,
    ))
}

fn html_fragment_to_text(fragment: &str) -> String {
    let mut out = String::with_capacity(fragment.len());
    let mut pos = 0usize;

    while pos < fragment.len() {
        if fragment.as_bytes()[pos] == b'<' {
            if let Some(tag_end) = find_tag_end(fragment, pos) {
                let tag = fragment[pos + 1..tag_end - 1].trim();
                let lower = tag.to_ascii_lowercase();
                if lower.starts_with("br") {
                    out.push('\n');
                } else if lower.starts_with("/p")
                    || lower.starts_with("/div")
                    || lower.starts_with("/tr")
                    || lower.starts_with("/td")
                    || lower.starts_with("/th")
                    || lower.starts_with("p")
                    || lower.starts_with("div")
                {
                    out.push('\n');
                }
                pos = tag_end;
                continue;
            }
            break;
        }

        if fragment.as_bytes()[pos] == b'&'
            && let Some((decoded, consumed)) = decode_html_entity(&fragment[pos..])
        {
            out.push_str(&decoded);
            pos += consumed;
            continue;
        }

        let ch = fragment[pos..]
            .chars()
            .next()
            .expect("valid fragment boundary");
        out.push(ch);
        pos += ch.len_utf8();
    }

    out.trim().to_string()
}

fn decode_html_entity(fragment: &str) -> Option<(String, usize)> {
    let end = fragment.find(';')?;
    let entity = &fragment[..=end];
    let decoded = match entity {
        "&amp;" => "&".to_string(),
        "&lt;" => "<".to_string(),
        "&gt;" => ">".to_string(),
        "&quot;" => "\"".to_string(),
        "&#39;" | "&#x27;" => "'".to_string(),
        _ if entity.starts_with("&#x") || entity.starts_with("&#X") => {
            let value = u32::from_str_radix(&entity[3..end], 16).ok()?;
            char::from_u32(value)?.to_string()
        }
        _ if entity.starts_with("&#") => {
            let value = entity[2..end].parse::<u32>().ok()?;
            char::from_u32(value)?.to_string()
        }
        _ => return None,
    };
    Some((decoded, end + 1))
}

fn parse_span_attr(tag: &str, attr: &str) -> usize {
    let pattern = format!(
        r#"(?i)\b{}\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s/>]+))"#,
        regex::escape(attr)
    );
    let Ok(re) = Regex::new(&pattern) else {
        return 1;
    };
    let Some(caps) = re.captures(tag) else {
        return 1;
    };
    let value = caps
        .get(1)
        .or_else(|| caps.get(2))
        .or_else(|| caps.get(3))
        .map(|value| value.as_str())
        .unwrap_or("1");
    value
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(1)
}

fn extract_table_span(text: &str, start: usize) -> TableExtraction {
    let Some(open_end) = find_tag_end(text, start) else {
        return TableExtraction::Failed;
    };
    let mut depth = 1usize;
    let mut nested = false;
    let mut pos = open_end;

    while pos < text.len() {
        if let Some(tag_start) = find_next_table_tag(text, pos) {
            if tag_start > pos {
                pos = tag_start;
            }
            if starts_tag(text, tag_start, "table") {
                let Some(tag_end) = find_tag_end(text, tag_start) else {
                    return TableExtraction::Failed;
                };
                depth += 1;
                nested = true;
                pos = tag_end;
                continue;
            }
            if starts_tag(text, tag_start, "/table") {
                let Some(tag_end) = find_tag_end(text, tag_start) else {
                    return TableExtraction::Failed;
                };
                depth -= 1;
                pos = tag_end;
                if depth == 0 {
                    return TableExtraction::Span {
                        html: text[start..pos].to_string(),
                        end: pos,
                        nested,
                    };
                }
                continue;
            }
        }
        let ch = text[pos..].chars().next().expect("valid table boundary");
        pos += ch.len_utf8();
    }

    TableExtraction::Failed
}

fn find_next_table_tag(text: &str, start: usize) -> Option<usize> {
    let mut pos = start;
    while pos < text.len() {
        if text.as_bytes()[pos] == b'<'
            && (starts_tag(text, pos, "table") || starts_tag(text, pos, "/table"))
        {
            return Some(pos);
        }
        let ch = text[pos..].chars().next()?;
        pos += ch.len_utf8();
    }
    None
}

fn starts_tag(text: &str, start: usize, name: &str) -> bool {
    if text.as_bytes().get(start) != Some(&b'<') {
        return false;
    }
    let Some(prefix) = text.get(start + 1..start + 1 + name.len()) else {
        return false;
    };
    if !prefix.eq_ignore_ascii_case(name) {
        return false;
    }
    match text.as_bytes().get(start + 1 + name.len()) {
        None => true,
        Some(b) if b.is_ascii_whitespace() || *b == b'>' || *b == b'/' => true,
        _ => false,
    }
}

fn find_tag(text: &str, start: usize, name: &str, closing: bool) -> Option<usize> {
    let target = if closing {
        format!("/{name}")
    } else {
        name.to_string()
    };
    let mut pos = start;
    while pos < text.len() {
        if starts_tag(text, pos, &target) {
            return Some(pos);
        }
        let ch = text[pos..].chars().next()?;
        pos += ch.len_utf8();
    }
    None
}

fn find_tag_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut pos = start + 1;

    while pos < bytes.len() {
        match bytes[pos] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'>' if !in_single && !in_double => return Some(pos + 1),
            _ => {}
        }
        pos += 1;
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
    for ch in chars {
        if ch == marker {
            len += 1;
        } else {
            break;
        }
    }

    (len >= 3).then_some((marker, len))
}

fn is_closing_fence_line(line: &str, marker: char, len: usize) -> bool {
    let trimmed = line.trim_start();
    let mut chars = trimmed.chars();
    let mut count = 0usize;

    while matches!(chars.clone().next(), Some(ch) if ch == marker) {
        chars.next();
        count += 1;
    }

    count >= len && chars.all(char::is_whitespace)
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
    let mut pos = start;

    while pos + run_len <= bytes.len() {
        if bytes[pos] == b'`' && bytes[pos..pos + run_len].iter().all(|byte| *byte == b'`') {
            return Some(pos);
        }
        pos += 1;
    }

    None
}

fn count_case_insensitive_occurrences(text: &str, needle: &str) -> usize {
    if needle.is_empty() || text.len() < needle.len() {
        return 0;
    }

    let haystack = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut count = 0usize;
    let mut start = 0usize;

    while let Some(index) = haystack[start..].find(&needle) {
        count += 1;
        start += index + needle.len();
    }

    count
}

fn line_end(text: &str, start: usize) -> usize {
    text[start..]
        .find('\n')
        .map(|offset| start + offset + 1)
        .unwrap_or(text.len())
}

#[derive(Debug)]
struct ParsedCell {
    text: String,
    rowspan: usize,
    colspan: usize,
}

#[derive(Debug)]
struct ParsedRow {
    cells: Vec<ParsedCell>,
    has_th: bool,
}

#[derive(Debug)]
struct ParsedTable {
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
}

#[derive(Debug)]
struct ExpandedTable {
    grid: Vec<Vec<Option<String>>>,
    width: usize,
}

enum TableExtraction {
    Span {
        html: String,
        end: usize,
        nested: bool,
    },
    Failed,
}
