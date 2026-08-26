//! 端到端流程编排
//!
//! 输入姓名 + 照片 → 分析面部特征 + 分析汉字笔画 → 构图规划 → 生成 Prompt → 生成图片
//!
//! 这是整个项目的核心流程，串联所有工具模块。

use anyhow::{Context, Result};
use tracing::info;

use crate::config::Config;
use crate::knowledge::character_db::CharacterDb;
use crate::tools::composition_planner::plan_composition;
use crate::tools::face_analysis::analyze_face_with_config;
use crate::tools::image_generator::generate_image_with_config;
use crate::tools::name_analysis::analyze_name_with_config;
use crate::tools::prompt_generator::generate_prompt;
use crate::types::{CompositionPlan, FaceFeatures, NameVisuals};

/// 端到端流程结果
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

/// 运行完整的端到端流程。
///
/// 1. 并行分析面部特征（GPT-4o）与汉字笔画（GPT-4o-mini）
/// 2. 构图规划：笔画 → 面部部位映射
/// 3. 生成"用汉字笔画拼凑人脸"的艺术 Prompt
/// 4. 调用 DALL-E 3 生成图片
///
/// # 参数
/// - `name`: 被分析者的中文姓名
/// - `image_url`: 人物照片的 URL 或 base64 编码
/// - `style`: 整体画风描述，如 "水墨写意"、"工笔白描"
/// - `config`: OpenAI 配置（API Key + 模型名称）
///
/// # 返回
/// [`PipelineResult`]，包含全流程的中间结果与最终图片 URL。
pub async fn run_pipeline(
    name: &str,
    image_url: &str,
    style: &str,
    config: &Config,
) -> Result<PipelineResult> {
    info!("=== 字相 Agent 端到端流程启动 ===");
    info!("姓名: {}, 照片: {}, 画风: {}", name, image_url, style);

    // ---- Step 1: 并行分析面部特征与汉字笔画 ----
    info!("[1/4] 正在并行分析面部特征与汉字笔画...");

    let (face_result, name_result) = tokio::join!(
        analyze_face_with_config(image_url, config),
        analyze_name_with_config(name, config),
    );

    let face = face_result.context("面部特征分析失败")?;
    let mut name_vis = name_result.context("汉字笔画分析失败")?;
    info!(
        "[1/4] 分析完成: 脸型={}, 汉字 {} 个",
        face.face_shape,
        name_vis.len()
    );

    // 知识库查表：为每个汉字的部首补充视觉元素
    let kb = CharacterDb::load();
    for cv in name_vis.iter_mut() {
        cv.visual_elements = kb.lookup(&cv.radical);
    }
    info!("知识库查表完成，为 {} 个汉字补充了视觉元素", name_vis.len());

    // ---- Step 2: 构图规划 ----
    info!("[2/4] 正在进行笔画到面部部位的映射规划...");
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

    // ---- Step 3: 生成 Prompt ----
    info!("[3/4] 正在生成笔画拼凑人脸 Prompt...");
    let prompt = generate_prompt(&plan, &face, style);
    info!("[3/4] Prompt 生成完成，共 {} 字", prompt.chars().count());

    // ---- Step 4: 生成图片 ----
    info!("[4/4] 正在调用 DALL-E 3 生成图片...");
    let image_url = generate_image_with_config(&prompt, config)
        .await
        .context("图片生成失败")?;
    info!("[4/4] 图片生成完成: {}", image_url);

    info!("=== 字相 Agent 端到端流程完成 ===");

    Ok(PipelineResult {
        face,
        name_vis,
        plan,
        prompt,
        image_url,
    })
}
