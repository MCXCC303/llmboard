//! llmboard-core:预设框架纯逻辑层(无 WIT / 无 IO 绑定),可宿主单测。
//!
//! 模块:
//! - preset:预设加载/校验、JSON 路径提取、computed 表达式
//! - normalize:响应 JSON → 归一化 BalanceInfo + extras
//! - template:预设模板 → 通用组件树(widgets)
//! - snapshot:快照契约 v2 与稳定签名
//! - state:设置/数据持久化结构
//! - backup:配置备份/恢复
//! - dates:日期工具
//! - usage:用量 CSV 聚合与逐日序列(DeepSeek usage fetcher 的数据层,可单测)

pub mod backup;
pub mod dates;
pub mod normalize;
pub mod preset;
pub mod snapshot;
pub mod state;
pub mod template;
pub mod usage;
