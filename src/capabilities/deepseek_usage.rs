//! DeepSeek 平台的代码内预设:用量导出(zip/CSV,近 7 日逐日序列)。
//!
//! 按"供应商独立行为归预设开发者、插件只留通用管线"的分工:
//! 本文件属于 DeepSeek 供应商包(预设 presets/deepseek.json + 本能力),
//! 在父模块 capabilities/mod.rs 注册后由预设端点 "fetcher":
//! "deepseek-usage-export" 引用。HTTP/zip 解包等 IO 在此完成,
//! CSV 聚合与逐日序列等纯逻辑在 llmboard-core::usage(可宿主单测)。

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::time::Duration;

use serde_json::Value;
use waki::Client;

use llmboard_core::{dates, usage};

use super::Capability;

/// 注册条目(登记于父模块 CAPABILITIES)。
pub const USAGE_EXPORT: Capability = Capability {
    name: "deepseek-usage-export",
    about: "DeepSeek 平台用量导出(zip/CSV,近 7 日逐日序列)",
    run: usage_export,
};

/// 缺省 UA 会被平台反爬拦截(429),与 DSBoard 一致
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:152.0) Gecko/20100101 Firefox/152.0";

/// DeepSeek 用量:平台导出接口(需要平台 token,API Key 无法访问该私有端点)。
/// 返回最近 {usage::DAYS} 天的逐日消费/输出 token/缓存命中率序列与汇总,供预设模板的 bars 组件使用。
///
/// 数据源:GET {platform}/api/v0/usage/export?start=&end=&tz=28800(Bearer 平台 token)
/// 响应为 zip,内含 amount-*.csv(逐模型逐日 token/调用)与 cost-*.csv(逐模型逐日成本)。
fn usage_export(
    base_url: Option<&str>,
    credential: &str,
) -> Result<BTreeMap<String, Value>, String> {
    let base = base_url
        .unwrap_or("https://platform.deepseek.com/api/v0/usage/export")
        .trim_end_matches('/');
    let (start, end) = usage::cn_window(usage::DAYS);
    let url = format!("{base}?start={start}&end={end}&tz={}", usage::TZ_SEC);

    let client = Client::new();
    let builder = client
        .get(&url)
        .header("Authorization", format!("Bearer {credential}"))
        .header("User-Agent", USER_AGENT)
        .header("x-client-bundle-id", "com.deepseek.chat")
        .header("x-client-platform", "web")
        .header("x-client-version", "1.0.0")
        .header("x-client-locale", "zh_CN")
        .header("x-client-timezone-offset", usage::TZ_SEC.to_string())
        .connect_timeout(Duration::from_secs(60));
    let resp = builder.send().map_err(|e| format!("导出请求失败: {url} ({e})"))?;
    let status = resp.status_code();
    let raw = resp.body().unwrap_or_default();
    if status != 200 {
        let preview = String::from_utf8_lossy(&raw[..raw.len().min(300)]);
        return Err(format!(
            "用量导出失败: HTTP {status} {preview} {}",
            export_hint(status)
        ));
    }
    tracing::info!(
        "[capability deepseek-usage-export] 用量导出 HTTP 200 bytes={}",
        raw.len()
    );

    let (amount_text, cost_text) = read_zip_csvs(&raw)?;
    let per_day = usage::aggregate_csvs(&amount_text, &cost_text)?;
    let today_days = (dates::unix_now() + usage::TZ_SEC).div_euclid(86_400);
    let mut out = usage::build_daily_series(&per_day, today_days);
    // 模型用量排行(表格真实数据源):展示名去前缀、≤8 行,按成本降序
    let models: Vec<serde_json::Value> = usage::aggregate_models(&amount_text, &cost_text)?
        .into_iter()
        .take(8)
        .map(|m| {
            serde_json::json!({
                "name": usage::model_display_name(&m.model),
                "cost": m.cost,
                "tokens": m.tokens,
            })
        })
        .collect();
    out.insert("models".to_string(), serde_json::Value::Array(models));
    // 当日模型用量(输出 token 降序,榜首即 100% 基准;供 hbar 模板)
    // 以及同一排序下的「每模型当日缓存命中率」序列(供 ringshare rate 模式)。
    // 两个序列共用 todayModelNames,顺序一致,模板可直接下标对齐。
    let day = usage::day_key(today_days);
    let mut today_models = usage::aggregate_models_on(&amount_text, &cost_text, &day)?;
    today_models.sort_by(|a, b| b.tokens.cmp(&a.tokens));
    let mut today_series: Vec<serde_json::Value> = Vec::new();
    let mut today_names: Vec<serde_json::Value> = Vec::new();
    let mut today_hit_rates: Vec<serde_json::Value> = Vec::new();
    let mut today_tokens = 0u64;
    let mut today_cost = 0.0f64;
    for m in today_models.into_iter().take(8) {
        today_names.push(serde_json::Value::String(usage::model_display_name(&m.model)));
        today_series.push(serde_json::json!(m.tokens));
        // 无缓存数据(当日该模型没有 hit/miss 行)按 0 处理:手环 rate 模式下 0 的环不渲染
        today_hit_rates.push(serde_json::json!(
            usage::hit_rate(m.hit_tokens, m.miss_tokens).unwrap_or(0.0)
        ));
        today_tokens += m.tokens;
        today_cost += m.cost;
    }
    out.insert("todayTokenSeries".to_string(), serde_json::Value::Array(today_series));
    out.insert("todayModelNames".to_string(), serde_json::Value::Array(today_names));
    out.insert(
        "todayModelHitRateSeries".to_string(),
        serde_json::Value::Array(today_hit_rates),
    );
    out.insert("todayTokens".to_string(), serde_json::json!(today_tokens));
    out.insert("todayCost".to_string(), serde_json::json!(usage::round2(today_cost)));
    Ok(out)
}

/// 解包 zip 并读出 amount-*.csv 与 cost-*.csv。
fn read_zip_csvs(bytes: &[u8]) -> Result<(String, String), String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("zip 解析失败: {e}"))?;
    let mut amount_text: Option<String> = None;
    let mut cost_text: Option<String> = None;
    for i in 0..archive.len() {
        let name = archive
            .by_index(i)
            .map(|f| f.name().to_string())
            .map_err(|e| format!("zip 读取失败: {e}"))?;
        if !name.ends_with(".csv") {
            continue;
        }
        if name.starts_with("amount-") && amount_text.is_none() {
            amount_text = Some(read_entry(&mut archive, i)?);
        } else if name.starts_with("cost-") && cost_text.is_none() {
            cost_text = Some(read_entry(&mut archive, i)?);
        }
    }
    let amount_text = amount_text.ok_or("zip 缺少 amount-*.csv")?;
    let cost_text = cost_text.ok_or("zip 缺少 cost-*.csv")?;
    Ok((amount_text, cost_text))
}

fn read_entry(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, i: usize) -> Result<String, String> {
    let mut entry = archive
        .by_index(i)
        .map_err(|e| format!("zip 条目读取失败: {e}"))?;
    let mut buf = Vec::new();
    entry
        .read_to_end(&mut buf)
        .map_err(|e| format!("zip 解压失败: {e}"))?;
    String::from_utf8(buf).map_err(|_| "CSV 编码非 UTF-8".to_string())
}

fn export_hint(status: u16) -> &'static str {
    match status {
        401 => "(token 无效或已过期,请重新从浏览器 F12 复制)",
        403 => "(访问被拒绝,检查平台 token 权限/网络环境)",
        429 => "(请求过于频繁或触发反爬,稍后重试)",
        s if (500..600).contains(&s) => "(平台服务端错误,稍后重试)",
        _ => "",
    }
}
