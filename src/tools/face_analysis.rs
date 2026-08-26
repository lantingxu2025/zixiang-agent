//! 面部分析工具
//!
//! 调用智谱 AI 的视觉模型 GLM-4.6V-Flash 分析人物照片的面部特征，
//! 并返回结构化的 [`FaceFeatures`] 数据。
//!
//! 主模型不可用时会自动回退到 `glm-4v-flash`。

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::types::FaceFeatures;

/// 智谱 AI 视觉对话 API 地址
const API_URL: &str = "https://open.bigmodel.cn/api/paas/v4/chat/completions";
/// 主模型（首选）
const PRIMARY_MODEL: &str = "glm-4.6v-flash";
/// 备用模型（主模型不可用时使用）
const FALLBACK_MODEL: &str = "glm-4v-flash";

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
// 调用错误类型
// ---------------------------------------------------------------------------

/// 调用模型时可能出现的错误分类。
enum CallError {
    /// 模型不可用（应尝试备用模型）
    ModelUnavailable(String),
    /// 其他错误（不应触发 fallback）
    Other(anyhow::Error),
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

/// 清理模型返回的 JSON 文本：去除首尾空白与可能的 markdown 代码块包裹。
fn clean_json_content(s: &str) -> String {
    let s = s.trim();
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

/// 根据 HTTP 状态码与响应体判断模型是否"不可用"。
///
/// 兼容中英文错误信息（"model not found" / "模型不存在" 等）。
fn is_model_unavailable(status: reqwest::StatusCode, body: &str) -> bool {
    if status.as_u16() == 404 {
        return true;
    }
    let body_lower = body.to_lowercase();
    let mentions_model = body_lower.contains("model") || body_lower.contains("模型");
    let indicates_unavailable = body_lower.contains("not found")
        || body_lower.contains("不存在")
        || body_lower.contains("invalid")
        || body_lower.contains("unavailable")
        || body_lower.contains("不支持")
        || body_lower.contains("no such");
    mentions_model && indicates_unavailable
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

/// 调用指定模型分析面部特征。
async fn call_model(
    model: &str,
    image_url: &str,
    api_key: &str,
) -> std::result::Result<FaceFeatures, CallError> {
    let body = build_request_body(model, image_url);

    info!("正在向 {} 发送分析请求...", model);

    let resp = reqwest::Client::new()
        .post(API_URL)
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| CallError::Other(anyhow!("发送请求到 {} 失败: {}", model, e)))?;

    let status = resp.status();
    let raw_text = resp
        .text()
        .await
        .map_err(|e| CallError::Other(anyhow!("读取 {} 响应体失败: {}", model, e)))?;

    if !status.is_success() {
        if is_model_unavailable(status, &raw_text) {
            warn!("{} 模型不可用（HTTP {}）：{}", model, status, raw_text);
            return Err(CallError::ModelUnavailable(format!(
                "{} 不可用（HTTP {}）",
                model, status
            )));
        }
        error!(
            "{} API 返回错误状态码 {}，响应: {}",
            model, status, raw_text
        );
        return Err(CallError::Other(anyhow!(
            "{} API 返回错误状态码 {}: {}",
            model,
            status,
            raw_text
        )));
    }

    info!("{} API 返回成功，正在解析响应", model);

    let chat_resp: ChatResponse = serde_json::from_str(&raw_text).map_err(|e| {
        CallError::Other(anyhow!(
            "解析 {} 的响应 JSON 失败: {}，原始响应: {}",
            model,
            e,
            raw_text
        ))
    })?;

    parse_face_features(&chat_resp, model).map_err(CallError::Other)
}

// ---------------------------------------------------------------------------
// 公开 API
// ---------------------------------------------------------------------------

/// 分析人物照片的面部特征。
///
/// 优先使用 GLM-4.6V-Flash 模型；若该模型不可用，自动回退到 `glm-4v-flash`。
///
/// # 参数
/// - `image_url`: 图片 URL 或 base64 编码字符串（裸 base64 会自动补上 data URL 前缀）
/// - `api_key`: 智谱 AI 的 API Key
///
/// # 返回
/// 解析成功的 [`FaceFeatures`]，包含脸型、眼型、眉型、鼻高、唇厚、发型与气质关键词。
pub async fn analyze_face(image_url: &str, api_key: &str) -> Result<FaceFeatures> {
    info!("开始分析面部特征，图片: {}", image_url);

    // 1. 尝试主模型
    match call_model(PRIMARY_MODEL, image_url, api_key).await {
        Ok(features) => {
            info!("使用主模型 {} 成功分析面部特征", PRIMARY_MODEL);
            return Ok(features);
        }
        Err(CallError::ModelUnavailable(reason)) => {
            warn!(
                "主模型 {} 不可用: {}，尝试回退到 {}",
                PRIMARY_MODEL, reason, FALLBACK_MODEL
            );
        }
        Err(CallError::Other(e)) => {
            error!("主模型 {} 调用失败: {}", PRIMARY_MODEL, e);
            return Err(e.context(format!("主模型 {} 调用失败", PRIMARY_MODEL)));
        }
    }

    // 2. 回退到备用模型
    match call_model(FALLBACK_MODEL, image_url, api_key).await {
        Ok(features) => {
            info!("使用备用模型 {} 成功分析面部特征", FALLBACK_MODEL);
            Ok(features)
        }
        Err(CallError::ModelUnavailable(reason)) => {
            error!("备用模型 {} 也不可用: {}", FALLBACK_MODEL, reason);
            Err(anyhow!(
                "主模型 {} 与备用模型 {} 均不可用（{}）",
                PRIMARY_MODEL,
                FALLBACK_MODEL,
                reason
            ))
        }
        Err(CallError::Other(e)) => {
            error!("备用模型 {} 调用失败: {}", FALLBACK_MODEL, e);
            Err(e.context(format!(
                "主模型 {} 不可用且备用模型 {} 调用失败",
                PRIMARY_MODEL, FALLBACK_MODEL
            )))
        }
    }
}

/// 便捷方法：从 [`Config`](crate::config::Config) 读取 API Key 并分析面部特征。
pub async fn analyze_face_with_config(
    image_url: &str,
    config: &crate::config::Config,
) -> Result<FaceFeatures> {
    analyze_face(image_url, &config.glm_api_key).await
}
