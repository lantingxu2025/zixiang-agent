//! axum Web 服务
//!
//! 托管 `static/` 前端页面，并提供 `POST /api/generate` 接口
//! 对接真实的 Agent pipeline。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tracing::{error, info};

use crate::agent::agent_loop::run_pipeline;
use crate::config::Config;
use crate::types::{CompositionPlan, FaceFeatures, NameVisuals};

/// 前端生成请求
#[derive(Debug, Deserialize)]
pub struct GenerateRequest {
    /// 被分析者的中文姓名
    pub name: String,
    /// 人物照片的 data URL（FileReader 读取的结果）
    pub image: String,
    /// 画风描述，如 "水墨"、"工笔"
    pub style: String,
}

/// 后端生成响应
#[derive(Debug, Serialize)]
pub struct GenerateResponse {
    /// 浏览器可直接显示的图片地址（URL 或 data URL）
    pub image_url: String,
    /// 生成的完整 Prompt
    pub prompt: String,
    /// 面部特征分析结果
    pub face: FaceFeatures,
    /// 构图计划
    pub plan: CompositionPlan,
    /// 姓名汉字视觉信息
    pub name_vis: NameVisuals,
}

/// 应用状态
#[derive(Clone)]
pub struct AppState {
    pub config: Option<Arc<Config>>,
}

/// 将 pipeline 返回的图片"位置"（URL 或本地文件路径）转换为浏览器可显示的地址。
///
/// - `http(s)://` → 原样返回
/// - 本地文件路径 → 读取文件并编码为 data URL（米醋等 base64 中转场景）
fn normalize_for_browser(location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.to_string();
    }
    // 本地 PNG 文件 → base64 data URL
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

/// 定位 static 目录（开发时从工作目录找，发布后从可执行文件旁找）。
fn locate_static_dir() -> Option<PathBuf> {
    // 1. 当前工作目录下的 static/
    let cwd = PathBuf::from("static");
    if cwd.is_dir() {
        return Some(cwd);
    }
    // 2. 可执行文件所在目录下的 static/
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

/// 健康检查接口
async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let configured = state.config.is_some();
    Json(serde_json::json!({
        "status": "ok",
        "configured": configured,
    }))
}

/// 生成接口：运行完整 pipeline 并返回结果
async fn generate_handler(
    State(state): State<AppState>,
    Json(req): Json<GenerateRequest>,
) -> Response {
    let config = match &state.config {
        Some(c) => c.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "未配置 API Key，请在 .env 中设置 OPENAI_API_KEY 后重启服务",
            )
                .into_response();
        }
    };

    info!("收到生成请求: 姓名={}, 画风={}", req.name, req.style);

    match run_pipeline(&req.name, &req.image, &req.style, &config).await {
        Ok(result) => {
            let image_url = normalize_for_browser(&result.image_url);
            let resp = GenerateResponse {
                image_url,
                prompt: result.prompt,
                face: result.face,
                plan: result.plan,
                name_vis: result.name_vis,
            };
            Json(resp).into_response()
        }
        Err(e) => {
            error!("生成失败: {:#}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("生成失败: {:#}", e),
            )
                .into_response()
        }
    }
}

/// 启动 Web 服务。
///
/// 即使未配置 API Key，也会启动以托管前端页面（离线演示模式可直接打开
/// `static/index.html`，但通过服务器访问时前端会自动调用真实 API）。
pub async fn serve(port: u16) {
    let config = Config::from_env().ok().map(Arc::new);
    if config.is_none() {
        tracing::warn!("未配置 API Key，Web 服务仍会启动托管前端页面，但 /api/generate 将返回错误");
    } else {
        info!("API 配置加载成功");
    }

    let static_dir = locate_static_dir().unwrap_or_else(|| {
        error!("未找到 static 目录，前端页面将无法托管");
        PathBuf::from("static")
    });

    let state = AppState { config };

    let app = Router::new()
        .route("/api/health", get(health))
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
