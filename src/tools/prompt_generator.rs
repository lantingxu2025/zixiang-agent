//! Prompt 生成器
//!
//! 把 [`CompositionPlan`] 与 [`FaceFeatures`] 组合成一段特殊 Prompt，
//! 指导图片模型用汉字的笔画（横竖撇捺）去"拼凑"出人脸。

use tracing::info;

use crate::types::{CompositionPlan, FaceFeatures};

// ---------------------------------------------------------------------------
// 特征值中文翻译
// ---------------------------------------------------------------------------

/// 将脸型英文值翻译为中文描述。
fn tr_face_shape(s: &str) -> &'static str {
    match s {
        "oval" => "鹅蛋形",
        "round" => "圆润",
        "long" => "修长",
        "square" => "方正",
        "heart" => "心形",
        _ => "椭圆",
    }
}

/// 将五官位置英文值翻译为中文方位描述。
fn tr_position(s: &str) -> &'static str {
    match s {
        "high" => "偏上",
        "low" => "偏下",
        _ => "居中",
    }
}

/// 将眼型英文值翻译为中文。
fn tr_eye_shape(s: &str) -> &'static str {
    match s {
        "narrow" => "细长",
        "round" => "圆润",
        "almond" => "杏形",
        "deep-set" => "深邃",
        _ => "自然",
    }
}

/// 将眉型英文值翻译为中文。
fn tr_eyebrow_shape(s: &str) -> &'static str {
    match s {
        "straight" => "平直",
        "arched" => "弧形",
        "angled" => "棱角",
        _ => "自然",
    }
}

/// 将唇型英文值翻译为中文。
fn tr_lip_shape(s: &str) -> &'static str {
    match s {
        "thin" => "薄唇",
        "medium" => "适中",
        "full" => "丰润",
        _ => "适中",
    }
}

/// 将眼距英文值翻译为中文。
fn tr_eye_distance(s: &str) -> &'static str {
    match s {
        "wide" => "较宽",
        "narrow" => "较近",
        _ => "适中",
    }
}

/// 将鼻型英文值翻译为中文。
fn tr_nose_shape(s: &str) -> &'static str {
    match s {
        "straight" => "挺直",
        "curved" => "微弯",
        "bulbous" => "圆头",
        _ => "自然",
    }
}

/// 将下巴形状英文值翻译为中文。
fn tr_chin_shape(s: &str) -> &'static str {
    match s {
        "pointed" => "尖削",
        "round" => "圆润",
        "square" => "方正",
        _ => "自然",
    }
}

// ---------------------------------------------------------------------------
// 公开 API
// ---------------------------------------------------------------------------

/// 生成"用汉字笔画拼凑人脸"的特殊 Prompt。
///
/// 把构图计划中的笔画映射与面部特征的位置/形状信息填入模板，
/// 指导图片模型用具体的汉字笔画（横、竖、撇、捺等）画人脸各部位。
///
/// # 参数
/// - `plan`: 构图计划（每个面部部位对应的笔画/部件）
/// - `face`: 面部特征（位置、形状等，用于填充模板）
/// - `style`: 整体画风描述，如"水墨写意"、"工笔白描"等
///
/// # 返回
/// 完整的中文 Prompt 字符串。
pub fn generate_prompt(plan: &CompositionPlan, face: &FaceFeatures, style: &str) -> String {
    info!("开始生成笔画拼凑人脸 Prompt，风格: {}", style);

    let face_shape_cn = tr_face_shape(&face.face_shape);
    let eye_pos_cn = tr_position(&face.eye_position);
    let eye_shape_cn = tr_eye_shape(&face.eye_shape);
    let eye_dist_cn = tr_eye_distance(&face.eye_distance);
    let eyebrow_shape_cn = tr_eyebrow_shape(&face.eyebrow_shape);
    let lip_shape_cn = tr_lip_shape(&face.lip_shape);
    let mouth_pos_cn = tr_position(&face.mouth_position);
    let nose_pos_cn = tr_position(&face.nose_position);
    let nose_shape_cn = tr_nose_shape(&face.nose_shape);
    let chin_shape_cn = tr_chin_shape(&face.chin_shape);
    let forehead_cn = tr_position(&face.forehead_height);

    let extra = if plan.extra_elements.is_empty() {
        "无".to_string()
    } else {
        plan.extra_elements.join("、")
    };

    let vibe = if face.overall_vibe.is_empty() {
        "沉稳内敛".to_string()
    } else {
        face.overall_vibe.join("、")
    };

    let prompt = format!(
        r#"一幅用汉字笔画拼凑成的人脸肖像。

面部整体轮廓由以下汉字的笔画构成：
{face_contour}

脸型为{face_shape_cn}，两眼间距{eye_dist_cn}，额头{forehead_cn}。

鼻子用"{nose_stroke}"画成：
- 从{nose_pos_cn}起笔，向下延伸，鼻型{nose_shape_cn}

眼睛用"{eye_stroke}"画成：
- {eye_shape_cn}形状，位于面部{eye_pos_cn}

眉毛用"{eyebrow_stroke}"画成：
- {eyebrow_shape_cn}形状

嘴巴用"{mouth_stroke}"画成：
- {lip_shape_cn}形状，位于面部{mouth_pos_cn}

下巴{chin_shape_cn}，用笔画勾勒出轮廓。

头发用以下笔画重复构成：
{hair}

背景用姓名中提取的部件作装饰：{extra}

整体风格为{style}，人物气质{vibe}。
画面要呈现出"字即是相，相即是字"的意境，既有汉字的结构美，又有人物的神态。
所有面部细节均以汉字笔画为笔触，不得使用普通线条。
请生成一幅艺术肖像。"#,
        face_contour = plan.face_contour,
        face_shape_cn = face_shape_cn,
        eye_dist_cn = eye_dist_cn,
        forehead_cn = forehead_cn,
        nose_stroke = plan.nose,
        nose_pos_cn = nose_pos_cn,
        nose_shape_cn = nose_shape_cn,
        eye_stroke = plan.eyes,
        eye_shape_cn = eye_shape_cn,
        eye_pos_cn = eye_pos_cn,
        eyebrow_stroke = plan.eyebrows,
        eyebrow_shape_cn = eyebrow_shape_cn,
        mouth_stroke = plan.mouth,
        lip_shape_cn = lip_shape_cn,
        mouth_pos_cn = mouth_pos_cn,
        chin_shape_cn = chin_shape_cn,
        hair = plan.hair,
        extra = extra,
        style = style,
        vibe = vibe,
    );

    info!("Prompt 生成完成，长度: {} 字", prompt.chars().count());

    prompt
}

/// 便捷方法：使用默认风格"水墨写意"生成 Prompt。
pub fn generate_prompt_default(plan: &CompositionPlan, face: &FaceFeatures) -> String {
    generate_prompt(plan, face, "水墨写意")
}
