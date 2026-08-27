//! 图片生成工具
//!
//! 调用图片生成模型（DALL-E 3 或第三方画图中转）生成"用汉字笔画拼凑人脸"的艺术图片。
//!
//! 支持任意 OpenAI 兼容的图片生成服务商：
//! - 返回 URL 的服务商（OpenAI、API2D）：直接返回图片 URL
//! - 返回 base64 的服务商（米醋等中转）：自动解码并保存到本地文件，返回文件路径
//!
//! 服务商配置通过 [`ApiProvider`](crate::config::ApiProvider) 控制。

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::config::{ApiProvider, ImageResponseFormat};

/// 默认图片生成模型（米醋 gpt-image2 工具使用的模型 ID）
const DEFAULT_MODEL: &str = "gpt-image-2";

// ---------------------------------------------------------------------------
// 请求 / 响应结构体
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ImageRequest {
    model: String,
    prompt: String,
    n: u8,
    size: String,
    /// 仅在服务商明确指定响应格式时携带；Auto 时省略，交给服务商默认行为
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<String>,
}

#[derive(Deserialize)]
struct ImageResponse {
    data: Vec<ImageData>,
}

#[derive(Deserialize)]
struct ImageData {
    /// 生成图片的 URL
    #[serde(default)]
    url: Option<String>,
    /// base64 编码的图片数据（部分中转服务商以此返回）
    #[serde(default)]
    b64_json: Option<String>,
    /// DALL-E 3 可能修改 Prompt，这里记录修改后的版本
    #[serde(default)]
    revised_prompt: Option<String>,
}

// ---------------------------------------------------------------------------
// 响应解析
// ---------------------------------------------------------------------------

/// 根据服务商配置从响应数据中提取图片"位置"（URL 或本地文件路径）。
///
/// - `Url` / `Auto`：优先取 `url`
/// - `B64Json` / `Auto` 回退：取 `b64_json`，解码后保存为本地 PNG 文件
fn resolve_image(data: &ImageData, fmt: ImageResponseFormat) -> Result<String> {
    match fmt {
        ImageResponseFormat::Url => data
            .url
            .clone()
            .ok_or_else(|| anyhow!("图片生成响应中没有 url 字段")),
        ImageResponseFormat::B64Json => save_b64_image(data),
        ImageResponseFormat::Auto => {
            if let Some(u) = &data.url {
                return Ok(u.clone());
            }
            if data.b64_json.is_some() {
                return save_b64_image(data);
            }
            bail!("图片生成响应中既没有 url 也没有 b64_json 字段")
        }
    }
}

/// 将 base64 图片数据解码并保存到本地 PNG 文件，返回文件路径。
fn save_b64_image(data: &ImageData) -> Result<String> {
    let b64 = data
        .b64_json
        .as_ref()
        .ok_or_else(|| anyhow!("图片生成响应中没有 b64_json 字段"))?;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("解码 base64 图片数据失败")?;

    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = format!("字相_{secs}.png");
    std::fs::write(&path, &bytes).with_context(|| format!("写入图片文件 {} 失败", path))?;
    info!(
        "服务商以 base64 返回图片，已保存到本地文件: {}（{} 字节）",
        path,
        bytes.len()
    );
    Ok(path)
}

// ---------------------------------------------------------------------------
// 核心调用逻辑
// ---------------------------------------------------------------------------

/// 调用指定服务商生成图片，返回图片 URL 或本地文件路径（内部实现）。
async fn generate_image_impl(
    prompt: &str,
    provider: &ApiProvider,
    model: &str,
    size: &str,
) -> Result<String> {
    let body = ImageRequest {
        model: model.to_string(),
        prompt: prompt.to_string(),
        n: 1,
        size: size.to_string(),
        response_format: match provider.image_response_format {
            ImageResponseFormat::Url => Some("url".to_string()),
            ImageResponseFormat::B64Json => Some("b64_json".to_string()),
            ImageResponseFormat::Auto => None,
        },
    };

    let url = provider.images_url();
    info!("正在调用 {} 生成图片（端点: {}）...", model, url);

    let resp = provider
        .apply_auth(reqwest::Client::new().post(&url))
        .json(&body)
        .send()
        .await
        .with_context(|| format!("发送图片生成请求失败（端点: {}）", url))?;

    let status = resp.status();
    let raw_text = resp.text().await.context("读取响应体失败")?;

    if !status.is_success() {
        error!("图片生成 API 返回错误状态码 {}，响应: {}", status, raw_text);
        bail!("图片生成 API 返回错误状态码 {}: {}", status, raw_text);
    }

    let image_resp: ImageResponse = serde_json::from_str(&raw_text)
        .with_context(|| format!("解析图片生成响应 JSON 失败，原始响应: {}", raw_text))?;

    let image_data = image_resp
        .data
        .first()
        .ok_or_else(|| anyhow!("图片生成响应中没有 data 字段"))?;

    if let Some(revised) = &image_data.revised_prompt {
        info!("模型修改了 Prompt: {}", revised);
    }

    let location = resolve_image(image_data, provider.image_response_format)?;
    info!("图片生成成功: {}", location);
    Ok(location)
}

// ---------------------------------------------------------------------------
// 公开 API
// ---------------------------------------------------------------------------

/// 生成图片，返回图片 URL 或本地文件路径。
///
/// 使用 DALL-E 3 模型生成 1024×1024 的图片。
/// 此入口固定走 OpenAI 官方；第三方服务商请用
/// [`generate_image_with_provider`] 或 [`generate_image_with_config`]。
///
/// # 参数
/// - `prompt`: 图片生成 Prompt
/// - `api_key`: API Key
///
/// # 返回
/// 生成图片的 URL（OpenAI / API2D）或本地 PNG 文件路径（base64 中转）。
pub async fn generate_image(prompt: &str, api_key: &str) -> Result<String> {
    let provider = ApiProvider::openai(api_key);
    generate_image_impl(prompt, &provider, DEFAULT_MODEL, "1024x1024").await
}

/// 使用显式传入的服务商配置生成图片。
pub async fn generate_image_with_provider(
    prompt: &str,
    provider: &ApiProvider,
    model: &str,
) -> Result<String> {
    generate_image_impl(prompt, provider, model, "1024x1024").await
}

/// 便捷方法：从 [`Config`](crate::config::Config) 读取图片端点配置并生成图片。
pub async fn generate_image_with_config(
    prompt: &str,
    config: &crate::config::Config,
) -> Result<String> {
    generate_image_impl(
        prompt,
        &config.image,
        &config.image_model,
        &config.image_size,
    )
    .await
}
