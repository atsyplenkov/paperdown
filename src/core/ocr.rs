use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use std::path::Path;
use tokio::fs;

const API_URL: &str = "https://api.z.ai/api/paas/v4/layout_parsing";

pub(crate) async fn build_payload(pdf_path: &Path) -> Result<Value> {
    let bytes = fs::read(pdf_path).await?;
    let encoded = STANDARD.encode(bytes);
    Ok(json!({
        "model": "glm-ocr",
        "file": format!("data:application/pdf;base64,{encoded}"),
        "return_crop_images": true
    }))
}

pub(crate) async fn call_layout_parsing(
    client: &reqwest::Client,
    api_key: &str,
    payload: Value,
) -> Result<Value> {
    let response = client
        .post(API_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&payload)
        .send()
        .await
        .context("Could not reach Z.AI OCR API")?;

    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(anyhow!(
            "Z.AI OCR request failed with HTTP {}: {}",
            status.as_u16(),
            text
        ));
    }

    let parsed: Value =
        serde_json::from_str(&text).context("Z.AI OCR API returned invalid JSON")?;
    if !parsed.is_object() {
        return Err(anyhow!("Z.AI OCR API returned an unexpected response type"));
    }
    Ok(parsed)
}

pub(crate) fn validate_layout_response(data: Value) -> Result<(String, Vec<Value>, Option<Value>)> {
    let markdown = data
        .get("md_results")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Z.AI OCR response is missing string field 'md_results'"))?
        .to_string();

    let layout_details = data
        .get("layout_details")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Z.AI OCR response is missing list field 'layout_details'"))?
        .clone();

    let usage = data.get("usage").filter(|v| v.is_object()).cloned();
    Ok((markdown, layout_details, usage))
}
