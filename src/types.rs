use serde::{Deserialize, Serialize};

/// 面部特征分析结果
///
/// 包含脸型、五官的相对位置/比例、五官具体形状与整体气质，
/// 用于后续以汉字笔画拼凑人脸，因此强调位置与比例信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceFeatures {
    /// 脸型: oval / round / long / square / heart
    pub face_shape: String,

    // ---- 五官的相对位置和比例 ----
    /// 眼距（两眼之间的距离）: wide / medium / narrow
    pub eye_distance: String,
    /// 眼睛在脸上的垂直位置: high / medium / low
    pub eye_position: String,
    /// 鼻子长度: long / medium / short
    pub nose_length: String,
    /// 鼻子位置: high / medium / low
    pub nose_position: String,
    /// 嘴巴宽度: wide / medium / narrow
    pub mouth_width: String,
    /// 嘴巴位置: high / medium / low
    pub mouth_position: String,
    /// 下巴形状: pointed / round / square
    pub chin_shape: String,
    /// 额头高度: high / medium / low
    pub forehead_height: String,

    // ---- 五官的具体形状 ----
    /// 眼型: narrow / round / almond / deep-set
    pub eye_shape: String,
    /// 眉型: straight / arched / angled
    pub eyebrow_shape: String,
    /// 鼻型: straight / curved / bulbous
    pub nose_shape: String,
    /// 唇型: thin / medium / full
    pub lip_shape: String,

    /// 整体气质关键词（3-5 个）
    #[serde(default)]
    pub overall_vibe: Vec<String>,
}

/// 单个汉字的视觉/字形信息
///
/// 包含笔画序列、结构类型、部件组成、部首与字形描述，
/// 用于后续以汉字笔画拼凑人脸。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterVisual {
    /// 汉字本身
    pub character: String,
    /// 笔画序列（按书写顺序），使用中文笔画名称
    /// 如：["横", "竖", "撇", "捺", "横撇", "竖钩", "横"]
    #[serde(default)]
    pub strokes: Vec<String>,
    /// 总笔画数
    pub stroke_count: u8,
    /// 结构类型: 左右 / 上下 / 独体 / 半包围 / 全包围
    pub structure: String,
    /// 部件组成，如 "李" → ["木", "子"]
    #[serde(default)]
    pub components: Vec<String>,
    /// 部首（中文名称）
    pub radical: String,
    /// 字形描述，说明结构特点和各部件的空间位置关系
    pub visual_description: String,
    /// 部首对应的视觉元素（由知识库 character_db 查表填充）
    #[serde(default)]
    pub visual_elements: Vec<String>,
}

/// 姓名中所有汉字的视觉信息集合
pub type NameVisuals = Vec<CharacterVisual>;

/// 构图计划：把汉字笔画映射到人脸各部位
///
/// 描述用哪些笔画/部件来"拼凑"人脸的每个部位。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositionPlan {
    /// 鼻子用笔
    pub nose: String,
    /// 眼睛用笔（含眼珠）
    pub eyes: String,
    /// 眉毛用笔
    pub eyebrows: String,
    /// 嘴巴用笔
    pub mouth: String,
    /// 脸型轮廓用笔
    pub face_contour: String,
    /// 头发用笔
    pub hair: String,
    /// 额外的装饰元素（部件、部首等）
    #[serde(default)]
    pub extra_elements: Vec<String>,
}
