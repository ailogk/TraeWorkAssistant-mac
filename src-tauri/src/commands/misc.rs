#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::process::Command;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::fs_utils;
use crate::jwt;
use crate::models::{CreditRecord, CreditsFile, DeviceMap, Settings};
use crate::state::AppState;

pub const INVITE_LINK: &str =
    "https://www.trae.cn/work-fission/4CP3KDBT5W9A?utm_source=copy_link&utm_medium=friends_invite";

// ---------------- 设备 ID ----------------

#[tauri::command]
pub fn device_reset(state: State<AppState>, user_id: String) -> Result<(), String> {
    let mut map: DeviceMap = fs_utils::read_json(&state.path("device_map.json"));
    map.remove(&user_id);
    fs_utils::write_json(&state.path("device_map.json"), &map)?;
    Ok(())
}

// ---------------- 开机自启（T11，tauri-plugin-autostart：Windows 写注册表 Run 项） ----------------

/// 查询开机自启状态
#[tauri::command]
pub fn autostart_status(app: AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

/// 设置开机自启（即时生效，安装版/便携版均写当前 exe 路径）
#[tauri::command]
pub fn autostart_set(app: AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let autolaunch = app.autolaunch();
    if enabled {
        autolaunch.enable().map_err(|e| e.to_string())
    } else {
        autolaunch.disable().map_err(|e| e.to_string())
    }
}

// ---------------- 代理请求日志 ----------------

#[derive(Serialize, Clone)]
pub struct ProxyLogEntry {
    pub id: String,
    pub timestamp: String,
    pub method: String,
    pub host: String,
    pub path: String,
    pub status: String,
    pub size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sse_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sse_tokens: Option<String>,
}

#[derive(Serialize)]
pub struct ProxyLogListResult {
    pub entries: Vec<ProxyLogEntry>,
    pub total: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyLogQueryOpts {
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub start_time: Option<String>,
    #[serde(default)]
    pub end_time: Option<String>,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

fn proxy_log_dir(state: &State<AppState>) -> std::path::PathBuf {
    let settings = state.settings();
    settings
        .proxy_log_path
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| state.logs_dir())
}

/// 解析单条代理日志，提取摘要信息
fn parse_proxy_entry(raw: &str, file_name: &str, index: usize) -> Option<ProxyLogEntry> {
    let lines: Vec<&str> = raw.lines().collect();
    if lines.is_empty() {
        return None;
    }

    // 找到时间戳行: [2024-01-15 10:30:00] METHOD host/path
    // 或: [2024-01-15 10:30:00] [WebSocket Upgrade] host/path
    let header_line = lines.iter().find(|l| l.starts_with('['))?;
    let timestamp = header_line
        .get(1..20)
        .unwrap_or("")
        .to_string();

    let rest = &header_line[header_line.find("] ").map(|i| i + 2).unwrap_or(0)..];

    let (method, host, path, status) = if rest.starts_with("[WebSocket") {
        // WebSocket 条目
        let hp = rest.find("] ").map(|i| &rest[i + 2..]).unwrap_or(rest);
        let (host, path) = split_host_path(hp);
        ("WebSocket".to_string(), host, path, "101 Upgrade".to_string())
    } else {
        // 普通请求
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        let raw_method = parts.first().unwrap_or(&"").to_string();
        let hp = parts.get(1).unwrap_or(&"");
        let (host, path) = split_host_path(hp);
        // 从内容中提取状态码
        let status = raw
            .lines()
            .find(|l| l.starts_with("--- Response:"))
            .and_then(|l| {
                l.trim_start_matches("--- Response: ")
                    .trim_end_matches(" ---")
                    .to_string()
                    .into()
            })
            .unwrap_or_else(|| "-".to_string());
        (format!("HTTP {}", raw_method), host, path, status)
    };

    Some(ProxyLogEntry {
        id: format!("{}:{}", file_name, index),
        timestamp,
        method,
        host,
        path,
        status,
        size: raw.len(),
        sse_model: extract_sse_field(raw, "model"),
        sse_tokens: extract_sse_tokens(raw),
    })
}

/// 从 SSE Summary 区块中提取指定字段
fn extract_sse_field(raw: &str, field: &str) -> Option<String> {
    let in_summary = raw.lines().skip_while(|l| !l.starts_with("--- SSE Summary ---"));
    for line in in_summary {
        let line = line.trim();
        if line.starts_with("--- ") && !line.starts_with("--- SSE Summary") {
            break;
        }
        if let Some(rest) = line.strip_prefix(&format!("  {}: ", field)) {
            return Some(rest.to_string());
        }
    }
    None
}

/// 提取 token 用量摘要字符串
fn extract_sse_tokens(raw: &str) -> Option<String> {
    let pt = extract_sse_field(raw, "prompt_tokens")?;
    let ct = extract_sse_field(raw, "completion_tokens").unwrap_or_else(|| "?".to_string());
    let tt = extract_sse_field(raw, "total_tokens").unwrap_or_else(|| "?".to_string());
    Some(format!("p:{} c:{} t:{}", pt, ct, tt))
}

fn split_host_path(hp: &str) -> (String, String) {
    // hp 可能是 "api.trae.cn/trae/api/..." 或 "api.trae.cn"
    if let Some(idx) = hp.find('/') {
        (hp[..idx].to_string(), hp[idx..].to_string())
    } else {
        (hp.to_string(), String::new())
    }
}

#[tauri::command]
pub fn proxy_logs_list(
    state: State<AppState>,
    opts: ProxyLogQueryOpts,
) -> Result<ProxyLogListResult, String> {
    let log_dir = proxy_log_dir(&state);
    if !log_dir.exists() {
        return Ok(ProxyLogListResult {
            entries: vec![],
            total: 0,
        });
    }

    // 列出所有 proxy_req_*.log 文件，按文件名升序（旧文件在前）
    // 这样 all_entries 中条目按时间正序排列（旧→新），reverse() 后得到正确的时间倒序（新→旧）
    let mut files: Vec<String> = std::fs::read_dir(&log_dir)
        .map_err(|e| format!("读取代理日志目录失败: {e}"))?
        .filter_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().to_string_lossy().to_string();
            // 只匹配 proxy_req_ 前缀，排除 proxy.log（操作日志）和其他日志
            if name.starts_with("proxy_req_") && name.ends_with(".log") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    files.sort_by(|a, b| a.cmp(b));

    let keyword = opts.keyword.as_deref().unwrap_or("");
    let start = opts.start_time.as_deref().unwrap_or("");
    let end = opts.end_time.as_deref().unwrap_or("");
    let offset = opts.offset.unwrap_or(0);
    let limit = opts.limit.unwrap_or(50);

    let mut all_entries: Vec<ProxyLogEntry> = Vec::new();

    for file_name in &files {
        let path = log_dir.join(file_name);
        let content = match std::fs::read(&path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
            Err(_) => continue,
        };

        // 按 ====== 分隔条目
        let mut index = 0;
        for chunk in content.split("================================================================================") {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                continue;
            }

            // 时间过滤
            if !start.is_empty() || !end.is_empty() {
                let ts = chunk
                    .lines()
                    .next()
                    .and_then(|l| l.get(1..20))
                    .unwrap_or("");
                if !start.is_empty() && ts < start {
                    continue;
                }
                if !end.is_empty() && ts > end {
                    continue;
                }
            }

            // 关键字过滤
            if !keyword.is_empty() && !chunk.to_lowercase().contains(&keyword.to_lowercase()) {
                continue;
            }

            if let Some(entry) = parse_proxy_entry(chunk, file_name, index) {
                all_entries.push(entry);
            }
            index += 1;
        }
    }

    // 文件按升序处理（旧→新），同文件内条目按写入顺序也是旧→新，
    // 因此 all_entries 整体为时间正序（旧→新），reverse() 后得到时间倒序（新→旧）
    all_entries.reverse();

    let total = all_entries.len();
    let entries = all_entries
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect();

    Ok(ProxyLogListResult { entries, total })
}

#[tauri::command]
pub fn proxy_log_detail(state: State<AppState>, id: String) -> Result<String, String> {
    // id 格式: "filename:index"
    let parts: Vec<&str> = id.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err("无效的日志 ID".into());
    }
    let file_name = parts[0];
    let index: usize = parts[1].parse().map_err(|_| "无效的索引")?;

    let log_dir = proxy_log_dir(&state);
    let path = log_dir.join(file_name);
    let content = std::fs::read(&path)
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        .map_err(|e| format!("读取日志文件失败: {e}"))?;

    let mut current = 0;
    for chunk in content.split("================================================================================") {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        if current == index {
            return Ok(chunk.to_string());
        }
        current += 1;
    }

    Err("找不到指定的日志条目".into())
}

// ---------------- JWT 解析 ----------------

#[derive(Serialize)]
pub struct JwtParseResult {
    pub user_id: Option<String>,
    pub exp_hours: Option<f64>,
    pub exp_timestamp: Option<i64>,
    pub status: String,
}

#[tauri::command]
pub fn jwt_parse(_app: AppHandle, _state: State<AppState>, jwt: String) -> JwtParseResult {
    let info = jwt::parse(&jwt);
    let status = jwt::status_of(info.exp_hours).to_string();
    JwtParseResult {
        user_id: info.user_id,
        exp_hours: info.exp_hours,
        exp_timestamp: info.exp_timestamp,
        status,
    }
}

// ---------------- 日志 ----------------

#[derive(Deserialize)]
pub struct LogsOpts {
    #[serde(default)]
    pub log_type: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct LogLine {
    pub time: String,
    pub log_type: String,
    pub message: String,
}

#[tauri::command]
pub fn logs_query(state: State<AppState>, opts: LogsOpts) -> Vec<LogLine> {
    let files = [
        ("proxy", "proxy.log"),
        ("checkin", "checkin.log"),
        ("switch", "switcher.log"),
    ];
    let mut out = Vec::new();
    for (t, fname) in files {
        if let Some(ref want) = opts.log_type {
            if want != "all" && want != t {
                continue;
            }
        }
        let p = state.path("logs").join(fname);
        if let Ok(bytes) = std::fs::read(&p) {
            let content = String::from_utf8_lossy(&bytes);
            for raw in content.lines() {
                let (time, msg) = split_time(raw);
                if let Some(ref date) = opts.date {
                    if !time.starts_with(date) {
                        continue;
                    }
                }
                if let Some(kw) = &opts.keyword {
                    if !msg.contains(kw) && !time.contains(kw) {
                        continue;
                    }
                }
                out.push(LogLine {
                    time,
                    log_type: t.to_string(),
                    message: msg,
                });
            }
        }
    }
    out.sort_by(|a, b| b.time.cmp(&a.time));
    let limit = opts.limit.unwrap_or(500);
    out.into_iter().take(limit).collect()
}

fn split_time(raw: &str) -> (String, String) {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    if raw.starts_with('[') {
        if let Some(end) = raw.find("] ") {
            let time = raw[1..end].to_string();
            return (time, raw[end + 2..].to_string());
        }
    }
    ("".to_string(), raw.to_string())
}

/// 清理指定类型日志文件（proxy/checkin/switch；all 为全清）。
/// 各写入方均为「每次追加时重新打开」，删除后文件按需自动重建，无需特殊处理。
/// 返回实际删除的文件数。
#[tauri::command]
pub fn logs_clear(state: State<AppState>, log_type: String) -> Result<u32, String> {
    let files = [
        ("proxy", "proxy.log"),
        ("checkin", "checkin.log"),
        ("switch", "switcher.log"),
    ];
    let mut removed = 0u32;
    for (t, fname) in files {
        if log_type != "all" && log_type != t {
            continue;
        }
        let p = state.path("logs").join(fname);
        match std::fs::remove_file(&p) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("删除 {fname} 失败: {e}")),
        }
    }
    crate::fs_utils::app_log(
        &state.data_dir,
        &format!("已清理日志: {log_type}（删除 {removed} 个文件）"),
    );
    Ok(removed)
}

// ---------------- 设置 ----------------

#[tauri::command]
pub fn settings_get(state: State<AppState>) -> Settings {
    state.settings()
}

#[tauri::command]
pub fn settings_set(state: State<AppState>, patch: serde_json::Value) -> Result<(), String> {
    let path = state.path("app_settings.json");
    // 读取现有设置，合并 patch 中出现的字段（真正的 patch 语义）
    let mut current: serde_json::Value = fs_utils::read_json(&path);
    // 文件不存在或内容为 null 时初始化为空对象，避免 patch 被丢弃
    if !current.is_object() {
        current = serde_json::json!({});
    }
    if let (Some(current_obj), Some(patch_obj)) =
        (current.as_object_mut(), patch.as_object())
    {
        for (k, v) in patch_obj {
            current_obj.insert(k.clone(), v.clone());
        }
    }
    fs_utils::write_json(&path, &current)
}

// ---------------- 积分历史（供看板/趋势图） ----------------

#[tauri::command]
pub fn credits_history(state: State<AppState>) -> Vec<CreditRecord> {
    fs_utils::read_json::<CreditsFile>(&state.path("credits_history.json")).records
}

// ---------------- 邀请 ----------------

#[derive(Serialize)]
pub struct Invite {
    pub url: String,
}

#[tauri::command]
pub fn invite_link(_app: AppHandle, _state: State<AppState>) -> Invite {
    Invite {
        url: INVITE_LINK.to_string(),
    }
}

// ---------------- 文件导出 ----------------

#[tauri::command]
pub fn write_text_file(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, content.as_bytes()).map_err(|e| format!("写入文件失败: {e}"))
}

/// 读取文本文件。仅允许 .json：当前唯一用途是账号导入（前端经文件对话框选择），
/// 收紧扩展名可避免前端被注入后借此读取任意敏感文件
#[tauri::command]
pub fn read_text_file(path: String) -> Result<String, String> {
    let ext = std::path::Path::new(&path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if ext != "json" {
        return Err("仅允许读取 .json 文件".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("读取文件失败: {e}"))?;
    String::from_utf8(bytes).map_err(|_| "文件不是有效的 UTF-8 文本".into())
}

// ---------------- 定时任务 ----------------
// Windows：schtasks 计划任务；macOS：launchd LaunchAgent（~/Library/LaunchAgents）

#[cfg(target_os = "windows")]
// 运行 schtasks 并正确解码输出。
// 关键：默认控制台代码页是 GBK（中文 Windows），schtasks 的中文报错(如"系统找不到指定的文件")
// 以 GBK 字节输出；若直接 from_utf8_lossy 会读成 ϵͳ... 乱码，导致 "找不到" 永远匹配不上、
// 错误文案变成乱码。前置 `chcp 65001` 让 schtasks 以 UTF-8 输出，从而能正确匹配与展示。
// 返回 (成功?, stdout, stderr)，三者均为 UTF-8 字符串。
fn run_schtasks(args: &[&str]) -> Result<(bool, String, String), String> {
    let mut full: Vec<String> = vec![
        "/c".to_string(),
        "chcp".to_string(),
        "65001".to_string(),
        ">nul".to_string(),
        "&&".to_string(),
        "schtasks".to_string(),
    ];
    for a in args {
        full.push((*a).to_string());
    }
    let out = Command::new("cmd")
        .args(&full)
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| format!("执行 schtasks 失败: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    Ok((out.status.success(), stdout, stderr))
}

#[cfg(target_os = "windows")]
/// 注册每日签到任务。async 派发：schtasks 调用约 1s，避免阻塞主线程
#[tauri::command(async)]
pub fn task_register(state: State<AppState>, time: String) -> Result<(), String> {
    // 直接调用 python 签到脚本（无界面、可定时），注入数据目录
    let py = state.python_exe.clone();
    let script = state.python_dir.join("auto_checkin.py");
    let data_dir = state.data_dir.to_string_lossy().to_string();
    // schtasks /TR 不会继承当前进程环境变量，需在命令行中显式设置 TRAEDATA_DIR。
    // 必须用 set "VAR=value"（带引号）以兼容含空格的路径（如 C:\Users\<带空格用户名>\...）；
    // 用 && 串联，仅当 set 成功后才执行 python。
    // 不再使用 /RL HIGHEST：签到脚本只读取/写入 %APPDATA% 并运行 python，无需提权，
    // 否则普通用户会卡在「access denied」而注册失败（详见问题分析报告）。
    let tr = format!(
        "cmd /c set \"TRAEDATA_DIR={}\" && \"{}\" \"{}\"",
        data_dir,
        py.replace('\\', "/"),
        script.to_string_lossy().replace('\\', "/")
    );
    let task_name = "TraeWorkAssistant_DailyCheckin";
    let (ok, _stdout, stderr) = run_schtasks(&[
        "/Create",
        "/TN",
        task_name,
        "/TR",
        tr.as_str(),
        "/SC",
        "DAILY",
        "/ST",
        time.as_str(),
        "/F",
    ])?;
    if !ok {
        let detail = stderr.trim();
        // 权限不足：最常见的失败原因（/RL HIGHEST 或普通用户受限）
        let is_access_denied = detail.contains("Access is denied")
            || detail.contains("ERROR: Access is denied")
            || detail.contains("拒绝访问")
            || detail.contains("权限");
        if is_access_denied {
            return Err(format!(
                "权限不足（Access Denied）。\n\n\
                 解决方法（任选其一）：\n\
                 1. 右键 TraeWorkAssistant →「以管理员身份运行」后重新点击「注册任务」\n\
                 2. 打开「管理员命令提示符」手动执行：\n\
                    schtasks /Create /TN TraeWorkAssistant_DailyCheckin /TR \"cmd /c set TRAEDATA_DIR={}&\\\"{}\\\" \\\"{}\\\"\" /SC DAILY /ST {} /F\n\
                 3. 如不需最高权限，可去掉 /RL HIGHEST 后重试",
                data_dir, py.replace('\\', "/"), script.to_string_lossy().replace('\\', "/"), time
            ));
        }
        return Err(detail.to_string());
    }
    Ok(())
}

/// 归一化路径用于比对：统一正斜杠 + 小写（Windows 路径大小写不敏感）。
#[cfg(target_os = "windows")]
fn norm_path(s: &str) -> String {
    s.replace('\\', "/").to_lowercase()
}

/// 基于已注册任务的 CSV /V 查询输出，校验其 /TR 是否仍指向当前安装布局。
#[cfg(target_os = "windows")]
fn drift_from_csv(csv_out: &str, py_exe: &str, script_path: &std::path::Path) -> Option<String> {
    let out = norm_path(csv_out);
    let script = norm_path(&script_path.to_string_lossy());
    if !out.contains(&script) {
        return Some(format!(
            "\n⚠️ 签到任务指向的脚本路径已失效（可能因升级迁移了安装目录），定时签到将静默失败，请重新「注册任务」。\n当前脚本位置: {}",
            script_path.display()
        ));
    }
    let script_dir_py = std::path::Path::new(py_exe);
    if script_dir_py.is_absolute() {
        let py = norm_path(py_exe);
        if !out.contains(&py) {
            return Some(
                "\n⚠️ 签到任务仍绑定旧版解释器路径，建议重新「注册任务」以切换到内置 Python 运行时。".to_string(),
            );
        }
    }
    None
}

/// 包装：实际发起 CSV /V 查询后做路径校验；查询失败不阻塞状态展示（返回 None）。
#[cfg(target_os = "windows")]
fn task_path_drift(py_exe: &str, script_path: &std::path::Path) -> Option<String> {
    let (ok, csv, _stderr) = run_schtasks(&[
        "/Query",
        "/TN",
        "TraeWorkAssistant_DailyCheckin",
        "/FO",
        "CSV",
        "/V",
    ])
    .ok()?;
    if !ok {
        return None;
    }
    drift_from_csv(&csv, py_exe, script_path)
}

/// 查询每日签到任务状态。async 派发：schtasks 调用约 1s，避免阻塞主线程
#[cfg(target_os = "windows")]
#[tauri::command(async)]
pub fn task_status(_app: AppHandle, state: State<AppState>) -> Result<String, String> {
    let (ok, stdout, stderr) =
        run_schtasks(&["/Query", "/TN", "TraeWorkAssistant_DailyCheckin", "/FO", "LIST"])?;
    if !ok {
        let detail = if !stderr.trim().is_empty() {
            stderr.trim()
        } else {
            stdout.trim()
        };
        // 任务本就不存在：返回友好提示而非带乱码的错误，避免前端叠加"查询失败："前缀
        if detail.contains("can't find")
            || detail.contains("找不到")
            || detail.contains("does not exist")
            || detail.contains("ERROR: The system cannot find")
            || detail.contains("系统找不到")
        {
            return Ok("未注册每日签到任务（请先在设置页点击「注册任务」）。".to_string());
        }
        return Err(detail.to_string());
    }
    // 路径漂移校验：仅在脚本目录为绝对路径（安装/便携版）时执行；
    // 开发期 python_dir 为相对路径（src-python），与任务内绝对路径不具可比性。
    let mut info = stdout;
    if state.python_dir.is_absolute() {
        let script = state.python_dir.join("auto_checkin.py");
        if let Some(warn) = task_path_drift(&state.python_exe, &script) {
            info.push_str(&warn);
        }
    }
    Ok(info)
}

/// 注销每日签到任务。async 派发：schtasks 调用约 1s，避免阻塞主线程
#[cfg(target_os = "windows")]
#[tauri::command(async)]
pub fn task_unregister(_app: AppHandle, _state: State<AppState>) -> Result<(), String> {
    let (ok, _stdout, stderr) =
        run_schtasks(&["/Delete", "/TN", "TraeWorkAssistant_DailyCheckin", "/F"])?;
    if !ok {
        let detail = stderr.trim();
        // 任务本就不存在：视为已删除，不报错
        if detail.contains("can't find")
            || detail.contains("找不到")
            || detail.contains("does not exist")
            || detail.contains("ERROR: The system cannot find")
            || detail.contains("系统找不到")
        {
            return Ok(());
        }
        return Err(detail.to_string());
    }
    Ok(())
}

// ---------------- macOS 定时任务（launchd LaunchAgent） ----------------

/// LaunchAgent 标识与 plist 路径
#[cfg(not(target_os = "windows"))]
const LAUNCHD_LABEL: &str = "com.traework.assistant.daily-checkin";

#[cfg(not(target_os = "windows"))]
fn launchd_plist_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"))
}

/// 解析 "HH:MM" 为 (hour, minute)
#[cfg(not(target_os = "windows"))]
fn parse_hhmm(time: &str) -> Result<(u32, u32), String> {
    let parts: Vec<&str> = time.trim().split(':').collect();
    if parts.len() != 2 {
        return Err(format!("时间格式应为 HH:MM，当前为 {time}"));
    }
    let h: u32 = parts[0]
        .parse()
        .map_err(|_| format!("小时解析失败: {}", parts[0]))?;
    let m: u32 = parts[1]
        .parse()
        .map_err(|_| format!("分钟解析失败: {}", parts[1]))?;
    if h > 23 || m > 59 {
        return Err(format!("时间超出范围: {time}"));
    }
    Ok((h, m))
}

/// 生成 launchd plist 内容（每日 HH:MM 运行签到脚本）
#[cfg(not(target_os = "windows"))]
fn build_plist(py_exe: &str, script: &std::path::Path, data_dir: &str, hour: u32, minute: u32, out_log: &str) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{py}</string>
        <string>{script}</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>TRAEDATA_DIR</key>
        <string>{data_dir}</string>
        <key>PYTHONIOENCODING</key>
        <string>utf-8</string>
    </dict>
    <key>StartCalendarInterval</key>
    <dict>
        <key>Hour</key>
        <integer>{hour}</integer>
        <key>Minute</key>
        <integer>{minute}</integer>
    </dict>
    <key>StandardOutPath</key>
    <string>{out_log}</string>
    <key>StandardErrorPath</key>
    <string>{out_log}</string>
</dict>
</plist>
"#,
        py = esc(py_exe),
        script = esc(&script.to_string_lossy()),
        data_dir = esc(data_dir),
        out_log = esc(out_log),
    )
}

/// 注册每日签到任务（macOS launchd）。async 派发：launchctl 调用约 1s，避免阻塞主线程
#[cfg(not(target_os = "windows"))]
#[tauri::command(async)]
pub fn task_register(state: State<AppState>, time: String) -> Result<(), String> {
    let (hour, minute) = parse_hhmm(&time)?;
    let py = state.python_exe.clone();
    let script = state.python_dir.join("auto_checkin.py");
    if !script.exists() {
        return Err(format!("找不到签到脚本: {}", script.display()));
    }
    let data_dir = state.data_dir.to_string_lossy().to_string();
    let out_log = state
        .logs_dir()
        .join("launchd_checkin.log")
        .to_string_lossy()
        .to_string();
    let plist = build_plist(&py, &script, &data_dir, hour, minute, &out_log);
    let plist_path = launchd_plist_path();
    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 LaunchAgents 目录失败: {e}"))?;
    }
    std::fs::write(&plist_path, plist.as_bytes())
        .map_err(|e| format!("写入 plist 失败: {e}"))?;
    // 重新加载（旧任务可能已加载，先 unload 忽略错误）
    let _ = Command::new("launchctl")
        .args(["unload", "-w", &plist_path.to_string_lossy()])
        .output();
    let out = Command::new("launchctl")
        .args(["load", "-w", &plist_path.to_string_lossy()])
        .output()
        .map_err(|e| format!("执行 launchctl 失败: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!("launchctl load 失败: {err}"));
    }
    Ok(())
}

/// 查询每日签到任务状态（macOS launchd）
#[cfg(not(target_os = "windows"))]
#[tauri::command(async)]
pub fn task_status(_app: AppHandle, state: State<AppState>) -> Result<String, String> {
    let plist_path = launchd_plist_path();
    if !plist_path.exists() {
        return Ok("未注册每日签到任务（请先在设置页点击「注册任务」）。".to_string());
    }
    let out = Command::new("launchctl")
        .args(["list", LAUNCHD_LABEL])
        .output()
        .map_err(|e| format!("执行 launchctl 失败: {e}"))?;
    let mut info = if out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let first = stdout.lines().next().unwrap_or("").to_string();
        format!(
            "每日签到任务已注册（launchd），触发时间见 plist：{}\n{}",
            plist_path.display(),
            first
        )
    } else {
        format!(
            "每日签到任务已注册但未加载（重启后自动生效），plist：{}",
            plist_path.display()
        )
    };
    // 路径漂移校验：plist 中的脚本路径是否与当前布局一致
    let script = state.python_dir.join("auto_checkin.py");
    if state.python_dir.is_absolute() {
        let content = std::fs::read_to_string(&plist_path).unwrap_or_default();
        let script_s = script.to_string_lossy().to_string();
        if !content.contains(&script_s) {
            info.push_str(&format!(
                "\n⚠️ 签到任务指向的脚本路径已失效（可能因升级迁移了安装目录），定时签到将静默失败，请重新「注册任务」。\n当前脚本位置: {}",
                script.display()
            ));
        }
    }
    Ok(info)
}

/// 注销每日签到任务（macOS launchd）
#[cfg(not(target_os = "windows"))]
#[tauri::command(async)]
pub fn task_unregister(_app: AppHandle, _state: State<AppState>) -> Result<(), String> {
    let plist_path = launchd_plist_path();
    if !plist_path.exists() {
        return Ok(()); // 不存在视为已删除
    }
    let _ = Command::new("launchctl")
        .args(["unload", "-w", &plist_path.to_string_lossy()])
        .output();
    std::fs::remove_file(&plist_path).map_err(|e| format!("删除 plist 失败: {e}"))?;
    Ok(())
}

#[cfg(all(test, target_os = "windows"))]
mod task_status_tests {
    use super::{drift_from_csv, norm_path};
    use std::path::Path;

    const PY: &str = "C:\\Program Files\\Trae Work 助手\\python\\python.exe";
    const SCRIPT: &str = "C:\\Program Files\\Trae Work 助手\\python\\auto_checkin.py";

    /// 构造模拟 schtasks /FO CSV /V 的输出行（Task To Run 字段含目标命令）
    fn csv_with(task_to_run: &str) -> String {
        format!(
            "\"主机名\",\"任务名\",\"2026/9/10 8:00:00\",\"就绪\",\"task_host\",...,\"{}\",\"tianw\",\"已启用\"",
            task_to_run
        )
    }

    #[test]
    fn paths_match_returns_none() {
        let tr = format!(
            "set \"TRAEDATA_DIR=C:/Users/tianw/AppData/Roaming/TraeWorkAssistant\" && \"{}\" \"{}\"",
            PY, SCRIPT
        );
        assert_eq!(
            drift_from_csv(&csv_with(&tr), PY, Path::new(SCRIPT)),
            None
        );
    }

    #[test]
    fn case_and_slash_insensitive_match() {
        // 任务里是全小写 + 正斜杠（task_register 落库形态），当前布局是反斜杠
        let tr = format!(
            "set \"TRAEDATA_DIR=c:/users/tianw/appdata/roaming/traeworkassistant\" && \"{}\" \"{}\"",
            norm_path(PY),
            norm_path(SCRIPT)
        );
        assert_eq!(
            drift_from_csv(&csv_with(&tr), PY, Path::new(SCRIPT)),
            None
        );
    }

    #[test]
    fn missing_script_dir_reports_stale() {
        // MSI→NSIS 迁移：任务仍指向旧安装目录
        let old_script = "D:\\Old\\Trae Work 助手\\python\\auto_checkin.py";
        let tr = format!(
            "set \"TRAEDATA_DIR=C:/Users/tianw/AppData/Roaming/TraeWorkAssistant\" && \"{}\" \"{}\"",
            PY, old_script
        );
        let r = drift_from_csv(&csv_with(&tr), PY, Path::new(SCRIPT))
            .expect("应检测到脚本路径漂移");
        assert!(r.contains("脚本路径已失效"));
    }

    #[test]
    fn old_system_python_reports_stale() {
        // 脚本路径仍在原目录，但任务绑定的是旧版系统 Python
        let sys_py = "C:/Users/tianw/AppData/Local/Programs/Python/Python312/python.exe";
        let tr = format!(
            "set \"TRAEDATA_DIR=C:/Users/tianw/AppData/Roaming/TraeWorkAssistant\" && \"{}\" \"{}\"",
            sys_py, SCRIPT
        );
        let r = drift_from_csv(&csv_with(&tr), PY, Path::new(SCRIPT))
            .expect("应检测到解释器路径漂移");
        assert!(r.contains("旧版解释器"));
    }

    #[test]
    fn bare_python_name_skips_interpreter_check() {
        // python_exe 为裸名（PATH 解析）时无法校验，不应误报
        let tr = format!(
            "set \"TRAEDATA_DIR=C:/Users/tianw/AppData/Roaming/TraeWorkAssistant\" && \"python\" \"{}\"",
            SCRIPT
        );
        assert_eq!(
            drift_from_csv(&csv_with(&tr), "python", Path::new(SCRIPT)),
            None
        );
    }
}
