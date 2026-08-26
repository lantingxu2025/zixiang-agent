//! 字相 Agent —— 用汉字笔画拼凑人脸
//!
//! 端到端流程：输入姓名 + 照片 URL → 分析 → 规划 → 生成 Prompt → 生成图片
//!
//! 三步工作流：
//! 1. GPT-4o 视觉分析 → 面部特征
//! 2. GPT-4o-mini 文本分析 → 汉字笔画 + 构图规划 + Prompt 生成
//! 3. DALL-E 3 → 生成图片

use std::env;
use std::process::ExitCode;

use tracing::{error, info};
use tracing_subscriber;

use zixiang_agent::agent::agent_loop;
use zixiang_agent::config::Config;

/// 默认画风
const DEFAULT_STYLE: &str = "水墨写意";

fn print_usage(program: &str) {
    eprintln!("字相 Agent —— 用汉字笔画拼凑人脸");
    eprintln!();
    eprintln!("用法:");
    eprintln!("  {program} serve [端口]            启动 Web 服务（默认端口 3000）");
    eprintln!("  {program} <姓名> <照片URL> [画风]   CLI 直接生成");
    eprintln!();
    eprintln!("参数:");
    eprintln!("  姓名      被分析者的中文姓名，如 \"李明\"");
    eprintln!("  照片URL   人物照片的 URL 或 base64 编码");
    eprintln!("  画风      可选，如 \"水墨写意\"、\"工笔白描\"，默认 \"{DEFAULT_STYLE}\"");
    eprintln!();
    eprintln!("环境变量（可通过 .env 配置，详见 .env.example）:");
    eprintln!("  OPENAI_API_KEY   API Key（必填）");
    eprintln!("  API_BASE_URL     API 基础地址（默认 OpenAI 官方）");
    eprintln!("  AUTH_STYLE        认证方式 bearer/header/query（默认 bearer）");
    eprintln!("  IMAGE_RESPONSE_FORMAT  图片响应 url/b64/auto（默认 auto）");
    eprintln!("  VISION_/TEXT_/IMAGE_ 前缀可分端点覆盖以上配置");
    eprintln!();
    eprintln!("示例:");
    eprintln!("  {program} 李明 https://example.com/photo.jpg");
    eprintln!("  {program} 李明 https://example.com/photo.jpg 工笔白描");
}

#[tokio::main]
async fn main() -> ExitCode {
    // 加载 .env（存在则加载，不存在则忽略）
    let _ = dotenv::dotenv();

    // 初始化 tracing 日志
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .init();

    let args: Vec<String> = env::args().collect();
    let program = args
        .first()
        .cloned()
        .unwrap_or_else(|| "zixiang-agent".to_string());

    // ---- serve 模式：启动 Web 服务 ----
    if args.len() >= 2 && args[1] == "serve" {
        let port = args
            .get(2)
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(3000);
        info!("启动 Web 服务模式，端口 {}", port);
        zixiang_agent::server::serve(port).await;
        return ExitCode::SUCCESS;
    }

    // ---- CLI 模式：直接生成 ----
    if args.len() < 3 {
        print_usage(&program);
        return ExitCode::from(2);
    }

    let name = &args[1];
    let image_url = &args[2];
    let style = args.get(3).map(|s| s.as_str()).unwrap_or(DEFAULT_STYLE);

    // 加载配置
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("错误: {}", e);
            eprintln!("请设置环境变量 OPENAI_API_KEY");
            return ExitCode::from(1);
        }
    };

    info!("=== 字相 Agent 启动 ===");
    info!("姓名: {}", name);
    info!("照片: {}", image_url);
    info!("画风: {}", style);

    match agent_loop::run_pipeline(name, image_url, style, &config).await {
        Ok(result) => {
            println!("=== 生成的 Prompt ===");
            println!("{}", result.prompt);
            println!();
            println!("=== 生成的图片 ===");
            println!("{}", result.image_url);
            info!("=== 字相 Agent 完成 ===");
            ExitCode::SUCCESS
        }
        Err(e) => {
            error!("流程失败: {:#}", e);
            eprintln!("错误: {:#}", e);
            ExitCode::from(1)
        }
    }
}
