//! 全局状态与持久化:settings.json / data.json。

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::preset::{self, Preset};
use crate::snapshot::BalanceInfo;

pub const SETTINGS_FILE: &str = "settings.json";
pub const DATA_FILE: &str = "data.json";
/// 手环端快应用包名(与 vela/src/manifest.json 的 package 一致)
pub const DEFAULT_PKG: &str = "io.github.mcxcc303.llmband";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// preset id → 是否启用
    pub enabled: BTreeMap<String, bool>,
    /// preset id → (表单字段 id → 值)。分类表单凭据表(预设 form.fields 驱动)
    pub credentials: BTreeMap<String, BTreeMap<String, String>>,
    /// v1 遗留:preset id → API Key(加载时自动迁移进 credentials.apiKey)
    pub api_keys: BTreeMap<String, String>,
    pub push_pkg: String,
    pub balance_interval_secs: u64,
    pub push_interval_secs: u64,
    /// 插件页面当前选中的供应商(分类表单)
    pub selected_provider: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: BTreeMap::new(),
            credentials: BTreeMap::new(),
            api_keys: BTreeMap::new(),
            push_pkg: DEFAULT_PKG.into(),
            balance_interval_secs: 120,
            push_interval_secs: 60,
            selected_provider: None,
        }
    }
}

/// 单个供应商的持久化数据。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProviderData {
    pub balance: Option<BalanceInfo>,
    /// 预设 extras + computed 结果(模板渲染数据源)
    pub extras: BTreeMap<String, Value>,
    /// 各端点归一化数据(端点名 → extract+computed 的 Value 映射表)
    pub endpoints: BTreeMap<String, Value>,
    pub last_ok_at: Option<i64>,
    pub last_error: Option<String>,
    pub error_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DataFile {
    pub version: u32,
    pub providers: BTreeMap<String, ProviderData>,
}

pub struct App {
    pub settings: Settings,
    pub data: DataFile,
    /// 启动时加载的预设(静态,运行期不变化)
    pub presets: Vec<Preset>,
    /// 宿主时区相对 UTC 分钟数
    pub tz_offset_min: i32,
    /// 当前已连接设备地址(取已连接列表第一个)
    pub device_addr: Option<String>,
    /// 是否已对该设备注册 interconnect 接收
    pub recv_registered: bool,
    /// 插件页面渲染 id(on_ui_render 传入,重绘用)
    pub page_element_id: Option<String>,
    /// 设备页卡片渲染 id(on_card_render 传入,重绘用)
    pub card_element_id: Option<String>,
    /// payload → timer id
    pub timer_ids: BTreeMap<String, u64>,
    /// UI 状态行(最近一次动作结果)
    pub status: String,
    pub status_at: i64,
    pub last_push_at: Option<i64>,
    /// 最近一次成功推送的快照稳定签名(变化检测用)
    pub last_pushed_signature: Option<String>,
    /// 最近一次成功推送的目标设备(换设备后即使数据相同也要重推)
    pub last_pushed_device: Option<String>,
    /// 最近一次收到快应用互联消息的 Unix 秒(短窗口去重)
    pub last_interconnect_at: Option<i64>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            data: DataFile::default(),
            presets: Vec::new(),
            tz_offset_min: 480, // 缺省 +08:00,on_load 后用宿主值覆盖
            device_addr: None,
            recv_registered: false,
            page_element_id: None,
            card_element_id: None,
            timer_ids: BTreeMap::new(),
            status: "未初始化".into(),
            status_at: 0,
            last_push_at: None,
            last_pushed_signature: None,
            last_pushed_device: None,
            last_interconnect_at: None,
        }
    }
}

impl App {
    pub fn is_enabled(&self, id: &str) -> bool {
        self.settings.enabled.get(id).copied().unwrap_or(false)
    }

    /// 主凭据(apiKey 字段)。
    pub fn api_key(&self, id: &str) -> Option<String> {
        self.credential(id, "apiKey")
    }

    /// 取供应商某个表单字段的凭据值(优先 credentials,回退 v1 api_keys)。
    pub fn credential(&self, id: &str, field: &str) -> Option<String> {
        if let Some(v) = self
            .settings
            .credentials
            .get(id)
            .and_then(|m| m.get(field))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            return Some(v);
        }
        if field == "apiKey" {
            return self
                .settings
                .api_keys
                .get(id)
                .map(|k| k.trim().to_string())
                .filter(|s| !s.is_empty());
        }
        None
    }

    /// 写入某个表单字段的凭据值(未保存,Save 后落盘)。
    pub fn set_credential(&mut self, id: &str, field: &str, value: String) {
        self.settings
            .credentials
            .entry(id.to_string())
            .or_default()
            .insert(field.to_string(), value);
    }

    pub fn preset_by_id(&self, id: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.id == id)
    }

    /// 从插件目录加载全部预设(启动时调用一次)。
    pub fn load_presets(&mut self) {
        self.presets = preset::load_all();
        tracing::info!(
            "[preset] 已加载 {} 个预设: {}",
            self.presets.len(),
            self.presets
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

static APP: OnceLock<Mutex<App>> = OnceLock::new();

pub fn app() -> &'static Mutex<App> {
    APP.get_or_init(|| Mutex::new(App::default()))
}

pub fn lock() -> MutexGuard<'static, App> {
    app()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 从插件目录加载 settings.json / data.json;文件缺失或损坏时使用默认值(不中断)。
pub fn init_from_disk() {
    let mut a = lock();
    if let Ok(text) = std::fs::read_to_string(SETTINGS_FILE) {
        if let Ok(s) = serde_json::from_str::<Settings>(&text) {
            a.settings = s;
        } else {
            tracing::warn!("settings.json 解析失败,使用默认设置");
        }
    }
    if let Ok(text) = std::fs::read_to_string(DATA_FILE) {
        if let Ok(d) = serde_json::from_str::<DataFile>(&text) {
            a.data = d;
        } else {
            tracing::warn!("data.json 解析失败,使用空数据");
        }
    }
    // v1 → v2 迁移:api_keys 并入 credentials.apiKey
    let legacy: Vec<(String, String)> = std::mem::take(&mut a.settings.api_keys).into_iter().collect();
    for (id, key) in legacy {
        a.settings
            .credentials
            .entry(id.clone())
            .or_default()
            .entry("apiKey".to_string())
            .or_insert(key);
    }
    // 清理:settings 中已不存在的预设条目
    let ids: std::collections::BTreeSet<String> =
        a.presets.iter().map(|p| p.id.clone()).collect();
    a.settings.enabled.retain(|id, _| ids.contains(id));
    a.settings.credentials.retain(|id, _| ids.contains(id));
    a.settings.api_keys.retain(|id, _| ids.contains(id));
    // 分类表单选中项失效时回退到第一个预设
    let sel_ok = a
        .settings
        .selected_provider
        .as_deref()
        .map(|s| ids.contains(s))
        .unwrap_or(false);
    if !sel_ok {
        a.settings.selected_provider = a.presets.first().map(|p| p.id.clone());
    }
}

pub fn save_settings() {
    let text = {
        let a = lock();
        serde_json::to_string_pretty(&a.settings).unwrap_or_else(|_| "{}".into())
    };
    if let Err(e) = std::fs::write(SETTINGS_FILE, text) {
        tracing::warn!("保存 settings.json 失败: {e}");
    }
}

pub fn save_data() {
    let text = {
        let a = lock();
        serde_json::to_string_pretty(&a.data).unwrap_or_else(|_| "{}".into())
    };
    if let Err(e) = std::fs::write(DATA_FILE, text) {
        tracing::warn!("保存 data.json 失败: {e}");
    }
}

pub fn set_status(msg: &str) {
    let mut a = lock();
    a.status = msg.to_string();
    a.status_at = crate::dates::unix_now();
    tracing::info!("[status] {msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let s = Settings::default();
        assert_eq!(s.push_pkg, DEFAULT_PKG);
        assert_eq!(s.balance_interval_secs, 120);
        assert_eq!(s.push_interval_secs, 60);
        assert!(s.enabled.is_empty());
    }

    #[test]
    fn enable_and_key_accessors() {
        let mut a = App::default();
        a.settings.enabled.insert("x".into(), true);
        a.set_credential("x", "apiKey", " sk-1 ".into());
        a.set_credential("x", "platformToken", " tok ".into());
        assert!(a.is_enabled("x"));
        assert!(!a.is_enabled("y"));
        assert_eq!(a.api_key("x").as_deref(), Some("sk-1")); // trim
        assert_eq!(a.credential("x", "platformToken").as_deref(), Some("tok"));
        assert_eq!(a.credential("x", "nope"), None);
    }

    #[test]
    fn legacy_api_keys_migrate_to_credentials() {
        let mut a = App::default();
        a.settings.api_keys.insert("demo".into(), "sk-legacy".into());
        // 直接调用迁移片段(init_from_disk 需要文件系统,这里复用逻辑验证)
        let legacy: Vec<(String, String)> =
            std::mem::take(&mut a.settings.api_keys).into_iter().collect();
        for (id, key) in legacy {
            a.settings
                .credentials
                .entry(id.clone())
                .or_default()
                .entry("apiKey".to_string())
                .or_insert(key);
        }
        assert_eq!(a.api_key("demo").as_deref(), Some("sk-legacy"));
    }
}
