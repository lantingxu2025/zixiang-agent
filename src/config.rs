//! 应用配置
//!
//! 支持通过环境变量（或 `.env` 文件）切换不同的 API 服务商，
//! 兼容 OpenAI、API2D、OpenRouter、米醋 API 等第三方接口。
//!
//! 采用"共享默认 + 分端点覆盖"的分层配置：
//! - 共享变量：`OPENAI_API_KEY` / `API_BASE_URL` / `AUTH_STYLE` ...
//! - 分端点覆盖：`VISION_API_KEY` / `IMAGE_API_BASE_URL` ... 优先级更高
//!
//! 端点分三类：
//! - `VISION_*`：视觉识别（analyze_face，GPT-4o）
//! - `TEXT_*`：文本推理（name_analysis，gpt-4o-mini）
//! - `IMAGE_*`：图片生成（generate_image，DALL-E 3 / 第三方画图中转）

use anyhow::{Context, Result};

/// 认证方式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStyle {
    /// `Authorization: Bearer <key>`（OpenAI / API2D / OpenRouter 默认）
    Bearer,
    /// 自定义请求头，直接放 key（不加 Bearer 前缀）。
    /// 头名由 `auth_header_name` 决定，默认 `Authorization`
    Header,
    /// URL query 参数传递 key。
    /// 参数名由 `auth_query_param` 决定，默认 `api_key`
    Query,
}

impl Default for AuthStyle {
    fn default() -> Self {
        Self::Bearer
    }
}

/// 图片生成响应格式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageResponseFormat {
    /// `data[].url`，返回可直接访问的图片 URL
    Url,
    /// `data[].b64_json`，返回 base64 编码的图片数据（米醋等中转常见）
    B64Json,
    /// 自动：优先取 url，其次 b64_json
    Auto,
}

impl Default for ImageResponseFormat {
    fn default() -> Self {
        Self::Auto
    }
}

/// 单个 API 端点的服务商配置
#[derive(Debug, Clone)]
pub struct ApiProvider {
    /// API 基础地址，应包含版本路径，如 `https://api.openai.com/v1`
    pub base_url: String,
    /// API Key
    pub api_key: String,
    /// 认证方式
    pub auth: AuthStyle,
    /// 自定义请求头名（auth=Header 时生效，默认 `Authorization`）
    pub auth_header_name: String,
    /// query 参数名（auth=Query 时生效，默认 `api_key`）
    pub auth_query_param: String,
    /// 额外请求头（如 OpenRouter 的 `HTTP-Referer` / `X-Title`）
    pub extra_headers: Vec<(String, String)>,
    /// 图片响应格式（仅图片端点有意义）
    pub image_response_format: ImageResponseFormat,
}

impl ApiProvider {
    /// 以 OpenAI 官方默认配置构造（Bearer 认证、官方 base url）。
    pub fn openai(api_key: impl Into<String>) -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: api_key.into(),
            auth: AuthStyle::Bearer,
            auth_header_name: "Authorization".to_string(),
            auth_query_param: "api_key".to_string(),
            extra_headers: Vec::new(),
            image_response_format: ImageResponseFormat::Auto,
        }
    }

    /// 拼接 chat completions 端点地址。
    pub fn chat_url(&self) -> String {
        join_url(&self.base_url, "chat/completions")
    }

    /// 拼接 images generations 端点地址。
    pub fn images_url(&self) -> String {
        join_url(&self.base_url, "images/generations")
    }

    /// 将认证信息与额外请求头应用到 [`reqwest::RequestBuilder`]。
    pub fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut req = match self.auth {
            AuthStyle::Bearer => {
                let value = format!("Bearer {}", self.api_key);
                req.header("Authorization", value.as_str())
            }
            AuthStyle::Header => req.header(self.auth_header_name.as_str(), self.api_key.as_str()),
            AuthStyle::Query => {
                req.query(&[(self.auth_query_param.as_str(), self.api_key.as_str())])
            }
        };
        for (k, v) in &self.extra_headers {
            req = req.header(k.as_str(), v.as_str());
        }
        req
    }
}

/// 拼接 base 与相对路径，去除多余的 `/`。
fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

// ---------------------------------------------------------------------------
// 应用配置
// ---------------------------------------------------------------------------

/// 应用配置
#[derive(Debug, Clone)]
pub struct Config {
    /// 视觉端点服务商配置（analyze_face）
    pub vision: ApiProvider,
    /// 文本端点服务商配置（name_analysis）
    pub text: ApiProvider,
    /// 图片生成端点服务商配置（generate_image）
    pub image: ApiProvider,
    /// 视觉模型名
    pub vision_model: String,
    /// 文本模型名
    pub text_model: String,
    /// 图片生成模型名
    pub image_model: String,
    /// 图片生成尺寸，如 "1024x1024"
    pub image_size: String,
}

impl Config {
    /// 从环境变量（或 `.env`）加载配置。
    ///
    /// 读取 `OPENAI_API_KEY`（或 `API_KEY`），其余变量缺省时回退到 OpenAI 官方默认值。
    pub fn from_env() -> Result<Self> {
        // 容错：即使 main 未调用，也尝试加载 .env
        let _ = dotenv::dotenv();

        // ---- 共享默认值 ----
        let shared_key = std::env::var("OPENAI_API_KEY")
            .ok()
            .or_else(|| std::env::var("API_KEY").ok())
            .filter(|s| !s.is_empty())
            .context("未找到 API Key，请设置 OPENAI_API_KEY 环境变量")?;

        let shared_base = env_opt("API_BASE_URL")
            .or_else(|| env_opt("API_BASE"))
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let shared_auth = parse_auth_style(env_or("AUTH_STYLE", "bearer"));
        let shared_header = env_or("AUTH_HEADER_NAME", "Authorization");
        let shared_query = env_or("AUTH_QUERY_PARAM", "api_key");
        let shared_extra = parse_extra_headers(&env_or("EXTRA_HEADERS", ""));
        let shared_image_fmt = parse_image_format(env_or("IMAGE_RESPONSE_FORMAT", "auto"));

        // ---- 分端点构造 ----
        let vision = build_provider(
            "VISION",
            &shared_key,
            &shared_base,
            shared_auth,
            &shared_header,
            &shared_query,
            &shared_extra,
            shared_image_fmt,
        );
        let text = build_provider(
            "TEXT",
            &shared_key,
            &shared_base,
            shared_auth,
            &shared_header,
            &shared_query,
            &shared_extra,
            shared_image_fmt,
        );
        let image = build_provider(
            "IMAGE",
            &shared_key,
            &shared_base,
            shared_auth,
            &shared_header,
            &shared_query,
            &shared_extra,
            shared_image_fmt,
        );

        let vision_model = env_or("VISION_MODEL", "gpt-4o");
        let text_model = env_or("TEXT_MODEL", "gpt-4o-mini");
        let image_model = env_or("IMAGE_MODEL", "dall-e-3");
        let image_size = env_or("IMAGE_SIZE", "1024x1024");

        Ok(Self {
            vision,
            text,
            image,
            vision_model,
            text_model,
            image_model,
            image_size,
        })
    }

    /// 用显式传入的 API Key 构造配置（全部走 OpenAI 官方默认）。
    pub fn new(openai_api_key: impl Into<String>) -> Self {
        let key = openai_api_key.into();
        Self {
            vision: ApiProvider::openai(key.clone()),
            text: ApiProvider::openai(key.clone()),
            image: ApiProvider::openai(key),
            vision_model: "gpt-4o".to_string(),
            text_model: "gpt-4o-mini".to_string(),
            image_model: "dall-e-3".to_string(),
            image_size: "1024x1024".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// 环境变量解析辅助
// ---------------------------------------------------------------------------

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

fn parse_auth_style(s: String) -> AuthStyle {
    match s.trim().to_ascii_lowercase().as_str() {
        "header" => AuthStyle::Header,
        "query" | "query_param" | "queryparam" => AuthStyle::Query,
        _ => AuthStyle::Bearer,
    }
}

fn parse_image_format(s: String) -> ImageResponseFormat {
    match s.trim().to_ascii_lowercase().as_str() {
        "url" => ImageResponseFormat::Url,
        "b64" | "b64_json" | "base64" => ImageResponseFormat::B64Json,
        _ => ImageResponseFormat::Auto,
    }
}

/// 解析额外请求头：`K1:V1;K2:V2`
fn parse_extra_headers(s: &str) -> Vec<(String, String)> {
    s.split(';')
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() {
                return None;
            }
            let (k, v) = entry.split_once(':')?;
            let k = k.trim();
            let v = v.trim();
            if k.is_empty() {
                return None;
            }
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

/// 按端点前缀构造服务商配置，缺省回退到共享默认值。
#[allow(clippy::too_many_arguments)]
fn build_provider(
    prefix: &str,
    shared_key: &str,
    shared_base: &str,
    shared_auth: AuthStyle,
    shared_header: &str,
    shared_query: &str,
    shared_extra: &[(String, String)],
    shared_image_fmt: ImageResponseFormat,
) -> ApiProvider {
    let base_url = env_opt(&format!("{prefix}_API_BASE_URL"))
        .or_else(|| env_opt(&format!("{prefix}_API_BASE")))
        .unwrap_or_else(|| shared_base.to_string());
    let api_key = env_opt(&format!("{prefix}_API_KEY")).unwrap_or_else(|| shared_key.to_string());
    let auth = env_opt(&format!("{prefix}_AUTH_STYLE"))
        .map(parse_auth_style)
        .unwrap_or(shared_auth);
    let auth_header_name = env_or(&format!("{prefix}_AUTH_HEADER_NAME"), shared_header);
    let auth_query_param = env_or(&format!("{prefix}_AUTH_QUERY_PARAM"), shared_query);
    let extra = env_opt(&format!("{prefix}_EXTRA_HEADERS"))
        .map(|s| parse_extra_headers(&s))
        .unwrap_or_else(|| shared_extra.to_vec());
    let image_response_format = env_opt(&format!("{prefix}_RESPONSE_FORMAT"))
        .map(parse_image_format)
        .unwrap_or(shared_image_fmt);

    ApiProvider {
        base_url,
        api_key,
        auth,
        auth_header_name,
        auth_query_param,
        extra_headers: extra,
        image_response_format,
    }
}
