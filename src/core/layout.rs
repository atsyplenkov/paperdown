use anyhow::Result;
use serde_json::{Value, json};
use std::collections::HashMap;

use super::assets;

const SCHEMA: &str = "paperdown.layout.v1";

pub(crate) fn render_layout_json(
    layout_details: &[Value],
    figure_replacements: &HashMap<String, String>,
    tables_raw_written: usize,
    source_rel: &str,
) -> Result<String> {
    let table_regions = count_table_regions(layout_details);
    let link_tables = table_regions > 0 && table_regions == tables_raw_written;
    let table_artifact_match = if link_tables { "order" } else { "none" };
    let mut table_index = 0usize;

    let pages: Vec<Value> = layout_details
        .iter()
        .enumerate()
        .map(|(page_index, page_blocks)| {
            let regions = page_blocks
                .as_array()
                .map(|blocks| {
                    blocks
                        .iter()
                        .enumerate()
                        .map(|(block_index, block)| {
                            render_region(
                                block,
                                block_index,
                                figure_replacements,
                                link_tables,
                                &mut table_index,
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            json!({
                "page": page_index + 1,
                "regions": regions,
            })
        })
        .collect();

    let rendered = json!({
        "schema": SCHEMA,
        "source_pdf": source_rel,
        "ocr_model": "glm-ocr",
        "bbox_format": {
            "order": "x1,y1,x2,y2",
            "origin": "top-left",
        },
        "table_artifact_match": table_artifact_match,
        "pages": pages,
    });

    let mut text = serde_json::to_string_pretty(&rendered)?;
    text.push('\n');
    Ok(text)
}

fn count_table_regions(layout_details: &[Value]) -> usize {
    layout_details
        .iter()
        .filter_map(Value::as_array)
        .flatten()
        .filter(|block| label(block) == Some("table"))
        .count()
}

fn render_region(
    block: &Value,
    index: usize,
    figure_replacements: &HashMap<String, String>,
    link_tables: bool,
    table_index: &mut usize,
) -> Value {
    let label = label(block);
    let artifact = match label {
        Some("image") => assets::extract_image_url(block)
            .and_then(|url| figure_replacements.get(&url).cloned())
            .map(Value::String)
            .unwrap_or(Value::Null),
        Some("table") if link_tables => {
            *table_index += 1;
            Value::String(format!("tables/table_{:03}.html", *table_index))
        }
        Some("table") => {
            *table_index += 1;
            Value::Null
        }
        _ => Value::Null,
    };

    json!({
        "index": index,
        "label": label.map(str::to_owned).map(Value::String).unwrap_or(Value::Null),
        "bbox": bbox(block),
        "content": block.get("content").cloned().unwrap_or(Value::Null),
        "artifact": artifact,
    })
}

fn label(block: &Value) -> Option<&str> {
    block.get("label").and_then(Value::as_str)
}

fn bbox(block: &Value) -> Value {
    block
        .get("bbox_2d")
        .or_else(|| block.get("bbox"))
        .cloned()
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::render_layout_json;
    use serde_json::{Value, json};
    use std::collections::HashMap;

    // Render `layout_details` through the real renderer and parse the output back
    // into a `Value` so assertions are structural, not brittle pretty-text checks.
    fn render(
        layout: &[Value],
        replacements: &HashMap<String, String>,
        tables_written: usize,
        source: &str,
    ) -> Value {
        let text =
            render_layout_json(layout, replacements, tables_written, source).expect("renders");
        serde_json::from_str(&text).expect("output is valid JSON")
    }

    #[test]
    fn full_render_links_image_and_table_and_preserves_latex() {
        // One page: title, a LaTeX formula, an image region (remote URL), a table.
        let layout = vec![json!([
            {"label": "title", "bbox_2d": [72, 41, 928, 96], "content": "Paper title"},
            {
                "label": "formula",
                "bbox_2d": [10, 10, 200, 50],
                "content": r"$\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}$"
            },
            {
                "label": "image",
                "bbox_2d": [90, 120, 910, 390],
                "content": "https://example.com/fig.png"
            },
            {
                "label": "table",
                "bbox_2d": [80, 530, 920, 780],
                "content": "<table>x</table>"
            }
        ])];
        // The figure URL resolves through the replacement map to a local file.
        let replacements = HashMap::from([(
            "https://example.com/fig.png".to_string(),
            "figures/fig-001-001.png".to_string(),
        )]);

        // One table region equals the number of tables actually written -> order linking.
        let doc = render(&layout, &replacements, 1, "paper.pdf");

        assert_eq!(doc["schema"], "paperdown.layout.v1");
        assert_eq!(doc["source_pdf"], "paper.pdf");
        assert_eq!(doc["ocr_model"], "glm-ocr");
        assert_eq!(doc["bbox_format"]["order"], "x1,y1,x2,y2");
        assert_eq!(doc["bbox_format"]["origin"], "top-left");
        assert_eq!(doc["table_artifact_match"], "order");

        let regions = doc["pages"][0]["regions"]
            .as_array()
            .expect("regions array");
        assert_eq!(doc["pages"][0]["page"], 1);
        assert_eq!(regions.len(), 4);

        // Title: bbox copied verbatim from bbox_2d.
        assert_eq!(regions[0]["label"], "title");
        assert_eq!(regions[0]["bbox"].clone(), json!([72, 41, 928, 96]));
        assert_eq!(regions[0]["content"], "Paper title");
        assert_eq!(regions[0]["artifact"], Value::Null);

        // Formula: raw LaTeX content is carried through unchanged.
        assert_eq!(regions[1]["label"], "formula");
        assert_eq!(
            regions[1]["content"].as_str(),
            Some(r"$\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}$"),
        );
        assert_eq!(regions[1]["artifact"], Value::Null);

        // Image: artifact resolved via the figure replacement map.
        assert_eq!(regions[2]["label"], "image");
        assert_eq!(regions[2]["artifact"], "figures/fig-001-001.png");

        // Table: first table region -> tables/table_001.html by document order.
        assert_eq!(regions[3]["label"], "table");
        assert_eq!(regions[3]["artifact"], "tables/table_001.html");
    }

    #[test]
    fn table_count_mismatch_skips_table_links() {
        // Two table regions, but three tables written: counts disagree.
        let layout = vec![json!([
            {"label": "table", "bbox_2d": [1, 2, 3, 4], "content": "<table>a</table>"},
            {"label": "table", "bbox_2d": [5, 6, 7, 8], "content": "<table>b</table>"}
        ])];
        let replacements = HashMap::new();

        let doc = render(&layout, &replacements, 3, "paper.pdf");

        assert_eq!(doc["table_artifact_match"], "none");
        let regions = doc["pages"][0]["regions"]
            .as_array()
            .expect("regions array");
        assert_eq!(regions.len(), 2);
        assert!(
            regions.iter().all(|r| r["artifact"].is_null()),
            "every table artifact must be null when region count != tables written"
        );
    }

    #[test]
    fn missing_or_non_string_fields_become_null() {
        // Region 0: non-string label, no bbox keys, no content.
        // Region 1: entirely absent label/bbox/content.
        let layout = vec![json!([{"label": 123}, {"weird": "block"}])];
        let replacements = HashMap::new();

        let doc = render(&layout, &replacements, 0, "paper.pdf");
        let regions = doc["pages"][0]["regions"]
            .as_array()
            .expect("regions array");

        for region in regions {
            assert_eq!(region["label"], Value::Null);
            assert_eq!(region["bbox"], Value::Null);
            assert_eq!(region["content"], Value::Null);
            assert_eq!(region["artifact"], Value::Null);
        }
    }

    #[test]
    fn non_array_page_emits_empty_regions() {
        // A page whose value is not an array must not panic; it yields no regions.
        let layout = vec![json!({"not": "an array"})];
        let replacements = HashMap::new();

        let doc = render(&layout, &replacements, 0, "paper.pdf");

        assert_eq!(doc["pages"][0]["page"], 1);
        assert_eq!(doc["pages"][0]["regions"].clone(), json!([]));
    }
}
