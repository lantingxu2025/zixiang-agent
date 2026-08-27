//! 端到端流程编排
//!
//! 输入姓名 + 照片 → 分析面部特征 + 分析汉字笔画 → 构图规划 → 生成 Prompt → 生成图片
//!
//! 这是整个项目的核心流程，串联所有工具模块。
//!
//! 提供两个入口：
//! - [`run_pipeline_stream`]：流式版本，逐步通过 channel 推送 [`PipelineEvent`]，
//!   供 Web 服务做 SSE 真实进度推送。
//! - [`run_pipeline`]：CLI 入口，内部复用流式版本，drain 事件后组装
//!   [`PipelineResult`] 返回。

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tracing::info;

use crate::config::Config;
use crate::knowledge::character_db::CharacterDb;
use crate::tools::composition_planner::plan_composition;
use crate::tools::face_analysis::analyze_face_with_config;
use crate::tools::image_generator::generate_image_with_config;
use crate::tools::name_analysis::analyze_name_with_config;
use crate::tools::prompt_generator::generate_prompt;
use crate::types::{CompositionPlan, FaceFeatures, NameVisuals};

/// 端到端流程结果（CLI 模式返回）
#[derive(Debug)]
pub struct PipelineResult {
    /// 面部特征分析结果
    pub face: FaceFeatures,
    /// 姓名汉字视觉信息
    pub name_vis: NameVisuals,
    /// 构图计划
    pub plan: CompositionPlan,
    /// 生成的 Prompt
    pub prompt: String,
    /// 生成图片的 URL
    pub image_url: String,
}

/// 流式流程事件
///
/// 由 [`run_pipeline_stream`] 在每个步骤完成时推送到 channel，
/// Web 层据此构造 SSE 事件推送给前端，实现真实进度反馈。
#[derive(Debug, Clone)]
pub enum PipelineEvent {
    /// 某个步骤状态变化：step 1-4，status 为 "active" / "done"
    Step { step: u8, status: &'static str },
    /// Step 1 产出的面部特征（与姓名分析并行完成，谁先完成谁先推送）
    Face(FaceFeatures),
    /// Step 1 产出的姓名汉字视觉信息
    NameVis(NameVisuals),
    /// Step 2 产出的构图计划
    Plan(CompositionPlan),
    /// Step 3 产出的 Prompt
    Prompt(String),
    /// Step 4 产出的图片位置（URL 或本地路径，server 会再规范化）
    Image(String),
    /// 错误（由调用方在 run_pipeline_stream 返回 Err 时推送）
    Error(String),
    /// 全流程完成
    Done,
}

/// 步骤状态辅助
const STEP_ACTIVE: &str = "active";
const STEP_DONE: &str = "done";

/// 运行完整端到端流程的流式版本。
///
/// 每完成一个步骤就向 `tx` 推送一个 [`PipelineEvent`]，便于调用方
/// （Web SSE handler）实时反馈进度。所有步骤完成后推送 [`PipelineEvent::Done`]。
///
/// 任一步骤失败立即返回 `Err`，调用方应据此推送错误信息。
///
/// # 参数
/// - `name`: 被分析者的中文姓名
/// - `image_url`: 人物照片的 URL 或 base64 编码
/// - `style`: 整体画风描述，如 "水墨写意"、"工笔白描"
/// - `config`: API 配置
/// - `tx`: 事件推送通道，调用方持有接收端
pub async fn run_pipeline_stream(
    name: &str,
    image_url: &str,
    style: &str,
    config: &Config,
    tx: mpsc::Sender<PipelineEvent>,
) -> Result<()> {
    info!("=== 字相 Agent 端到端流程启动（流式）===");
    info!("姓名: {}, 照片: {}, 画风: {}", name, image_url, style);

    let send = |ev: PipelineEvent| {
        let tx = tx.clone();
        async move {
            let _ = tx.send(ev).await;
        }
    };

    // ---- Step 1: 并行分析面部特征与汉字笔画 ----
    info!("[1/4] 正在并行分析面部特征与汉字笔画...");
    let _ = send(PipelineEvent::Step {
        step: 1,
        status: STEP_ACTIVE,
    })
    .await;

    let (face_result, name_result) = tokio::join!(
        analyze_face_with_config(image_url, config),
        analyze_name_with_config(name, config),
    );

    let face = face_result.context("面部特征分析失败")?;
    let _ = send(PipelineEvent::Face(face.clone())).await;
    info!("[1/4] 面部分析完成: 脸型={}", face.face_shape);

    let mut name_vis = name_result.context("汉字笔画分析失败")?;
    let _ = send(PipelineEvent::NameVis(name_vis.clone())).await;
    info!("[1/4] 汉字分析完成: {} 个字", name_vis.len());

    let _ = send(PipelineEvent::Step {
        step: 1,
        status: STEP_DONE,
    })
    .await;

    // 知识库查表：为每个汉字的部首补充视觉元素
    let kb = CharacterDb::load();
    for cv in name_vis.iter_mut() {
        cv.visual_elements = kb.lookup(&cv.radical);
    }
    info!("知识库查表完成，为 {} 个汉字补充了视觉元素", name_vis.len());

    // ---- Step 2: 构图规划 ----
    info!("[2/4] 正在进行笔画到面部部位的映射规划...");
    let _ = send(PipelineEvent::Step {
        step: 2,
        status: STEP_ACTIVE,
    })
    .await;

    let mut plan = plan_composition(&face, &name_vis);
    info!("[2/4] 构图规划完成");
    info!("  鼻子: {}", plan.nose);
    info!("  眼睛: {}", plan.eyes);
    info!("  眉毛: {}", plan.eyebrows);
    info!("  嘴巴: {}", plan.mouth);
    info!("  轮廓: {}", plan.face_contour);
    info!("  头发: {}", plan.hair);

    // 注入知识库视觉元素到额外装饰
    let visuals = kb.visual_elements_for(&name_vis);
    if !visuals.is_empty() {
        plan.extra_elements
            .push(format!("视觉元素（部首意象）：{}", visuals.join("、")));
        info!("注入知识库视觉元素: {:?}", visuals);
    }

    let _ = send(PipelineEvent::Plan(plan.clone())).await;
    let _ = send(PipelineEvent::Step {
        step: 2,
        status: STEP_DONE,
    })
    .await;

    // ---- Step 3: 生成 Prompt ----
    info!("[3/4] 正在生成笔画拼凑人脸 Prompt...");
    let _ = send(PipelineEvent::Step {
        step: 3,
        status: STEP_ACTIVE,
    })
    .await;

    let prompt = generate_prompt(&plan, &face, style);
    info!("[3/4] Prompt 生成完成，共 {} 字", prompt.chars().count());

    let _ = send(PipelineEvent::Prompt(prompt.clone())).await;
    let _ = send(PipelineEvent::Step {
        step: 3,
        status: STEP_DONE,
    })
    .await;

    // ---- Step 4: 生成图片 ----
    info!("[4/4] 正在调用图片模型生成图片...");
    let _ = send(PipelineEvent::Step {
        step: 4,
        status: STEP_ACTIVE,
    })
    .await;

    let image_url = generate_image_with_config(&prompt, config)
        .await
        .context("图片生成失败")?;
    info!("[4/4] 图片生成完成: {}", image_url);

    let _ = send(PipelineEvent::Image(image_url.clone())).await;
    let _ = send(PipelineEvent::Step {
        step: 4,
        status: STEP_DONE,
    })
    .await;

    let _ = send(PipelineEvent::Done).await;
    info!("=== 字相 Agent 端到端流程完成 ===");

    Ok(())
}

/// 运行完整的端到端流程（CLI 入口）。
///
/// 内部复用 [`run_pipeline_stream`]：创建一个 channel，在后台 task 中
/// 运行流式版本并 drain 事件，最后组装 [`PipelineResult`] 返回。
/// 这样 CLI 与 Web 共享同一份流程逻辑，避免重复实现。
///
/// # 参数
/// - `name`: 被分析者的中文姓名
/// - `image_url`: 人物照片的 URL 或 base64 编码
/// - `style`: 整体画风描述，如 "水墨写意"、"工笔白描"
/// - `config`: API 配置
pub async fn run_pipeline(
    name: &str,
    image_url: &str,
    style: &str,
    config: &Config,
) -> Result<PipelineResult> {
    let (tx, mut rx) = mpsc::channel::<PipelineEvent>(64);

    let name_owned = name.to_string();
    let image_owned = image_url.to_string();
    let style_owned = style.to_string();
    let config_owned = config.clone();

    let handle = tokio::spawn(async move {
        run_pipeline_stream(
            &name_owned,
            &image_owned,
            &style_owned,
            &config_owned,
            tx,
        )
        .await
    });

    // drain 事件，收集最后的结果
    let mut face: Option<FaceFeatures> = None;
    let mut name_vis: NameVisuals = Vec::new();
    let mut plan: Option<CompositionPlan> = None;
    let mut prompt = String::new();
    let mut image_url = String::new();

    while let Some(ev) = rx.recv().await {
        match ev {
            PipelineEvent::Face(f) => face = Some(f),
            PipelineEvent::NameVis(n) => name_vis = n,
            PipelineEvent::Plan(p) => plan = Some(p),
            PipelineEvent::Prompt(p) => prompt = p,
            PipelineEvent::Image(u) => image_url = u,
            PipelineEvent::Error(e) => return Err(anyhow::anyhow!(e)),
            PipelineEvent::Step { .. } | PipelineEvent::Done => {}
        }
    }

    // 等待后台 task 完成，拿到 Result
    match handle.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e),
        Err(join_err) => return Err(anyhow::anyhow!("后台任务 join 失败: {}", join_err)),
    }

    Ok(PipelineResult {
        face: face.context("流程未产出面部特征")?,
        name_vis,
        plan: plan.context("流程未产出构图计划")?,
        prompt,
        image_url,
    })
}
