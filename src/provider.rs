//! 供应商抓取:按预设描述发起 HTTP 请求(wasi-http via waki),交给 core 归一化。
//!
//! 阻塞式,勿在 UI 渲染路径调用。HTTP 细节全部来自预设(method/url/headers/body);
//! 复杂数据(如 zip/CSV 导出)走代码内预设注册表(见 capabilities/mod.rs)。

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use waki::Client;

use llmboard_core::normalize::{self, Normalized};
use llmboard_core::preset::{EndpointSpec, Preset};
use llmboard_core::snapshot::BalanceInfo;

pub enum FetchOutcome {
    Available(BalanceInfo, BTreeMap<String, Value>),
    /// 接口明确表示余额不可用(如 DeepSeek is_available=false)
    Unavailable,
}

/// 抓取余额并归一化。
pub fn fetch_balance(preset: &Preset, api_key: &str, checked_at: i64) -> Result<FetchOutcome> {
    let spec = &preset.balance;
    let url = spec.url.replace("{key}", api_key);

    let client = Client::new();
    let builder = match spec.method.to_ascii_uppercase().as_str() {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        _ => return Err(anyhow!("不支持的 HTTP 方法: {}", spec.method)), // validate 已拦截
    };
    // waki 的 header 名只接受 &'static str:预设头部名有界且少量,泄漏换取生命周期
    fn leak_static(s: &str) -> &'static str {
        Box::leak(s.to_string().into_boxed_str())
    }
    let auth_value = format!("{}{}", preset.auth.prefix, api_key);
    let mut builder = builder
        .header(leak_static(&preset.auth.header), auth_value)
        .connect_timeout(Duration::from_secs(spec.timeout_secs));
    for (k, v) in &spec.headers {
        builder = builder.header(leak_static(k), v.clone());
    }
    let builder = match &spec.body {
        Some(body) => builder.body(body.clone()),
        None => builder,
    };

    let resp = builder
        .send()
        .map_err(|e| anyhow!("请求失败: {url} ({e})"))?;
    let status = resp.status_code();
    let raw = resp.body().unwrap_or_default();
    if status != 200 {
        let preview = String::from_utf8_lossy(&raw[..raw.len().min(300)]);
        return Err(anyhow!(
            "HTTP {status} {url} body={preview} {}",
            http_hint(status)
        ));
    }
    let body: Value = serde_json::from_slice(&raw).context("响应非 JSON")?;

    match normalize::normalize_response(preset, &body, checked_at)
        .map_err(|e| anyhow!("{e}"))?
    {
        Normalized::Available(balance, extras) => Ok(FetchOutcome::Available(balance, extras)),
        Normalized::Unavailable => Ok(FetchOutcome::Unavailable),
    }
}

/// 抓取附加端点(声明式 REST 或内置 fetcher)并归一化。
/// 返回 None 表示端点判定不可用。
#[allow(clippy::too_many_arguments)]
pub fn fetch_endpoint(
    preset: &Preset,
    name: &str,
    endpoint: &EndpointSpec,
    credential: &str,
    balance: Option<&BalanceInfo>,
    extras: &BTreeMap<String, Value>,
    endpoints: &BTreeMap<String, Value>,
) -> Result<Option<BTreeMap<String, Value>>> {
    let body: Value = if let Some(fetcher) = &endpoint.fetcher {
        let map = crate::capabilities::run(fetcher, endpoint.url.as_deref(), credential)
            .map_err(|e| anyhow!("{e}"))?;
        serde_json::to_value(&map).context("fetcher 结果序列化失败")?
    } else {
        let url = endpoint
            .url
            .clone()
            .ok_or_else(|| anyhow!("端点缺少 url"))?
            .replace("{key}", credential);
        let client = Client::new();
        let builder = match endpoint.method.as_deref().unwrap_or("GET").to_ascii_uppercase().as_str() {
            "GET" => client.get(&url),
            "POST" => client.post(&url),
            other => return Err(anyhow!("不支持的 HTTP 方法: {other}")),
        };
        fn leak_static(s: &str) -> &'static str {
            Box::leak(s.to_string().into_boxed_str())
        }
        let auth_value = format!("{}{}", preset.auth.prefix, credential);
        let mut builder = builder
            .header(leak_static(&preset.auth.header), auth_value)
            .connect_timeout(Duration::from_secs(endpoint.timeout_secs));
        for (k, v) in &endpoint.headers {
            builder = builder.header(leak_static(k), v.clone());
        }
        let builder = match &endpoint.body {
            Some(body) => builder.body(body.clone()),
            None => builder,
        };
        let resp = builder
            .send()
            .map_err(|e| anyhow!("端点请求失败: {url} ({e})"))?;
        let status = resp.status_code();
        let raw = resp.body().unwrap_or_default();
        if status != 200 {
            let preview = String::from_utf8_lossy(&raw[..raw.len().min(300)]);
            return Err(anyhow!(
                "HTTP {status} {url} body={preview} {}",
                http_hint(status)
            ));
        }
        serde_json::from_slice(&raw).context("端点响应非 JSON")?
    };

    match normalize::normalize_endpoint(name, endpoint, &body, balance, extras, endpoints)
        .map_err(|e| anyhow!("{e}"))?
    {
        Some(map) => Ok(Some(map)),
        None => Ok(None),
    }
}

fn http_hint(status: u16) -> &'static str {
    match status {
        401 => "(401 未授权:凭据无效/过期)",
        402 => "(402 余额不足/欠费)",
        403 => "(403 禁止访问:检查凭据权限或 IP)",
        404 => "(404 接口不存在:检查预设 url)",
        429 => "(429 请求过频/限流,稍后重试)",
        s if (500..600).contains(&s) => "(服务端错误,稍后重试)",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_hint_covers_statuses() {
        assert!(http_hint(401).contains("401"));
        assert!(http_hint(429).contains("429"));
        assert_eq!(http_hint(200), "");
    }
}
