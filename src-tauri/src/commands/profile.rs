use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, State};

use crate::fs_utils;
use crate::state::AppState;

/// 登录态快照信息
#[derive(Serialize, Clone)]
pub struct ProfileInfo {
    pub slot: String,
    pub size_bytes: u64,
    pub file_count: u64,
    pub last_modified: String,
}

/// profiles 根目录：%APPDATA%\TraeWorkAssistant\data\profiles\
fn profiles_dir(state: &State<AppState>) -> PathBuf {
    state.data_dir.join("data").join("profiles")
}

/// 递归计算目录大小和文件数
fn dir_stats(path: &std::path::Path) -> (u64, u64) {
    let mut size = 0u64;
    let mut count = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let (s, c) = dir_stats(&p);
                size += s;
                count += c;
            } else {
                size += entry.metadata().map(|m| m.len()).unwrap_or(0);
                count += 1;
            }
        }
    }
    (size, count)
}

/// 格式化文件大小
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// 列出所有已保存的登录态快照
#[tauri::command]
pub fn profile_list(state: State<AppState>) -> Vec<ProfileInfo> {
    let dir = profiles_dir(&state);
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                let slot = entry.file_name().to_string_lossy().to_string();
                let (size, count) = dir_stats(&entry.path());
                let last_modified = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| {
                        let dt = chrono::DateTime::<chrono::Local>::from(std::time::SystemTime::UNIX_EPOCH + d);
                        dt.format("%Y-%m-%d %H:%M:%S").to_string()
                    })
                    .unwrap_or_else(|| "-".to_string());
                out.push(ProfileInfo {
                    slot,
                    size_bytes: size,
                    file_count: count,
                    last_modified,
                });
            }
        }
    }
    // 按修改时间倒序
    out.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
    out
}

/// 备份当前 TRAE 登录态到指定 slot（跨平台桥脚本）
#[tauri::command]
pub fn profile_backup(
    app: AppHandle,
    state: State<AppState>,
    user_id: String,
) -> Result<(), String> {
    fs_utils::app_log(&state.data_dir, &format!("开始备份登录态: user_id={user_id}"));

    crate::commands::bridge::run_bridge_async(
        &app,
        &state.data_dir,
        "BackupCurrent",
        Some(&user_id),
        "profile-progress",
        "profile-done",
        Some(serde_json::json!({ "action": "backup" })),
    )
}

/// 恢复指定 slot 的登录态（关闭 TRAE → 恢复 → 启动 TRAE）
#[tauri::command]
pub fn profile_restore(
    app: AppHandle,
    state: State<AppState>,
    user_id: String,
) -> Result<(), String> {
    // 检查快照是否存在
    let slot_dir = profiles_dir(&state).join(&user_id);
    if !slot_dir.exists() {
        return Err(format!("账号 {} 的登录态快照不存在", user_id));
    }

    fs_utils::app_log(&state.data_dir, &format!("开始恢复登录态: user_id={user_id}"));

    crate::commands::bridge::run_bridge_async(
        &app,
        &state.data_dir,
        "RestoreOnly",
        Some(&user_id),
        "profile-progress",
        "profile-done",
        Some(serde_json::json!({ "action": "restore" })),
    )
}

/// 删除指定 slot 的登录态快照
#[tauri::command]
pub fn profile_delete(state: State<AppState>, user_id: String) -> Result<(), String> {
    let slot_dir = profiles_dir(&state).join(&user_id);
    if !slot_dir.exists() {
        return Ok(()); // 不存在视为已删除
    }
    std::fs::remove_dir_all(&slot_dir)
        .map_err(|e| format!("删除快照失败: {e}"))?;
    fs_utils::app_log(&state.data_dir, &format!("已删除登录态快照: user_id={user_id}"));
    Ok(())
}

/// 格式化辅助函数（给前端展示用）
#[tauri::command]
pub fn profile_format_size(bytes: u64) -> String {
    format_size(bytes)
}
