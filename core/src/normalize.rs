//! 响应归一化:把供应商响应 JSON 按预设提取/计算为 BalanceInfo + extras + 端点数据。
//! 纯函数,无 IO,可宿主单测。

use std::collections::BTreeMap;

use serde_json::Value;

use crate::preset::{self, EvalCtx, Preset};
use crate::snapshot::BalanceInfo;

pub enum Normalized {
    Available(BalanceInfo, BTreeMap<String, Value>),
    /// 接口明确表示余额不可用(如 DeepSeek is_available=false)
    Unavailable,
}

/// 按预设把余额响应 JSON 归一化。
pub fn normalize_response(
    preset: &Preset,
    body: &Value,
    checked_at: i64,
) -> Result<Normalized, String> {
    let spec = &preset.balance;

    // 不可用判定
    if let Some(uw) = &spec.unavailable_when {
        if uw
            .path
            .paths()
            .iter()
            .any(|p| preset::resolve(body, p) == Some(&uw.equals))
        {
            tracing::info!("[provider {}] 接口返回不可用", preset.id);
            return Ok(Normalized::Unavailable);
        }
    }

    // extras
    let mut extras = BTreeMap::new();
    for (k, s) in &spec.extract.extras {
        match preset::extract(s, body) {
            Ok(Some(val)) => {
                extras.insert(k.clone(), val);
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("[provider {}] extras.{k} 提取失败: {e}", preset.id),
        }
    }

    // 余额字段初值
    let extract_num = |s: Option<&preset::ExtractSpec>, what: &str| -> Option<f64> {
        let s = s?;
        match preset::extract(s, body) {
            Ok(Some(val)) => preset::as_f64(&val),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!("[provider {}] {what} 提取失败: {e}", preset.id);
                None
            }
        }
    };
    let total = extract_num(spec.extract.total.as_ref(), "total");
    let top_up = extract_num(spec.extract.top_up.as_ref(), "topUp");
    let granted = extract_num(spec.extract.granted.as_ref(), "granted");
    let mut currency = "CNY".to_string();
    if let Some(s) = &spec.extract.currency {
        if let Ok(Some(val)) = preset::extract(s, body) {
            if let Some(c) = val.as_str() {
                if !c.trim().is_empty() {
                    currency = c.trim().to_string();
                }
            }
        }
    }

    // computed 派生(名称为 total/topUp/granted 时写回余额字段,其余进 extras)
    let mut balance = BalanceInfo {
        total: total.unwrap_or(0.0),
        top_up,
        granted,
        currency,
        checked_at,
    };
    let mut computed_total_done = false;
    for (name, expr) in &spec.computed {
        let parsed =
            preset::parse_expr(expr).map_err(|e| format!("computed.{name} 表达式非法: {e}"))?;
        let ctx = EvalCtx {
            balance: Some(&balance),
            extras: &extras,
            endpoints: &BTreeMap::new(),
        };
        let Some(val) = preset::eval(&parsed, &ctx) else {
            tracing::warn!("[provider {}] computed.{name} 求值失败(引用缺失)", preset.id);
            continue;
        };
        match name.as_str() {
            "total" => {
                balance.total = val;
                computed_total_done = true;
            }
            "topUp" => balance.top_up = Some(val),
            "granted" => balance.granted = Some(val),
            _ => {
                extras.insert(name.clone(), serde_json::json!(val));
            }
        }
    }

    // 总额校验:extract.total 或 computed.total 必须产出数值
    if total.is_none() && !computed_total_done {
        return Err(format!(
            "余额总额字段缺失或非数值: {}",
            spec.extract
                .total
                .as_ref()
                .map(|s| s.path.display())
                .unwrap_or_default()
        ));
    }

    Ok(Normalized::Available(balance, extras))
}

/// 端点归一化:extract → 并入已归一化端点表 → computed(可引用 balance/extras/前置端点)。
/// 返回 None 表示端点判定不可用。endpoints 按名称排序依次归一化,
/// computed 只能引用排在前面的端点(确定性,文档约定)。
pub fn normalize_endpoint(
    name: &str,
    endpoint: &preset::EndpointSpec,
    body: &Value,
    balance: Option<&BalanceInfo>,
    extras: &BTreeMap<String, Value>,
    endpoints: &BTreeMap<String, Value>,
) -> Result<Option<BTreeMap<String, Value>>, String> {
    if let Some(uw) = &endpoint.unavailable_when {
        if uw
            .path
            .paths()
            .iter()
            .any(|p| preset::resolve(body, p) == Some(&uw.equals))
        {
            tracing::info!("[endpoint {name}] 接口返回不可用");
            return Ok(None);
        }
    }

    // 第一遍:extract
    let mut merged = endpoints.clone();
    let mut map = BTreeMap::new();
    for (k, s) in &endpoint.extract {
        match preset::extract(s, body) {
            Ok(Some(val)) => {
                map.insert(k.clone(), val);
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("[endpoint {name}] extract.{k} 提取失败: {e}"),
        }
    }
    // 空提取 + 无 computed → 原样保留整个响应,便于模板直接引用
    if map.is_empty() && endpoint.computed.is_empty() {
        map.insert("response".to_string(), body.clone());
    }

    // 第二遍:computed(把本端点已提取部分并入上下文,便于 self 引用)
    merged.insert(name.to_string(), serde_json::json!(map));
    let ctx = EvalCtx {
        balance,
        extras,
        endpoints: &merged,
    };
    for (cname, expr) in &endpoint.computed {
        let parsed =
            preset::parse_expr(expr).map_err(|e| format!("computed.{cname} 表达式非法: {e}"))?;
        let Some(val) = preset::eval(&parsed, &ctx) else {
            tracing::warn!("[endpoint {name}] computed.{cname} 求值失败(引用缺失)");
            continue;
        };
        map.insert(cname.clone(), serde_json::json!(val));
    }

    Ok(Some(map))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn load(json: &str) -> Preset {
        let p: Preset = serde_json::from_str(json).unwrap();
        preset::validate(&p).unwrap();
        p
    }

    #[test]
    fn deepseek_style_response() {
        let preset = load(
            r#"{
                "schemaVersion": 2, "id": "demo", "name": "Demo",
                "auth": { "type": "bearer", "header": "Authorization", "prefix": "Bearer " },
                "balance": {
                    "method": "GET", "url": "https://example.com/balance",
                    "extract": {
                        "total": { "path": "balance_infos[0].total_balance" },
                        "topUp": { "path": "balance_infos[0].topped_up_balance", "optional": true },
                        "granted": { "path": "balance_infos[0].granted_balance", "optional": true },
                        "currency": { "path": "balance_infos[0].currency", "default": "CNY" }
                    },
                    "unavailableWhen": { "path": "is_available", "equals": false }
                },
                "template": { "widgets": [] }
            }"#,
        );
        let body = json!({
            "is_available": true,
            "balance_infos": [{
                "currency": "CNY",
                "total_balance": "128.47",
                "topped_up_balance": "118.47",
                "granted_balance": "10.00"
            }]
        });
        match normalize_response(&preset, &body, 1000).unwrap() {
            Normalized::Available(b, _) => {
                assert_eq!(b.total, 128.47);
                assert_eq!(b.top_up, Some(118.47));
                assert_eq!(b.granted, Some(10.0));
                assert_eq!(b.currency, "CNY");
            }
            Normalized::Unavailable => panic!("应可用"),
        }
        let mut un = body.clone();
        un["is_available"] = json!(false);
        assert!(matches!(
            normalize_response(&preset, &un, 1000).unwrap(),
            Normalized::Unavailable
        ));
    }

    #[test]
    fn openrouter_style_computed() {
        let preset = load(
            r#"{
                "schemaVersion": 2, "id": "router", "name": "Router",
                "auth": { "type": "bearer", "header": "Authorization", "prefix": "Bearer " },
                "balance": {
                    "method": "GET", "url": "https://example.com/key",
                    "extract": {
                        "currency": { "default": "USD" },
                        "extras": {
                            "limit": { "path": "data.limit", "optional": true },
                            "usedCost": { "path": "data.usage.total_cost", "optional": true }
                        }
                    },
                    "computed": {
                        "total": "sub(extras.limit, extras.usedCost)",
                        "topUp": "extras.limit",
                        "usedPercent": "mul(div(extras.usedCost, extras.limit), 100)"
                    }
                },
                "template": { "widgets": [] }
            }"#,
        );
        let body = json!({"data": {"limit": 10.0, "usage": {"total_cost": 2.5}}});
        match normalize_response(&preset, &body, 2000).unwrap() {
            Normalized::Available(b, extras) => {
                assert_eq!(b.total, 7.5);
                assert_eq!(b.top_up, Some(10.0));
                assert_eq!(b.currency, "USD");
                assert_eq!(extras.get("usedPercent").and_then(preset::as_f64), Some(25.0));
            }
            Normalized::Unavailable => panic!("应可用"),
        }
        let bad = json!({"data": {"usage": {"total_cost": 2.5}}});
        assert!(normalize_response(&preset, &bad, 2000).is_err());
    }

    #[test]
    fn endpoint_normalize_extract_and_computed() {
        let preset = load(
            r#"{
                "schemaVersion": 2, "id": "ep", "name": "EP",
                "auth": { "type": "bearer", "header": "Authorization", "prefix": "Bearer " },
                "form": { "fields": [{ "id": "apiKey", "label": "Key" }] },
                "balance": {
                    "method": "GET", "url": "https://example.com/balance",
                    "extract": { "total": { "path": "total" } }
                },
                "endpoints": {
                    "usage": {
                        "method": "GET",
                        "url": "https://example.com/usage",
                        "extract": {
                            "costSeries": { "path": "data.cost", "optional": true },
                            "tokenSeries": { "path": "data.tokens", "optional": true }
                        },
                        "computed": { "total7d": "add(usage.costSeries[0], usage.costSeries[1])" }
                    }
                },
                "template": { "widgets": [] }
            }"#,
        );
        let body = json!({"data": {"cost": [1.0, 2.0], "tokens": [100, 200]}});
        let ep = &preset.endpoints["usage"];
        let map = normalize_endpoint(
            "usage",
            ep,
            &body,
            None,
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            map.get("costSeries").and_then(Value::as_array).unwrap().len(),
            2
        );
        assert_eq!(map.get("total7d").and_then(preset::as_f64), Some(3.0));

        // unavailableWhen → None
        let un_body = json!({"ok": false});
        let ep2 = preset::EndpointSpec {
            method: Some("GET".into()),
            url: Some("https://example.com/x".into()),
            auth_field: "apiKey".into(),
            headers: Default::default(),
            body: None,
            timeout_secs: 10,
            fetcher: None,
            extract: Default::default(),
            computed: Default::default(),
            unavailable_when: Some(preset::UnavailableWhen {
                path: preset::PathSpec::One("ok".into()),
                equals: json!(false),
            }),
        };
        assert!(normalize_endpoint("u2", &ep2, &un_body, None, &BTreeMap::new(), &BTreeMap::new())
            .unwrap()
            .is_none());
    }
}
