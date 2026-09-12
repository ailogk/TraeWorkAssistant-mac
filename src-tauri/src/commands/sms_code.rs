//! d1jiema 接码平台对接：余额 / 取号 / 收码 / 释放 / 拉黑 / 发短信 / 历史记录。
//!
//! 安全约定：
//! - Token 仅存本地 conf/sms_code.json，回显给前端时脱敏（前 4 位 + ****）；
//! - 前端提交的 token 含 "****" 视为未修改，保留原值；
//! - 日志输出同样脱敏。
//!
//! 限频（服务端硬性规则）：
//! - queryUsed 每分钟最多 1 次，超频封 API → 本地 60s 缓存兜底；
//! - leftAmount 缓存 30s，避免频繁查询。
//!
//! API 约定：GET https://api.d1jiema.com/zc/data.php?code=xxx&token=xxx&...
//! 成功返回内容文本；失败返回 "ERROR:错误信息"。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::State;

use crate::fs_utils;
use crate::models::SmsCodeSettings;
use crate::state::AppState;

const D1JIEMA_BASE: &str = "https://api.d1jiema.com/zc/data.php";

/// 注册链接（含邀请码）
pub const D1JIEMA_SIGNUP_URL: &str =
    "https://www.d1jiema.com/appweb/signUp.html?inviter=1lngsgtr";
/// 登录 / 个人中心（创建 API Token、充值入口）
pub const D1JIEMA_PORTAL_URL: &str = "https://www.d1jiema.com/appweb/signIn.html";

/// 余额缓存：(查询时刻, 余额)
static BALANCE_CACHE: Mutex<Option<(Instant, f64)>> = Mutex::new(None);
/// queryUsed 历史记录缓存：(查询时刻, 记录行)
static HISTORY_CACHE: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);

/// Token 脱敏：保留前 4 位 + ****；短 token 全部打码
fn mask_token(token: &str) -> String {
    let t = token.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.chars().count() <= 4 {
        return "****".into();
    }
    format!("{}****", t.chars().take(4).collect::<String>())
}

/// 读取设置（不脱敏，内部使用）
fn load_settings(state: &AppState) -> SmsCodeSettings {
    fs_utils::read_json(&state.conf_path("sms_code.json"))
}

/// 统一请求入口：拼 token + code + 业务参数，处理网络错误与 ERROR: 前缀。
fn d1jiema_get(
    state: &AppState,
    code: &str,
    params: &[(&str, String)],
) -> Result<String, String> {
    let s = load_settings(state);
    let token = s.token.trim().to_string();
    if token.is_empty() {
        return Err("尚未配置 Token：请到「注册账号」页点击 Token 设置，先注册 d1jiema 并创建 API Token".into());
    }
    let req = ureq::get(D1JIEMA_BASE)
        .timeout(Duration::from_secs(30))
        .query("token", &token)
        .query("code", code);
    let req = params.iter().fold(req, |r, (k, v)| r.query(k, v));
    let resp = req.call().map_err(|e| match e {
        ureq::Error::Status(c, _) => format!("d1jiema 接口返回 HTTP {c}"),
        ureq::Error::Transport(t) => format!("网络请求失败: {t}"),
    })?;
    let body = resp
        .into_string()
        .map_err(|e| format!("读取响应失败: {e}"))?
        .trim()
        .to_string();
    if let Some(err) = body.strip_prefix("ERROR:") {
        let msg = err.trim();
        // 常见错误转译为用户可读提示
        let friendly = if msg.contains("token") || msg.to_lowercase().contains("登录") {
            format!("{msg}（Token 可能失效，请到设置中重新粘贴）")
        } else if msg.contains("余额") {
            format!("{msg}（账户余额不足，请充值）")
        } else {
            msg.to_string()
        };
        // 日志脱敏：错误信息里偶尔会带回显 token
        fs_utils::app_log(
            &state.data_dir,
            &format!("d1jiema {code} 失败: {friendly} (token={})", mask_token(&token)),
        );
        return Err(friendly);
    }
    Ok(body)
}

// ---------------- 设置 ----------------

/// 查询接码设置（token 脱敏返回）
#[tauri::command]
pub fn sms_code_get_settings(state: State<AppState>) -> SmsCodeSettings {
    let mut s = load_settings(&state);
    s.token = mask_token(&s.token);
    s
}

/// 保存接码设置；token 含 "****" 视为未修改保留原值
#[tauri::command]
pub fn sms_code_set_settings(
    state: State<AppState>,
    settings: SmsCodeSettings,
) -> Result<(), String> {
    let path = state.conf_path("sms_code.json");
    let mut current: SmsCodeSettings = fs_utils::read_json(&path);

    if !settings.token.contains("****") {
        current.token = settings.token.trim().to_string();
    }
    current.keyword = settings.keyword.trim().to_string();
    current.card_type = match settings.card_type.as_str() {
        "实卡" | "虚卡" | "全部" => settings.card_type.clone(),
        _ => "全部".into(),
    };
    current.province = settings.province.trim().to_string();

    fs_utils::write_json(&path, &current)?;
    // 设置变更后清空余额/历史缓存，保证下次查询用新 token
    *BALANCE_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *HISTORY_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(())
}

// ---------------- 业务命令 ----------------

/// 查询余额（缓存 30s）。返回 (余额, 是否来自缓存)。Err 时缓存不更新。
#[tauri::command(async)]
pub fn sms_code_balance(state: State<AppState>) -> Result<f64, String> {
    let mut cache = BALANCE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = *cache {
        if at.elapsed() < Duration::from_secs(30) {
            return Ok(v);
        }
    }
    let body = d1jiema_get(&state, "leftAmount", &[])?;
    let amount: f64 = body
        .parse()
        .map_err(|_| format!("余额返回值无法解析: {body}"))?;
    *cache = Some((Instant::now(), amount));
    Ok(amount)
}

/// 取号：phone 为 None 随机取号；Some 指定号码。
/// 成功返回手机号。
#[tauri::command(async)]
pub fn sms_code_get_phone(
    state: State<AppState>,
    phone: Option<String>,
) -> Result<String, String> {
    let s = load_settings(&state);
    let mut params: Vec<(&str, String)> = Vec::new();
    // keyWord 强烈建议传：不传可能取到收不了目标短信的号
    if !s.keyword.is_empty() {
        params.push(("keyWord", s.keyword.clone()));
    }
    if let Some(p) = phone.as_deref() {
        let p = p.trim();
        if !p.is_empty() {
            params.push(("phone", p.to_string()));
        }
    }
    if !s.province.is_empty() {
        params.push(("province", s.province.clone()));
    }
    if !s.card_type.is_empty() {
        params.push(("cardType", s.card_type.clone()));
    }
    let number = d1jiema_get(&state, "getPhone", &params)?;
    fs_utils::app_log(
        &state.data_dir,
        &format!(
            "d1jiema 取号成功: {} (keyword={}, cardType={})",
            number, s.keyword, s.card_type
        ),
    );
    Ok(number)
}

/// 收短信（单次查询，轮询节奏由前端控制）。
/// 返回短信内容；平台"尚未收到"时返回 Ok(None) 语义由 is_pending 标记：
/// 这里统一返回字符串，前端判断 [尚未收到]。
#[tauri::command(async)]
pub fn sms_code_get_msg(state: State<AppState>, phone: String) -> Result<String, String> {
    let s = load_settings(&state);
    if s.keyword.is_empty() {
        return Err("短信关键词为空：请先在设置中填写关键词（如 Trae）".into());
    }
    d1jiema_get(
        &state,
        "getMsg",
        &[("phone", phone.trim().to_string()), ("keyWord", s.keyword)],
    )
}

/// 释放号码（失败提示跳过，勿重试）
#[tauri::command(async)]
pub fn sms_code_release(state: State<AppState>, phone: String) -> Result<String, String> {
    d1jiema_get(&state, "release", &[("phone", phone.trim().to_string())])
}

/// 拉黑号码（失败提示跳过，勿重试）
#[tauri::command(async)]
pub fn sms_code_block(state: State<AppState>, phone: String) -> Result<String, String> {
    d1jiema_get(&state, "block", &[("phone", phone.trim().to_string())])
}

/// 发送短信（仅支持向 1069 网关号发送）
#[tauri::command(async)]
pub fn sms_code_send(
    state: State<AppState>,
    phone: String,
    to_phone: String,
    content: String,
) -> Result<String, String> {
    let to = to_phone.trim();
    if !to.starts_with("1069") {
        return Err("目标号码必须是 1069 开头的短信网关号（平台禁止向个人手机号发送）".into());
    }
    if content.trim().is_empty() {
        return Err("发送内容不能为空".into());
    }
    d1jiema_get(
        &state,
        "send",
        &[
            ("phone", phone.trim().to_string()),
            ("toPhone", to.to_string()),
            ("content", content.trim().to_string()),
        ],
    )
}

/// 历史记录（服务端限频 1 次/分钟 → 本地 60s 缓存）。
/// 返回 (记录行列表, 缓存是否命中)。
#[tauri::command(async)]
pub fn sms_code_query_used(state: State<AppState>) -> Result<(Vec<String>, bool), String> {
    let mut cache = HISTORY_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, ref rows)) = *cache {
        if at.elapsed() < Duration::from_secs(60) {
            return Ok((rows.clone(), true));
        }
    }
    let body = d1jiema_get(&state, "queryUsed", &[])?;
    let rows: Vec<String> = body
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    *cache = Some((Instant::now(), rows.clone()));
    Ok((rows, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_mask() {
        assert_eq!(mask_token("abcdefgh1234"), "abcd****");
        assert_eq!(mask_token("abc"), "****");
        assert_eq!(mask_token(""), "");
    }
}
