//! 配置/数据备份与恢复。
//!
//! 宿主更新插件时会整体删除插件目录,settings.json / data.json 无法跨版本保留;
//! 通过宿主 dialog 让用户导出/导入备份文件。

use serde::{Deserialize, Serialize};

use crate::state::{self, DataFile, Settings};

pub const BACKUP_KIND: &str = "llmboard-backup";
pub const BACKUP_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupFile {
    pub kind: String,
    pub version: u32,
    pub exported_at: i64,
    pub settings: Settings,
    pub data: DataFile,
}

/// 把当前 settings + data 序列化为备份 JSON。
pub fn encode_backup() -> Result<Vec<u8>, String> {
    let (settings, data) = {
        let a = state::lock();
        (a.settings.clone(), a.data.clone())
    };
    let file = BackupFile {
        kind: BACKUP_KIND.into(),
        version: BACKUP_VERSION,
        exported_at: crate::dates::unix_now(),
        settings,
        data,
    };
    serde_json::to_vec_pretty(&file).map_err(|e| format!("序列化备份失败: {e}"))
}

/// 解析备份文件并覆盖当前 settings + data。
pub fn apply_backup(bytes: &[u8]) -> Result<String, String> {
    let file: BackupFile =
        serde_json::from_slice(bytes).map_err(|e| format!("备份文件不是有效 JSON: {e}"))?;
    if file.kind != BACKUP_KIND {
        return Err(format!("不是本插件的备份文件(kind={})", file.kind));
    }
    if file.version != BACKUP_VERSION {
        return Err(format!(
            "备份版本不受支持: {} (当前支持 {BACKUP_VERSION})",
            file.version
        ));
    }

    {
        let mut a = state::lock();
        let mut settings = file.settings;
        // 清理备份中已不存在的预设条目(预设集随版本迭代变化)
        settings
            .enabled
            .retain(|id, _| a.presets.iter().any(|p| &p.id == id));
        settings
            .api_keys
            .retain(|id, _| a.presets.iter().any(|p| &p.id == id));
        a.settings = settings;
        a.data = file.data;
        // 恢复后强制下一轮重新推送(签名缓存作废)
        a.last_pushed_signature = None;
        a.last_pushed_device = None;
    }
    state::save_settings();
    state::save_data();

    let (providers, has_balance) = {
        let a = state::lock();
        let has = a
            .data
            .providers
            .values()
            .any(|d| d.balance.is_some());
        (a.data.providers.len(), has)
    };
    Ok(format!(
        "备份已恢复: {providers} 个供应商数据 · 余额{}",
        if has_balance { "有" } else { "无" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_roundtrip() {
        let mut settings = Settings::default();
        settings.enabled.insert("demo".into(), true);
        settings.api_keys.insert("demo".into(), "sk-123".into());
        let file = BackupFile {
            kind: BACKUP_KIND.into(),
            version: BACKUP_VERSION,
            exported_at: 1_754_000_000,
            settings,
            data: DataFile::default(),
        };
        let bytes = serde_json::to_vec(&file).unwrap();
        let parsed: BackupFile = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.kind, BACKUP_KIND);
        assert_eq!(parsed.settings.enabled.get("demo"), Some(&true));
    }
}
