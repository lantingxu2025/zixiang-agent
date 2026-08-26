use anyhow::{Context, Result};

/// 应用配置
#[derive(Debug, Clone)]
pub struct Config {
    /// 智谱 AI API Key
    pub glm_api_key: String,
}

impl Config {
    /// 从环境变量加载配置。
    ///
    /// 依次尝试 `GLM_API_KEY` 与 `ZHIPU_API_KEY`。
    pub fn from_env() -> Result<Self> {
        let glm_api_key = std::env::var("GLM_API_KEY")
            .or_else(|_| std::env::var("ZHIPU_API_KEY"))
            .context("未找到智谱 AI API Key，请设置 GLM_API_KEY 或 ZHIPU_API_KEY 环境变量")?;
        Ok(Self { glm_api_key })
    }

    /// 用显式传入的 API Key 构造配置。
    pub fn new(glm_api_key: impl Into<String>) -> Self {
        Self {
            glm_api_key: glm_api_key.into(),
        }
    }
}
