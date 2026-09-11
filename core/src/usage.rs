//! 用量聚合(usage fetcher 的数据层):解析 DeepSeek 平台导出的 amount/cost CSV,
//! 聚合成逐日指标,再转成最近 N 天的展示序列(消费/输出 token/缓存命中率)。
//! 纯逻辑(无网络/无 zip),可宿主单测;HTTP 与 zip 解包留在插件侧代码内预设(capabilities/)。

use std::collections::BTreeMap;

use serde_json::Value;

use crate::dates;

/// 平台时区 UTC+8(秒)
pub const TZ_SEC: i64 = 28_800;
/// 展示天数
pub const DAYS: i64 = 7;

/// 单模型聚合结果(导出窗口内合计,成本/输出 token 排行数据源)。
#[derive(Debug, Clone, Default)]
pub struct ModelAgg {
    /// 平台 CSV 中的原始模型名
    pub model: String,
    /// 输出 token 合计
    pub tokens: u64,
    /// 成本合计(CNY)
    pub cost: f64,
}

/// 模型名 → 标记名映射表 —— 开发者在此登记(能力包数据层)。
/// 键 = 模型 id 的规范化小写(仅保留字母数字,如 deepseek-v4-flash → deepseekv4flash),
/// 也兼容去掉 deepseek- 前缀的短键(v4flash)。匹配优先于 model_short_name 的兜底改名。
pub const MODEL_TAGS: &[(&str, &str)] = &[
    ("deepseekv4flash", "Flash"),
    ("deepseekv4flashvisionexp", "Vision"),
    ("deepseekv4pro", "Pro"),
];

/// 规范化模型名:小写并去掉 - / _ 与空格(用于映射表键匹配)。
fn norm_model_key(model: &str) -> String {
    model
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 模型展示名:先查 MODEL_TAGS 映射(整名与去前缀两种键),未命中走 model_short_name 兜底。
pub fn model_display_name(model: &str) -> String {
    let key = norm_model_key(model);
    let key_short = key.strip_prefix("deepseek").unwrap_or(&key);
    for (k, tag) in MODEL_TAGS {
        if *k == key || *k == key_short {
            return (*tag).to_string();
        }
    }
    model_short_name(model)
}

/// 模型展示名:去掉 "deepseek-" 前缀,连字符转空格,词首字母大写
/// (deepseek-chat → Chat;deepseek-reasoner → Reasoner;deepseek-v4-pro → V4 Pro)。
pub fn model_short_name(model: &str) -> String {
    let stem = model.strip_prefix("deepseek-").unwrap_or(model);
    let mut out = String::new();
    for (i, w) in stem.split('-').enumerate() {
        if w.is_empty() {
            continue;
        }
        if i > 0 {
            out.push(' ');
        }
        let mut cs = w.chars();
        if let Some(c) = cs.next() {
            out.extend(c.to_uppercase());
        }
        out.push_str(cs.as_str());
    }
    out
}

/// 单日聚合结果。
#[derive(Debug, Clone, Copy, Default)]
pub struct DayAgg {
    /// Paid + Granted 合计(CNY)
    pub cost: f64,
    pub output_tokens: u64,
    /// input_cache_hit_tokens
    pub hit_tokens: u64,
    /// input_cache_miss_tokens
    pub miss_tokens: u64,
}

/// 平台导出窗口:本地(CN)0 点对齐的最近 days 天。
pub fn cn_window(days: i64) -> (i64, i64) {
    let now = dates::unix_now();
    let today_days = (now + TZ_SEC).div_euclid(86_400);
    let local_midnight = |d: i64| d * 86_400 - TZ_SEC;
    (local_midnight(today_days - (days - 1)), local_midnight(today_days + 1))
}

/// 聚合 amount/cost CSV → date → DayAgg。
/// 平台 CSV 时间戳为 +08:00,date 直接取 start_time_iso 前缀(平台本地日)。
/// 某天的平台本地日键(YYYY-MM-DD),用于过滤「当日」数据行。
pub fn day_key(today_days: i64) -> String {
    let (y, m, dd) = dates::civil_from_days(today_days);
    format!("{y:04}-{m:02}-{dd:02}")
}

pub fn aggregate_csvs(
    amount_text: &str,
    cost_text: &str,
) -> Result<BTreeMap<String, DayAgg>, String> {
    let mut per_day: BTreeMap<String, DayAgg> = BTreeMap::new();

    // amount: user_id,start_time_iso,end_time_iso,model,api_key_name,api_key,type,price,amount
    let amount_text = amount_text.strip_prefix('\u{feff}').unwrap_or(amount_text);
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(amount_text.as_bytes());
    for rec in reader.records() {
        let rec = rec.map_err(|e| format!("amount CSV 解析失败: {e}"))?;
        if rec.iter().all(|f| f.is_empty()) {
            continue;
        }
        let date = rec
            .get(1)
            .and_then(|s| s.get(..10))
            .ok_or("amount CSV 缺少日期")?;
        let type_ = rec.get(6).unwrap_or_default();
        let amount: u64 = rec
            .get(8)
            .unwrap_or_default()
            .parse()
            .map_err(|_| format!("amount 非数字: {}", rec.get(8).unwrap_or_default()))?;
        let entry = per_day.entry(date.to_string()).or_default();
        match type_ {
            "output_tokens" => entry.output_tokens += amount,
            "input_cache_hit_tokens" => entry.hit_tokens += amount,
            "input_cache_miss_tokens" => entry.miss_tokens += amount,
            _ => {}
        }
    }

    // cost: user_id,start_time_iso,end_time_iso,model,wallet_type,cost,currency
    let cost_text = cost_text.strip_prefix('\u{feff}').unwrap_or(cost_text);
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(cost_text.as_bytes());
    for rec in reader.records() {
        let rec = rec.map_err(|e| format!("cost CSV 解析失败: {e}"))?;
        if rec.iter().all(|f| f.is_empty()) {
            continue;
        }
        let date = rec
            .get(1)
            .and_then(|s| s.get(..10))
            .ok_or("cost CSV 缺少日期")?;
        let cost: f64 = rec
            .get(5)
            .unwrap_or_default()
            .parse()
            .map_err(|_| format!("cost 非数字: {}", rec.get(5).unwrap_or_default()))?;
        per_day.entry(date.to_string()).or_default().cost += cost;
    }

    if per_day.is_empty() {
        return Err("CSV 无有效数据行".into());
    }
    Ok(per_day)
}

/// 把逐日聚合转成最近 DAYS 天的展示序列(缺日补 0;命中率缺日补 0.0)。
/// today_days:本地(CN)今天距 epoch 的天数,序列窗口为 [today-6, today]。
pub fn build_daily_series(
    per_day: &BTreeMap<String, DayAgg>,
    today_days: i64,
) -> BTreeMap<String, Value> {
    let mut cost_series = Vec::new();
    let mut token_series = Vec::new();
    let mut hit_rate_series = Vec::new();
    let mut labels = Vec::new();
    let mut total7d = 0.0f64;
    let mut tokens7d = 0u64;
    let mut hit7d = 0u64;
    let mut miss7d = 0u64;
    for d in (today_days - (DAYS - 1))..=today_days {
        let (y, m, dd) = dates::civil_from_days(d);
        let key = format!("{y:04}-{m:02}-{dd:02}");
        let agg = per_day.get(&key).copied().unwrap_or_default();
        cost_series.push(round2(agg.cost));
        token_series.push(agg.output_tokens);
        hit_rate_series.push(if agg.hit_tokens + agg.miss_tokens > 0 {
            round1(agg.hit_tokens as f64 / (agg.hit_tokens + agg.miss_tokens) as f64 * 100.0)
        } else {
            0.0
        });
        labels.push(format!("{m}/{dd}"));
        total7d += agg.cost;
        tokens7d += agg.output_tokens;
        hit7d += agg.hit_tokens;
        miss7d += agg.miss_tokens;
    }

    let mut out = BTreeMap::new();
    out.insert("costSeries".to_string(), serde_json::to_value(cost_series).unwrap());
    out.insert("tokenSeries".to_string(), serde_json::to_value(token_series).unwrap());
    out.insert("hitRateSeries".to_string(), serde_json::to_value(hit_rate_series).unwrap());
    out.insert("dayLabels".to_string(), serde_json::to_value(labels).unwrap());
    out.insert("total7d".to_string(), serde_json::json!(round2(total7d)));
    out.insert("tokens7d".to_string(), serde_json::json!(tokens7d));
    // 7 日总体缓存命中率:无调用数据时为 null(模板渲染为 "--")
    out.insert(
        "cacheHitRate7d".to_string(),
        if hit7d + miss7d > 0 {
            serde_json::json!(round1(hit7d as f64 / (hit7d + miss7d) as f64 * 100.0))
        } else {
            serde_json::Value::Null
        },
    );
    out.insert("days".to_string(), serde_json::json!(DAYS));
    out.insert("checkedAt".to_string(), serde_json::json!(dates::unix_now()));
    out
}

/// 按模型聚合 amount/cost CSV → 模型用量列表(按成本降序;仅保留有成本或有输出

/// token 的模型)。数据行与 aggregate_csvs 同一列布局:

/// amount: user_id,start_time_iso,end_time_iso,model,api_key_name,api_key,type,price,amount

/// cost:   user_id,start_time_iso,end_time_iso,model,wallet_type,cost,currency

/// 无模型数据返回空列表(不报错,模板端对空表格整块隐藏)。

pub fn aggregate_models(amount_text: &str, cost_text: &str) -> Result<Vec<ModelAgg>, String> {

    let mut by_model: BTreeMap<String, ModelAgg> = BTreeMap::new();



    let amount_text = amount_text.strip_prefix('\u{feff}').unwrap_or(amount_text);

    let mut reader = csv::ReaderBuilder::new()

        .trim(csv::Trim::All)

        .from_reader(amount_text.as_bytes());

    for rec in reader.records() {

        let rec = rec.map_err(|e| format!("amount CSV 解析失败: {e}"))?;

        if rec.iter().all(|f| f.is_empty()) {

            continue;

        }

        let model = rec.get(3).unwrap_or_default();

        if model.is_empty() || rec.get(6).unwrap_or_default() != "output_tokens" {

            continue;

        }

        let amount: u64 = rec

            .get(8)

            .unwrap_or_default()

            .parse()

            .map_err(|_| format!("amount 非数字: {}", rec.get(8).unwrap_or_default()))?;

        by_model.entry(model.to_string()).or_default().tokens += amount;

    }



    let cost_text = cost_text.strip_prefix('\u{feff}').unwrap_or(cost_text);

    let mut reader = csv::ReaderBuilder::new()

        .trim(csv::Trim::All)

        .from_reader(cost_text.as_bytes());

    for rec in reader.records() {

        let rec = rec.map_err(|e| format!("cost CSV 解析失败: {e}"))?;

        if rec.iter().all(|f| f.is_empty()) {

            continue;

        }

        let model = rec.get(3).unwrap_or_default();

        if model.is_empty() {

            continue;

        }

        let cost: f64 = rec

            .get(5)

            .unwrap_or_default()

            .parse()

            .map_err(|_| format!("cost 非数字: {}", rec.get(5).unwrap_or_default()))?;

        by_model.entry(model.to_string()).or_default().cost += cost;

    }



    let mut out: Vec<ModelAgg> = by_model

        .into_iter()

        .map(|(model, mut m)| {

            m.model = model;

            m

        })

        .filter(|m| m.cost > 0.0 || m.tokens > 0)

        .collect();

    for m in &mut out {

        m.cost = round2(m.cost);

    }

    out.sort_by(|a, b| b.cost.total_cmp(&a.cost));

    Ok(out)

}



/// 按模型聚合**某一天**(平台本地日 YYYY-MM-DD,如「当日」)的用量。
/// 实现:先按日期列过滤数据行,再复用 aggregate_models 聚合。
pub fn aggregate_models_on(
    amount_text: &str,
    cost_text: &str,
    day: &str,
) -> Result<Vec<ModelAgg>, String> {
    let keep = |line: &str| -> bool {
        // 表头/空行保留(聚合会忽略);数据行取第 2 列日期前缀比较
        let l = line.strip_prefix("\u{feff}").unwrap_or(line);
        let Some((_, rest)) = l.split_once(',') else {
            return true;
        };
        let Some((date, _)) = rest.split_once(',') else {
            return true;
        };
        // 表头行(csv Reader 会将其当作头行消费,必须保留)
        if line.starts_with("user_id") {
            return true;
        }
        date.get(..10) == Some(day)
    };
    let filter_text = |text: &str| -> String {
        text.lines()
            .filter(|l| keep(l))
            .collect::<Vec<&str>>()
            .join("\n")
    };
    aggregate_models(&filter_text(amount_text), &filter_text(cost_text))
}

pub fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

pub fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cn_window_aligns_to_midnight() {
        let (start, end) = cn_window(7);
        assert_eq!((start + TZ_SEC).rem_euclid(86_400), 0);
        assert_eq!((end + TZ_SEC).rem_euclid(86_400), 0);
        assert_eq!(end - start, 7 * 86_400);
    }

    #[test]
    fn aggregate_csvs_tracks_cache_tokens() {
        let amount = "\u{feff}user_id,start_time_iso,end_time_iso,model,api_key_name,api_key,type,price,amount\n00000000-0000-0000-0000-000000000000,2026-07-13T00:00:00+08:00,2026-07-14T00:00:00+08:00,deepseek-v4-pro,key-a,sk-x,output_tokens,0.000006,352921\n00000000-0000-0000-0000-000000000000,2026-07-13T00:00:00+08:00,2026-07-14T00:00:00+08:00,deepseek-v4-pro,key-a,sk-x,input_cache_hit_tokens,0.000000025,105402624\n00000000-0000-0000-0000-000000000000,2026-07-13T00:00:00+08:00,2026-07-14T00:00:00+08:00,deepseek-v4-pro,key-a,sk-x,input_cache_miss_tokens,0.000003,1885620\n00000000-0000-0000-0000-000000000000,2026-07-13T00:00:00+08:00,2026-07-14T00:00:00+08:00,deepseek-v4-pro,key-a,sk-x,request_count,,666\n";
        let cost = "\u{feff}user_id,start_time_iso,end_time_iso,model,wallet_type,cost,currency\n00000000-0000-0000-0000-000000000000,2026-07-13T00:00:00+08:00,2026-07-14T00:00:00+08:00,deepseek-v4-pro,Paid,10.4094516000000000,CNY\n";
        let per_day = aggregate_csvs(amount, cost).unwrap();
        let agg = per_day.get("2026-07-13").copied().unwrap();
        assert!((agg.cost - 10.4094516).abs() < 1e-6);
        assert_eq!(agg.output_tokens, 352_921);
        assert_eq!(agg.hit_tokens, 105_402_624);
        assert_eq!(agg.miss_tokens, 1_885_620);
    }

    #[test]
    fn daily_series_computes_hit_rate() {
        // 固定"今天"= 2026-07-13,窗口 07-07..07-13
        let today = dates::days_from_civil(2026, 7, 13);
        let mut per_day = BTreeMap::new();
        per_day.insert(
            "2026-07-11".to_string(),
            DayAgg { cost: 1.0, output_tokens: 10, hit_tokens: 9, miss_tokens: 1 },
        );
        per_day.insert(
            "2026-07-13".to_string(),
            DayAgg { cost: 2.0, output_tokens: 20, hit_tokens: 80, miss_tokens: 20 },
        );
        let out = build_daily_series(&per_day, today);
        let rates = out["hitRateSeries"].as_array().unwrap();
        assert_eq!(rates.len(), 7);
        assert_eq!(rates[0].as_f64().unwrap(), 0.0); // 缺日补 0
        assert_eq!(rates[4].as_f64().unwrap(), 90.0); // 07-11: 9/(9+1)
        assert_eq!(rates[6].as_f64().unwrap(), 80.0); // 07-13: 80/(80+20)
        // 总体命中率 (9+80)/(10+100)*100 → 80.9
        assert_eq!(out["cacheHitRate7d"].as_f64().unwrap(), 80.9);
        assert_eq!(out["total7d"].as_f64().unwrap(), 3.0);
        assert_eq!(out["tokens7d"].as_u64().unwrap(), 30);

        // 全无调用数据 → cacheHitRate7d 为 null
        let empty = build_daily_series(&BTreeMap::new(), today);
        assert!(empty["cacheHitRate7d"].is_null());
    }

    #[test]
    fn model_short_name_and_aggregate_models() {
        assert_eq!(model_short_name("deepseek-chat"), "Chat");
        assert_eq!(model_short_name("deepseek-reasoner"), "Reasoner");
        assert_eq!(model_short_name("deepseek-v4-pro"), "V4 Pro");
        // MODEL_TAGS 映射优先:V4 系列 → 标记名
        assert_eq!(model_display_name("deepseek-v4-flash"), "Flash");
        assert_eq!(model_display_name("deepseek-v4-flash-vision-exp"), "Vision");
        assert_eq!(model_display_name("deepseek-v4-pro"), "Pro");
        assert_eq!(model_display_name("DeepSeek-V4-Flash"), "Flash"); // 大小写/前缀无关
        assert_eq!(model_display_name("deepseek-chat"), "Chat"); // 未登记走兜底

        let amount = "\u{feff}user_id,start_time_iso,end_time_iso,model,api_key_name,api_key,type,price,amount\n00000000-0000-0000-0000-000000000000,2026-07-13T00:00:00+08:00,2026-07-14T00:00:00+08:00,deepseek-chat,key-a,sk-x,output_tokens,0.000006,352921\n00000000-0000-0000-0000-000000000000,2026-07-13T00:00:00+08:00,2026-07-14T00:00:00+08:00,deepseek-reasoner,key-a,sk-x,output_tokens,0.00003,666\n00000000-0000-0000-0000-000000000000,2026-07-13T00:00:00+08:00,2026-07-14T00:00:00+08:00,deepseek-chat,key-a,sk-x,input_cache_hit_tokens,0.000000025,105402624\n";
        let cost = "\u{feff}user_id,start_time_iso,end_time_iso,model,wallet_type,cost,currency\n00000000-0000-0000-0000-000000000000,2026-07-13T00:00:00+08:00,2026-07-14T00:00:00+08:00,deepseek-reasoner,Paid,10.4094516000000000,CNY\n00000000-0000-0000-0000-000000000000,2026-07-13T00:00:00+08:00,2026-07-14T00:00:00+08:00,deepseek-chat,Paid,0.1234560000000000,CNY\n";
        let models = aggregate_models(amount, cost).unwrap();
        // 按成本降序:reasoner 10.41 > chat 0.12
        assert_eq!(models[0].model, "deepseek-reasoner");
        assert_eq!(models[1].model, "deepseek-chat");
        assert!((models[0].cost - 10.41).abs() < 1e-9);
        assert!((models[1].cost - 0.12).abs() < 1e-9);
        assert_eq!(models[1].tokens, 352_921); // hit 行不计入输出 tokens
        assert_eq!(models[0].tokens, 666);

        // 空数据 → 空列表(不报错)
        assert!(aggregate_models("", "").unwrap().is_empty());

    }


    #[test]
    fn aggregate_models_on_filters_by_day() {
        let amount = r#"user_id,start_time_iso,end_time_iso,model,api_key_name,api_key,type,price,amount
a,2026-07-13T10:00:00+08:00,x,deepseek-chat,k,sk,output_tokens,1,100
a,2026-07-14T10:00:00+08:00,x,deepseek-chat,k,sk,output_tokens,1,200
a,2026-07-14T10:00:00+08:00,x,deepseek-reasoner,k,sk,output_tokens,1,300"#;
        let cost = r#"user_id,start_time_iso,end_time_iso,model,wallet_type,cost,currency
a,2026-07-13T10:00:00+08:00,x,deepseek-chat,Paid,0.5,CNY
a,2026-07-14T10:00:00+08:00,x,deepseek-chat,Paid,0.6,CNY"#;
        let m = aggregate_models_on(amount, cost, "2026-07-13").unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].model, "deepseek-chat");
        assert_eq!(m[0].tokens, 100);
        assert!((m[0].cost - 0.5).abs() < 1e-9);
        assert_eq!(aggregate_models_on(amount, cost, "2020-01-01").unwrap().len(), 0);
        assert_eq!(day_key(0), "1970-01-01");
    }

}
