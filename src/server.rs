//! axum Web 服务
//!
//! 托管 `static/` 前端页面，提供完整的控制台 API：
//! - `GET  /api/status`   — 检查服务状态、API Key 配置情况与当前模型名
//! - `POST /api/config`    — 在网页中写入 / 更新 API Key（无需手动编辑 .env）
//! - `POST /api/generate` — 以 SSE 流式运行完整 pipeline，逐步推送真实进度

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tracing::{error, info, warn};

use crate::agent::agent_loop::{PipelineEvent, run_pipeline_stream};
use crate::config::Config;

// ---------------------------------------------------------------------------
// 请求结构体
// ---------------------------------------------------------------------------

/// 前端生成请求
#[derive(Debug, Deserialize)]
pub struct GenerateRequest {
    pub name: String,
    /// data URL（FileReader 读取的结果）
    pub image: String,
    pub style: String,
}

/// 配置请求：前端提交 API Key
#[derive(Debug, Deserialize)]
pub struct ConfigRequest {
    pub api_key: String,
    /// 可选：API 基础地址
    pub api_base: Option<String>,
    /// 可选：视觉模型名称
    pub vision_model: Option<String>,
    /// 可选：图片生成专用 API Key
    pub image_api_key: Option<String>,
    /// 可选：图片生成专用 API 基础地址
    pub image_api_base: Option<String>,
    /// 可选：图片生成模型名称
    pub image_model: Option<String>,
}

// ---------------------------------------------------------------------------
// 应用状态
// ---------------------------------------------------------------------------

/// 应用状态：运行时动态持有配置，支持网页端热更新
#[derive(Clone)]
pub struct AppState {
    /// Arc 内部可变，允许 handler 中替换
    config: Arc<std::sync::RwLock<Option<Config>>>,
}

impl AppState {
    fn new() -> Self {
        let config = Config::from_env().ok();
        Self {
            config: Arc::new(std::sync::RwLock::new(config)),
        }
    }

    /// 获取当前配置的快照
    fn get_config(&self) -> Option<Config> {
        self.config.read().unwrap().clone()
    }

    /// 更新配置
    fn set_config(&self, new: Config) {
        *self.config.write().unwrap() = Some(new);
    }

    fn is_configured(&self) -> bool {
        self.config.read().unwrap().is_some()
    }
}

// ---------------------------------------------------------------------------
// 工具函数
// ---------------------------------------------------------------------------

/// 将 pipeline 返回的图片"位置"转换为浏览器可显示的地址。
///
/// - `http(s)://` 开头视为 URL，原样返回
/// - 否则视为本地文件路径，读取后编码为 data URL 返回
fn normalize_for_browser(location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.to_string();
    }
    match std::fs::read(location) {
        Ok(bytes) => {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            format!("data:image/png;base64,{}", b64)
        }
        Err(e) => {
            error!("读取本地图片文件 {} 失败: {}", location, e);
            location.to_string()
        }
    }
}

/// 把 [`PipelineEvent`] 转为 SSE [`Event`]。
///
/// 每个事件用 `event:` 字段区分类型，`data:` 字段携带 JSON 或纯文本。
fn event_from(ev: PipelineEvent) -> Event {
    match ev {
        PipelineEvent::Step { step, status } => Event::default()
            .event("step")
            .data(format!(r#"{{"step":{},"status":"{}"}}"#, step, status)),
        PipelineEvent::Face(f) => Event::default()
            .event("face")
            .data(serde_json::to_string(&f).unwrap_or_else(|_| "{}".to_string())),
        PipelineEvent::NameVis(n) => Event::default()
            .event("name_vis")
            .data(serde_json::to_string(&n).unwrap_or_else(|_| "[]".to_string())),
        PipelineEvent::Plan(p) => Event::default()
            .event("plan")
            .data(serde_json::to_string(&p).unwrap_or_else(|_| "{}".to_string())),
        PipelineEvent::Prompt(s) => Event::default().event("prompt").data(s),
        PipelineEvent::Image(u) => {
            // 图片路径规范化：本地文件转 data URL，远程 URL 原样
            Event::default()
                .event("image")
                .data(normalize_for_browser(&u))
        }
        PipelineEvent::Error(msg) => Event::default().event("error").data(msg),
        PipelineEvent::Done => Event::default().event("done").data(""),
    }
}

/// 定位 static 目录
fn locate_static_dir() -> Option<PathBuf> {
    let cwd = PathBuf::from("static");
    if cwd.is_dir() {
        return Some(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let p = parent.join("static");
            if p.is_dir() {
                return Some(p);
            }
        }
    }
    None
}

/// 将 API Key 写入 .env 文件（保留已有内容，仅更新相关行）
fn write_env_file(
    api_key: &str,
    api_base: Option<&str>,
    vision_model: Option<&str>,
    image_api_key: Option<&str>,
    image_api_base: Option<&str>,
    image_model: Option<&str>,
) -> std::io::Result<()> {
    let env_path = PathBuf::from(".env");
    let existing = std::fs::read_to_string(&env_path).unwrap_or_default();
    let replace_image_key = image_api_key.is_some();
    let replace_image_base = image_api_base.is_some();
    let replace_image_model = image_model.is_some();

    let mut lines: Vec<String> = existing
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with("OPENAI_API_KEY=")
                && !trimmed.starts_with("API_KEY=")
                && !trimmed.starts_with("API_BASE_URL=")
                && !trimmed.starts_with("API_BASE=")
                && !trimmed.starts_with("VISION_MODEL=")
                && !trimmed.starts_with("TEXT_MODEL=")
                && (!replace_image_key || !trimmed.starts_with("IMAGE_API_KEY="))
                && (!replace_image_base
                    || (!trimmed.starts_with("IMAGE_API_BASE_URL=")
                        && !trimmed.starts_with("IMAGE_API_BASE=")))
                && (!replace_image_model || !trimmed.starts_with("IMAGE_MODEL="))
        })
        .map(String::from)
        .collect();

    // 在文件头部写入核心配置
    lines.insert(0, format!("OPENAI_API_KEY={}", api_key));
    if let Some(base) = api_base {
        if !base.is_empty() {
            lines.insert(1, format!("API_BASE_URL={}", base));
        }
    }
    if let Some(model) = vision_model {
        if !model.is_empty() {
            lines.push(format!("VISION_MODEL={}", model));
            lines.push(format!("TEXT_MODEL={}", model));
        }
    }
    if let Some(key) = image_api_key {
        if !key.is_empty() {
            lines.push(format!("IMAGE_API_KEY={}", key));
        }
    }
    if let Some(base) = image_api_base {
        if !base.is_empty() {
            lines.push(format!("IMAGE_API_BASE_URL={}", base));
        }
    }
    if let Some(model) = image_model {
        if !model.is_empty() {
            lines.push(format!("IMAGE_MODEL={}", model));
        }
    }

    std::fs::write(&env_path, lines.join("\n") + "\n")
}

// ---------------------------------------------------------------------------
// API Handlers
// ---------------------------------------------------------------------------

/// `GET /api/status` — 服务状态与配置检查
///
/// 返回是否已配置 API Key，以及当前各端点使用的模型名与 base url，
/// 便于前端在配置卡片中展示。
async fn status_handler(State(state): State<AppState>) -> impl IntoResponse {
    let cfg = state.get_config();
    let (configured, models) = match cfg {
        Some(c) => (
            true,
            serde_json::json!({
                "vision_model": c.vision_model,
                "text_model": c.text_model,
                "image_model": c.image_model,
                "image_size": c.image_size,
                "vision_base_url": c.vision.base_url,
                "image_base_url": c.image.base_url,
            }),
        ),
        None => (false, serde_json::Value::Null),
    };
    Json(serde_json::json!({
        "status": "ok",
        "configured": configured,
        "models": models,
    }))
}

/// `POST /api/config` — 在网页中设置 API Key，热更新配置
async fn config_handler(State(state): State<AppState>, Json(req): Json<ConfigRequest>) -> Response {
    let api_key = req.api_key.trim();
    if api_key.is_empty() {
        return (StatusCode::BAD_REQUEST, "API Key 不能为空").into_response();
    }

    let vision_model = req
        .vision_model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty());
    let image_api_key = req
        .image_api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty());
    let image_api_base = req
        .image_api_base
        .as_deref()
        .map(str::trim)
        .filter(|base| !base.is_empty());
    let image_model = req
        .image_model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty());

    // 1. 写入 .env 文件
    if let Err(e) = write_env_file(
        api_key,
        req.api_base.as_deref(),
        vision_model,
        image_api_key,
        image_api_base,
        image_model,
    ) {
        warn!("写入 .env 失败: {}", e);
        // 文件写入失败不阻止流程，仍可在内存中生效
    } else {
        info!("API Key 已写入 .env");
    }

    // 2. 用新 Key 构造配置
    // 注意：edition 2024 起 set_var 为 unsafe（多线程下修改全局环境有 UB 风险），
    // 这里保持原有行为，用 unsafe 块包裹；运行时仅在配置热更新时调用，影响可控。
    unsafe {
        std::env::set_var("OPENAI_API_KEY", api_key);
        if let Some(base) = req.api_base.as_deref().filter(|base| !base.is_empty()) {
            std::env::set_var("API_BASE_URL", base);
        }
        if let Some(model) = vision_model {
            std::env::set_var("VISION_MODEL", model);
            std::env::set_var("TEXT_MODEL", model);
        }
        if let Some(key) = image_api_key {
            std::env::set_var("IMAGE_API_KEY", key);
        }
        if let Some(base) = image_api_base {
            std::env::set_var("IMAGE_API_BASE_URL", base);
        }
        if let Some(model) = image_model {
            std::env::set_var("IMAGE_MODEL", model);
        }
    }
    let config = Config::from_env();

    match config {
        Ok(c) => {
            state.set_config(c);
            info!("API 配置热更新成功");
            Json(serde_json::json!({
                "success": true,
                "message": "配置已保存并生效",
            }))
            .into_response()
        }
        Err(e) => {
            error!("配置加载失败: {:#}", e);
            (StatusCode::BAD_REQUEST, format!("配置无效: {:#}", e)).into_response()
        }
    }
}

/// `POST /api/generate` — 以 SSE 流式运行完整 pipeline
///
/// 响应 `Content-Type: text/event-stream`，逐步推送以下事件：
/// - `step`   : `{"step":1..4,"status":"active"|"done"}`
/// - `face`    : 面部特征 JSON
/// - `name_vis`: 姓名汉字视觉信息 JSON
/// - `plan`    : 构图计划 JSON
/// - `prompt`  : 生成的 Prompt 文本
/// - `image`   : 图片 URL 或 data URL
/// - `error`   : 错误信息
/// - `done`    : 全流程完成
///
/// 前端用 `fetch` + `ReadableStream` 读取并解析 SSE 帧。
async fn generate_handler(
    State(state): State<AppState>,
    Json(req): Json<GenerateRequest>,
) -> Response {
    let config = match state.get_config() {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "尚未配置 API Key，请先在上方配置区填入密钥",
            )
                .into_response();
        }
    };

    info!("收到生成请求: 姓名={}, 画风={}", req.name, req.style);

    // pipeline 在后台 task 中运行，通过 mpsc channel 推送事件
    let (tx, rx) = mpsc::channel::<PipelineEvent>(64);

    let name = req.name;
    let image = req.image;
    let style = req.style;
    tokio::spawn(async move {
        let result = run_pipeline_stream(&name, &image, &style, &config, tx.clone()).await;
        if let Err(e) = result {
            let _ = tx.send(PipelineEvent::Error(format!("{e:#}"))).await;
        }
        let _ = tx.send(PipelineEvent::Done).await;
    });

    let stream = ReceiverStream::new(rx).map(|ev| Ok::<_, Infallible>(event_from(ev)));

    Sse::new(stream)
        .keep_alive(KeepAlive::new())
        .into_response()
}

// ---------------------------------------------------------------------------
// 启动
// ---------------------------------------------------------------------------

/// 启动 Web 服务（整个项目的唯一入口）。
///
/// 即使用户尚未配置 API Key，服务也会启动——
/// 用户可以在网页中填入 Key，无需编辑 .env 或重启。
pub async fn serve(port: u16) {
    let state = AppState::new();

    if !state.is_configured() {
        warn!("尚未配置 API Key，请在网页中填入密钥");
    } else {
        info!("API 配置加载成功");
    }

    let static_dir = locate_static_dir().unwrap_or_else(|| {
        error!("未找到 static 目录");
        PathBuf::from("static")
    });

    let app = Router::new()
        .route("/api/status", get(status_handler))
        .route("/api/config", post(config_handler))
        .route("/api/generate", post(generate_handler))
        .nest_service("/", ServeDir::new(static_dir))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    info!("字相 Web 服务启动于 http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("绑定端口失败");
    axum::serve(listener, app).await.expect("服务器运行失败");
}
