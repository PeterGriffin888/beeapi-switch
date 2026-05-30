//! Fetch model catalog from a BeeAPI-compatible endpoint.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub owned_by: Option<String>,
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
    #[serde(default)]
    owned_by: Option<String>,
}

/// Normalize a base into an OpenAI-style `/v1` root.
fn openai_root(base: &str) -> String {
    let trimmed = base.trim().trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        trimmed.to_string()
    } else {
        format!("{}/v1", trimmed)
    }
}

pub async fn fetch(base: &str, api_key: &str, use_key_id: Option<&str>) -> Result<Vec<ModelInfo>> {
    if api_key.trim().is_empty() {
        return Err(anyhow!("请先填写 API Key"));
    }
    let url = format!("{}/models", openai_root(base));
    let client = reqwest::Client::builder()
        .user_agent("beeapi-switch/0.1")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("创建 HTTP 客户端失败")?;

    let mut req = client.get(&url).bearer_auth(api_key.trim());
    if let Some(id) = use_key_id {
        req = req.header("x-use-key-id", id);
    }

    let resp = req
        .send()
        .await
        .with_context(|| format!("请求 {} 失败", url))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(200).collect();
        return Err(anyhow!(
            "{} 返回 {}: {}",
            url,
            status,
            if snippet.is_empty() {
                "(空响应)".into()
            } else {
                snippet
            }
        ));
    }

    let parsed: ModelsResponse = resp.json().await.context("解析模型列表 JSON 失败")?;

    let mut list: Vec<ModelInfo> = parsed
        .data
        .into_iter()
        .map(|m| ModelInfo {
            id: m.id,
            owned_by: m.owned_by,
        })
        .collect();
    list.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(list)
}
