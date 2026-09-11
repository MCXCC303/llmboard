//! 插件 UI:分类表单设置页 + 设备页卡片。
//!
//! 分类表单:顶部下拉框(SELECT,ui 接口原生元素)选择供应商种类,
//! 下方只渲染选中供应商的预设表单(启用开关 + form.fields 凭据输入)。
//! 下拉/表单均由预设驱动:新增供应商预设即可在 UI 中出现对应分类与表单。

use crate::astrobox::psys_host::ui::{self, ElementType, Event, FlexDirection};

use llmboard_core::template::currency_symbol;
use llmboard_core::{dates, state};

use crate::engine;

// 全局动作/输入 id
pub const BTN_SAVE_SETTINGS: &str = "btn-save-settings";
pub const BTN_POLL_NOW: &str = "btn-poll-now";
pub const BTN_PUSH_NOW: &str = "btn-push-now";
pub const BTN_HOUSEKEEPING: &str = "btn-housekeeping";
pub const BTN_BACKUP: &str = "btn-backup";
pub const BTN_RESTORE: &str = "btn-restore";
pub const INPUT_BALANCE_INTERVAL: &str = "input-balance-interval";
pub const INPUT_PUSH_INTERVAL: &str = "input-push-interval";
/// 供应商分类下拉框 id(CHANGE 事件)
pub const SELECT_PROVIDER: &str = "select-provider";
/// 供应商开关按钮 id 前缀(tgl-<preset-id>)
pub const TGL_PREFIX: &str = "tgl-";
/// 凭据输入 id 前缀(cred-<preset-id>--<field-id>)
pub const CRED_PREFIX: &str = "cred-";

fn p(text: &str) -> ui::Element {
    ui::Element::new(ElementType::P, Some(text))
}

/// 输入框上方提示文本:比正文更小的字号。
fn hint(text: &str) -> ui::Element {
    p(text).size(12).margin(2)
}

/// 大按钮配色:橙色=配置/持久化,绿色=数据拉取,浅蓝=设备动作。
fn button_style(event_id: &str) -> (&'static str, &'static str) {
    match event_id {
        BTN_SAVE_SETTINGS | BTN_BACKUP | BTN_RESTORE => ("#F97316", "#FFFFFF"),
        BTN_POLL_NOW => ("#22C55E", "#062D1A"),
        BTN_PUSH_NOW | BTN_HOUSEKEEPING => ("#38BDF8", "#082F49"),
        _ => ("#11182C", "#FFFFFF"),
    }
}

fn btn(label: &str, event_id: &str) -> ui::Element {
    let (bg, fg) = button_style(event_id);
    ui::Element::new(ElementType::Button, Some(label))
        .padding(10)
        .margin(4)
        .radius(10)
        .bg(bg)
        .text_color(fg)
        .width_full()
        .on(Event::Click, event_id)
}

fn input(event_id: &str, value: &str) -> ui::Element {
    ui::Element::new(ElementType::Input, Some(value))
        .padding(8)
        .radius(8)
        .border(1, "#3A4A6B")
        .width_full()
        .on(Event::Input, event_id)
}

fn fmt_ago(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

fn fmt_epoch(epoch: i64, tz_offset_min: i32) -> String {
    let local = epoch + tz_offset_min as i64 * 60;
    let h = local.div_euclid(3600).rem_euclid(24);
    let m = local.div_euclid(60).rem_euclid(60);
    let s = local.rem_euclid(60);
    format!("{h:02}:{m:02}:{s:02}")
}

// ============================== 数据行快照 ==============================

struct FieldRow {
    id: String,
    label: String,
    hint: String,
    secret: bool,
    value: String,
}

struct ProviderRow {
    id: String,
    name: String,
    accent: String,
    enabled: bool,
    fields: Vec<FieldRow>,
    status_line: String,
}

/// 预设表单字段:preset.form.fields 为空时回退到单个 apiKey 字段。
fn preset_fields(preset: &llmboard_core::preset::Preset) -> Vec<(String, String, String, bool)> {
    if !preset.form.fields.is_empty() {
        return preset
            .form
            .fields
            .iter()
            .map(|f| (f.id.clone(), f.label.clone(), f.hint.clone(), f.secret))
            .collect();
    }
    vec![(
        "apiKey".to_string(),
        preset.auth.key_label.clone(),
        preset.auth.key_hint.clone(),
        true,
    )]
}

/// 单次取锁收集所有渲染需要的数据(避免渲染路径上的递归加锁)。
fn collect_rows() -> Vec<ProviderRow> {
    let a = state::lock();
    let now = dates::unix_now();
    a.presets
        .iter()
        .map(|preset| {
            let enabled = a.is_enabled(&preset.id);
            let fields = preset_fields(preset)
                .into_iter()
                .map(|(fid, label, hint, secret)| FieldRow {
                    value: a.credential(&preset.id, &fid).unwrap_or_default(),
                    id: fid,
                    label,
                    hint,
                    secret,
                })
                .collect::<Vec<_>>();
            let api_key = a.api_key(&preset.id).unwrap_or_default();
            let status_line = if !enabled {
                "未启用".to_string()
            } else if api_key.is_empty() {
                "未配置 API Key".to_string()
            } else {
                match a.data.providers.get(&preset.id) {
                    Some(d) if d.balance.is_some() => {
                        let b = d.balance.as_ref().unwrap();
                        let sym = currency_symbol(&b.currency);
                        let ago = now - b.checked_at;
                        let ep = d.endpoints.len();
                        format!(
                            "余额 {sym}{:.2} {} · {} 前 · {} 端点",
                            b.total,
                            b.currency,
                            fmt_ago(ago.max(0)),
                            ep
                        )
                    }
                    Some(d) if d.last_error.is_some() => {
                        format!("错误: {}", d.last_error.as_deref().unwrap_or(""))
                    }
                    Some(_) => "已配置,等待首次查询".to_string(),
                    None => "已配置,等待首次查询".to_string(),
                }
            };
            ProviderRow {
                id: preset.id.clone(),
                name: preset.name.clone(),
                accent: preset.accent.clone(),
                enabled,
                fields,
                status_line,
            }
        })
        .collect()
}

fn selected_row<'a>(rows: &'a [ProviderRow], settings: &state::Settings) -> Option<&'a ProviderRow> {
    if let Some(sel) = settings.selected_provider.as_deref() {
        if let Some(r) = rows.iter().find(|r| r.id == sel) {
            return Some(r);
        }
    }
    rows.first()
}

// ============================== 卡片 ==============================

/// 设备详情页看板卡片(on_card_render)。
pub fn render_card(card_id: &str) {
    state::lock().card_element_id = Some(card_id.to_string());
    let root = build_card();
    ui::render(card_id, root);
}

fn build_card() -> ui::Element {
    let rows = collect_rows();
    let (connected, status_text) = {
        let a = state::lock();
        (a.device_addr.is_some(), a.status.clone())
    };

    let (badge_text, badge_bg, badge_fg) = if connected {
        ("已连接 · 自动推送中", "#163B2C", "#87E9C6")
    } else {
        ("未连接设备", "#3B2A2A", "#F0B7B7")
    };
    let badge = ui::Element::new(ElementType::P, Some(badge_text))
        .padding(6)
        .radius(999)
        .bg(badge_bg)
        .text_color(badge_fg);

    let mut root = ui::Element::new(ElementType::Div, None)
        .flex()
        .flex_direction(FlexDirection::Column)
        .padding(12)
        .child(badge);

    let active: Vec<&ProviderRow> = rows.iter().filter(|r| r.enabled).collect();
    if active.is_empty() {
        root = root.child(p("未启用任何供应商(打开插件页面选择并配置)"));
    } else {
        for r in active {
            root = root.child(p(&format!("{} · {}", r.name, r.status_line)));
        }
    }

    root.child(p(&status_text))
        .child(btn("立即轮询", BTN_POLL_NOW))
        .child(btn("立即推送", BTN_PUSH_NOW))
}

// ============================== 设置页(分类表单) ==============================

/// 插件页面设置表单(on_ui_render)。
pub fn render_page(element_id: &str) {
    state::lock().page_element_id = Some(element_id.to_string());
    let root = build_page();
    ui::render(element_id, root);
}

fn build_page() -> ui::Element {
    let rows = collect_rows();
    let (balance_interval, push_interval, status_line, selected_id) = {
        let a = state::lock();
        let tz = a.tz_offset_min;
        let line = format!(
            "状态: {} ({})",
            a.status,
            if a.status_at == 0 {
                "-".to_string()
            } else {
                fmt_epoch(a.status_at, tz)
            }
        );
        (
            a.settings.balance_interval_secs,
            a.settings.push_interval_secs,
            line,
            selected_row(&rows, &a.settings).map(|r| r.id.clone()),
        )
    };

    // 1) 供应商分类下拉框
    // 旧版 ui 的 OPTION 无选中态设置能力(无 prop/selected/value),宿主重绘后默认
    // 显示第一个 option。把当前选中的供应商置顶,使下拉框显示与表单保持一致;
    // 其余选项保持原顺序(id 排序,稳定排序)。
    let mut select = ui::Element::new(ElementType::Select, None)
        .width_full()
        .margin(4)
        .on(Event::Change, SELECT_PROVIDER);
    let mut ordered: Vec<&ProviderRow> = rows.iter().collect();
    ordered.sort_by_key(|r| !(Some(&r.id) == selected_id.as_ref()));
    for r in &ordered {
        // content 参数是 Option<&str>:选项内容有界且少量,泄漏换取生命周期
        let label: &'static str = Box::leak(format!("{} · {}", r.id, r.name).into_boxed_str());
        select = select.child(ui::Element::new(ElementType::Option, Some(label)));
    }

    let mut root = ui::Element::new(ElementType::Div, None)
        .flex()
        .flex_direction(FlexDirection::Column)
        .padding(16)
        .child(hint(&format!(
            "LLMBand · 预设驱动多供应商用量看板(共 {} 个预设)",
            rows.len()
        )))
        .child(hint("选择供应商种类,填写对应预设表单。新增供应商只需向 presets/ 目录添加预设文件(见 docs/PRESETS.md)。"))
        .child(select);

    // 2) 选中供应商的表单(由预设 form.fields 驱动)
    if let Some(r) = rows.iter().find(|r| Some(&r.id) == selected_id.as_ref()) {
        let mut section = ui::Element::new(ElementType::Div, None)
            .flex()
            .flex_direction(FlexDirection::Column)
            .margin(4)
            .child(
                p(&format!("{} — {}", r.name, if r.enabled { "已启用" } else { "未启用" }))
                    .text_color(r.accent.as_str()),
            )
            .child(btn(
                &format!("{} {}", if r.enabled { "☑ 关闭" } else { "☐ 启用" }, r.name),
                &format!("{TGL_PREFIX}{}", r.id),
            ));
        for f in &r.fields {
            let mut h = format!("{}", f.label);
            if !f.hint.is_empty() {
                h.push_str(&format!("({})", f.hint));
            }
            if f.secret {
                h.push_str(" · 密钥字段");
            }
            section = section
                .child(hint(&h))
                .child(input(&format!("{CRED_PREFIX}{}--{}", r.id, f.id), &f.value));
        }
        section = section.child(p(&r.status_line));
        root = root.child(section);
    } else if !rows.is_empty() {
        root = root.child(p("未选择供应商"));
    }

    // 3) 全局设置与动作
    root = root
        .child(hint("余额轮询间隔(秒,默认 120):"))
        .child(input(INPUT_BALANCE_INTERVAL, &balance_interval.to_string()))
        .child(hint("推送间隔(秒,默认 60):"))
        .child(input(INPUT_PUSH_INTERVAL, &push_interval.to_string()))
        .child(btn("保存设置", BTN_SAVE_SETTINGS))
        .child(btn("立即轮询余额与端点", BTN_POLL_NOW))
        .child(btn("推送手环", BTN_PUSH_NOW))
        .child(btn("检测设备", BTN_HOUSEKEEPING))
        .child(hint(
            "跨版本持久化(宿主更新插件会清空插件目录,更新前备份、更新后恢复):",
        ))
        .child(btn("备份配置/数据", BTN_BACKUP))
        .child(btn("从备份恢复", BTN_RESTORE))
        .child(p(&status_line));
    root
}

/// 手动重绘页面与卡片(宿主不会自动刷新,动作执行后调用)。
fn rerender() {
    let (page, card) = {
        let a = state::lock();
        (a.page_element_id.clone(), a.card_element_id.clone())
    };
    if let Some(id) = page {
        ui::render(&id, build_page());
    }
    if let Some(id) = card {
        ui::render(&id, build_card());
    }
}

// ============================== 事件分发 ==============================

/// 宿主传回的事件载荷是 JSON(如 {"type":"input","value":"aaa","checked":false});
/// 取 value 字段作为输入框/下拉框实际内容。
fn payload_value(payload: &str) -> String {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .and_then(|v| v.get("value").and_then(|x| x.as_str()).map(String::from))
        .unwrap_or_else(|| payload.to_string())
}

/// 解析下拉选项 "id · name" → id。
fn parse_option_id(value: &str) -> String {
    value
        .split_once(" · ")
        .map(|(id, _)| id.trim().to_string())
        .unwrap_or_else(|| value.trim().to_string())
}

/// on_ui_event 分发。所有事件先打日志,便于确认事件是否送达。
pub async fn handle_ui_event(event_id: &str, event: &Event, payload: &str) {
    tracing::info!("[ui-event] id={event_id} event={event:?} payload={payload}");

    match event {
        Event::Input => {
            // 输入值即时写入内存状态(保存按钮落盘);输入过程不重绘,避免打断焦点。
            let value = payload_value(payload);
            let mut a = state::lock();
            if let Some(rest) = event_id.strip_prefix(CRED_PREFIX) {
                if let Some((id, field)) = rest.split_once("--") {
                    if a.preset_by_id(id).is_some() {
                        a.set_credential(id, field, value);
                    }
                }
            } else {
                match event_id {
                    INPUT_BALANCE_INTERVAL => {
                        if let Ok(v) = value.parse::<u64>() {
                            a.settings.balance_interval_secs = v.max(30);
                        }
                    }
                    INPUT_PUSH_INTERVAL => {
                        if let Ok(v) = value.parse::<u64>() {
                            a.settings.push_interval_secs = v.max(10);
                        }
                    }
                    _ => {}
                }
            }
            return;
        }
        Event::Change => {
            // 分类下拉框:切换选中供应商并重绘其表单
            if event_id == SELECT_PROVIDER {
                let id = parse_option_id(&payload_value(payload));
                let exists = { state::lock().preset_by_id(&id).is_some() };
                if exists {
                    state::lock().settings.selected_provider = Some(id.clone());
                    tracing::info!("[ui-event] 选择供应商 {id}");
                    rerender();
                }
            }
            return;
        }
        Event::Click => {
            if let Some(id) = event_id.strip_prefix(TGL_PREFIX) {
                let exists = { state::lock().preset_by_id(id).is_some() };
                if exists {
                    let enabled = {
                        let mut a = state::lock();
                        let e = a.settings.enabled.entry(id.to_string()).or_insert(false);
                        *e = !*e;
                        *e
                    };
                    tracing::info!("[ui-event] 供应商 {id} → {}", if enabled { "启用" } else { "关闭" });
                    rerender();
                    return;
                }
            }
            match event_id {
                BTN_SAVE_SETTINGS => {
                    state::save_settings();
                    engine::arm_timers().await;
                    state::set_status("设置已保存,定时器已更新");
                }
                BTN_POLL_NOW => engine::poll_all().await,
                BTN_PUSH_NOW => engine::push_now(true).await,
                BTN_HOUSEKEEPING => engine::refresh_device().await,
                BTN_BACKUP => engine::backup_to_file().await,
                BTN_RESTORE => engine::restore_from_file().await,
                other => {
                    tracing::warn!("[ui-event] 未识别的点击 id: {other}");
                    return;
                }
            }
        }
        _ => return,
    }

    // 动作执行完重绘,让状态行/卡片立即反映结果
    rerender();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_value_extracts_input_text() {
        let p = r#"{"type":"input","value":"sk-abc123","checked":false}"#;
        assert_eq!(payload_value(p), "sk-abc123");
    }

    #[test]
    fn payload_value_falls_back_to_raw() {
        assert_eq!(payload_value("plain-text"), "plain-text");
        assert_eq!(payload_value(r#"{"type":"click","clientX":255,"value":""}"#), "");
    }

    #[test]
    fn option_id_parsing() {
        assert_eq!(parse_option_id("deepseek · DeepSeek"), "deepseek");
        assert_eq!(parse_option_id("deepseek"), "deepseek");
    }

    #[test]
    fn prefix_strip() {
        assert_eq!("demo".strip_prefix(TGL_PREFIX), None);
        assert_eq!("tgl-demo".strip_prefix(TGL_PREFIX), Some("demo"));
        assert_eq!("cred-demo--apiKey".strip_prefix(CRED_PREFIX), Some("demo--apiKey"));
        assert_eq!("demo--apiKey".split_once("--"), Some(("demo", "apiKey")));
    }

    #[test]
    fn collect_rows_uses_default_api_key_field() {
        // 空 form 回退 apiKey 字段
        let preset = llmboard_core::preset::Preset {
            schema_version: llmboard_core::preset::SCHEMA_VERSION,
            id: "t".into(),
            name: "T".into(),
            homepage: None,
            accent: "#000000".into(),
            auth: llmboard_core::preset::AuthSpec {
                kind: "bearer".into(),
                header: "Authorization".into(),
                prefix: "Bearer ".into(),
                key_label: "Key".into(),
                key_hint: "hint".into(),
            },
            form: Default::default(),
            balance: llmboard_core::preset::BalanceSpec {
                method: "GET".into(),
                url: "https://example.com/b".into(),
                headers: Default::default(),
                body: None,
                timeout_secs: 10,
                extract: llmboard_core::preset::BalanceExtract {
                    total: Some(llmboard_core::preset::ExtractSpec {
                        path: llmboard_core::preset::PathSpec::One("total".into()),
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
            template: Default::default(),
        };
        let fields = preset_fields(&preset);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].0, "apiKey");
        assert_eq!(fields[0].1, "Key");
    }
}
