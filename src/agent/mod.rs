/// 端到端流程编排模块。
///
/// 文件名为 `loop.rs`，但 `loop` 是 Rust 关键字，
/// 因此通过 `#[path]` 映射为模块名 `agent_loop`。
#[path = "loop.rs"]
pub mod agent_loop;
