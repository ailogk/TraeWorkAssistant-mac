//! 跨平台切换桥脚本启动器（switch.rs / profile.rs 共用）。
//!
//! Windows 调 ps/trae-switch-bridge.ps1（PowerShell），
//! macOS 调 ps/trae-switch-bridge.py（python3，仅标准库）。
//! 两个脚本以相同的 NDJSON 协议输出进度（stage/status/message/time），
//! 本模块统一负责 spawn + stdout 事件转发 + stderr 落盘 switcher.log。

use std::io::{BufRead, BufReader, Write};
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};

use tauri::{AppHandle, Emitter};

use crate::fs_utils;

/// 桥脚本路径（按平台选择脚本文件名）
pub fn bridge_path() -> std::path::PathBuf {
    let name = if cfg!(target_os = "macos") {
        "trae-switch-bridge.py"
    } else {
        "trae-switch-bridge.ps1"
    };
    crate::state::resolve_ps_dir().join(name)
}

/// 跨平台启动桥脚本。
/// action ∈ {Switch, ResetMachineId, BackupCurrent, RestoreOnly, ResetDeviceIds, SaveCurrentLogin}
pub fn spawn_bridge(action: &str, user_id: Option<&str>) -> Result<Child, String> {
    let bridge = bridge_path();
    if !bridge.exists() {
        return Err(format!("找不到切换脚本: {}", bridge.display()));
    }
    let bridge_s = bridge.to_string_lossy().to_string();

    let mut cmd;
    #[cfg(target_os = "windows")]
    {
        cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", &bridge_s, "-Action", action]);
        if let Some(uid) = user_id {
            cmd.args(["-UserId", uid]);
        }
        cmd.arg("-Json");
        // CREATE_NO_WINDOW：隐藏切换时闪出的黑色控制台窗口
        cmd.creation_flags(0x08000000);
    }
    #[cfg(target_os = "macos")]
    {
        cmd = Command::new("python3");
        cmd.args([&bridge_s, "--action", action]);
        if let Some(uid) = user_id {
            cmd.args(["--user-id", uid]);
        }
        cmd.arg("--json");
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        return Err("当前平台不支持登录态切换".into());
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.spawn().map_err(|e| format!("启动桥脚本失败: {e}"))
}

/// 启动桥脚本并异步转发进度事件：
/// - stdout NDJSON 逐行 → `progress_event`（原样透传 JSON 行）
/// - 命中 stage=done/fatal → `done_event`（{success, raw, ...done_extra}）
/// - 进程结束若未输出过 done/fatal → 兜底发一次 `done_event`
/// - stderr 逐行追加到 data/logs/switcher.log（防管道写满死锁）
/// `done_extra`：附加到 done 事件的静态字段（如 profile 的 {"action":"backup"}）
#[allow(clippy::too_many_arguments)]
pub fn run_bridge_async(
    app: &AppHandle,
    data_dir: &std::path::Path,
    action: &str,
    user_id: Option<&str>,
    progress_event: &str,
    done_event: &str,
    done_extra: Option<serde_json::Value>,
) -> Result<(), String> {
    let mut child = spawn_bridge(action, user_id)?;
    let stdout = child.stdout.take().ok_or("桥脚本无输出")?;
    let stderr = child.stderr.take();

    let app2 = app.clone();
    let progress = progress_event.to_string();
    let done = done_event.to_string();

    // stdout 线程：NDJSON -> 进度/完成事件
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        let mut done_emitted = false;
        for line in reader.lines() {
            if let Ok(l) = line {
                let l = l.trim().to_string();
                if l.is_empty() {
                    continue;
                }
                let _ = app2.emit(&progress, &l);
                if l.contains("\"stage\":\"done\"") || l.contains("\"stage\":\"fatal\"") {
                    let success = l.contains("\"stage\":\"done\"");
                    done_emitted = true;
                    let mut payload = serde_json::json!({ "success": success, "raw": l });
                    if let (Some(obj), Some(extra)) = (payload.as_object_mut(), done_extra.as_ref()) {
                        for (k, v) in extra {
                            obj.insert(k.clone(), v.clone());
                        }
                    }
                    let _ = app2.emit(&done, payload);
                }
            }
        }
        let exit_status = child.wait();
        // 仅当脚本未输出 done/fatal 时才在结束时兜底 emit，避免重复发两次完成事件
        if !done_emitted {
            let success = matches!(&exit_status, Ok(s) if s.success());
            let mut payload =
                serde_json::json!({ "success": success, "raw": format!("exit: {:?}", exit_status) });
            if let (Some(obj), Some(extra)) = (payload.as_object_mut(), done_extra.as_ref()) {
                for (k, v) in extra {
                    obj.insert(k.clone(), v.clone());
                }
            }
            let _ = app2.emit(&done, payload);
        }
    });

    // stderr 线程：防止管道缓冲区写满导致子进程死锁
    if let Some(stderr) = stderr {
        let data_dir = data_dir.to_path_buf();
        std::thread::spawn(move || {
            let log_path = data_dir.join("logs").join("switcher.log");
            let _ = std::fs::create_dir_all(log_path.parent().unwrap_or(std::path::Path::new(".")));
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(l) = line {
                    let l = format!("[stderr] {}", l.trim());
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log_path)
                    {
                        let _ = writeln!(f, "[{}] {}", fs_utils::now_ts(), l);
                    }
                }
            }
        });
    }

    Ok(())
}
