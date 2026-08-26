//! 汉字部首 → 视觉元素知识库
//!
//! 在编译期通过 `include_str!` 将 `data/characters.json` 嵌入二进制，
//! 运行时无需读取磁盘文件即可查表。
//!
//! 这保证姓名特征**可控、可复现**——视觉元素来自本地知识库，
//! 而非完全依赖 LLM 的随机发挥。

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use tracing::warn;

use crate::types::NameVisuals;

/// 单个部首的知识库条目
#[derive(Debug, Clone, Deserialize)]
struct CharacterEntry {
    /// 该部首关联的视觉元素（英文关键词，供图片生成 Prompt 使用）
    #[serde(default)]
    visual: Vec<String>,
}

/// 汉字部首 → 视觉元素知识库
#[derive(Debug, Clone, Default)]
pub struct CharacterDb {
    entries: HashMap<String, CharacterEntry>,
}

impl CharacterDb {
    /// 从编译期嵌入的 JSON 加载知识库。
    ///
    /// 解析失败时返回空库（不 panic），以保证流程不中断。
    pub fn load() -> Self {
        const JSON: &str = include_str!("../../data/characters.json");
        match serde_json::from_str::<HashMap<String, CharacterEntry>>(JSON) {
            Ok(entries) => {
                tracing::info!("汉字知识库加载完成，共 {} 个部首条目", entries.len());
                Self { entries }
            }
            Err(e) => {
                warn!("解析 characters.json 失败，知识库将为空: {}", e);
                Self::default()
            }
        }
    }

    /// 查询某个部首对应的视觉元素列表。
    pub fn lookup(&self, radical: &str) -> Vec<String> {
        self.entries
            .get(radical)
            .map(|e| e.visual.clone())
            .unwrap_or_default()
    }

    /// 从姓名所有汉字的部首中收集视觉元素（去重，保留首次出现顺序）。
    pub fn visual_elements_for(&self, name_vis: &NameVisuals) -> Vec<String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut result = Vec::new();
        for cv in name_vis {
            for v in self.lookup(&cv.radical) {
                if seen.insert(v.clone()) {
                    result.push(v);
                }
            }
        }
        result
    }
}
