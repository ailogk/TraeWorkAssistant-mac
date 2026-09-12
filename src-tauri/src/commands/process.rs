//! F-47 进程管理增强：三级关闭策略
//!
//! 参考社区实现（oss-ecosystem-research §5.3 rotate.rs）的三级关闭思路：
//! 1. **优雅关闭**：向主进程发送关闭请求（Windows: taskkill 不带 /F 发送 WM_CLOSE；
//!    macOS: osascript `quit app`），等待最长 3s，让 Electron 尽量正常落盘
//!    （避免强杀导致 leveldb/vscdb 文件锁与数据损坏）；
//! 2. **强杀进程树**：Windows taskkill /T /F；macOS pkill -9 -x，等待最长 2s；
//! 3. **人工介入**：仍存活则返回 Err，由前端 toast 提示用户手动关闭。
//!
//! 匹配策略：仅按主程序映像名/进程名精确匹配，crashpad-helper 等子进程不会独立
//! 命中；树杀/整组清理阶段随主进程一并清理。
//! Windows 下所有子进程均以 CREATE_NO_WINDOW（0x08000000）拉起，不闪烁控制台窗口。

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// 检查任一映像名是否仍在运行（精确匹配进程名）
#[cfg(target_os = "windows")]
pub fn images_running(images: &[&str]) -> Vec<String> {
    let mut alive = Vec::new();
    for img in images {
        let running = Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {img}"), "/NH"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map(|o| {
                let s = String::from_utf8_lossy(&o.stdout);
                s.to_lowercase().contains(&img.to_lowercase())
            })
            .unwrap_or(false);
        if running {
            alive.push(img.to_string());
        }
    }
    alive
}

/// macOS：pgrep -x 精确匹配进程名（不区分大小写由 pgrep 自身保证大小写不敏感）
#[cfg(not(target_os = "windows"))]
pub fn images_running(images: &[&str]) -> Vec<String> {
    let mut alive = Vec::new();
    for img in images {
        let running = Command::new("pgrep")
            .arg("-x")
            .arg(img)
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false);
        if running {
            alive.push(img.to_string());
        }
    }
    alive
}

#[cfg(target_os = "windows")]
fn run_taskkill(args: &[&str]) {
    let _ = Command::new("taskkill")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output();
}

/// macOS：向应用发送优雅退出请求（等同点击 Cmd+Q，让 Electron 正常落盘）
#[cfg(target_os = "macos")]
fn graceful_quit(img: &str) {
    let script = format!("tell application \"{img}\" to quit");
    let _ = Command::new("osascript")
        .args(["-e", &script])
        .output();
}

/// macOS：按进程名强杀（-x 精确匹配；不 -9 时先发 SIGTERM）
#[cfg(not(target_os = "windows"))]
fn force_kill(img: &str, sigkill: bool) {
    let mut cmd = Command::new("pkill");
    if sigkill {
        cmd.arg("-9");
    }
    cmd.arg("-x").arg(img);
    let _ = cmd.output();
}

/// 三级关闭指定映像名的进程。
/// 返回 Ok(()) 表示全部退出；返回 Err 提示人工介入。
pub fn graceful_kill_images(images: &[&str]) -> Result<(), String> {
    let alive = images_running(images);
    if alive.is_empty() {
        return Ok(());
    }

    // ---- 第一级：优雅关闭，最长等待 3s ----
    #[cfg(target_os = "windows")]
    for img in &alive {
        // taskkill 不带 /F：向有窗口的进程发送关闭消息
        run_taskkill(&["/IM", img]);
    }
    #[cfg(target_os = "macos")]
    for img in &alive {
        graceful_quit(img);
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    for img in &alive {
        force_kill(img, false);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if images_running(images).is_empty() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    // ---- 第二级：强杀进程树，最长等待 2s ----
    #[cfg(target_os = "windows")]
    for img in &alive {
        run_taskkill(&["/T", "/F", "/IM", img]);
    }
    #[cfg(not(target_os = "windows"))]
    for img in &alive {
        force_kill(img, true);
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if images_running(images).is_empty() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    // ---- 第三级：人工介入 ----
    // 终判兜底：等待循环的最后一次探测与超时退出之间存在竞态窗口，
    // 进程可能在窗口内恰好全部退出 —— 此时不应误报失败，
    // 否则上层（如 open_trae_app 的 ? 传播）会中止本应继续的启动流程。
    let still = images_running(images);
    if still.is_empty() {
        return Ok(());
    }
    Err(format!(
        "进程 {} 未能自动关闭（优雅关闭与强制结束均失败），请手动关闭后重试",
        still.join("、")
    ))
}
