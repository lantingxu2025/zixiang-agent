//! 构图规划器
//!
//! 把汉字的笔画映射到人脸的不同部位，生成 [`CompositionPlan`]。
//!
//! 根据面部特征（脸型、眼型、眉型、唇型等）从姓名的笔画中
//! 选择合适的类型，决定用哪些笔画/部件来"拼凑"人脸的每个部位。

use std::collections::HashSet;

use tracing::info;

use crate::types::{CompositionPlan, FaceFeatures, NameVisuals};

// ---------------------------------------------------------------------------
// 笔画工具函数
// ---------------------------------------------------------------------------

/// 从姓名的所有汉字中收集全部笔画（按书写顺序）。
fn collect_all_strokes(name_vis: &NameVisuals) -> Vec<String> {
    let mut all = Vec::new();
    for cv in name_vis {
        all.extend(cv.strokes.iter().cloned());
    }
    all
}

/// 检查笔画列表中是否存在精确匹配某名称的笔画。
fn has_stroke(strokes: &[String], name: &str) -> bool {
    strokes.iter().any(|s| s == name)
}

/// 选择笔画：优先精确匹配 `ideal`，否则按 `fallbacks` 顺序尝试，
/// 都找不到则返回 `ideal`（作为理想方案，即使姓名中暂无此笔画）。
fn pick(strokes: &[String], ideal: &str, fallbacks: &[&str]) -> String {
    if has_stroke(strokes, ideal) {
        return ideal.to_string();
    }
    for fb in fallbacks {
        if has_stroke(strokes, fb) {
            return fb.to_string();
        }
    }
    ideal.to_string()
}

/// 统计含某关键字的笔画数量。
fn count_by_keyword(strokes: &[String], keyword: char) -> usize {
    strokes.iter().filter(|s| s.contains(keyword)).count()
}

// ---------------------------------------------------------------------------
// 各部位的规划函数
// ---------------------------------------------------------------------------

/// 规划鼻子用笔。
///
/// 鼻梁是纵向的，基础用"竖"；弯鼻用带折的笔画，圆鼻头在末端加"点"。
fn plan_nose(face: &FaceFeatures, strokes: &[String]) -> String {
    let bridge = pick(strokes, "竖", &["竖钩", "竖折", "撇"]);

    let mut desc = match face.nose_shape.as_str() {
        "curved" => {
            let curved = pick(strokes, "竖折", &["横折", "撇折"]);
            format!("{}（弯曲鼻梁）", curved)
        }
        "bulbous" => {
            let tip = pick(strokes, "点", &["捺"]);
            format!("{} + {}（鼻头）", bridge, tip)
        }
        _ => bridge,
    };

    desc = match face.nose_length.as_str() {
        "long" => format!("{}（长）", desc),
        "short" => format!("{}（短）", desc),
        _ => desc,
    };

    desc
}

/// 规划眼睛用笔。
///
/// 眼珠始终用"点"；眼形决定轮廓笔画：
/// - narrow → 横折（细长）
/// - round → 竖弯钩/点（圆润）
/// - almond → 横折（微弧）
/// - deep-set → 横折 + 点（深邃）
fn plan_eyes(face: &FaceFeatures, strokes: &[String]) -> String {
    let pupil = pick(strokes, "点", &["捺"]);

    let outline = match face.eye_shape.as_str() {
        "narrow" => {
            let s = pick(strokes, "横折", &["横", "横撇"]);
            format!("{}（细长眼形）", s)
        }
        "round" => {
            let s = pick(strokes, "竖弯钩", &["竖钩", "点", "横折"]);
            format!("{}（圆润眼形）", s)
        }
        "almond" => {
            let s = pick(strokes, "横折", &["横撇", "横折钩"]);
            format!("{}（杏眼）", s)
        }
        "deep-set" => {
            let s = pick(strokes, "横折", &["横", "横撇"]);
            format!("{} + {}（深邃）", s, pupil)
        }
        _ => pick(strokes, "横折", &["横"]),
    };

    format!("{} + {}（眼珠）", outline, pupil)
}

/// 规划眉毛用笔。
///
/// - straight → 横
/// - arched → 横折 / 撇
/// - angled → 撇 / 横折钩
fn plan_eyebrows(face: &FaceFeatures, strokes: &[String]) -> String {
    match face.eyebrow_shape.as_str() {
        "straight" => pick(strokes, "横", &["横折", "横折钩"]),
        "arched" => pick(strokes, "横折", &["撇", "横撇", "横折钩"]),
        "angled" => pick(strokes, "撇", &["横折钩", "横折", "横撇"]),
        _ => pick(strokes, "横", &["横折"]),
    }
}

/// 规划嘴巴用笔。
///
/// - thin → 横
/// - medium → 横折
/// - full → 横折 + 点（饱满）
/// 嘴巴宽度附加修饰。
fn plan_mouth(face: &FaceFeatures, strokes: &[String]) -> String {
    let base = match face.lip_shape.as_str() {
        "thin" => pick(strokes, "横", &["横折"]),
        "full" => {
            let main = pick(strokes, "横折", &["横折钩", "横"]);
            let plump = pick(strokes, "点", &["捺"]);
            format!("{} + {}（饱满）", main, plump)
        }
        _ => pick(strokes, "横折", &["横", "横折钩"]),
    };

    match face.mouth_width.as_str() {
        "wide" => format!("{}（宽）", base),
        "narrow" => format!("{}（窄）", base),
        _ => base,
    }
}

/// 规划脸型轮廓用笔。
///
/// 脸型决定整体轮廓的笔画方向：
/// - long → 多纵向笔画（竖、撇、捺）
/// - round → 多横向笔画（横、横折）
/// - square → 棱角（横折钩、竖）
/// - heart → 上宽下尖（撇、捺、横折钩）
/// - oval → 圆润流畅（撇、捺、横折钩）
///
/// 下巴形状与额头高度进一步修饰轮廓。
fn plan_face_contour(face: &FaceFeatures, strokes: &[String]) -> String {
    let mut parts: Vec<String> = match face.face_shape.as_str() {
        "long" => vec![
            pick(strokes, "竖", &["竖钩", "竖折"]),
            pick(strokes, "撇", &["撇折"]),
            pick(strokes, "捺", &["点"]),
        ],
        "round" => vec![
            pick(strokes, "横", &["横折"]),
            pick(strokes, "横折", &["横折钩", "横撇"]),
            pick(strokes, "竖弯钩", &["横折钩", "弯钩"]),
        ],
        "square" => vec![
            pick(strokes, "横折钩", &["横折", "竖钩"]),
            pick(strokes, "竖", &["竖钩", "竖折"]),
        ],
        "heart" => vec![
            pick(strokes, "撇", &["撇折"]),
            pick(strokes, "捺", &["点"]),
            pick(strokes, "横折钩", &["横折", "竖钩"]),
        ],
        _ => vec![
            // oval / 默认
            pick(strokes, "撇", &["撇折"]),
            pick(strokes, "捺", &["点"]),
            pick(strokes, "横折钩", &["横折", "竖钩"]),
        ],
    };

    // 下巴形状影响下半部轮廓
    let chin = match face.chin_shape.as_str() {
        "pointed" => {
            let left = pick(strokes, "撇", &["撇折"]);
            let right = pick(strokes, "捺", &["点"]);
            format!("{} + {}（尖下巴）", left, right)
        }
        "round" => {
            let s = pick(strokes, "竖弯钩", &["弯钩", "横折钩"]);
            format!("{}（圆下巴）", s)
        }
        "square" => {
            let s = pick(strokes, "横折钩", &["横折", "竖钩"]);
            format!("{}（方下巴）", s)
        }
        _ => String::new(),
    };
    if !chin.is_empty() {
        parts.push(chin);
    }

    // 额头高度修饰
    let joined = parts.join("、");
    match face.forehead_height.as_str() {
        "high" => format!("{}（高额头，上部加长）", joined),
        "low" => format!("{}（低额头，上部紧凑）", joined),
        _ => joined,
    }
}

/// 规划头发用笔。
///
/// 头发用撇、捺的重复（飘逸感），根据姓名中实际笔画数量决定重复次数。
fn plan_hair(strokes: &[String]) -> String {
    let pie_count = count_by_keyword(strokes, '撇');
    let na_count = count_by_keyword(strokes, '捺');

    match (pie_count > 0, na_count > 0) {
        (true, true) => format!(
            "撇、捺交替（约{}笔重复）",
            pie_count.min(5) + na_count.min(5)
        ),
        (true, false) => format!("撇×{}（重复排列）", pie_count.min(5)),
        (false, true) => format!("捺×{}（重复排列）", na_count.min(5)),
        (false, false) => {
            let fb = pick(strokes, "撇", &["横撇", "撇折", "点"]);
            format!("{}（重复排列）", fb)
        }
    }
}

/// 收集额外装饰元素：姓名中各汉字的部件与部首（去重）。
fn collect_extra_elements(name_vis: &NameVisuals) -> Vec<String> {
    let mut elements = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for cv in name_vis {
        for comp in &cv.components {
            if !comp.is_empty() && seen.insert(format!("comp:{}", comp)) {
                elements.push(format!("部件「{}」", comp));
            }
        }
        if !cv.radical.is_empty() && seen.insert(format!("rad:{}", cv.radical)) {
            elements.push(format!("部首「{}」", cv.radical));
        }
    }

    elements
}

// ---------------------------------------------------------------------------
// 公开 API
// ---------------------------------------------------------------------------

/// 把汉字笔画映射到人脸各部位，生成构图计划。
///
/// 根据面部特征（脸型、眼型、眉型、唇型等）从姓名的笔画中选择
/// 合适的类型，决定用哪些笔画/部件来"拼凑"人脸的每个部位。
///
/// # 参数
/// - `face`: 面部特征分析结果
/// - `name_vis`: 姓名中所有汉字的视觉信息
///
/// # 返回
/// [`CompositionPlan`]，包含每个面部部位对应的笔画/部件描述。
pub fn plan_composition(face: &FaceFeatures, name_vis: &NameVisuals) -> CompositionPlan {
    info!(
        "开始构图规划：脸型={}, 眼型={}, 眉型={}, 唇型={}",
        face.face_shape, face.eye_shape, face.eyebrow_shape, face.lip_shape
    );

    let strokes = collect_all_strokes(name_vis);
    info!("姓名中共提取 {} 个笔画: {:?}", strokes.len(), strokes);

    if strokes.is_empty() {
        info!("姓名中未提取到笔画，将使用理想笔画方案规划");
    }

    let nose = plan_nose(face, &strokes);
    let eyes = plan_eyes(face, &strokes);
    let eyebrows = plan_eyebrows(face, &strokes);
    let mouth = plan_mouth(face, &strokes);
    let face_contour = plan_face_contour(face, &strokes);
    let hair = plan_hair(&strokes);
    let extra_elements = collect_extra_elements(name_vis);

    let plan = CompositionPlan {
        nose,
        eyes,
        eyebrows,
        mouth,
        face_contour,
        hair,
        extra_elements,
    };

    info!("构图规划完成：{:?}", plan);

    plan
}
