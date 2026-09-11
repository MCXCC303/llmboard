//! 引擎:定时器布防、事件分发、设备发现与互联注册、快照推送。
//!
//! 定时器 payload:
//! - "balance"     每 balance_interval_secs 轮询全部启用供应商的余额
//! - "push"        每 push_interval_secs 推送快照
//! - "housekeeping" 30s:刷新已连接设备、注册互联接收、刷新时区

use crate::astrobox::psys_host::{device, interconnect, register, thirdpartyapp, timer};

use crate::provider::{self, FetchOutcome};
use llmboard_core::state;
use llmboard_core::{dates, snapshot};

/// 设备详情页卡片 id(register-card 用)
pub const CARD_ID: &str = "llmboard-card";
/// 看板卡名
pub const CARD_NAME: &str = "LLMBand 用量";
/// 快应用互联消息短窗口去重(秒)
const INTERCONNECT_DEDUP_SECS: i64 = 3;

/// on_load 中调用(block_on):加载持久化与预设、注册卡片与互联接收、布防定时器。
pub async fn init() {
    state::lock().load_presets();
    state::init_from_disk();

    crate::capabilities::audit_boot(); // 预设 fetcher 引用 ↔ 注册表接线审计

    let off = crate::astrobox::psys_host::os::timezone_offset_minutes().await;
    {
        state::lock().tz_offset_min = off;
        tracing::info!("[init] 宿主时区偏移 {off} 分钟");
    }

    if register::register_card(register::CardType::Element, CARD_ID, CARD_NAME)
        .await
        .is_ok()
    {
        tracing::info!("[init] 已注册设备页卡片 {CARD_ID}");
    } else {
        tracing::warn!("[init] 注册卡片失败(可能未授权)");
    }

    refresh_device().await;
    arm_timers().await;

    let status = {
        let a = state::lock();
        match (&a.device_addr, a.recv_registered) {
            (None, _) => format!("插件已启动:已加载 {} 个预设,未连接设备", a.presets.len()),
            (Some(_), false) => format!(
                "插件已启动:已加载 {} 个预设;互联接收未注册成功(检查权限,housekeeping 会重试)",
                a.presets.len()
            ),
            (Some(_), true) => format!(
                "插件已启动:已加载 {} 个预设,等待手环快应用打开并发送消息",
                a.presets.len()
            ),
        }
    };
    state::set_status(&status);
}

/// 重新布防全部定时器(设置变更后调用)。
pub async fn arm_timers() {
    let (balance_ms, push_ms) = {
        let a = state::lock();
        (
            a.settings.balance_interval_secs.saturating_mul(1000),
            a.settings.push_interval_secs.saturating_mul(1000),
        )
    };

    // 清理旧定时器
    let old: Vec<u64> = state::lock().timer_ids.values().copied().collect();
    for id in old {
        timer::clear_timer(id).await;
    }

    let mut ids = std::collections::BTreeMap::new();
    ids.insert(
        "balance".to_string(),
        timer::set_interval(balance_ms, "balance").await,
    );
    ids.insert(
        "push".to_string(),
        timer::set_interval(push_ms, "push").await,
    );
    ids.insert(
        "housekeeping".to_string(),
        timer::set_interval(30_000, "housekeeping").await,
    );
    state::lock().timer_ids = ids;
    tracing::info!(
        "[timer] balance {}s · push {}s · housekeeping 30s",
        balance_ms / 1000,
        push_ms / 1000
    );
}

/// on_event 分发。
pub async fn handle_event(
    event_type: crate::exports::astrobox::psys_plugin::event::EventType,
    payload: &str,
) {
    use crate::exports::astrobox::psys_plugin::event::EventType;
    match event_type {
        EventType::Timer => {
            let which = serde_json::from_str::<serde_json::Value>(payload)
                .ok()
                .and_then(|v| {
                    v.get("payload")
                        .and_then(serde_json::Value::as_str)
                        .map(String::from)
                });
            match which.as_deref() {
                Some("balance") => poll_all().await,
                Some("push") => push_now(false).await,
                Some("housekeeping") => housekeeping().await,
                other => tracing::debug!("[timer] 未识别的定时器载荷: {other:?}"),
            }
        }
        EventType::InterconnectMessage => {
            // 手环侧快应用有任何活动(打开/请求状态/主动刷新)都立即回一版快照
            handle_interconnect_message(payload).await;
        }
        other => tracing::debug!("[event] {:?} len={}", other, payload.len()),
    }
}

/// 快应用互联消息应答:手环打开快应用/请求刷新时立即强推一版快照。
pub async fn handle_interconnect_message(payload: &str) {
    let now = dates::unix_now();
    let duplicate = {
        let a = state::lock();
        a.last_interconnect_at
            .map(|t| now.saturating_sub(t) < INTERCONNECT_DEDUP_SECS)
            .unwrap_or(false)
    };
    if duplicate {
        tracing::info!(
            "[interconnect] 忽略 {INTERCONNECT_DEDUP_SECS}s 内的重复快应用消息(len={})",
            payload.len()
        );
        return;
    }
    state::lock().last_interconnect_at = Some(now);

    tracing::info!(
        "[interconnect] 收到快应用消息(len={}) 立即强推快照",
        payload.len()
    );
    push_now(true).await;
}

/// 刷新已连接设备;设备变化时重新注册互联接收。
pub async fn refresh_device() {
    let devices = device::get_connected_device_list().await;
    let addr = devices.first().map(|d| d.addr.clone());
    let pkg = state::lock().settings.push_pkg.clone();

    let need_register = {
        let mut a = state::lock();
        if addr != a.device_addr {
            a.recv_registered = false;
            a.device_addr = addr.clone();
        }
        !a.recv_registered && addr.is_some()
    };

    match &addr {
        Some(a) => tracing::info!("[device] 已连接设备: {}({})", devices[0].name, a),
        None => tracing::debug!("[device] 无已连接设备"),
    }

    if need_register {
        if let Some(a) = &addr {
            match register::register_interconnect_recv(a, &pkg).await {
                Ok(()) => {
                    state::lock().recv_registered = true;
                    tracing::info!("[interconnect] 已注册互联接收: {a} {pkg}");
                }
                Err(()) => {
                    tracing::warn!(
                        "[interconnect] 注册互联接收失败(检查 register_interconnect_recv 权限)"
                    );
                }
            }
        }
    }
}

async fn housekeeping() {
    refresh_device().await;
    let off = crate::astrobox::psys_host::os::timezone_offset_minutes().await;
    state::lock().tz_offset_min = off;
}

/// 轮询全部启用的供应商(串行,避免并发打爆资源受限宿主):
/// 先查余额,再按名称顺序抓取附加端点(端点 computed 可引用排在前面的端点)。
pub async fn poll_all() {
    // 工作清单:启用的供应商 + 已填写的表单凭据(克隆,避免持锁跨越 await)
    let jobs: Vec<(String, std::collections::BTreeMap<String, String>)> = {
        let a = state::lock();
        a.presets
            .iter()
            .filter(|p| a.is_enabled(&p.id))
            .filter_map(|p| {
                let fields: std::collections::BTreeMap<String, String> = p
                    .form
                    .fields
                    .iter()
                    .filter_map(|f| {
                        a.credential(&p.id, &f.id).map(|v| (f.id.clone(), v))
                    })
                    .collect();
                if fields.is_empty() {
                    None
                } else {
                    Some((p.id.clone(), fields))
                }
            })
            .collect()
    };
    if jobs.is_empty() {
        state::set_status("未启用任何供应商或未配置凭据(插件页面选择供应商并保存)");
        return;
    }

    let now = dates::unix_now();
    let mut ok = 0usize;
    let mut unavail = 0usize;
    let mut fail = 0usize;
    let mut ep_ok = 0usize;
    let mut ep_fail = 0usize;
    for (id, fields) in jobs {
        let preset = { state::lock().preset_by_id(&id).cloned() };
        let Some(preset) = preset else { continue };
        let preset_id = preset.id.clone();
        let preset_name = preset.name.clone();

        // 1) 余额(主凭据 apiKey)
        if let Some(key) = fields.get("apiKey") {
            match provider::fetch_balance(&preset, key, now) {
                Ok(FetchOutcome::Available(info, extras)) => {
                    {
                        let mut a = state::lock();
                        let d = a.data.providers.entry(id.clone()).or_default();
                        d.balance = Some(info);
                        d.extras = extras;
                        d.last_ok_at = Some(now);
                        d.last_error = None;
                        d.error_at = None;
                    }
                    ok += 1;
                    tracing::info!("[poll] {preset_id} 余额更新成功");
                }
                Ok(FetchOutcome::Unavailable) => {
                    {
                        let mut a = state::lock();
                        let d = a.data.providers.entry(id.clone()).or_default();
                        d.last_error = Some("接口返回余额不可用(欠费/冻结/无可用额度)".into());
                        d.error_at = Some(now);
                    }
                    unavail += 1;
                    tracing::warn!("[poll] {preset_id} 余额不可用");
                }
                Err(e) => {
                    let msg = e.to_string();
                    tracing::warn!("[poll] {preset_name}({preset_id}) 余额失败: {msg}");
                    {
                        let mut a = state::lock();
                        let d = a.data.providers.entry(id.clone()).or_default();
                        d.last_error = Some(msg);
                        d.error_at = Some(now);
                    }
                    fail += 1;
                }
            }
        }

        // 2) 附加端点(按名排序;失败只影响该端点,不阻断余额与其他端点)
        let (balance_snap, extras_snap) = {
            let a = state::lock();
            (
                a.data.providers.get(&id).and_then(|d| d.balance.clone()),
                a.data
                    .providers
                    .get(&id)
                    .map(|d| d.extras.clone())
                    .unwrap_or_default(),
            )
        };
        let mut acc: std::collections::BTreeMap<String, serde_json::Value> =
            std::collections::BTreeMap::new();
        for (name, ep) in &preset.endpoints {
            let Some(cred) = fields.get(&ep.auth_field) else {
                tracing::warn!(
                    "[poll] {preset_id} 端点 {name} 缺少凭据字段 {}",
                    ep.auth_field
                );
                continue;
            };
            match provider::fetch_endpoint(
                &preset,
                name,
                ep,
                cred,
                balance_snap.as_ref(),
                &extras_snap,
                &acc,
            ) {
                Ok(Some(map)) => {
                    acc.insert(name.clone(), serde_json::json!(map));
                    ep_ok += 1;
                    tracing::info!("[poll] {preset_id} 端点 {name} 更新成功");
                }
                Ok(None) => tracing::warn!("[poll] {preset_id} 端点 {name} 不可用"),
                Err(e) => {
                    tracing::warn!("[poll] {preset_name}({preset_id}) 端点 {name} 失败: {e}");
                    ep_fail += 1;
                }
            }
        }
        if !acc.is_empty() {
            let mut a = state::lock();
            let d = a.data.providers.entry(id.clone()).or_default();
            for (k, v) in acc {
                d.endpoints.insert(k, v);
            }
        }
    }
    state::save_data();
    state::set_status(&format!(
        "轮询完成: 余额 {ok} 成功 · {unavail} 不可用 · {fail} 失败 · 端点 {ep_ok} 成功 / {ep_fail} 失败"
    ));
}

/// 构建并推送快照到手环快应用。
pub async fn push_now(force: bool) {
    let (addr, pkg, json, signature) = {
        let a = state::lock();
        let snap = snapshot::build_snapshot(&a);
        let signature = snapshot::stable_signature(&snap);
        let json = serde_json::to_string(&snap).unwrap_or_else(|_| "{}".into());
        (
            a.device_addr.clone(),
            a.settings.push_pkg.clone(),
            json,
            signature,
        )
    };

    let Some(addr) = addr else {
        state::set_status("未连接设备,跳过推送(请先在 AstroBox 连接手环)");
        return;
    };

    // 定时推送做变化检测;手环打开应用/手动按钮走 force=true,始终回复。
    if !force {
        let unchanged = {
            let a = state::lock();
            a.last_pushed_device.as_deref() == Some(addr.as_str())
                && a.last_pushed_signature.as_deref() == Some(signature.as_str())
        };
        if unchanged {
            tracing::info!("[push] 快照无变化({} 字节),跳过推送", json.len());
            state::set_status(&format!("快照无变化,跳过推送({} 字节)", json.len()));
            return;
        }
    }

    // 推送前检测:确认手环端已安装目标快应用。
    match thirdpartyapp::get_thirdparty_app_list(&addr).await {
        Ok(apps) => match apps.iter().find(|app| app.package_name == pkg) {
            Some(app) => tracing::info!(
                "[precheck] 目标应用已安装: {} version_code={}",
                app.package_name,
                app.version_code
            ),
            None => {
                let installed: Vec<&str> = apps.iter().map(|a| a.package_name.as_str()).collect();
                tracing::warn!("[precheck] 设备 {addr} 未安装 {pkg};已安装快应用: {installed:?}");
                state::set_status(&format!(
                    "推送失败:手环未安装 {pkg},请先通过 AstroBox 安装 vela 快应用"
                ));
                return;
            }
        },
        Err(()) => {
            tracing::warn!("[precheck] 无法获取第三方应用列表,跳过检测继续推送");
        }
    }

    match interconnect::send_qaic_message(&addr, &pkg, &json).await {
        Ok(()) => {
            let t = dates::unix_now();
            {
                let mut a = state::lock();
                a.last_push_at = Some(t);
                a.last_pushed_signature = Some(signature);
                a.last_pushed_device = Some(addr.clone());
            }
            tracing::info!("[push] 已发送快照 {} 字节 → {addr} {pkg}", json.len());
            state::set_status(&format!("已推送快照 {} 字节 → {pkg}", json.len()));
        }
        Err(()) => {
            tracing::warn!("[push] send_qaic_message 失败: {addr} {pkg}");
            state::set_status("推送失败:设备不在线/未安装快应用/未授权 interconnect");
        }
    }
}

/// 把当前 settings + data 通过宿主保存对话框导出为备份 JSON。
pub async fn backup_to_file() {
    use crate::astrobox::psys_host::dialog;

    let bytes = match llmboard_core::backup::encode_backup() {
        Ok(b) => b,
        Err(e) => {
            state::set_status(&format!("备份失败: {e}"));
            return;
        }
    };

    let filter = dialog::FilterConfig {
        multiple: false,
        extensions: vec!["json".to_string()],
        default_directory: String::new(),
        default_file_name: format!("llmboard-backup-{}.json", dates::unix_now()),
    };
    let Ok(session) = dialog::save_file_start(&filter).await else {
        state::set_status("备份已取消或无法打开保存窗口");
        return;
    };

    for chunk in bytes.chunks(64 * 1024) {
        if let Err(()) = dialog::save_file_write_chunk(session.session_id, chunk).await {
            let _ = dialog::save_file_abort(session.session_id).await;
            state::set_status("备份失败:写入文件出错");
            return;
        }
    }
    match dialog::save_file_finish(session.session_id).await {
        Ok(()) => {
            tracing::info!("[backup] 已导出备份 {} 字节 → {}", bytes.len(), session.name);
            state::set_status(&format!(
                "备份完成: {} 字节 · 更新插件后请用“从备份恢复”导入",
                bytes.len()
            ));
        }
        Err(()) => {
            let _ = dialog::save_file_abort(session.session_id).await;
            state::set_status("备份失败:无法完成保存");
        }
    }
}

/// 通过宿主文件选择框挑选备份 JSON 并恢复 settings + data。
pub async fn restore_from_file() {
    use crate::astrobox::psys_host::dialog;

    let config = dialog::PickConfig {
        read: true,
        copy_to: None,
    };
    let filter = dialog::FilterConfig {
        multiple: false,
        extensions: vec!["json".to_string()],
        default_directory: String::new(),
        default_file_name: String::new(),
    };
    let picked = dialog::pick_file(&config, &filter).await;
    if picked.name.is_empty() && picked.data.is_empty() {
        state::set_status("未选择备份文件(已取消)");
        return;
    }
    if picked.data.is_empty() {
        state::set_status("读取备份文件失败(文件为空或无法读取)");
        return;
    }

    match llmboard_core::backup::apply_backup(&picked.data) {
        Ok(summary) => {
            tracing::info!("[backup] 已从 {} 恢复({summary})", picked.name);
            arm_timers().await;
            state::set_status(&format!("{summary}(来自 {})", picked.name));
        }
        Err(e) => state::set_status(&format!("恢复失败: {e}")),
    }
}
