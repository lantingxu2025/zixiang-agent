//! 字相 Agent —— 用汉字笔画拼凑人脸
//!
//! 端到端流程：
//! 1. 输入姓名 + 照片 URL
//! 2. 调用 GLM-4.6V-Flash 分析面部特征 → FaceFeatures
//! 3. 调用 GLM-5.2 分析姓名中每个汉字的笔画/部件 → NameVisuals
//! 4. 根据面部特征将笔画映射到人脸各部位 → CompositionPlan
//! 5. 生成"用汉字笔画拼凑人脸"的艺术 Prompt → 输出

use std::env;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use tracing::{error, info};
use tracing_subscriber;

use zixiang_agent::config::Config;
use zixiang_agent::tools::composition_planner::plan_composition;
use zixiang_agent::tools::face_analysis::analyze_face;
use zixiang_agent::tools::name_analysis::analyze_name;
use zixiang_agent::tools::prompt_generator::generate_prompt;

/// 默认画风
const DEFAULT_STYLE: &str = "水墨写意";

fn print_usage(program: &str) {
    eprintln!("字相 Agent —— 用汉字笔画拼凑人脸");
    eprintln!();
    eprintln!("用法:");
    eprintln!("  {program} <姓名> <照片URL> [画风]");
    eprintln!();
    eprintln!("参数:");
    eprintln!("  姓名      被分析者的中文姓名，如 \"李明\"");
    eprintln!("  照片URL   人物照片的 URL 或 base64 编码");
    eprintln!("  画风      可选，如 \"水墨写意\"、\"工笔白描\"，默认 \"{DEFAULT_STYLE}\"");
    eprintln!();
    eprintln!("环境变量:");
    eprintln!("  GLM_API_KEY   智谱 AI API Key（或 ZHIPU_API_KEY）");
    eprintln!();
    eprintln!("示例:");
    eprintln!("  {program} 李明 https://example.com/photo.jpg");
    eprintln!("  {program} 李明 https://example.com/photo.jpg 工笔白描");
}

#[tokio::main]
async fn main() -> ExitCode {
    // 初始化 tracing 日志
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .init();

    let args: Vec<String> = env::args().collect();
    let program = args.first().cloned().unwrap_or_else(|| "zixiang-agent".to_string());

    // 解析命令行参数
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
            eprintln!("请设置环境变量 GLM_API_KEY 或 ZHIPU_API_KEY");
            return ExitCode::from(1);
        }
    };

    info!("=== 字相 Agent 启动 ===");
    info!("姓名: {}", name);
    info!("照片: {}", image_url);
    info!("画风: {}", style);

    match run(name, image_url, style, &config).await {
        Ok(prompt) => {
            println!("{}", prompt);
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

/// 执行完整的端到端流程。
async fn run(name: &str, image_url: &str, style: &str, config: &Config) -> Result<String> {
    // ---- Step 1: 并行分析面部特征与汉字笔画 ----
    info!("[1/4] 正在并行分析面部特征与汉字笔画...");

    let (face_result, name_result) = tokio::join!(
        analyze_face(image_url, &config.glm_api_key),
        analyze_name(name, &config.glm_api_key),
    );

    let face = face_result.context("面部特征分析失败")?;
    info!("[1/4] 面部特征分析完成: {:?}", face.face_shape);

    let name_vis = name_result.context("汉字笔画分析失败")?;
    info!("[1/4] 汉字笔画分析完成，共 {} 个字", name_vis.len());

    if name_vis.is_empty() {
        bail!("姓名中未提取到有效的汉字信息");
    }

    // ---- Step 2: 构图规划 ----
    info!("[2/4] 正在进行笔画到面部部位的映射规划...");
    let plan = plan_composition(&face, &name_vis);
    info!("[2/4] 构图规划完成");
    info!("  鼻子: {}", plan.nose);
    info!("  眼睛: {}", plan.eyes);
    info!("  眉毛: {}", plan.eyebrows);
    info!("  嘴巴: {}", plan.mouth);
    info!("  轮廓: {}", plan.face_contour);
    info!("  头发: {}", plan.hair);

    // ---- Step 3: 生成 Prompt ----
    info!("[3/4] 正在生成笔画拼凑人脸 Prompt...");
    let prompt = generate_prompt(&plan, &face, style);
    info!("[3/4] Prompt 生成完成，共 {} 字", prompt.chars().count());

    // ---- Step 4: 输出 ----
    info!("[4/4] 完成！");

    Ok(prompt)
}
