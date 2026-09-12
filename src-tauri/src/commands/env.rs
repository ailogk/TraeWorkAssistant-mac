use serde::Serialize;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::process::Command;
use tauri::{AppHandle, State};

use crate::state::AppState;

#[derive(Serialize)]
pub struct EnvStatus {
    pub installed: bool,
    pub running: bool,
    pub version: Option<String>,
    pub path: Option<String>,
}

/// 环境探测。async 派发：detect_trae 回退路径含注册表递归搜索（Windows）
/// 与 PowerShell/plutil 版本读取（冷启动数百 ms），同步命令跑主线程会冻结 UI
#[tauri::command(async)]
pub fn env_check(_app: AppHandle, state: State<AppState>) -> EnvStatus {
    let (installed, path, version) = detect_trae(state.settings().trae_path);
    let running = is_running();
    EnvStatus {
        installed,
        running,
        version,
        path,
    }
}

#[tauri::command]
pub fn open_trae_website(_app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/c", "start", "https://www.trae.cn"])
            .creation_flags(0x08000000)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        Command::new("open")
            .arg("https://www.trae.cn")
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 启动本地 Trae Work 客户端。
/// 若传入 proxy_port（代理运行中），自动注入 `--proxy-server` 让 Trae 走本地代理，
/// 无需用户在 Trae 设置里手动配置代理。
/// async 派发：注入代理前会执行 F-47 三级关闭（最长约 5s 轮询等待），
/// 同步命令跑主线程会冻结 UI。
#[tauri::command(async)]
pub fn open_trae_app(_app: AppHandle, state: State<AppState>, proxy_port: Option<u16>) -> Result<(), String> {
    let (installed, path, _) = detect_trae(state.settings().trae_path);
    if !installed {
        return Err("未检测到本地 Trae Work 安装，请在「设置 → 代理与签到」中指定应用路径".into());
    }
    let exe = path.ok_or("未找到 Trae Work 可执行文件路径")?;
    // F-47：自动探测成功的路径持久化到 app_settings.json 兜底，
    // 之后即使注册表/默认目录变化，也能用上次成功的路径直接启动
    persist_detected_path(&state, "trae_path", &exe);
    // 代理注入要求 Trae 以 --proxy-server 启动。Electron 单实例下，已运行的窗口会忽略新启动
    // 参数，再次点击只会聚焦旧窗口，导致全程不走代理、无法捕获账号。故注入代理前先关闭现有
    // 进程，确保参数真正生效（F-47 三级关闭：优雅关闭→树杀强杀→人工介入提示）。
    // （无代理时正常打开，不杀进程。）
    if proxy_port.is_some() {
        // 仅按检测到的 exe 映像名查杀：避免按整个候选列表（含国际版 Trae）
        // 误杀用户其他版本
        let img = process_name_from_path(&exe)
            .ok_or("无法解析 Trae Work 可执行文件名")?;
        crate::commands::process::graceful_kill_images(&[img.as_str()])?;
    }
    launch_trae(&exe, proxy_port)?;
    Ok(())
}

/// 从检测到的路径推导目标进程名（用于精确查杀）。
/// Windows：exe 文件名去扩展名；macOS：.app 包名去掉 .app 后缀（进程名 = 包名），
/// 若直接指向 Contents/MacOS 内的可执行文件则取文件名。
fn process_name_from_path(path: &str) -> Option<String> {
    let name = std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())?;
    #[cfg(target_os = "macos")]
    let name = name
        .strip_suffix(".app")
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            // Contents/MacOS/<binary> 形式，直接取文件名
            name
        });
    let _ = path;
    Some(name)
}

/// 按平台启动 Trae：Windows 直接 spawn exe；
/// macOS 支持 .app 包（open -a）与裸可执行文件（直接 spawn）两种形态。
fn launch_trae(path: &str, proxy_port: Option<u16>) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new(path);
        if let Some(port) = proxy_port {
            // Electron/Chromium 支持 --proxy-server 启动参数
            cmd.arg(format!("--proxy-server=http://127.0.0.1:{port}"));
        }
        cmd.spawn()
            .map_err(|e| format!("启动 Trae Work 失败: {e}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let is_app_bundle = std::path::Path::new(path)
            .extension()
            .map(|e| e == "app")
            .unwrap_or(false)
            && std::path::Path::new(path).is_dir();
        if is_app_bundle {
            let mut cmd = Command::new("open");
            cmd.arg("-a").arg(path);
            if let Some(port) = proxy_port {
                cmd.arg("--args").arg(format!("--proxy-server=http://127.0.0.1:{port}"));
            }
            cmd.spawn()
                .map_err(|e| format!("启动 Trae Work 失败: {e}"))?;
        } else {
            let mut cmd = Command::new(path);
            if let Some(port) = proxy_port {
                cmd.arg(format!("--proxy-server=http://127.0.0.1:{port}"));
            }
            cmd.spawn()
                .map_err(|e| format!("启动 Trae Work 失败: {e}"))?;
        }
    }
    Ok(())
}

/// exe 路径持久化兜底（F-47）：自动探测成功时把结果写入 app_settings.json，
/// 之后即使注册表/默认目录变化，也能用上次成功的路径直接启动。
/// 用户手动指定（设置页）优先级更高，且仅在探测值与存量值不同时写盘。
fn persist_detected_path(state: &State<AppState>, key: &str, exe: &str) {
    let path = state.path("app_settings.json");
    let mut current: serde_json::Value = crate::fs_utils::read_json(&path);
    if !current.is_object() {
        current = serde_json::json!({});
    }
    let changed = current
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s != exe)
        .unwrap_or(true);
    if changed {
        if let Some(obj) = current.as_object_mut() {
            obj.insert(key.to_string(), serde_json::json!(exe));
        }
        let _ = crate::fs_utils::write_json(&path, &current);
    }
}

/// 用户指定路径是否有效：Windows 要求是文件；macOS 允许 .app 包目录或可执行文件
fn custom_path_valid(p: &str) -> bool {
    let path = std::path::Path::new(p);
    if path.is_file() {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        path.is_dir() && p.ends_with(".app")
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn detect_trae(custom: Option<String>) -> (bool, Option<String>, Option<String>) {
    // 优先使用用户在设置中指定的路径（兼容自定义安装目录）
    if let Some(p) = custom {
        let p = p.trim().to_string();
        if !p.is_empty() && custom_path_valid(&p) {
            let version = version_of(&p);
            return (true, Some(p), version);
        }
    }
    #[cfg(target_os = "windows")]
    {
        let candidates = [
            "%LOCALAPPDATA%\\Programs\\TRAE SOLO CN\\TRAE SOLO CN.exe",
            "%LOCALAPPDATA%\\Programs\\TRAE SOLO\\TRAE SOLO.exe",
            "%ProgramFiles%\\TRAE SOLO CN\\TRAE SOLO CN.exe",
            "%ProgramFiles%\\TRAE SOLO\\TRAE SOLO.exe",
            "%LOCALAPPDATA%\\Programs\\Trae\\Trae.exe",
            "%ProgramFiles%\\Trae\\Trae.exe",
        ];
        for c in candidates {
            let expanded = expand_env(c);
            if std::path::Path::new(&expanded).exists() {
                let version = version_of(&expanded);
                return (true, Some(expanded), version);
            }
        }
        // 回退：注册表查询
        if let Some(p) = registry_trae_path() {
            let version = version_of(&p);
            return (true, Some(p), version);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        let candidates = [
            "/Applications/TRAE SOLO CN.app".to_string(),
            format!("{home}/Applications/TRAE SOLO CN.app"),
            "/Applications/TRAE SOLO.app".to_string(),
            format!("{home}/Applications/TRAE SOLO.app"),
            "/Applications/Trae.app".to_string(),
            format!("{home}/Applications/Trae.app"),
        ];
        for c in candidates {
            if std::path::Path::new(&c).is_dir() {
                let version = version_of(&c);
                return (true, Some(c), version);
            }
        }
    }
    (false, None, None)
}

#[cfg(target_os = "windows")]
fn expand_env(p: &str) -> String {
    p.replace("%LOCALAPPDATA%", &std::env::var("LOCALAPPDATA").unwrap_or_default())
        .replace("%ProgramFiles%", &std::env::var("ProgramFiles").unwrap_or_default())
}

#[cfg(target_os = "windows")]
fn version_of(path: &str) -> Option<String> {
    let ps = format!(
        "(Get-Item '{}').VersionInfo.FileVersion",
        path.replace('\'', "''")
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps])
        .creation_flags(0x08000000)
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// macOS：从 .app 包的 Info.plist 读取版本号
#[cfg(target_os = "macos")]
fn version_of(path: &str) -> Option<String> {
    let plist = if path.ends_with(".app") {
        format!("{path}/Contents/Info.plist")
    } else {
        path.to_string()
    };
    if !std::path::Path::new(&plist).is_file() {
        return None;
    }
    let out = Command::new("defaults")
        .args(["read", &plist, "CFBundleShortVersionString"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn version_of(_path: &str) -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
fn registry_trae_path() -> Option<String> {
    for root in ["HKCU", "HKLM"] {
        let out = match Command::new("reg")
            .args([
                "query",
                &format!("{root}\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall"),
                "/s",
                "/f",
                "TRAE",
            ])
            .creation_flags(0x08000000)
            .output()
        {
            Ok(o) => o,
            Err(_) => continue,
        };
        let s = String::from_utf8_lossy(&out.stdout);
        // reg query /s 以 HKEY_ 开头的行分隔每个注册表键，逐键解析
        let mut icon: Option<String> = None;
        let mut loc: Option<String> = None;
        let mut name_ok = false;
        let mut best: Option<String> = None;
        for line in s.lines() {
            let line = line.trim();
            if line.starts_with("HKEY_") {
                if name_ok {
                    if let Some(p) = resolve_reg_candidate(&icon, &loc) {
                        best = Some(p);
                        break;
                    }
                }
                icon = None;
                loc = None;
                name_ok = false;
                continue;
            }
            if let Some(v) = line.strip_prefix("DisplayName") {
                if let Some(val) = v.split("REG_SZ").nth(1) {
                    if val.to_uppercase().contains("TRAE") {
                        name_ok = true;
                    }
                }
            } else if let Some(v) = line.strip_prefix("DisplayIcon") {
                if let Some(val) = v.split("REG_SZ").nth(1) {
                    icon = Some(val.trim().to_string());
                }
            } else if let Some(v) = line.strip_prefix("InstallLocation") {
                if let Some(val) = v.split("REG_SZ").nth(1) {
                    loc = Some(val.trim().to_string());
                }
            }
        }
        if name_ok {
            if let Some(p) = resolve_reg_candidate(&icon, &loc) {
                best = Some(p);
            }
        }
        if best.is_some() {
            return best;
        }
    }
    None
}

/// 从注册表 DisplayIcon / InstallLocation 推导 exe 路径
#[cfg(target_os = "windows")]
fn resolve_reg_candidate(icon: &Option<String>, loc: &Option<String>) -> Option<String> {
    if let Some(icon) = icon {
        if icon.to_lowercase().ends_with(".exe") && std::path::Path::new(icon).is_file() {
            return Some(icon.clone());
        }
    }
    if let Some(loc) = loc {
        for name in ["TRAE SOLO CN.exe", "TRAE SOLO.exe", "Trae.exe"] {
            let cand = format!("{loc}\\{name}");
            if std::path::Path::new(&cand).is_file() {
                return Some(cand);
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn is_running() -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq TRAE SOLO CN.exe", "/NH"])
        .creation_flags(0x08000000)
        .output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.contains("TRAE SOLO CN.exe")
        }
        Err(_) => false,
    }
}

#[cfg(not(target_os = "windows"))]
fn is_running() -> bool {
    // macOS Electron 进程名 = .app 包名
    ["TRAE SOLO CN", "TRAE SOLO"].iter().any(|name| {
        Command::new("pgrep")
            .arg("-x")
            .arg(name)
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false)
    })
}
