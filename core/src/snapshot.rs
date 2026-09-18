//! 快照契约 v2:多供应商 + 通用组件树(widgets)。
//!
//! 手环端 vela 快应用只依赖这份通用结构渲染,不感知任何具体供应商。

use serde::{Deserialize, Serialize};

use crate::state::{App, ProviderData};
use crate::template;
use crate::{dates, preset};

pub const SNAPSHOT_V: u32 = 2;
/// 新鲜度阈值(秒):余额检查时间距今不超过该值视为 current
pub const FRESH_SECS: i64 = 300;
/// 单个快照最大供应商数量(超出按 id 排序截断)
pub const MAX_PROVIDERS: usize = 12;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceInfo {
    pub total: f64,
    pub top_up: Option<f64>,
    pub granted: Option<f64>,
    pub currency: String,
    /// Unix 秒,用于新鲜度判定
    pub checked_at: i64,
}

/// 表格行(竖表头 + ≤3 数据格)。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TableRow {
    /// 竖表头单元格
    pub name: String,
    pub cells: Vec<String>,
}

/// 通用组件:由预设模板渲染而来,手环端按 type 渲染。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Widget {
    /// balance | kv | bar | ring | ringshare | ringseg | bars | stack | hbar | table | note
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// 展示文本(插件端已按 format 渲染为最终字符串)
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// bar/ring 百分比 0-100
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<f64>,
    /// bars/stack 数值序列(手环端归一化渲染)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series: Option<Vec<f64>>,
    /// bars 序列标签(如逐日 MM/DD;手环端显示首/中/末锚点)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_labels: Option<Vec<String>>,
    /// 所属区块组(同组相邻组件之间不画分隔线)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// table 顶部列头(可选,≤3 列)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table_columns: Option<Vec<String>>,
    /// table 行数据(每行竖表头 + ≤3 数据格)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table_rows: Option<Vec<TableRow>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// 分段/占比组件逐项颜色(ringseg/ringshare 段色;缺省由手环端按 accent 阶梯)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colors: Option<Vec<String>>,
    /// ringshare 取值语义:true = series 是 0-100 的比率(每项弧长 = 该值本身,不做占比归一化)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<bool>,
}

impl Widget {
    pub fn note(value: impl Into<String>) -> Self {
        Widget {
            kind: "note".into(),
            label: None,
            value: value.into(),
            hint: None,
            percent: None,
            series: None,
            series_labels: None,
            group: None,
            table_columns: None,
            table_rows: None,
            color: None,
            colors: None,
            rate: None,
        }
    }
}

/// 供应商视图:preset 的运行时实例(启用的供应商才会出现)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub accent: String,
    /// ok | stale | error | unconfigured
    pub status: String,
    /// 状态描述(不含相对时间,避免推送签名抖动;相对时间由手环端按时间字段计算)
    pub status_text: String,
    pub balance: Option<BalanceInfo>,
    pub widgets: Vec<Widget>,
    /// 预设 template.band 透传的手环显示规则(手环端三层合并的中间层)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub v: u32,
    pub generated_at: i64,
    /// current | cached | unavailable(整体新鲜度,手环端状态胶囊)
    pub freshness: String,
    pub providers: Vec<ProviderView>,
}

fn fmt_ago(secs: i64) -> String {
    if secs < 60 {
        format!("{secs} 秒前")
    } else if secs < 3600 {
        format!("{} 分钟前", secs / 60)
    } else if secs < 86_400 {
        format!("{} 小时前", secs / 3600)
    } else {
        format!("{} 天前", secs / 86_400)
    }
}

/// 构造快照:遍历启用的预设,组合持久化数据与模板。
pub fn build_snapshot(app: &App) -> Snapshot {
    let now = dates::unix_now();
    let mut providers: Vec<ProviderView> = Vec::new();
    let mut any_fresh = false;
    let mut any_data = false;

    for preset in &app.presets {
        if !app.is_enabled(&preset.id) {
            continue;
        }
        let data = app.data.providers.get(&preset.id);
        let key_configured = app
            .api_key(&preset.id)
            .map(|k| !k.is_empty())
            .unwrap_or(false);

        let view = provider_view(preset, key_configured, data, now);
        if view.balance.is_some() {
            any_data = true;
            if let Some(b) = &view.balance {
                if now - b.checked_at <= FRESH_SECS {
                    any_fresh = true;
                }
            }
        }
        providers.push(view);
    }

    providers.truncate(MAX_PROVIDERS);

    let freshness = if any_fresh {
        "current"
    } else if any_data {
        "cached"
    } else {
        "unavailable"
    };

    Snapshot {
        v: SNAPSHOT_V,
        generated_at: now,
        freshness: freshness.into(),
        providers,
    }
}

fn provider_view(
    preset: &preset::Preset,
    key_configured: bool,
    data: Option<&ProviderData>,
    now: i64,
) -> ProviderView {
    let mut view = ProviderView {
        id: preset.id.clone(),
        name: preset.name.clone(),
        accent: preset.accent.clone(),
        status: "unconfigured".into(),
        status_text: String::new(),
        balance: None,
        widgets: Vec::new(),
        display: preset.template.band.clone(),
    };

    if !key_configured {
        view.status = "unconfigured".into();
        view.status_text = "未配置 API Key".into();
        view.widgets = vec![Widget::note("未配置 API Key\n请在 AstroBox 插件页面设置")];
        return view;
    }
    let Some(d) = data else {
        view.status = "unconfigured".into();
        view.status_text = "等待首次查询".into();
        view.widgets = vec![Widget::note("等待首次查询")];
        return view;
    };

    if let Some(b) = &d.balance {
        let age = now - b.checked_at;
        if age <= FRESH_SECS {
            view.status = "ok".into();
            view.status_text = format!("已连接 · {}", fmt_ago(age));
        } else {
            view.status = "stale".into();
            view.status_text = format!("缓存 · {}", fmt_ago(age));
        }
        view.balance = Some(b.clone());
        view.widgets = template::render_widgets(preset, Some(b), &d.extras, &d.endpoints);
    } else if let Some(err) = &d.last_error {
        view.status = "error".into();
        view.status_text = "获取失败".into();
        view.widgets = vec![Widget::note(format!("获取失败\n{err}"))];
    } else {
        view.status = "unconfigured".into();
        view.status_text = "尚未获取到数据".into();
        view.widgets = vec![Widget::note("尚未获取到数据")];
    }
    view
}

/// 推送变化检测用的稳定签名:排除 generatedAt / 相对时间文案。
/// 状态翻转(ok→stale)、余额变化、模板渲染结果变化都会触发推送。
pub fn stable_signature(snap: &Snapshot) -> String {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct StableProvider<'a> {
        id: &'a str,
        status: &'a str,
        balance: Option<&'a BalanceInfo>,
        widgets: &'a [Widget],
        display: Option<&'a serde_json::Value>,
    }

    let provs: Vec<StableProvider> = snap
        .providers
        .iter()
        .map(|p| StableProvider {
            id: &p.id,
            status: &p.status,
            balance: p.balance.as_ref(),
            widgets: &p.widgets,
            display: p.display.as_ref(),
        })
        .collect();

    serde_json::to_string(&provs).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::{
        AuthSpec, BalanceExtract, BalanceSpec, ExtractSpec, FormSpec, PathSpec, Preset,
        TemplateSpec, WidgetSpec,
    };

    fn test_preset(id: &str) -> preset::Preset {
        Preset {
            schema_version: preset::SCHEMA_VERSION,
            id: id.into(),
            name: format!("Test {id}"),
            homepage: None,
            accent: "#4D6BFE".into(),
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
                url: "https://example.com/balance".into(),
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
                widgets: vec![WidgetSpec {
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
                    rate: None,
                }],
                band: None,
            },
        }
    }

    #[test]
    fn signature_ignores_generated_at_but_not_data() {
        let mut app = App::default();
        app.presets = vec![test_preset("demo")];
        app.settings.enabled.insert("demo".into(), true);
        app.settings.api_keys.insert("demo".into(), "sk-x".into());

        let snap = build_snapshot(&app);
        assert_eq!(snap.v, SNAPSHOT_V);
        assert_eq!(snap.providers[0].status, "unconfigured"); // 尚无数据
        let sig = stable_signature(&snap);

        // 只有 generatedAt 变化 → 签名不变
        let mut s2 = snap.clone();
        s2.generated_at += 100;
        assert_eq!(stable_signature(&s2), sig);

        // 注入余额数据 → 签名变化
        app.data.providers.entry("demo".into()).or_default().balance = Some(BalanceInfo {
            total: 12.0,
            top_up: None,
            granted: None,
            currency: "CNY".into(),
            checked_at: dates::unix_now(),
        });
        let snap2 = build_snapshot(&app);
        assert_ne!(stable_signature(&snap2), sig);
        assert_eq!(snap2.providers[0].status, "ok");
        assert!(snap2.providers[0].widgets.iter().any(|w| w.kind == "balance"));
    }

    #[test]
    fn disabled_providers_are_excluded() {
        let mut app = App::default();
        app.presets = vec![test_preset("a"), test_preset("b")];
        app.settings.enabled.insert("a".into(), true);
        let snap = build_snapshot(&app);
        assert_eq!(snap.providers.len(), 1);
        assert_eq!(snap.providers[0].id, "a");
        assert_eq!(snap.freshness, "unavailable");
    }
}