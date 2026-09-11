//! 模板渲染:把预设 template 中的 widgets 规格渲染为通用组件树。
//!
//! 值模板语法:{ref:format},如 "{balance.total:money}"。
//! ref:balance.total / balance.topUp / balance.granted / balance.currency /
//!     extras.<key> / <endpoint名>.<路径> / meta.name
//! format:money(货币) / percent / number / compact(K/M/B) / raw(原样)

use std::collections::BTreeMap;

use serde_json::Value;

use crate::preset::{self, EvalCtx, Preset, WidgetSpec};
use crate::snapshot::{BalanceInfo, Widget};

const EMPTY: &str = "--";
/// bars 序列最多柱子数
const BARS_MAX: usize = 14;

/// 把预设模板渲染成组件树。
pub fn render_widgets(
    preset: &Preset,
    balance: Option<&BalanceInfo>,
    extras: &BTreeMap<String, Value>,
    endpoints: &BTreeMap<String, Value>,
) -> Vec<Widget> {
    let mut out = Vec::new();
    for spec in &preset.template.widgets {
        if let Some(mut w) = render_widget(preset, spec, balance, extras, endpoints) {
            // 区块组由 spec 声明,渲染后统一写入(同组相邻组件间手环端不画分隔线)
            w.group = spec.group.clone();
            out.push(w);
        }
    }
    out
}

fn render_widget(
    preset: &Preset,
    spec: &WidgetSpec,
    balance: Option<&BalanceInfo>,
    extras: &BTreeMap<String, Value>,
    endpoints: &BTreeMap<String, Value>,
) -> Option<Widget> {
    let ctx = RenderCtx {
        preset,
        balance,
        extras,
        endpoints,
    };
    match spec.kind.as_str() {
        "balance" => {
            let value = render_value(
                spec.value.as_deref().unwrap_or("{balance.total:money}"),
                &ctx,
            );
            if spec.hide_empty && is_empty(&value) {
                return None;
            }
            Some(Widget {
                kind: "balance".into(),
                label: spec.label.as_deref().map(|l| render_value(l, &ctx)),
                value,
                hint: spec.hint.as_deref().map(|h| render_value(h, &ctx)),
                percent: None,
                series: None,
                series_labels: None,
                color: Some(spec.color.clone().unwrap_or_else(|| preset.accent.clone())),
            ..Default::default()
            })
        }
        "note" => Some(Widget {
            kind: "note".into(),
            label: None,
            value: render_value(spec.value.as_deref().unwrap_or(""), &ctx),
            hint: None,
            percent: None,
            series: None,
            series_labels: None,
            color: spec.color.clone(),
            ..Default::default()
        }),
        "bars" => {
            let value = render_value(spec.value.as_deref().unwrap_or(""), &ctx);
            if spec.hide_empty && is_empty(&value) {
                return None;
            }
            let series = spec.series.as_deref().and_then(|s| resolve_series(s, &ctx));
            if series.is_none() {
                return None; // 序列缺失时整块跳过
            }
            Some(Widget {
                kind: "bars".into(),
                label: spec.label.as_deref().map(|l| render_value(l, &ctx)),
                value,
                hint: spec.hint.as_deref().map(|h| render_value(h, &ctx)),
                percent: None,
                series,
                series_labels: spec
                    .series_labels
                    .as_deref()
                    .and_then(|s| resolve_series_labels(s, &ctx)),
                color: Some(spec.color.clone().unwrap_or_else(|| preset.accent.clone())),
            ..Default::default()
            })
        }
        "ringshare" => {
            let value = render_value(spec.value.as_deref().unwrap_or(""), &ctx);
            if spec.hide_empty && is_empty(&value) {
                return None;
            }
            let series = spec.series.as_deref().and_then(|s| resolve_series(s, &ctx));
            if series.is_none() {
                return None; // 序列缺失时整块跳过
            }
            Some(Widget {
                kind: "ringshare".into(),
                label: spec.label.as_deref().map(|l| render_value(l, &ctx)),
                value,
                hint: spec.hint.as_deref().map(|h| render_value(h, &ctx)),
                percent: None,
                series,
                series_labels: spec
                    .series_labels
                    .as_deref()
                    .and_then(|s| resolve_series_labels(s, &ctx)),
                color: Some(spec.color.clone().unwrap_or_else(|| preset.accent.clone())),
                colors: spec.colors.clone(),
            ..Default::default()
            })
        }
        "ringseg" => {
            let value = render_value(spec.value.as_deref().unwrap_or(""), &ctx);
            if spec.hide_empty && is_empty(&value) {
                return None;
            }
            let series = spec.series.as_deref().and_then(|s| resolve_series(s, &ctx));
            if series.is_none() {
                return None; // 序列缺失时整块跳过
            }
            Some(Widget {
                kind: "ringseg".into(),
                label: spec.label.as_deref().map(|l| render_value(l, &ctx)),
                value,
                hint: spec.hint.as_deref().map(|h| render_value(h, &ctx)),
                percent: None,
                series,
                series_labels: spec
                    .series_labels
                    .as_deref()
                    .and_then(|s| resolve_series_labels(s, &ctx)),
                color: Some(spec.color.clone().unwrap_or_else(|| preset.accent.clone())),
                colors: spec.colors.clone(),
            ..Default::default()
            })
        }
        "stack" => {
            let value = render_value(spec.value.as_deref().unwrap_or(""), &ctx);
            if spec.hide_empty && is_empty(&value) {
                return None;
            }
            let series = spec.series.as_deref().and_then(|s| resolve_series(s, &ctx));
            if series.is_none() || series.as_ref().map(|s| s.len()).unwrap_or(0) < 2 {
                return None; // 堆叠占比至少需要 2 段
            }
            Some(Widget {
                kind: "stack".into(),
                label: spec.label.as_deref().map(|l| render_value(l, &ctx)),
                value,
                hint: spec.hint.as_deref().map(|h| render_value(h, &ctx)),
                percent: None,
                series,
                series_labels: None,
                color: Some(spec.color.clone().unwrap_or_else(|| preset.accent.clone())),
            ..Default::default()
            })
        }
        "hbar" => {
            let value = render_value(spec.value.as_deref().unwrap_or(""), &ctx);
            if spec.hide_empty && is_empty(&value) {
                return None;
            }
            let series = spec.series.as_deref().and_then(|s| resolve_series(s, &ctx));
            if series.is_none() {
                return None; // 序列缺失时整块跳过
            }
            Some(Widget {
                kind: "hbar".into(),
                label: spec.label.as_deref().map(|l| render_value(l, &ctx)),
                value,
                hint: spec.hint.as_deref().map(|h| render_value(h, &ctx)),
                percent: None,
                series,
                // 行标签复用 seriesLabels(每根水平条一行,标签 = 对应条目名)
                series_labels: spec
                    .series_labels
                    .as_deref()
                    .and_then(|s| resolve_series_labels(s, &ctx)),
                color: Some(spec.color.clone().unwrap_or_else(|| preset.accent.clone())),
            ..Default::default()
            })
        }
        kind @ ("kv" | "bar" | "ring") => {
            let value = render_value(spec.value.as_deref().unwrap_or(""), &ctx);
            if spec.hide_empty && is_empty(&value) {
                return None;
            }
            let percent = spec
                .percent
                .as_deref()
                .and_then(|p| resolve_percent(p, &ctx));
            let barish = kind == "bar" || kind == "ring";
            if barish && percent.is_none() && value == EMPTY {
                return None;
            }
            Some(Widget {
                kind: kind.into(),
                label: spec.label.as_deref().map(|l| render_value(l, &ctx)),
                value,
                hint: spec.hint.as_deref().map(|h| render_value(h, &ctx)),
                percent,
                series: None,
                series_labels: None,
                color: Some(spec.color.clone().unwrap_or_else(|| preset.accent.clone())),
            ..Default::default()
            })
        }
        "table" => {
            // 表格:label 为区块标题;每行 name 为竖表头, cells ≤3 数据格(均已渲染)。
            // 行数据两种来源:静态 rows,或 from(端点/extras 的对象数组)+ rowName/rowCells 动态行。
            let mut w = Widget {
                kind: "table".into(),
                label: spec.label.as_deref().map(|l| render_value(l, &ctx)),
                value: render_value(spec.value.as_deref().unwrap_or(""), &ctx),
                hint: spec.hint.as_deref().map(|h| render_value(h, &ctx)),
                color: Some(spec.color.clone().unwrap_or_else(|| preset.accent.clone())),
                ..Default::default()
            };
            if let Some(cols) = &spec.columns {
                w.table_columns =
                    Some(cols.iter().map(|c| render_value(c, &ctx)).collect::<Vec<String>>());
            }
            if let Some(rows) = &spec.rows {
                w.table_rows = Some(
                    rows.iter()
                        .map(|row| crate::snapshot::TableRow {
                            name: render_value(&row.name, &ctx),
                            cells: row
                                .cells
                                .iter()
                                .map(|c| render_value(c, &ctx))
                                .collect::<Vec<String>>(),
                        })
                        .collect::<Vec<_>>(),
                );
            } else if let Some(from) = &spec.from {
                let items = resolve_objects(from, &ctx);
                if items.is_empty() {
                    return None; // 数据源缺失/为空:整块隐藏(与 bars 序列缺失一致)
                }
                let name_tpl = spec.row_name.clone().unwrap_or_else(|| "{item.name}".into());
                let cell_tpls = spec.row_cells.as_deref().unwrap_or(&[]);
                let mut rows: Vec<crate::snapshot::TableRow> = Vec::new();
                for obj in items.iter().take(8) {
                    let name = render_scoped(&name_tpl, obj, &ctx);
                    if name.trim().is_empty() || name == EMPTY {
                        continue; // 行名缺失 → 跳过该行
                    }
                    let cells = cell_tpls
                        .iter()
                        .map(|t| render_scoped(t, obj, &ctx))
                        .collect::<Vec<String>>();
                    rows.push(crate::snapshot::TableRow { name, cells });
                }
                if rows.is_empty() {
                    return None;
                }
                w.table_rows = Some(rows);
            }
            Some(w)
        }
        _ => None, // validate 已拦截非法类型
    }
}

struct RenderCtx<'a> {
    preset: &'a Preset,
    balance: Option<&'a BalanceInfo>,
    extras: &'a BTreeMap<String, Value>,
    endpoints: &'a BTreeMap<String, Value>,
}

impl<'a> RenderCtx<'a> {
    fn resolve_str(&self, name: &str) -> Option<String> {
        if name == "meta.name" {
            return Some(self.preset.name.clone());
        }
        let Some((ns, path)) = name.split_once('.') else {
            return None;
        };
        match ns {
            "balance" => match path {
                "currency" => Some(
                    self.balance
                        .as_ref()
                        .map(|b| b.currency.clone())
                        .unwrap_or_else(|| "CNY".into()),
                ),
                _ => {
                    let ctx = EvalCtx {
                        balance: self.balance,
                        extras: self.extras,
                        endpoints: self.endpoints,
                    };
                    ctx.resolve_ref(name).map(|n| n.to_string())
                }
            },
            "extras" => self.extras.get(path).map(display_string),
            other => {
                // 端点命名空间:<name>.<路径>
                let map = self.endpoints.get(other)?;
                preset::resolve(map, path).map(display_string)
            }
        }
    }
}

fn display_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => EMPTY.to_string(),
    }
}

/// 渲染带 {ref:format} 占位符的模板字符串;未解析占位符显示 "--"。
fn render_value(tpl: &str, ctx: &RenderCtx) -> String {
    let mut out = String::new();
    let mut rest = tpl;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            // 没有右括号:原样输出剩余
            out.push_str(after);
            return out;
        };
        let token = &after[..end];
        let (name, format) = match token.split_once(':') {
            Some((n, f)) => (n.trim(), Some(f.trim())),
            None => (token.trim(), None),
        };
        if let Some(s) = ctx.resolve_str(name) {
            let format = format.unwrap_or(if s.parse::<f64>().is_ok() { "number" } else { "raw" });
            out.push_str(&apply_format(&s, format, ctx));
        } else {
            out.push_str(EMPTY);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn resolve_percent(spec: &str, ctx: &RenderCtx) -> Option<f64> {
    let spec = spec.trim();
    if let Ok(n) = spec.parse::<f64>() {
        return Some(n.clamp(0.0, 100.0));
    }
    if preset::parse_ref(spec).is_some() {
        let eval_ctx = EvalCtx {
            balance: ctx.balance,
            extras: ctx.extras,
            endpoints: ctx.endpoints,
        };
        return eval_ctx.resolve_ref(spec).map(|n| n.clamp(0.0, 100.0));
    }
    None
}

/// 解析 bars 序列引用(如 usage.costSeries)为数值数组。
fn resolve_series(spec: &str, ctx: &RenderCtx) -> Option<Vec<f64>> {
    let spec = spec.trim();
    let Some((ns, path)) = spec.split_once('.') else {
        return None;
    };
    let map = ctx.endpoints.get(ns)?;
    let val = preset::resolve(map, path)?;
    let arr = val.as_array()?;
    let nums: Vec<f64> = arr.iter().filter_map(preset::as_f64).collect();
    if nums.is_empty() {
        return None;
    }
    Some(nums.into_iter().take(BARS_MAX).collect())
}

/// 解析 bars 序列标签引用(如 usage.dayLabels)为字符串数组。
fn resolve_series_labels(spec: &str, ctx: &RenderCtx) -> Option<Vec<String>> {
    let spec = spec.trim();
    let Some((ns, path)) = spec.split_once('.') else {
        return None;
    };
    let map = ctx.endpoints.get(ns)?;
    let val = preset::resolve(map, path)?;
    let arr = val.as_array()?;
    let strs: Vec<String> = arr
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    if strs.is_empty() {
        return None;
    }
    Some(strs.into_iter().take(BARS_MAX).collect())
}

/// 解析表格 from 引用(端点/extras 命名空间下的对象数组)为对象列表。
fn resolve_objects<'a>(spec: &str, ctx: &RenderCtx<'a>) -> Vec<&'a Value> {
    let Some((ns, path)) = spec.trim().split_once('.') else {
        return Vec::new();
    };
    let val = if ns == "extras" {
        ctx.extras.get(path)
    } else {
        ctx.endpoints.get(ns).and_then(|m| preset::resolve(m, path))
    };
    match val {
        Some(Value::Array(a)) => a.iter().filter(|v| v.is_object()).collect(),
        _ => Vec::new(),
    }
}

/// 渲染作用域模板(动态表格行/单元格):占位符仅支持 {item.<路径>:格式}。
fn render_scoped(tpl: &str, item: &Value, ctx: &RenderCtx) -> String {
    let mut out = String::new();
    let mut rest = tpl;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            out.push_str(after);
            return out;
        };
        let token = &after[..end];
        let (name, format) = match token.split_once(':') {
            Some((n, f)) => (n.trim(), Some(f.trim())),
            None => (token.trim(), None),
        };
        if let Some(path) = name.strip_prefix("item.") {
            let v = preset::resolve(item, path).filter(|v| !v.is_null());
            match v {
                Some(v) => {
                    let s = display_string(v);
                    let format = format.unwrap_or(if s.parse::<f64>().is_ok() { "number" } else { "raw" });
                    out.push_str(&apply_format(&s, format, ctx));
                }
                None => out.push_str(EMPTY),
            }
        } else {
            out.push_str(EMPTY);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn is_empty(v: &str) -> bool {
    v.trim().is_empty() || v == EMPTY
}

/// 货币符号映射(手环端显示用)。
pub fn currency_symbol(code: &str) -> &'static str {
    match code.to_ascii_uppercase().as_str() {
        "CNY" | "RMB" => "¥",
        "USD" => "$",
        "EUR" => "€",
        "JPY" => "¥",
        "GBP" => "£",
        "HKD" => "HK$",
        "KRW" => "₩",
        _ => "",
    }
}

fn apply_format(raw: &str, format: &str, ctx: &RenderCtx) -> String {
    let currency = ctx
        .balance
        .as_ref()
        .map(|b| b.currency.as_str())
        .unwrap_or("CNY");
    let sym = currency_symbol(currency);
    let as_num = raw.parse::<f64>().ok();
    match format {
        "money" => match as_num {
            Some(n) => {
                if sym.is_empty() {
                    format!("{n:.2} {currency}")
                } else {
                    format!("{sym}{n:.2}")
                }
            }
            None => EMPTY.to_string(),
        },
        "percent" => match as_num {
            Some(n) => format!("{n:.1}%"),
            None => EMPTY.to_string(),
        },
        "number" => match as_num {
            Some(n) => {
                let s = format!("{n:.2}");
                let s = s.trim_end_matches('0').trim_end_matches('.');
                s.to_string()
            }
            None => EMPTY.to_string(),
        },
        "compact" => match as_num {
            Some(n) => compact(n),
            None => EMPTY.to_string(),
        },
        _ => raw.to_string(),
    }
}

pub fn compact(n: f64) -> String {
    let a = n.abs();
    if a >= 1e9 {
        format!("{:.1}B", n / 1e9)
    } else if a >= 1e6 {
        format!("{:.1}M", n / 1e6)
    } else if a >= 1e3 {
        format!("{:.1}K", n / 1e3)
    } else {
        format!("{n:.0}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::{
        AuthSpec, BalanceExtract, BalanceSpec, ExtractSpec, FormSpec, PathSpec, TemplateSpec,
        WidgetSpec,
    };
    use serde_json::json;

    fn demo_preset(widgets: Vec<WidgetSpec>) -> Preset {
        Preset {
            schema_version: 2,
            id: "demo".into(),
            name: "Demo Provider".into(),
            homepage: None,
            accent: "#FF8800".into(),
            auth: AuthSpec {
                kind: "bearer".into(),
                header: "Authorization".into(),
                prefix: "Bearer ".into(),
                key_label: "API Key".into(),
                key_hint: String::new(),
            },
            form: FormSpec::default(),
            balance: BalanceSpec {
                method: "GET".into(),
                url: "https://example.com/x".into(),
                headers: Default::default(),
                body: None,
                timeout_secs: 10,
                extract: BalanceExtract {
                    total: Some(ExtractSpec {
                        path: PathSpec::One("total".into()),
                        optional: false,
                        default: None,
                    }),
                    top_up: None,
                    granted: None,
                    currency: None,
                    extras: Default::default(),
                },
                unavailable_when: None,
                computed: Default::default(),
            },
            endpoints: Default::default(),
            template: TemplateSpec {
                widgets,
                band: None,
            },
        }
    }

    fn bal() -> BalanceInfo {
        BalanceInfo {
            total: 128.47,
            top_up: Some(118.0),
            granted: Some(10.47),
            currency: "CNY".into(),
            checked_at: 0,
        }
    }

    fn no_ep() -> BTreeMap<String, Value> {
        BTreeMap::new()
    }

    #[test]
    fn renders_money_and_hide_empty() {
        let preset = demo_preset(vec![
            WidgetSpec {
                kind: "balance".into(),
                label: None,
                value: Some("{balance.total:money}".into()),
                hint: None,
                percent: None,
                series: None,
                series_labels: None,
                color: None,
                hide_empty: false,
                group: None,
                columns: None,
                rows: None,
                from: None,
                row_name: None,
                row_cells: None,
                colors: None,
            },
            WidgetSpec {
                kind: "kv".into(),
                label: Some("充值".into()),
                value: Some("{balance.topUp:money}".into()),
                hint: None,
                percent: None,
                series: None,
                series_labels: None,
                color: None,
                hide_empty: true,
                group: None,
                columns: None,
                rows: None,
                from: None,
                row_name: None,
                row_cells: None,
                colors: None,
            },
            WidgetSpec {
                kind: "kv".into(),
                label: Some("赠送".into()),
                value: Some("{balance.granted:money}".into()),
                hint: None,
                percent: None,
                series: None,
                series_labels: None,
                color: None,
                hide_empty: true,
                group: None,
                columns: None,
                rows: None,
                from: None,
                row_name: None,
                row_cells: None,
                colors: None,
            },
        ]);
        let widgets = render_widgets(&preset, Some(&bal()), &BTreeMap::new(), &no_ep());
        assert_eq!(widgets.len(), 3);
        assert_eq!(widgets[0].value, "¥128.47");
        assert_eq!(widgets[1].value, "¥118.00");
        assert_eq!(widgets[2].value, "¥10.47");
        assert_eq!(widgets[0].color.as_deref(), Some("#FF8800"));

        let mut b = bal();
        b.granted = None;
        let widgets = render_widgets(&preset, Some(&b), &BTreeMap::new(), &no_ep());
        assert_eq!(widgets.len(), 2);
    }

    #[test]
    fn renders_percent_bar_from_extras() {
        let preset = demo_preset(vec![WidgetSpec {
            kind: "bar".into(),
            label: Some("已用额度".into()),
            value: Some("{extras.usedCost:money}".into()),
            hint: None,
            percent: Some("extras.usedPercent".into()),
            series: None,
            series_labels: None,
            color: None,
            hide_empty: false,
            group: None,
            columns: None,
            rows: None,
            from: None,
            row_name: None,
            row_cells: None,
            colors: None,
        }]);
        let extras = BTreeMap::from([
            ("usedCost".to_string(), json!(2.5)),
            ("usedPercent".to_string(), json!(25.0)),
        ]);
        let widgets = render_widgets(&preset, Some(&bal()), &extras, &no_ep());
        assert_eq!(widgets.len(), 1);
        assert_eq!(widgets[0].kind, "bar");
        assert_eq!(widgets[0].percent, Some(25.0));
        assert_eq!(widgets[0].value, "¥2.50");
    }

    #[test]
    fn renders_bars_series_from_endpoint() {
        let preset = demo_preset(vec![WidgetSpec {
            kind: "bars".into(),
            label: Some("近7日消费".into()),
            value: Some("{usage.total7d:money}".into()),
            hint: None,
            percent: None,
            series: Some("usage.costSeries".into()),
            series_labels: None,
            color: None,
            hide_empty: false,
            group: None,
            columns: None,
            rows: None,
            from: None,
            row_name: None,
            row_cells: None,
            colors: None,
        }]);
        let endpoints = BTreeMap::from([(
            "usage".to_string(),
            json!({"costSeries": [1.0, 2.0, 0.5], "total7d": 3.5}),
        )]);
        let widgets = render_widgets(&preset, Some(&bal()), &BTreeMap::new(), &endpoints);
        assert_eq!(widgets.len(), 1);
        assert_eq!(widgets[0].kind, "bars");
        assert_eq!(widgets[0].value, "¥3.50");
        assert_eq!(widgets[0].series.as_deref(), Some(&[1.0, 2.0, 0.5][..]));
        assert_eq!(widgets[0].color.as_deref(), Some("#FF8800"));

        // 序列缺失 → 整块跳过
        let widgets = render_widgets(&preset, Some(&bal()), &BTreeMap::new(), &no_ep());
        assert_eq!(widgets.len(), 0);
    }

    #[test]
    fn unresolved_refs_become_dash() {
        let preset = demo_preset(vec![WidgetSpec {
            kind: "kv".into(),
            label: Some("x".into()),
            value: Some("{extras.missing:money}".into()),
            hint: None,
            percent: None,
            series: None,
            series_labels: None,
            color: None,
            hide_empty: false,
            group: None,
            columns: None,
            rows: None,
            from: None,
            row_name: None,
            row_cells: None,
            colors: None,
        }]);
        let widgets = render_widgets(&preset, Some(&bal()), &BTreeMap::new(), &no_ep());
        assert_eq!(widgets[0].value, "--");
    }

    #[test]
    fn currency_symbols() {
        assert_eq!(currency_symbol("CNY"), "¥");
        assert_eq!(currency_symbol("USD"), "$");
        assert_eq!(currency_symbol("XXX"), "");
    }

    #[test]
    fn renders_table_with_group() {
        let preset = demo_preset(vec![WidgetSpec {
            kind: "table".into(),
            label: Some("价格".into()),
            value: None,
            hint: None,
            percent: None,
            series: None,
            series_labels: None,
            color: None,
            hide_empty: false,
            group: Some("pricing".into()),
            columns: Some(vec!["高峰".into(), "空闲".into()]),
            rows: Some(vec![crate::preset::TableRowSpec {
                name: "命中".into(),
                cells: vec!["{extras.hit:number}".into(), "0.05".into()],
            }]),
            from: None,
            row_name: None,
            row_cells: None,
            colors: None,
        }]);
        let extras = BTreeMap::from([("hit".to_string(), json!(9.0))]);
        let widgets = render_widgets(&preset, Some(&bal()), &extras, &no_ep());
        assert_eq!(widgets.len(), 1);
        let w = &widgets[0];
        assert_eq!(w.kind, "table");
        assert_eq!(w.group.as_deref(), Some("pricing"));
        assert_eq!(w.table_columns.as_deref(), Some(&["高峰".to_string(), "空闲".to_string()][..]));
        let rows = w.table_rows.as_ref().unwrap();
        assert_eq!(rows[0].name, "命中");
        assert_eq!(rows[0].cells[0], "9");
        assert_eq!(rows[0].cells[1], "0.05");
    }

    #[test]
    fn renders_table_from_dynamic_rows() {
        let preset = demo_preset(vec![WidgetSpec {
            kind: "table".into(),
            label: Some("模型用量".into()),
            value: None,
            hint: None,
            percent: None,
            series: None,
            series_labels: None,
            color: None,
            hide_empty: false,
            group: None,
            columns: Some(vec!["成本".into(), "输出".into()]),
            rows: None,
            from: Some("usage.models".into()),
            row_name: None, // 默认 {item.name}
            row_cells: Some(vec!["{item.cost:money}".into(), "{item.tokens:compact}".into()]),
            colors: None,
        }]);
        let endpoints = BTreeMap::from([(
            "usage".to_string(),
            json!({"models": [
                { "name": "Chat", "cost": 4.12, "tokens": 8200000 },
                { "name": "Reasoner", "cost": 1.55, "tokens": 1100000 }
            ]}),
        )]);
        let widgets = render_widgets(&preset, Some(&bal()), &BTreeMap::new(), &endpoints);
        assert_eq!(widgets.len(), 1);
        let rows = widgets[0].table_rows.as_ref().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "Chat");
        assert_eq!(rows[0].cells[0], "¥4.12");
        assert_eq!(rows[0].cells[1], "8.2M");
        assert_eq!(rows[1].name, "Reasoner");
        assert_eq!(rows[1].cells[1], "1.1M");

        // 数据源缺失 → 整块隐藏
        let widgets = render_widgets(&preset, Some(&bal()), &BTreeMap::new(), &no_ep());
        assert_eq!(widgets.len(), 0);
    }
    #[test]
    fn dynamic_table_survives_endpoint_normalize() {
        // 回归:fetcher 产出的 models 必须经 endpoint extract 白名单进入端点命名空间,
        // 否则 table from=usage.models 取不到数据被整块隐藏(deepseek 预设曾漏配 extract)。
        let json = r#"{
            "schemaVersion": 2,
            "id": "demo",
            "name": "Demo",
            "auth": { "type": "bearer", "header": "Authorization" },
            "form": { "fields": [ { "id": "apiKey", "label": "API Key" } ] },
            "balance": {
                "method": "GET",
                "url": "https://example.com/b",
                "extract": { "total": { "path": "total" } }
            },
            "endpoints": {
                "usage": {
                    "fetcher": "demo-fetcher",
                    "extract": {
                        "costSeries": { "path": "costSeries" },
                        "models": { "path": "models" }
                    }
                }
            },
            "template": { "widgets": [
                { "type": "table", "label": "模型用量", "columns": ["成本", "输出"],
                  "from": "usage.models",
                  "rowName": "{item.name}",
                  "rowCells": ["{item.cost:money}", "{item.tokens:compact}"] }
            ] }
        }"#;
        let preset: Preset = serde_json::from_str(json).unwrap();
        crate::preset::validate(&preset).unwrap();
        // fetcher 形产出(extract 之前)
        let body = serde_json::json!({
            "costSeries": [1.0, 2.0],
            "models": [
                { "name": "Chat", "cost": 4.12, "tokens": 8200000 },
                { "name": "Reasoner", "cost": 1.55, "tokens": 1100000 }
            ]
        });
        let ep = preset.endpoints.get("usage").unwrap();
        let map = crate::normalize::normalize_endpoint(
            "usage",
            ep,
            &body,
            None,
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap()
        .unwrap();
        assert!(map.contains_key("models"), "extract 必须保留 models");
        let endpoints = BTreeMap::from([("usage".to_string(), serde_json::to_value(map).unwrap())]);
        let widgets = render_widgets(&preset, Some(&bal()), &BTreeMap::new(), &endpoints);
        let w = widgets.iter().find(|w| w.kind == "table").expect("table 应渲染");
        let rows = w.table_rows.as_ref().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "Chat");
        assert_eq!(rows[0].cells[0], "¥4.12");
        assert_eq!(rows[1].name, "Reasoner");
        assert_eq!(rows[1].cells[1], "1.1M");
    }

    #[test]
    fn mimo_preset_end_to_end_render() {
        // mimo 连通性预设全链路:真实 /v1/models 响应 → balance 归一化(computed.total 兜底)
        // → models 端点数组提取 → 模板渲染(kv×2 + 动态表格 + note)。
        // 回归目标:任意一步失败(如数组未进入端点命名空间)都会让该供应商整页只剩 note。
        let preset: Preset =
            serde_json::from_str(include_str!("../../presets/mimo.json")).unwrap();
        crate::preset::validate(&preset).unwrap();
        let body = serde_json::json!({
            "object": "list",
            "data": [
                { "id": "mimo-v2.5", "object": "model", "owned_by": "xiaomi" },
                { "id": "mimo-v2.5-pro", "object": "model", "owned_by": "xiaomi" },
                { "id": "mimo-v2.5-asr", "object": "model", "owned_by": "xiaomi" },
                { "id": "mimo-v2.5-tts", "object": "model", "owned_by": "xiaomi" },
                { "id": "mimo-v2.5-tts-voiceclone", "object": "model", "owned_by": "xiaomi" },
                { "id": "mimo-v2.5-tts-voicedesign", "object": "model", "owned_by": "xiaomi" }
            ]
        });
        let (balance, extras) = match crate::normalize::normalize_response(&preset, &body, 1_000)
            .unwrap()
        {
            crate::normalize::Normalized::Available(b, e) => (b, e),
            crate::normalize::Normalized::Unavailable => panic!("mimo 不应判定不可用"),
        };
        assert_eq!(balance.total, 0.0);
        let ep = preset.endpoints.get("models").unwrap();
        let map = crate::normalize::normalize_endpoint(
            "models",
            ep,
            &body,
            Some(&balance),
            &extras,
            &BTreeMap::new(),
        )
        .unwrap()
        .unwrap();
        assert!(map.contains_key("list"), "端点 extract 必须保留 data 数组");
        let endpoints = BTreeMap::from([("models".to_string(), serde_json::to_value(map).unwrap())]);
        let widgets = render_widgets(&preset, Some(&balance), &extras, &endpoints);
        let row = |mes| {
            serde_json::json!({
                "v": 2, "generatedAt": 1_000, "freshness": "current",
                "providers": [{ "id": "mimo", "name": "Xiaomi MiMo", "accent": "#FF6900",
                    "status": "ok", "statusText": "已连接", "balance": null, "widgets": mes }],
            }).to_string()
        };
        println!("=== mimo snapshot JSON ===
{}", row(
            widgets.iter().map(|w| serde_json::to_value(w).unwrap()).collect::<Vec<_>>()
        ));
        assert_eq!(widgets.len(), 4);
        assert_eq!(widgets[0].kind, "kv");
        assert_eq!(widgets[0].value, "OpenAI 兼容 /v1");
        assert_eq!(widgets[1].kind, "kv");
        assert_eq!(widgets[1].value, "未开放");
        let w = widgets.iter().find(|w| w.kind == "table").expect("table 应渲染");
        let rows = w.table_rows.as_ref().unwrap();
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[0].name, "mimo-v2.5");
        assert_eq!(rows[0].cells[0], "xiaomi");
        assert_eq!(rows[5].name, "mimo-v2.5-tts-voicedesign");
        let note = widgets.iter().find(|w| w.kind == "note").expect("note 应渲染");
        assert!(note.value.contains("余额"));
    }
}
