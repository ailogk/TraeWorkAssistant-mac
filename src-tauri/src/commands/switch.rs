use tauri::{AppHandle, State};

use crate::fs_utils;
use crate::state::AppState;

// async：切换前 JWT 预检含网络调用（最长 15s），同步命令会冻结 UI
// （项目约定：阻塞型命令一律 #[tauri::command(async)]）
#[tauri::command(async)]
pub fn switch_account(
    app: AppHandle,
    state: State<AppState>,
    user_id: String,
    // 续期 JWT 流程专用：目标账号 JWT 本就可能已吊销（续期正是为了重抓），
    // 跳过 TRAE 切换前 JWT 预检，否则预检 401 会把续期链路拦死
    skip_jwt_probe: Option<bool>,
) -> Result<(), String> {
    let bridge = crate::commands::bridge::bridge_path();
    fs_utils::app_log(
        &state.data_dir,
        &format!(
            "switch/重置/保存 已到达 Rust: bridge={:?}, 存在={}",
            bridge,
            bridge.exists()
        ),
    );

    if !skip_jwt_probe.unwrap_or(false) {
        // TRAE 切换前 JWT 服务端预检（issue #9）——目标账号 JWT 被服务端吊销时本地快照仍完好，
        // 切换恢复后 IDE 一联网即被登出，用户感知为「切换了但没反应」。
        // 401 判死时提前中止并给出补救指引；网络故障 fail-open 不阻断（见函数内实现）。
        crate::commands::accounts::probe_trae_jwt_alive(&state, &user_id)?;
    }

    fs_utils::app_log(&state.data_dir, &format!("开始切换账号: user_id={user_id}"));

    crate::commands::bridge::run_bridge_async(
        &app,
        &state.data_dir,
        "Switch",
        Some(&user_id),
        "switch-progress",
        "switch-done",
        None,
    )
}

/// 保存当前登录态：关闭 Trae → 精准备份到 userId 槽位 → 重新启动
/// 通过 NDJSON 事件流式返回进度，前端订阅 save-login-progress / save-login-done
#[tauri::command]
pub fn save_current_login(
    app: AppHandle,
    state: State<AppState>,
    user_id: String,
) -> Result<(), String> {
    fs_utils::app_log(&state.data_dir, &format!("开始保存当前登录态: user_id={user_id}"));

    crate::commands::bridge::run_bridge_async(
        &app,
        &state.data_dir,
        "SaveCurrentLogin",
        Some(&user_id),
        "save-login-progress",
        "save-login-done",
        None,
    )
}

/// 设备标识重置：调用桥脚本的 ResetDeviceIds 动作
/// 通过 NDJSON 事件流式返回进度，前端订阅 device-reset-progress / device-reset-done
#[tauri::command]
pub fn reset_device_ids(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    fs_utils::app_log(&state.data_dir, "开始设备标识重置");

    crate::commands::bridge::run_bridge_async(
        &app,
        &state.data_dir,
        "ResetDeviceIds",
        None,
        "device-reset-progress",
        "device-reset-done",
        None,
    )
}
