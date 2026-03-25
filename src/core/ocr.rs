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
    call_layout_parsing_at_url(client, api_key, payload, API_URL).await
}

pub(crate) async fn call_layout_parsing_at_url(
    client: &reqwest::Client,
    api_key: &str,
    payload: Value,
    api_url: &str,
) -> Result<Value> {
    let response = client
        .post(api_url)
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&payload)
        .send()
        .await
        .context("Could not reach Z.AI OCR API")?;

    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        if status.as_u16() == 429 {
            return Err(anyhow!(
                "Z.AI OCR rate limit (HTTP 429). Lower --ocr-workers (e.g. 1) or reduce concurrent jobs sharing this API key."
            ));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn call_layout_parsing_429_returns_actionable_error() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("local addr");

            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut read_buf = [0u8; 4096];
                let _ = stream.read(&mut read_buf).await.expect("read request");
                let body = r#"{"error":{"code":"1302","message":"Rate limit reached for requests"}}"#;
                let response = format!(
                    "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("write response");
            });

            let client = reqwest::Client::new();
            let err = call_layout_parsing_at_url(
                &client,
                "test-key",
                json!({"model": "glm-ocr", "file": "data:application/pdf;base64,AA=="}),
                &format!("http://{addr}"),
            )
            .await
            .expect_err("expected 429 error")
            .to_string();

            server.await.expect("server done");
            assert!(err.contains("Z.AI OCR rate limit (HTTP 429)"));
            assert!(err.contains("--ocr-workers"));
        });
    }
}
