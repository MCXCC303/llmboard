//! 预设(Provider Preset)框架 —— "预设套接"核心。
//!
//! 每个供应商用一个 JSON 预设文件描述:
//! - 分类表单(form):插件设置页的凭据字段(如 apiKey + platformToken)
//! - 认证方式(目前支持 Bearer Header)
//! - 余额接口 + 附加端点(endpoints,声明式 JSON REST 或内置 fetcher)
//! - 派生计算(computed 表达式,如 OpenRouter 的 limit - used_cost)
//! - 表盘模板(widgets 列表 + band 显示规则)
//!
//! 插件启动时扫描 presets/*.json,校验并注册;纯 JSON 的声明式供应商零代码接入。
//! 需要代码级逻辑(zip/CSV 导出、多请求聚合等)时按"fork + 代码内预设 + 核心注册"
//! 模式扩展:预设仍以 JSON 表达,自定义抓取实现于插件侧 capabilities/ 模块并在
//! 其注册表登记一行,预设端点用 "fetcher" 引用(流程见 docs/EXTENSIONS.md)。

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::snapshot::BalanceInfo;

pub const PRESET_DIR: &str = "presets";
pub const SCHEMA_VERSION: u32 = 2;

// ============================== 数据结构 ==============================

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSpec {
    #[serde(rename = "type")]
    pub kind: String,
    /// 凭据放置的请求头名(如 Authorization)
    pub header: String,
    /// 值前缀(如 "Bearer ")
    #[serde(default)]
    pub prefix: String,
    /// 插件设置页默认字段标签(apiKey 字段)
    #[serde(default = "default_key_label")]
    pub key_label: String,
    /// 插件设置页默认字段提示
    #[serde(default)]
    pub key_hint: String,
}

fn default_key_label() -> String {
    "API Key".into()
}

/// 分类表单字段:插件设置页按此渲染输入框。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormField {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub hint: String,
    /// 密钥类字段(UI 显示提示,值不落日志)
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormSpec {
    #[serde(default)]
    pub fields: Vec<FormField>,
}

/// 提取路径:单个路径或候选路径数组(取第一个非 null 的命中)。
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PathSpec {
    One(String),
    Many(Vec<String>),
}

impl Default for PathSpec {
    fn default() -> Self {
        PathSpec::One(String::new())
    }
}

impl PathSpec {
    pub fn paths(&self) -> Vec<&str> {
        match self {
            PathSpec::One(p) => vec![p],
            PathSpec::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
    pub fn display(&self) -> String {
        self.paths().join(" | ")
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractSpec {
    /// 可为空:仅提供 default 兜底(如 currency: {default: "USD"})
    #[serde(default)]
    pub path: PathSpec,
    /// 缺失时是否允许(默认 false:缺失即报错)
    #[serde(default)]
    pub optional: bool,
    /// 缺失时的兜底值(JSON 字面量)
    #[serde(default)]
    pub default: Option<Value>,
}

/// 接口"不可用"判定:响应中某字段等于给定值时,视为数据不可用。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnavailableWhen {
    pub path: PathSpec,
    pub equals: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceExtract {
    /// 余额总额(可省略,改由 computed.total 提供)
    #[serde(default)]
    pub total: Option<ExtractSpec>,
    #[serde(default)]
    pub top_up: Option<ExtractSpec>,
    #[serde(default)]
    pub granted: Option<ExtractSpec>,
    #[serde(default)]
    pub currency: Option<ExtractSpec>,
    /// 附加字段(供模板/computed 引用,如 OpenRouter 的 limit / used_cost)
    #[serde(default)]
    pub extras: BTreeMap<String, ExtractSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceSpec {
    /// GET | POST
    pub method: String,
    /// 完整 URL;支持 {key} 占位符替换为主凭据
    pub url: String,
    /// 额外固定请求头
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// POST 请求体(可选)
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    pub extract: BalanceExtract,
    #[serde(default)]
    pub unavailable_when: Option<UnavailableWhen>,
    /// 派生计算:name → 表达式(add/sub/mul/div 嵌套,引用 balance.* / extras.* / <endpoint>.*)
    /// 名称为 total/topUp/granted 时写入余额字段,其余写入 extras
    #[serde(default)]
    pub computed: BTreeMap<String, String>,
}

/// 附加端点:声明式 JSON REST,或引用插件侧登记的代码内预设(见 capabilities 注册表)。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointSpec {
    /// GET | POST;使用 fetcher 时可为空
    #[serde(default)]
    pub method: Option<String>,
    /// 完整 URL,支持 {key} 占位符;使用 fetcher 时是 fetcher 的基地址
    #[serde(default)]
    pub url: Option<String>,
    /// 认证用哪个表单字段的值(默认 apiKey)
    #[serde(default = "default_auth_field")]
    pub auth_field: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// POST 请求体(可选)
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// 内置抓取器名(复杂供应商数据,如 zip/CSV 导出);声明后忽略 method/url 的 REST 语义
    #[serde(default)]
    pub fetcher: Option<String>,
    /// key → 提取路径;computed 引用 <endpoint>.<key>
    #[serde(default)]
    pub extract: BTreeMap<String, ExtractSpec>,
    /// 端点内派生计算(两遍:先全部 extract,再统一 computed)
    #[serde(default)]
    pub computed: BTreeMap<String, String>,
    #[serde(default)]
    pub unavailable_when: Option<UnavailableWhen>,
}

fn default_auth_field() -> String {
    "apiKey".into()
}

fn default_timeout_secs() -> u64 {
    10
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetSpec {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub label: Option<String>,
    /// 值模板,支持 {ref:format} 占位符(如 "{balance.total:money}")
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub hint: Option<String>,
    /// bar/ring 的百分比数值引用(纯 ref,如 "extras.usedPercent")
    #[serde(default)]
    pub percent: Option<String>,
    /// bars/stack 的数值序列引用(纯 ref,解析为数组,如 "usage.costSeries")
    #[serde(default)]
    pub series: Option<String>,
    /// bars 的序列标签引用(纯 ref,解析为字符串数组,如 "usage.dayLabels")
    #[serde(default)]
    pub series_labels: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    /// 值为空/不可解析时跳过该 widget
    #[serde(default)]
    pub hide_empty: bool,
    /// 区块组:同组相邻 widget 之间不画分隔线(如"充值/赠送"同组)
    #[serde(default)]
    pub group: Option<String>,
    /// table 顶部列头(可选;竖表头在行首列)
    #[serde(default)]
    pub columns: Option<Vec<String>>,
    /// table 行数据:每行 name 为竖表头, cells 至多 3 列
    #[serde(default)]
    pub rows: Option<Vec<TableRowSpec>>,
    /// table 动态行数据源:引用端点/extras 里的对象数组(如 "usage.models");与 rows 互斥
    #[serde(default)]
    pub from: Option<String>,
    /// 动态行行名模板(默认 "{item.name}";占位符仅支持 item.<路径>)
    #[serde(default)]
    pub row_name: Option<String>,
    /// 动态行单元格模板(1-3 个,{item.<路径>:格式})
    #[serde(default)]
    pub row_cells: Option<Vec<String>>,
    /// 分段/占比类组件的逐项颜色(如 ringseg/ringshare 的段色;缺省按 accent 阶梯)
    #[serde(default)]
    pub colors: Option<Vec<String>>,
}

/// 预设表格行:name 为竖表头单元格。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRowSpec {
    pub name: String,
    #[serde(default)]
    pub cells: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateSpec {
    #[serde(default)]
    pub widgets: Vec<WidgetSpec>,
    /// 手环端显示规则(原样透传到快照 ProviderView.display)
    #[serde(default)]
    pub band: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default = "default_accent")]
    pub accent: String,
    pub auth: AuthSpec,
    /// 分类表单字段(插件设置页);为空时默认一个 apiKey 字段
    #[serde(default)]
    pub form: FormSpec,
    pub balance: BalanceSpec,
    /// 附加端点(如 usage),模板里以 <endpoint名>.<key> 引用
    #[serde(default)]
    pub endpoints: BTreeMap<String, EndpointSpec>,
    #[serde(default)]
    pub template: TemplateSpec,
}

fn default_accent() -> String {
    "#4D6BFE".into()
}

// ============================== 加载与校验 ==============================

/// 扫描插件目录下的 presets/,逐个加载校验。
/// 无效文件只告警不中断(框架对坏预设容错),便于开发者调试。
pub fn load_all() -> Vec<Preset> {
    let mut out: Vec<Preset> = Vec::new();
    let entries = match std::fs::read_dir(PRESET_DIR) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("[preset] 无法读取目录 {PRESET_DIR}: {e}");
            return out;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        match load_one(&path) {
            Ok(preset) => out.push(preset),
            Err(e) => tracing::warn!("[preset] 跳过无效预设 {}: {e}", path.display()),
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|p| {
        if !seen.insert(p.id.clone()) {
            tracing::warn!("[preset] 重复预设 id {},保留第一个", p.id);
            false
        } else {
            true
        }
    });
    out
}

pub fn load_one(path: &Path) -> Result<Preset, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读取失败: {e}"))?;
    let preset: Preset =
        serde_json::from_str(&text).map_err(|e| format!("JSON 解析失败: {e}"))?;
    validate(&preset)?;
    Ok(preset)
}

/// 接线审计(纯收集,可单测):返回预设中所有**未被宿主能力注册表登记**的
/// fetcher 引用 —— (预设 id, 端点名, fetcher 名),按预设/端点顺序稳定输出。
/// known 由插件侧 capabilities 注册表注入(见 wasm/src/capabilities/mod.rs),
/// 宿主启动时据此告警,注册遗漏不会拖到轮询失败才暴露。
pub fn unknown_fetcher_refs(presets: &[Preset], known: &[&str]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for p in presets {
        for (name, ep) in &p.endpoints {
            if let Some(f) = &ep.fetcher {
                if !known.contains(&f.as_str()) {
                    out.push((p.id.clone(), name.clone(), f.clone()));
                }
            }
        }
    }
    out
}

/// 引用命名空间:balance(字段)/ extras / 各端点名 / meta.name
fn known_namespaces(p: &Preset) -> Vec<String> {
    let mut v = vec!["balance".to_string(), "extras".to_string(), "meta".to_string()];
    for name in p.endpoints.keys() {
        v.push(name.clone());
    }
    v
}

fn check_ref_namespace(r: &str, p: &Preset) -> Result<(), String> {
    if r == "meta.name" {
        return Ok(());
    }
    let Some((ns, path)) = r.split_once('.') else {
        return Err(format!("引用缺少命名空间: {r}"));
    };
    if path.is_empty() {
        return Err(format!("引用缺少路径: {r}"));
    }
    let ok = match ns {
        "balance" => ["total", "topUp", "granted", "currency"].contains(&path),
        "extras" => true,
        other => p.endpoints.contains_key(other),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("引用命名空间未知: {r}(可用: {})", known_namespaces(p).join("/")))
    }
}

/// 扫描模板字符串里的 {ref:format} 占位符并校验命名空间。
fn check_template_refs(tpl: &str, p: &Preset) -> Result<(), String> {
    let mut rest = tpl;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else { break };
        let token = &after[..end];
        let name = token.split_once(':').map(|(n, _)| n).unwrap_or(token).trim();
        if !name.is_empty() {
            check_ref_namespace(name, p)?;
        }
        rest = &after[end + 1..];
    }
    Ok(())
}

/// 校验动态表格模板里的 {item.<路径>[:格式]} 占位符(仅允许 item.* 命名空间)。
fn check_scoped_template_refs(tpl: &str) -> Result<(), String> {
    let mut rest = tpl;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else { break };
        let token = &after[..end];
        let name = token.split_once(':').map(|(n, _)| n).unwrap_or(token).trim();
        if !name.is_empty() {
            let Some(path) = name.strip_prefix("item.") else {
                return Err(format!("占位符仅支持 item.<路径>: {name}"));
            };
            if path.is_empty() || tokenize(path).is_none() {
                return Err(format!("路径语法非法: {name}"));
            }
        }
        rest = &after[end + 1..];
    }
    Ok(())
}

/// 预设规范校验(字段级,给出可读错误)。
pub fn validate(p: &Preset) -> Result<(), String> {
    if p.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "schemaVersion 不支持: {} (当前 {SCHEMA_VERSION})",
            p.schema_version
        ));
    }
    if p.id.is_empty()
        || !p
            .id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(format!("id 非法: {:?}(仅允许小写字母/数字/-/_)", p.id));
    }
    if p.name.trim().is_empty() {
        return Err("name 不能为空".into());
    }
    if p.auth.kind != "bearer" {
        return Err(format!(
            "auth.type 不支持: {:?}(当前仅支持 bearer)",
            p.auth.kind
        ));
    }
    if p.auth.header.trim().is_empty() {
        return Err("auth.header 不能为空".into());
    }
    // 表单字段:id 唯一且非空,不含 UI 事件 id 分隔符 "--"
    let mut seen_fields = std::collections::BTreeSet::new();
    for f in &p.form.fields {
        if f.id.trim().is_empty() {
            return Err("form 字段 id 不能为空".into());
        }
        if f.id.contains("--") {
            return Err(format!("form 字段 id 不允许包含 --: {}", f.id));
        }
        if !seen_fields.insert(f.id.clone()) {
            return Err(format!("form 字段 id 重复: {}", f.id));
        }
    }
    // 端点名的命名空间约束
    for name in p.endpoints.keys() {
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') || name.is_empty() {
            return Err(format!("端点名非法: {name}(仅允许字母/数字/-/_)"));
        }
        if name == "balance" || name == "extras" || name == "meta" {
            return Err(format!("端点名与内置命名空间冲突: {name}"));
        }
    }
    // 余额接口
    let m = p.balance.method.to_ascii_uppercase();
    if m != "GET" && m != "POST" {
        return Err(format!(
            "balance.method 不支持: {:?}(仅 GET/POST)",
            p.balance.method
        ));
    }
    if !(p.balance.url.starts_with("https://") || p.balance.url.starts_with("http://")) {
        return Err(format!(
            "balance.url 必须以 http(s):// 开头: {}",
            p.balance.url
        ));
    }
    let check_paths = |spec: Option<&ExtractSpec>, what: &str| -> Result<(), String> {
        if let Some(s) = spec {
            for path in s.path.paths() {
                if path.is_empty() {
                    continue; // 空路径合法:仅 default 兜底
                }
                tokenize(path).ok_or_else(|| format!("{what} 路径语法非法: {path}"))?;
            }
        }
        Ok(())
    };
    check_paths(p.balance.extract.total.as_ref(), "total")?;
    check_paths(p.balance.extract.top_up.as_ref(), "topUp")?;
    check_paths(p.balance.extract.granted.as_ref(), "granted")?;
    check_paths(p.balance.extract.currency.as_ref(), "currency")?;
    for (k, s) in &p.balance.extract.extras {
        for path in s.path.paths() {
            if path.is_empty() {
                continue;
            }
            tokenize(path).ok_or_else(|| format!("extras.{k} 路径语法非法: {path}"))?;
        }
    }
    if let Some(uw) = &p.balance.unavailable_when {
        for path in uw.path.paths() {
            tokenize(path).ok_or_else(|| format!("unavailableWhen 路径语法非法: {path}"))?;
        }
    }
    // balance computed
    for (name, expr) in &p.balance.computed {
        parse_expr(expr).map_err(|e| format!("computed.{name} 表达式非法: {e}"))?;
    }
    let has_computed_total = p.balance.computed.contains_key("total");
    if p.balance.extract.total.is_none() && !has_computed_total {
        return Err("余额总额无来源:extract.total 与 computed.total 至少提供一个".into());
    }
    // 附加端点
    for (name, ep) in &p.endpoints {
        if let Some(fetcher) = &ep.fetcher {
            if fetcher.trim().is_empty() {
                return Err(format!("endpoints.{name}.fetcher 不能为空"));
            }
            if !p.form.fields.iter().any(|f| &f.id == &ep.auth_field) {
                return Err(format!(
                    "endpoints.{name}.authField 未在 form 字段中定义: {}",
                    ep.auth_field
                ));
            }
        } else {
            let m = ep.method.as_deref().unwrap_or("").to_ascii_uppercase();
            if m != "GET" && m != "POST" {
                return Err(format!(
                    "endpoints.{name}.method 不支持: {:?}(仅 GET/POST,或用 fetcher)",
                    ep.method
                ));
            }
            match &ep.url {
                Some(u) if u.starts_with("https://") || u.starts_with("http://") => {}
                _ => return Err(format!("endpoints.{name}.url 必须以 http(s):// 开头")),
            }
        }
        for (k, s) in &ep.extract {
            for path in s.path.paths() {
                if path.is_empty() {
                    continue;
                }
                tokenize(path)
                    .ok_or_else(|| format!("endpoints.{name}.extract.{k} 路径语法非法: {path}"))?;
            }
        }
        for (cname, expr) in &ep.computed {
            parse_expr(expr)
                .map_err(|e| format!("endpoints.{name}.computed.{cname} 表达式非法: {e}"))?;
        }
        if let Some(uw) = &ep.unavailable_when {
            for path in uw.path.paths() {
                tokenize(path)
                    .ok_or_else(|| format!("endpoints.{name}.unavailableWhen 路径语法非法: {path}"))?;
            }
        }
    }
    // 模板 widget 类型与引用预检
    for (i, w) in p.template.widgets.iter().enumerate() {
        if !["balance", "kv", "bar", "ring", "ringshare", "ringseg", "bars", "stack", "hbar", "table", "note"].contains(&w.kind.as_str()) {
            return Err(format!(
                "template.widgets[{i}].type 非法: {:?}(支持 balance/kv/bar/ring/ringshare/ringseg/bars/stack/hbar/table/note)",
                w.kind
            ));
        }
        if let Some(colors) = &w.colors {
            if colors.is_empty() {
                return Err(format!("template.widgets[{i}].colors 不能为空数组"));
            }
            for (ci, c) in colors.iter().enumerate() {
                let ok = (c.len() == 7 || c.len() == 4)
                    && c.starts_with('#')
                    && c[1..].chars().all(|ch| ch.is_ascii_hexdigit());
                if !ok {
                    return Err(format!(
                        "template.widgets[{i}].colors[{ci}] 非十六进制颜色: {c:?}"
                    ));
                }
            }
        }
        if w.kind == "table" {
            if let Some(cols) = &w.columns {
                if cols.is_empty() || cols.len() > 3 {
                    return Err(format!("template.widgets[{i}](table) columns 需 1-3 列(实际 {})", cols.len()));
                }
            }
            if w.from.is_some() {
                // 动态行:from 引用对象数组,rowName/rowCells 用 {item.*} 模板
                if w.rows.is_some() {
                    return Err(format!("template.widgets[{i}](table) rows 与 from 不能同时声明"));
                }
                let from = w.from.as_deref().unwrap_or_default();
                check_ref_namespace(from, p)
                    .map_err(|e| format!("template.widgets[{i}](table) from: {e}"))?;
                if let Some(name_tpl) = &w.row_name {
                    check_scoped_template_refs(name_tpl)
                        .map_err(|e| format!("template.widgets[{i}](table) rowName: {e}"))?;
                }
                let Some(cells) = &w.row_cells else {
                    return Err(format!("template.widgets[{i}](table) 动态行(from)需声明 rowCells"));
                };
                if cells.is_empty() || cells.len() > 3 {
                    return Err(format!(
                        "template.widgets[{i}](table) rowCells 需 1-3 个模板(实际 {})",
                        cells.len()
                    ));
                }
                for (ci, tpl) in cells.iter().enumerate() {
                    check_scoped_template_refs(tpl)
                        .map_err(|e| format!("template.widgets[{i}](table) rowCells[{ci}]: {e}"))?;
                }
            } else {
                // 静态行
                let Some(rows) = &w.rows else {
                    return Err(format!("template.widgets[{i}](table) 缺少 rows(或声明 from 用动态行)"));
                };
                if w.row_name.is_some() || w.row_cells.is_some() {
                    return Err(format!("template.widgets[{i}](table) rowName/rowCells 仅动态行(from)可用"));
                }
                if rows.is_empty() || rows.len() > 8 {
                    return Err(format!("template.widgets[{i}](table) rows 需 1-8 行(实际 {})", rows.len()));
                }
                for (ri, row) in rows.iter().enumerate() {
                    if row.name.trim().is_empty() {
                        return Err(format!("template.widgets[{i}](table) rows[{ri}].name 不能为空"));
                    }
                    if row.cells.is_empty() || row.cells.len() > 3 {
                        return Err(format!(
                            "template.widgets[{i}](table) rows[{ri}].cells 需 1-3 列(实际 {})",
                            row.cells.len()
                        ));
                    }
                }
            }
        }
        if let Some(pct) = &w.percent {
            check_ref_namespace(pct, p)
                .map_err(|e| format!("template.widgets[{i}].percent: {e}"))?;
        }
        if let Some(series) = &w.series {
            check_ref_namespace(series, p)
                .map_err(|e| format!("template.widgets[{i}].series: {e}"))?;
        }
        if let Some(sl) = &w.series_labels {
            check_ref_namespace(sl, p)
                .map_err(|e| format!("template.widgets[{i}].seriesLabels: {e}"))?;
        }
        for (what, tpl) in [
            ("label", w.label.as_deref()),
            ("value", w.value.as_deref()),
            ("hint", w.hint.as_deref()),
        ] {
            if let Some(tpl) = tpl {
                check_template_refs(tpl, p)
                    .map_err(|e| format!("template.widgets[{i}].{what}: {e}"))?;
            }
        }
    }
    Ok(())
}

// ============================== JSON 路径解析 ==============================

pub(crate) enum Tok {
    Key(String),
    Index(usize),
}

/// 把 "balance_infos[0].total_balance" 之类的路径拆成 token 序列。
pub(crate) fn tokenize(path: &str) -> Option<Vec<Tok>> {
    let b = path.as_bytes();
    let mut toks: Vec<Tok> = Vec::new();
    let mut key = String::new();
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            b'.' => {
                if key.is_empty() {
                    // 数组索引后的点(如 a[0].b)合法;其余场景(开头/连续点)非法
                    if matches!(toks.last(), Some(Tok::Index(_))) {
                        i += 1;
                        continue;
                    }
                    return None;
                }
                toks.push(Tok::Key(std::mem::take(&mut key)));
                i += 1;
            }
            b'[' => {
                if !key.is_empty() {
                    toks.push(Tok::Key(std::mem::take(&mut key)));
                }
                i += 1;
                let mut n = String::new();
                while i < b.len() && b[i].is_ascii_digit() {
                    n.push(b[i] as char);
                    i += 1;
                }
                if n.is_empty() || b.get(i) != Some(&b']') {
                    return None;
                }
                toks.push(Tok::Index(n.parse().ok()?));
                i += 1;
            }
            c if (c as char).is_ascii_alphanumeric() || c == b'_' || c == b'-' => {
                key.push(c as char);
                i += 1;
            }
            _ => return None,
        }
    }
    if !key.is_empty() {
        toks.push(Tok::Key(key));
    }
    if toks.is_empty() {
        return None;
    }
    Some(toks)
}

/// 解析路径并返回响应 JSON 中的值(不存在返回 None)。
pub fn resolve<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let toks = tokenize(path)?;
    let mut cur = root;
    for t in toks {
        match t {
            Tok::Key(k) => cur = cur.as_object()?.get(&k)?,
            Tok::Index(i) => cur = cur.as_array()?.get(i)?,
        }
    }
    Some(cur)
}

/// 按提取规格从响应取值;缺省/兜底逻辑:
/// - 任一候选路径命中非 null → 返回该值
/// - optional → None
/// - 有 default → 返回 default
/// - 否则 Err(可读错误)
pub fn extract(spec: &ExtractSpec, root: &Value) -> Result<Option<Value>, String> {
    for path in spec.path.paths() {
        if let Some(v) = resolve(root, path) {
            if !v.is_null() {
                return Ok(Some(v.clone()));
            }
        }
    }
    if spec.optional {
        return Ok(None);
    }
    if let Some(d) = &spec.default {
        return Ok(Some(d.clone()));
    }
    Err(format!("响应缺少字段: {}", spec.path.display()))
}

/// 数值化:数字原样,字符串 trim 后解析。
pub fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

// ============================== computed 表达式 ==============================

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Call(Op, Box<Expr>, Box<Expr>),
    Ref(String),
    Num(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

/// 解析形如 sub(extras.limit, extras.usedCost) / mul(div(a,b),100) / 42 的表达式。
pub fn parse_expr(s: &str) -> Result<Expr, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("空表达式".into());
    }
    if let Ok(n) = s.parse::<f64>() {
        return Ok(Expr::Num(n));
    }
    if let Some(paren) = s.find('(') {
        if !s.ends_with(')') {
            return Err(format!("缺少右括号: {s}"));
        }
        let name = s[..paren].trim();
        let inner = &s[paren + 1..s.len() - 1];
        let args = split_args(inner)?;
        if args.len() != 2 {
            return Err(format!("运算符 {name} 需要 2 个参数(实际 {})", args.len()));
        }
        let op = match name {
            "add" => Op::Add,
            "sub" => Op::Sub,
            "mul" => Op::Mul,
            "div" => Op::Div,
            _ => return Err(format!("未知运算符: {name}(支持 add/sub/mul/div)")),
        };
        return Ok(Expr::Call(
            op,
            Box::new(parse_expr(args[0])?),
            Box::new(parse_expr(args[1])?),
        ));
    }
    parse_ref(s)
        .map(|r| Expr::Ref(r.to_string()))
        .ok_or_else(|| format!("非法引用或字面量: {s}"))
}

/// 引用语法:<命名空间>.<路径>,如 balance.total / extras.limit / usage.costSeries[0]
/// 路径段允许数组下标([] 数字),与 JSON 路径解析一致。
pub fn parse_ref(s: &str) -> Option<&str> {
    let valid = s.split('.').all(|seg| {
        !seg.is_empty()
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '[' || c == ']')
    });
    if !valid || !s.contains('.') {
        return None;
    }
    Some(s)
}

fn split_args(inner: &str) -> Result<Vec<&str>, String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, c) in inner.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    return Err("括号不匹配".into());
                }
                depth -= 1;
            }
            ',' if depth == 0 => {
                parts.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&inner[start..]);
    Ok(parts)
}

/// 表达式求值上下文。
pub struct EvalCtx<'a> {
    pub balance: Option<&'a BalanceInfo>,
    pub extras: &'a BTreeMap<String, Value>,
    /// 各端点归一化后的数据(name → Value 映射表)
    pub endpoints: &'a BTreeMap<String, Value>,
}

impl<'a> EvalCtx<'a> {
    pub fn resolve_ref(&self, name: &str) -> Option<f64> {
        let Some((ns, path)) = name.split_once('.') else {
            return None;
        };
        match ns {
            "balance" => match path {
                "total" => self.balance.map(|b| b.total),
                "topUp" => self.balance.and_then(|b| b.top_up),
                "granted" => self.balance.and_then(|b| b.granted),
                _ => None,
            },
            "extras" => self.extras.get(path).and_then(as_f64),
            other => {
                let map = self.endpoints.get(other)?;
                resolve(map, path).and_then(as_f64)
            }
        }
    }
}

pub fn eval(e: &Expr, ctx: &EvalCtx) -> Option<f64> {
    match e {
        Expr::Num(n) => Some(*n),
        Expr::Ref(r) => ctx.resolve_ref(r),
        Expr::Call(op, a, b) => {
            let x = eval(a, ctx)?;
            let y = eval(b, ctx)?;
            match op {
                Op::Add => Some(x + y),
                Op::Sub => Some(x - y),
                Op::Mul => Some(x * y),
                Op::Div => {
                    if y == 0.0 {
                        None
                    } else {
                        Some(x / y)
                    }
                }
            }
        }
    }
}

// ============================== 测试 ==============================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolve_walks_objects_and_arrays() {
        let v = json!({"balance_infos": [{"currency": "CNY", "total_balance": "128.47"}]});
        assert_eq!(
            resolve(&v, "balance_infos[0].total_balance").and_then(as_f64),
            Some(128.47)
        );
        assert_eq!(
            resolve(&v, "balance_infos[0].currency").and_then(Value::as_str),
            Some("CNY")
        );
        assert!(resolve(&v, "balance_infos[1].x").is_none());
        assert!(resolve(&v, "nope").is_none());
    }

    #[test]
    fn tokenize_rejects_bad_syntax() {
        assert!(tokenize("a[0]").is_some());
        assert!(tokenize("a.b[0][1].c").is_some());
        assert!(tokenize("a[].b").is_none());
        assert!(tokenize(".a").is_none());
        assert!(tokenize("a..b").is_none());
        assert!(tokenize("a[b]").is_none());
    }

    #[test]
    fn extract_falls_back_and_errors() {
        let root = json!({"a": 1});
        let spec = ExtractSpec {
            path: PathSpec::One("missing".into()),
            optional: false,
            default: Some(json!(7)),
        };
        assert_eq!(extract(&spec, &root).unwrap(), Some(json!(7)));
        let spec2 = ExtractSpec {
            path: PathSpec::One("missing".into()),
            optional: true,
            default: None,
        };
        assert_eq!(extract(&spec2, &root).unwrap(), None);
        let spec3 = ExtractSpec {
            path: PathSpec::One("missing".into()),
            optional: false,
            default: None,
        };
        assert!(extract(&spec3, &root).is_err());
        let spec4 = ExtractSpec {
            path: PathSpec::Many(vec!["missing".into(), "a".into()]),
            optional: false,
            default: None,
        };
        assert_eq!(extract(&spec4, &root).unwrap(), Some(json!(1)));
    }

    #[test]
    fn expr_parse_and_eval() {
        let e = parse_expr("sub(extras.limit, extras.usedCost)").unwrap();
        let ctx = EvalCtx {
            balance: None,
            extras: &BTreeMap::from([
                ("limit".to_string(), json!(10.0)),
                ("usedCost".to_string(), json!(2.5)),
            ]),
            endpoints: &BTreeMap::new(),
        };
        assert_eq!(eval(&e, &ctx), Some(7.5));

        let e2 = parse_expr("mul(div(extras.usedCost, extras.limit), 100)").unwrap();
        assert!((eval(&e2, &ctx).unwrap() - 25.0).abs() < 1e-9);

        // 缺失引用 → None 传播
        let ctx2 = EvalCtx {
            balance: None,
            extras: &BTreeMap::new(),
            endpoints: &BTreeMap::new(),
        };
        assert_eq!(eval(&e, &ctx2), None);

        // 除零 → None
        let e3 = parse_expr("div(1, 0)").unwrap();
        assert_eq!(eval(&e3, &ctx), None);

        assert!(parse_expr("pow(1,2)").is_err());
        assert!(parse_expr("balance.total").is_ok());
        assert!(parse_expr("usage.costSeries").is_ok());
        assert!(parse_expr("extras.").is_err());
        assert!(parse_expr("lonely").is_err());
    }

    #[test]
    fn endpoint_ref_resolves_through_endpoints_map() {
        let endpoints = BTreeMap::from([(
            "usage".to_string(),
            json!({"costSeries": [1.0, 2.5], "dayLabels": ["a", "b"]}),
        )]);
        let ctx = EvalCtx {
            balance: None,
            extras: &BTreeMap::new(),
            endpoints: &endpoints,
        };
        // 数组路径 + 下标
        assert_eq!(ctx.resolve_ref("usage.costSeries[1]"), Some(2.5));
        assert_eq!(ctx.resolve_ref("usage.missing"), None);
    }

    #[test]
    fn validate_v2_form_and_endpoints() {
        let json = r#"{
            "schemaVersion": 2,
            "id": "demo",
            "name": "Demo",
            "auth": { "type": "bearer", "header": "Authorization", "prefix": "Bearer " },
            "form": { "fields": [
                { "id": "apiKey", "label": "API Key", "secret": true },
                { "id": "platformToken", "label": "Platform Token", "secret": true }
            ]},
            "balance": {
                "method": "GET",
                "url": "https://example.com/balance",
                "extract": { "total": { "path": "total" } }
            },
            "endpoints": {
                "usage": {
                    "fetcher": "demo-fetcher",
                    "authField": "platformToken",
                    "extract": { "costSeries": { "path": "costSeries" } }
                }
            },
            "template": {
                "widgets": [
                    { "type": "bars", "label": "近7日", "value": "{usage.total7d:money}", "series": "usage.costSeries" }
                ],
                "band": { "widgetStyles": { "bars": { "height": 56 } } }
            }
        }"#;
        let p: Preset = serde_json::from_str(json).unwrap();
        validate(&p).unwrap();
        // 坏引用被拦截
        let mut bad = p.clone();
        bad.template.widgets[0].series = Some("nope.x".into());
        assert!(validate(&bad).is_err());
        // authField 未在表单中定义
        let mut bad2 = p.clone();
        bad2.endpoints.get_mut("usage").unwrap().auth_field = "nope".into();
        assert!(validate(&bad2).is_err());
    }

    #[test]
    fn unknown_fetcher_refs_reports_only_unregistered() {
        let p: Preset = serde_json::from_str(
            r#"{
                "schemaVersion": 2,
                "id": "demo",
                "name": "Demo",
                "auth": { "type": "bearer", "header": "Authorization" },
                "balance": {
                    "method": "GET",
                    "url": "https://example.com/balance",
                    "extract": { "total": { "path": "total" } }
                },
                "endpoints": {
                    "usage": { "fetcher": "demo-fetcher", "extract": {} },
                    "extra": { "fetcher": "ghost-fetcher", "extract": {} }
                }
            }"#,
        )
        .unwrap();
        let known = ["demo-fetcher"];
        let bad = unknown_fetcher_refs(&[p.clone()], &known);
        assert_eq!(
            bad,
            vec![("demo".to_string(), "extra".to_string(), "ghost-fetcher".to_string())]
        );
        assert!(unknown_fetcher_refs(&[p], &["demo-fetcher", "ghost-fetcher"]).is_empty());
    }

    #[test]
    fn validate_table_dynamic_row_modes() {
        // 动态行合法:from + rowName + rowCells
        let ok = r#"{
            "schemaVersion": 2, "id": "demo", "name": "Demo",
            "auth": { "type": "bearer", "header": "Authorization" },
            "form": { "fields": [ { "id": "apiKey", "label": "API Key" } ] },
            "balance": { "method": "GET", "url": "https://example.com/b", "extract": { "total": { "path": "total" } } },
            "endpoints": { "usage": { "fetcher": "demo-fetcher", "extract": {} } },
            "template": { "widgets": [
                { "type": "table", "label": "模型", "from": "usage.models",
                  "rowName": "{item.name}", "rowCells": ["{item.cost:money}"] }
            ] }
        }"#;
        let p: Preset = serde_json::from_str(ok).unwrap();
        validate(&p).unwrap();
        // rows 与 from 冲突
        let bad = r#"{
            "schemaVersion": 2, "id": "demo", "name": "Demo",
            "auth": { "type": "bearer", "header": "Authorization" },
            "form": { "fields": [ { "id": "apiKey", "label": "API Key" } ] },
            "balance": { "method": "GET", "url": "https://example.com/b", "extract": { "total": { "path": "total" } } },
            "endpoints": { "usage": { "fetcher": "demo-fetcher", "extract": {} } },
            "template": { "widgets": [
                { "type": "table", "label": "模型", "from": "usage.models",
                  "rows": [{ "name": "a", "cells": ["1"] }] }
            ] }
        }"#;
        let p2: Preset = serde_json::from_str(bad).unwrap();
        assert!(validate(&p2).is_err());
        // rowCells 引用非 item.* 占位符
        let bad2 = r#"{
            "schemaVersion": 2, "id": "demo", "name": "Demo",
            "auth": { "type": "bearer", "header": "Authorization" },
            "form": { "fields": [ { "id": "apiKey", "label": "API Key" } ] },
            "balance": { "method": "GET", "url": "https://example.com/b", "extract": { "total": { "path": "total" } } },
            "endpoints": { "usage": { "fetcher": "demo-fetcher", "extract": {} } },
            "template": { "widgets": [
                { "type": "table", "label": "模型", "from": "usage.models",
                  "rowCells": ["{balance.total:money}"] }
            ] }
        }"#;
        let p3: Preset = serde_json::from_str(bad2).unwrap();
        assert!(validate(&p3).is_err());
        // 静态 rows + rowCells → 非法
        let bad3 = r#"{
            "schemaVersion": 2, "id": "demo", "name": "Demo",
            "auth": { "type": "bearer", "header": "Authorization" },
            "form": { "fields": [ { "id": "apiKey", "label": "API Key" } ] },
            "balance": { "method": "GET", "url": "https://example.com/b", "extract": { "total": { "path": "total" } } },
            "endpoints": { "usage": { "fetcher": "demo-fetcher", "extract": {} } },
            "template": { "widgets": [
                { "type": "table", "label": "模型", "rows": [{ "name": "a", "cells": ["1"] }],
                  "rowCells": ["{item.x}"] }
            ] }
        }"#;
        let p4: Preset = serde_json::from_str(bad3).unwrap();
        assert!(validate(&p4).is_err());
    }

    #[test]
    fn ship_presets_validate() {
        // 随包发布的预设必须通过校验(含 mimo 连通性预设与 deepseek 代码内 fetcher 预设)
        let cases: &[(&str, &str)] = &[
            (include_str!("../../presets/mimo.json"), "../../presets/mimo.json"),
            (include_str!("../../presets/deepseek.json"), "../../presets/deepseek.json"),
        ];
        for (raw, p) in cases {
            let preset: Preset = serde_json::from_str(raw).expect("预设 JSON 可解析");
            validate(&preset).unwrap_or_else(|e| panic!("{p} 校验失败: {e}"));
        }
    }
}
