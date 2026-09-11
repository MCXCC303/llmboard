//! 代码内预设(capabilities)注册表 —— 插件侧"核心注册"的唯一代码文件。
//!
//! 预设套接的供应商能力分两档(详见 docs/EXTENSIONS.md 的 fork 工作流):
//!
//! - **声明式预设**(wasm/presets/*.json):method/url/extract/computed/unavailableWhen
//!   全部在 JSON 里描述。纯 JSON 供应商零代码,文件放进 presets/ 即被加载;
//! - **代码内预设**(本目录):声明式表达不了的数据(zip/CSV 导出、多请求聚合、
//!   私有协议)用 Rust 实现,并**在本文件登记一行**;预设 JSON 的端点用
//!   "fetcher": "<能力名>" 声明调用。
//!
//! 能力契约:入参为 (fetcher 基地址, 凭据),产出与"声明式端点 extract 之后"同形
//! 的 BTreeMap<String, Value>(键即模板可引用的 <端点名>.<key>),引擎把它
//! 落盘到 endpoints.<端点名>。HTTP/zip 等 IO 放这里;纯聚合逻辑放
//! llmboard-core(可宿主单测)。
//!
//! 引擎启动时审计全部预设的 fetcher 引用(见 engine::audit_capability_refs):
//! 未登记的名称在日志中直接报错并列出已注册清单 —— 忘记注册/拼写不一致不会
//! 拖到轮询失败才暴露。

use std::collections::BTreeMap;

use serde_json::Value;

mod deepseek_usage;

/// 代码内预设的执行函数签名。
pub type CapabilityFn =
    fn(base_url: Option<&str>, credential: &str) -> Result<BTreeMap<String, Value>, String>;

/// 一个代码内预设。
pub struct Capability {
    /// 全局唯一能力名;预设 JSON 端点用 "fetcher": "<name>" 引用。
    pub name: &'static str,
    /// 一句话说明(错误提示与日志清单中展示)。
    pub about: &'static str,
    pub run: CapabilityFn,
}

/// ─────────────────────────────────────────────────────────────────────────
/// 核心注册表 —— 新增代码内预设 = 在本表登记一行(名称建议 <预设id>-<动作>)。
/// 登记后即可在 presets/<id>.json 的端点上声明 "fetcher": "<能力名>"。
/// ─────────────────────────────────────────────────────────────────────────
pub static CAPABILITIES: &[Capability] = &[deepseek_usage::USAGE_EXPORT];

/// 已登记清单 [(名称, 说明)],供启动审计与错误提示使用。
pub fn inventory() -> Vec<(&'static str, &'static str)> {
    CAPABILITIES.iter().map(|c| (c.name, c.about)).collect()
}

/// 启动审计:全部预设端点引用的 fetcher 必须已登记。
/// 未登记 → error 级日志(预设 id/端点/能力名 + 已登记清单),并返回问题数。
/// 纯收集逻辑在 llmboard-core(可单测),本函数只做注册表侧注入与日志。
pub fn audit_boot() -> usize {
    use llmboard_core::{preset, state};
    let presets = state::lock().presets.clone();
    let known: Vec<&str> = CAPABILITIES.iter().map(|c| c.name).collect();
    let problems = preset::unknown_fetcher_refs(&presets, &known);
    if problems.is_empty() {
        tracing::info!(
            "[capabilities] 启动审计通过:{} 个预设引用的 fetcher 均已登记({})",
            presets.len(),
            known.join(", ")
        );
        return 0;
    }
    for (pid, ep, name) in &problems {
        tracing::error!(
            "[capabilities] 预设 {pid} 端点 {ep} 引用未登记的 fetcher: {name}(已登记: {};注册见 src/capabilities/mod.rs)",
            known.join(", ")
        );
    }
    problems.len()
}

/// 分发执行;未登记的名称给出含清单的可读错误。
pub fn run(
    name: &str,
    base_url: Option<&str>,
    credential: &str,
) -> Result<BTreeMap<String, Value>, String> {
    match CAPABILITIES.iter().find(|c| c.name == name) {
        Some(c) => (c.run)(base_url, credential),
        None => {
            let list = inventory()
                .iter()
                .map(|(n, a)| format!("{n}({a})"))
                .collect::<Vec<_>>()
                .join("; ");
            Err(format!(
                "未注册的 fetcher: {name}(已注册: {};注册方法见 src/capabilities/mod.rs)",
                if list.is_empty() { "无".into() } else { list }
            ))
        }
    }
}
