//! 面部分析工具
//!
//! 调用视觉模型分析人物照片的面部特征，并返回结构化的 [`FaceFeatures`]。
//!
//! 支持任意 OpenAI 兼容的第三方服务商（API2D、OpenRouter、米醋 API 等），
//! 通过 [`ApiProvider`](crate::config::ApiProvider) 配置 base url、认证方式等。

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::config::ApiProvider;
use crate::types::FaceFeatures;

/// 默认视觉模型（standalone 入口使用）
const DEFAULT_MODEL: &str = "gpt-4o";

// ---------------------------------------------------------------------------
// 请求 / 响应结构体
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    temperature: f64,
    response_format: ResponseFormat,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: Vec<ContentPart>,
}

/// 多模态消息内容片段，使用内部标签 `type` 区分文本与图片。
#[derive(Serialize)]
#[serde(tag = "type")]
enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

#[derive(Serialize)]
struct ImageUrl {
    url: String,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    format_type: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    /// content 可能是字符串（JSON 文本）或对象
    content: serde_json::Value,
}

// ---------------------------------------------------------------------------
// 工具函数
// ---------------------------------------------------------------------------

/// 规范化图片输入：
/// - `http(s)://` 开头视为 URL，原样返回
/// - `data:` 开头视为已格式化的 data URL，原样返回
/// - 否则视为裸 base64，补上 jpeg 的 data URL 前缀
fn normalize_image_url(image_url: &str) -> String {
    if image_url.starts_with("http://") || image_url.starts_with("https://") {
        image_url.to_string()
    } else if image_url.starts_with("data:") {
        image_url.to_string()
    } else {
        info!("检测到 base64 图片输入，自动添加 data URL 前缀");
        format!("data:image/jpeg;base64,{}", image_url)
    }
}

/// 构建提示词，要求模型以严格 JSON 返回面部特征。
///
/// 明确告知模型：分析结果将用于后续用汉字笔画拼凑人脸，
/// 因此需要准确的面部比例和位置信息。
fn build_prompt() -> String {
    r#"请分析这张照片中人物的面部特征。这些特征将用于后续用汉字笔画拼凑人脸，因此需要你准确判断面部的比例和各五官的相对位置。
严格按照以下 JSON 格式返回，不要有任何额外文字：
{"face_shape": "...", "eye_distance": "...", "eye_position": "...", "nose_length": "...", "nose_position": "...", "mouth_width": "...", "mouth_position": "...", "chin_shape": "...", "forehead_height": "...", "eye_shape": "...", "eyebrow_shape": "...", "nose_shape": "...", "lip_shape": "...", "overall_vibe": ["...", "...", "..."]}

字段取值范围（每项只能从给定选项中选一个最接近的）：
- face_shape: oval / round / long / square / heart（脸型）
- eye_distance: wide / medium / narrow（眼距，两眼之间的水平距离）
- eye_position: high / medium / low（眼睛在脸上的垂直位置）
- nose_length: long / medium / short（鼻子长度）
- nose_position: high / medium / low（鼻子在脸上的位置）
- mouth_width: wide / medium / narrow（嘴巴宽度）
- mouth_position: high / medium / low（嘴巴在脸上的垂直位置）
- chin_shape: pointed / round / square（下巴形状）
- forehead_height: high / medium / low（额头高度）
- eye_shape: narrow / round / almond / deep-set（眼型）
- eyebrow_shape: straight / arched / angled（眉型）
- nose_shape: straight / curved / bulbous（鼻型）
- lip_shape: thin / medium / full（唇型）
- overall_vibe: 3-5 个描述整体气质的关键词数组

请基于照片中人物的真实面部比例作答，比例与位置信息越准确越好。"#.to_string()
}

/// 构建请求体。
fn build_request_body(model: &str, image_url: &str) -> ChatRequest {
    let url = normalize_image_url(image_url);
    ChatRequest {
        model: model.to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![
                ContentPart::Text {
                    text: build_prompt(),
                },
                ContentPart::ImageUrl {
                    image_url: ImageUrl { url },
                },
            ],
        }],
        temperature: 0.2,
        response_format: ResponseFormat {
            format_type: "json_object".to_string(),
        },
    }
}

/// 清理模型返回的 JSON 文本：去除推理标签与可能的 markdown 代码块包裹。
fn clean_json_content(s: &str) -> String {
    let s = match s.find("</think>") {
        Some(end) => &s[end + "</think>".len()..],
        None => s,
    }
    .trim();
    if s.starts_with("```") {
        let s = s
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim();
        let s = s.trim_end_matches("```").trim();
        s.to_string()
    } else {
        s.to_string()
    }
}

/// 从 ChatResponse 的 content 中提取并解析 FaceFeatures。
fn parse_face_features(chat_resp: &ChatResponse, model: &str) -> Result<FaceFeatures> {
    let content_value = chat_resp
        .choices
        .first()
        .ok_or_else(|| anyhow!("{} 响应中没有 choices 字段", model))?
        .message
        .content
        .clone();

    // content 可能是 JSON 字符串，也可能是对象
    let content_str = match &content_value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(_) => content_value.to_string(),
        _ => {
            error!("{} 响应 content 格式异常: {}", model, content_value);
            bail!("{} 响应 content 格式异常", model);
        }
    };

    let cleaned = clean_json_content(&content_str);

    match serde_json::from_str::<FaceFeatures>(&cleaned) {
        Ok(features) => {
            info!("{} 面部特征解析成功: {:?}", model, features);
            Ok(features)
        }
        Err(e) => {
            // 解析失败时打印原始响应内容，便于调试
            error!(
                "{} 返回的 JSON 解析失败: {}，原始内容: {}",
                model, e, content_str
            );
            Err(e).with_context(|| {
                format!(
                    "解析 {} 返回的面部特征 JSON 失败，原始内容: {}",
                    model, content_str
                )
            })
        }
    }
}

// ---------------------------------------------------------------------------
// 核心调用逻辑
// ---------------------------------------------------------------------------

/// 调用指定服务商的视觉模型分析面部特征。
async fn call_model(provider: &ApiProvider, model: &str, image_url: &str) -> Result<FaceFeatures> {
    let body = build_request_body(model, image_url);
    let url = provider.chat_url();

    info!(
        "正在向 API 发送分析请求（模型: {}，端点: {}）...",
        model, url
    );

    let resp = provider
        .apply_auth(reqwest::Client::new().post(&url))
        .json(&body)
        .send()
        .await
        .with_context(|| format!("发送请求失败（模型: {}，端点: {}）", model, url))?;

    let status = resp.status();
    let raw_text = resp.text().await.context("读取响应体失败")?;

    if !status.is_success() {
        error!(
            "{} API 返回错误状态码 {}，响应: {}",
            model, status, raw_text
        );
        bail!("{} API 返回错误状态码 {}: {}", model, status, raw_text);
    }

    info!("{} API 返回成功，正在解析响应", model);

    let chat_resp: ChatResponse = serde_json::from_str(&raw_text)
        .with_context(|| format!("解析 {} 的响应 JSON 失败，原始响应: {}", model, raw_text))?;

    parse_face_features(&chat_resp, model)
}

// ---------------------------------------------------------------------------
// 公开 API
// ---------------------------------------------------------------------------

/// 分析人物照片的面部特征。
///
/// 使用 GPT-4o 视觉模型分析图片，返回结构化的面部特征。
///
/// # 参数
/// - `image_url`: 图片 URL 或 base64 编码字符串（裸 base64 会自动补上 data URL 前缀）
/// - `api_key`: API Key（此入口固定走 OpenAI 官方；第三方服务商请用
///   [`analyze_face_with_provider`] 或 [`analyze_face_with_config`]
///
/// # 返回
/// 解析成功的 [`FaceFeatures`]，包含脸型、五官位置/比例、五官形状与气质关键词。
pub async fn analyze_face(image_url: &str, api_key: &str) -> Result<FaceFeatures> {
    info!("开始分析面部特征，图片: {}", image_url);
    let provider = ApiProvider::openai(api_key);
    call_model(&provider, DEFAULT_MODEL, image_url).await
}

/// 使用显式传入的服务商配置分析面部特征。
///
/// 适用于自定义服务商（米醋 / API2D / OpenRouter 等）。
pub async fn analyze_face_with_provider(
    image_url: &str,
    provider: &ApiProvider,
    model: &str,
) -> Result<FaceFeatures> {
    info!(
        "开始分析面部特征（模型: {}，端点: {}），图片: {}",
        model,
        provider.chat_url(),
        image_url
    );
    call_model(provider, model, image_url).await
}

/// 便捷方法：从 [`Config`](crate::config::Config) 读取视觉端点配置并分析面部特征。
pub async fn analyze_face_with_config(
    image_url: &str,
    config: &crate::config::Config,
) -> Result<FaceFeatures> {
    analyze_face_with_provider(image_url, &config.vision, &config.vision_model).await
}
