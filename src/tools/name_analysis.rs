//! 姓名（汉字）分析工具
//!
//! 调用文本模型分析姓名中每个汉字的笔画序列、结构类型、部件组成、
//! 部首与字形描述，用于后续以汉字笔画拼凑人脸。
//!
//! 支持任意 OpenAI 兼容的第三方服务商（API2D、OpenRouter 等），
//! 通过 [`ApiProvider`](crate::config::ApiProvider) 配置。

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::config::ApiProvider;
use crate::types::CharacterVisual;

/// 默认文本模型（standalone 入口使用）
const DEFAULT_MODEL: &str = "gpt-4o-mini";

// ---------------------------------------------------------------------------
// 请求 / 响应结构体
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<TextMessage>,
    temperature: f64,
    response_format: ResponseFormat,
}

#[derive(Serialize)]
struct TextMessage {
    role: String,
    content: String,
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

/// 判断字符是否为汉字（CJK 统一表意文字及扩展 A / 兼容区）。
fn is_chinese_char(c: char) -> bool {
    matches!(
        c,
        '\u{4E00}'..='\u{9FFF}'     // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}'  // CJK Unified Ideographs Extension A
        | '\u{F900}'..='\u{FAFF}'  // CJK Compatibility Ideographs
    )
}

/// 构建分析单个汉字的提示词。
///
/// 明确告知模型：结果将用于后续以汉字笔画拼凑人脸，因此需要准确的
/// 笔画序列和部件信息。
fn build_prompt(character: &str) -> String {
    format!(
        r#"请分析汉字"{ch}"的字形结构。这些信息将用于后续以汉字笔画拼凑人脸的视觉创作，因此需要准确的笔画序列和部件信息。严格按照以下 JSON 格式返回，不要有任何额外文字：
{{
  "character": "{ch}",
  "strokes": ["...", "..."],
  "stroke_count": 0,
  "structure": "...",
  "components": ["...", "..."],
  "radical": "...",
  "visual_description": "..."
}}

字段说明：
- character: 汉字本身
- strokes: 笔画序列，按书写顺序排列，使用中文笔画名称。常见笔画名称：横、竖、撇、捺、点、提、横折、竖折、横撇、撇折、横折钩、竖钩、竖弯钩、斜钩、弯钩、横折折折钩等
- stroke_count: 总笔画数（整数，应与 strokes 数组长度一致）
- structure: 结构类型，取值之一：左右 / 上下 / 独体 / 半包围 / 全包围
- components: 部件组成（如"李"→["木", "子"]；独体字返回单元素数组，如"人"→["人"]）
- radical: 部首（中文名称，如"木"、"亻"、"口"等）
- visual_description: 字形描述，说明结构特点和各部件的空间位置关系（如"上下结构，上方为'木'，下方为'子'，整体呈纵向排列"）"#,
        ch = character,
    )
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

/// 从 ChatResponse 的 content 中提取并解析 CharacterVisual。
fn parse_character_visual(chat_resp: &ChatResponse, character: &str) -> Result<CharacterVisual> {
    let content_value = chat_resp
        .choices
        .first()
        .ok_or_else(|| anyhow!("分析汉字 {} 时响应中没有 choices 字段", character))?
        .message
        .content
        .clone();

    // content 可能是 JSON 字符串，也可能是对象
    let content_str = match &content_value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(_) => content_value.to_string(),
        _ => {
            error!(
                "分析汉字 {} 时响应 content 格式异常: {}",
                character, content_value
            );
            bail!("分析汉字 {} 时响应 content 格式异常", character);
        }
    };

    let cleaned = clean_json_content(&content_str);

    let mut result = match serde_json::from_str::<CharacterVisual>(&cleaned) {
        Ok(cv) => cv,
        Err(e) => {
            // 解析失败时打印原始响应内容，便于调试
            error!(
                "汉字 {} 返回的 JSON 解析失败: {}，原始内容: {}",
                character, e, content_str
            );
            return Err(e).with_context(|| {
                format!(
                    "解析汉字 {} 返回的 JSON 失败，原始内容: {}",
                    character, content_str
                )
            });
        }
    };

    // 一致性校验：stroke_count 应与 strokes.len() 一致
    if !result.strokes.is_empty() && result.stroke_count as usize != result.strokes.len() {
        warn!(
            "汉字 {} 的 stroke_count ({}) 与 strokes.len() ({}) 不一致，以 strokes 为准",
            result.character, result.stroke_count, result.strokes.len()
        );
        result.stroke_count = result.strokes.len() as u8;
    }

    info!(
        "汉字 {} 分析完成：{} 画，{}结构，部件 {:?}",
        result.character, result.stroke_count, result.structure, result.components
    );

    Ok(result)
}

// ---------------------------------------------------------------------------
// 核心调用逻辑
// ---------------------------------------------------------------------------

/// 使用指定服务商分析单个汉字（内部实现）。
async fn analyze_character_impl(
    character: &str,
    provider: &ApiProvider,
    model: &str,
) -> Result<CharacterVisual> {
    let chars: Vec<char> = character.chars().collect();
    if chars.len() != 1 || !is_chinese_char(chars[0]) {
        bail!("analyze_character 需要传入单个汉字，收到: {}", character);
    }
    let ch = chars[0];

    info!("开始分析汉字: {}（模型: {}，端点: {}）", ch, model, provider.chat_url());

    let body = ChatRequest {
        model: model.to_string(),
        messages: vec![TextMessage {
            role: "user".to_string(),
            content: build_prompt(&ch.to_string()),
        }],
        temperature: 0.1,
        response_format: ResponseFormat {
            format_type: "json_object".to_string(),
        },
    };

    info!("正在向 API 发送分析请求（汉字 {}）...", ch);

    let resp = provider
        .apply_auth(reqwest::Client::new().post(provider.chat_url()))
        .json(&body)
        .send()
        .await
        .context(format!("发送请求分析汉字 {} 失败", ch))?;

    let status = resp.status();
    let raw_text = resp.text().await.context("读取响应体失败")?;

    if !status.is_success() {
        error!(
            "分析汉字 {} 时 API 返回错误状态码 {}，响应: {}",
            ch, status, raw_text
        );
        bail!(
            "分析汉字 {} 时 API 返回错误状态码 {}: {}",
            ch,
            status,
            raw_text
        );
    }

    info!("汉字 {} API 返回成功，正在解析响应", ch);

    let chat_resp: ChatResponse = serde_json::from_str(&raw_text).with_context(|| {
        format!(
            "解析汉字 {} 的响应 JSON 失败，原始响应: {}",
            ch, raw_text
        )
    })?;

    parse_character_visual(&chat_resp, &ch.to_string())
}

/// 使用指定服务商分析姓名中所有汉字（内部实现）。
async fn analyze_name_impl(
    name: &str,
    provider: &ApiProvider,
    model: &str,
) -> Result<Vec<CharacterVisual>> {
    info!("开始分析姓名: {}", name);

    let chars: Vec<char> = name.chars().filter(|c| is_chinese_char(*c)).collect();

    if chars.is_empty() {
        bail!("姓名中没有有效的汉字: {}", name);
    }

    info!("姓名包含 {} 个汉字: {:?}", chars.len(), chars);

    let mut results = Vec::with_capacity(chars.len());
    for c in &chars {
        match analyze_character_impl(&c.to_string(), provider, model).await {
            Ok(cv) => results.push(cv),
            Err(e) => {
                error!("分析汉字 {} 失败: {}", c, e);
                return Err(e.context(format!("分析汉字 {} 失败", c)));
            }
        }
    }

    info!("姓名分析完成，共解析 {} 个汉字", results.len());
    Ok(results)
}

// ---------------------------------------------------------------------------
// 公开 API
// ---------------------------------------------------------------------------

/// 分析单个汉字，提取笔画序列、结构类型、部件组成、部首与字形描述。
///
/// 此入口固定走 OpenAI 官方；第三方服务商请用
/// [`analyze_character_with_provider`]。
///
/// # 参数
/// - `character`: 单个汉字
/// - `api_key`: API Key
pub async fn analyze_character(character: &str, api_key: &str) -> Result<CharacterVisual> {
    let provider = ApiProvider::openai(api_key);
    analyze_character_impl(character, &provider, DEFAULT_MODEL).await
}

/// 使用显式传入的服务商配置分析单个汉字。
pub async fn analyze_character_with_provider(
    character: &str,
    provider: &ApiProvider,
    model: &str,
) -> Result<CharacterVisual> {
    analyze_character_impl(character, provider, model).await
}

/// 分析姓名中所有汉字，返回每个汉字的视觉/字形信息。
///
/// 自动过滤非汉字字符。按顺序逐字分析。此入口固定走 OpenAI 官方；
/// 第三方服务商请用 [`analyze_name_with_provider`]。
///
/// # 参数
/// - `name`: 姓名（如 "李明"）
/// - `api_key`: API Key
pub async fn analyze_name(name: &str, api_key: &str) -> Result<Vec<CharacterVisual>> {
    let provider = ApiProvider::openai(api_key);
    analyze_name_impl(name, &provider, DEFAULT_MODEL).await
}

/// 使用显式传入的服务商配置分析姓名中所有汉字。
pub async fn analyze_name_with_provider(
    name: &str,
    provider: &ApiProvider,
    model: &str,
) -> Result<Vec<CharacterVisual>> {
    analyze_name_impl(name, provider, model).await
}

/// 便捷方法：从 [`Config`](crate::config::Config) 读取文本端点配置并分析姓名。
pub async fn analyze_name_with_config(
    name: &str,
    config: &crate::config::Config,
) -> Result<Vec<CharacterVisual>> {
    analyze_name_with_provider(name, &config.text, &config.text_model).await
}
