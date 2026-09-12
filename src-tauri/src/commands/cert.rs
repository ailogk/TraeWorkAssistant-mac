#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::process::Command;
use tauri::{AppHandle, State};

use crate::python::spawn_script;
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct CertStatus {
    pub installed: bool,
}

#[tauri::command(async)]
pub fn cert_status(_app: AppHandle, _state: State<AppState>) -> CertStatus {
    #[cfg(target_os = "windows")]
    let installed = {
        let out = Command::new("certutil")
            .args(["-store", "Root"])
            .creation_flags(0x08000000)
            .output();
        match out {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                s.contains("TraeDeviceProxyCA")
            }
            Err(_) => false,
        }
    };
    #[cfg(not(target_os = "windows"))]
    let installed = {
        // macOS：在钥匙串中查找自签 CA（含登录钥匙串与系统钥匙串）
        let out = Command::new("security")
            .args(["find-certificate", "-a", "-c", "TraeDeviceProxyCA"])
            .output();
        match out {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                s.contains("TraeDeviceProxyCA")
            }
            Err(_) => false,
        }
    };
    CertStatus { installed }
}

/// 检查当前解析到的 Python 环境能否导入指定模块。
/// dev 环境回退系统 Python 时，cryptography 缺失是 --gen-ca 失败的头号原因。
fn python_import_ok(state: &AppState, module: &str) -> bool {
    let mut cmd = Command::new(&state.python_exe);
    cmd.args(["-c", &format!("import {module}")]);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x08000000);
    cmd.output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 当前解释器是否为应用内嵌 embeddable 运行时（以特征文件 python3*._pth 判定）。
/// 内嵌运行时不含 pip（构建期已装齐依赖），自愈失败时应提示重装应用而非手动 pip。
fn is_embedded_runtime(state: &AppState) -> bool {
    std::path::Path::new(&state.python_exe)
        .parent()
        .map(|dir| {
            std::fs::read_dir(dir)
                .map(|entries| {
                    entries.filter_map(|e| e.ok()).any(|e| {
                        e.file_name().to_string_lossy().starts_with("python3")
                            && e.file_name().to_string_lossy().ends_with("._pth")
                    })
                })
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// 用当前 Python 环境安装依赖包（cryptography 缺失时自愈，吸收 main a3301c7）。
fn pip_install(state: &AppState, pkgs: &[&str]) -> Result<(), String> {
    let mut cmd = Command::new(&state.python_exe);
    cmd.args(["-m", "pip", "install", "--disable-pip-version-check", "--no-input"])
        .args(pkgs);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x08000000);
    let out = cmd
        .output()
        .map_err(|e| format!("启动 pip 失败: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    // 新版 Homebrew/系统 Python 启用 PEP 668（externally-managed）时需 --break-system-packages
    let mut retry = Command::new(&state.python_exe);
    retry
        .args(["-m", "pip", "install", "--disable-pip-version-check", "--no-input", "--break-system-packages"])
        .args(pkgs);
    #[cfg(target_os = "windows")]
    retry.creation_flags(0x08000000);
    let out = retry
        .output()
        .map_err(|e| format!("启动 pip 失败: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = tail_400(&String::from_utf8_lossy(&out.stderr));
    Err(format!(
        "exit {}：{}",
        out.status.code().unwrap_or(-1),
        if err.is_empty() { "无错误输出".to_string() } else { err }
    ))
}

/// 截取输出尾部（最多 400 字符），用于把子进程真实报错带回给前端 toast。
fn tail_400(s: &str) -> String {
    let trimmed = s.trim();
    let mut chars = trimmed.chars();
    let total = trimmed.chars().count();
    if total <= 400 {
        return trimmed.to_string();
    }
    for _ in 0..(total - 400) {
        chars.next();
    }
    chars.collect()
}

#[tauri::command(async)]
pub fn cert_install(app: AppHandle, state: State<AppState>) -> Result<CertStatus, String> {
    // 0. 依赖自检（吸收 main a3301c7）：dev 环境回退系统 Python 时 cryptography
    //    缺失是 --gen-ca 失败的头号原因，先自愈再继续；已内置运行时则秒过。
    if !python_import_ok(&state, "cryptography") {
        let pkgs: &[&str] = if cfg!(target_os = "windows") {
            &["cryptography>=42.0.0", "pywin32>=306"]
        } else {
            &["cryptography>=42.0.0"]
        };
        pip_install(&state, pkgs).map_err(|e| {
            // 内嵌 embeddable 运行时不含 pip（构建期已装齐依赖）：依赖缺失说明
            // 安装目录被损坏（如杀软误删），提示重装而非引导手动 pip（无 pip 可用）
            if is_embedded_runtime(&state) {
                format!(
                    "应用内置 Python 运行时依赖异常且无法自动修复（{}）。内置运行时已随安装包自带全部依赖，请重新安装本应用恢复。",
                    e
                )
            } else {
                format!(
                    "Python 缺少 cryptography 模块且自动安装失败（{}）。请手动执行：\"{}\" -m pip install cryptography",
                    e, state.python_exe
                )
            }
        })?;
    }

    // 1. 确保 CA 证书已生成（data_dir/certs/ca.cer）
    let cer = state.path("certs").join("ca.cer");
    if !cer.exists() {
        // capture=true 回收子进程 stdout/stderr，失败时把真实原因（如
        // ModuleNotFoundError: No module named 'cryptography'）返回给前端，
        // 而不是静默吞掉后只报一句不含原因的「生成失败」。
        let out = spawn_script(&state, "device_proxy.py", &["--gen-ca".to_string()], true)?
            .wait_with_output()
            .map_err(|e| format!("等待 CA 证书生成进程失败: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            let detail = if stderr.trim().is_empty() {
                tail_400(&stdout)
            } else {
                tail_400(&stderr)
            };
            if detail.is_empty() {
                return Err(format!(
                    "CA 证书生成失败（退出码 {:?}）",
                    out.status.code()
                ));
            }
            // 依赖自愈已跑过仍报 No module named：环境异常，按场景给出可执行的修复指引
            let hint = if detail.contains("No module named") {
                if is_embedded_runtime(&state) {
                    "。提示：内置运行时已随安装包自带全部依赖，报缺模块说明安装目录被损坏（如杀软误删），请重新安装本应用恢复。".to_string()
                } else {
                    format!(
                        "。提示：Python 依赖缺失，请在 \"{}\" 中执行 -m pip install cryptography 后重试（解释器路径可查 app.log 的 python_exe= 行）",
                        state.python_exe
                    )
                }
            } else {
                String::new()
            };
            return Err(format!("CA 证书生成失败: {}{}", detail, hint));
        }
    }
    if !cer.exists() {
        return Err("CA 证书生成失败，无法安装".into());
    }
    #[cfg(target_os = "windows")]
    let cer_arg = cer.to_string_lossy().replace('\\', "/").to_string();
    #[cfg(not(target_os = "windows"))]
    let cer_arg = cer.to_string_lossy().to_string();

    // 2. 安装到系统受信任根证书：
    //    Windows：certutil -addstore Root（触发 UAC，-PassThru 拿真实退出码）；
    //    macOS：security add-trusted-cert 加入登录钥匙串信任设置（可能弹系统授权框）。
    #[cfg(target_os = "windows")]
    {
        let ps = format!(
            "$p = Start-Process certutil -ArgumentList @('-addstore','-f','Root','\"{}\"') -Verb RunAs -Wait -PassThru; exit $p.ExitCode",
            cer_arg
        );
        let status = Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps])
            .creation_flags(0x08000000)
            .status()
            .map_err(|e| format!("启动证书安装失败: {e}"))?;
        if !status.success() {
            return Err(format!(
                "证书安装失败或被取消（certutil 退出码 {:?}；可能需要管理员权限或被组策略拒绝）",
                status.code()
            ));
        }
    }
    #[cfg(target_os = "macos")]
    {
        let keychain = format!(
            "{}/Library/Keychains/login.keychain-db",
            std::env::var("HOME").unwrap_or_default()
        );
        let status = Command::new("security")
            .args([
                "add-trusted-cert",
                "-r",
                "trustRoot",
                "-p",
                "ssl",
                "-k",
                &keychain,
                &cer_arg,
            ])
            .status()
            .map_err(|e| format!("启动证书安装失败: {e}"))?;
        if !status.success() {
            return Err(format!(
                "证书安装失败或被取消（security 退出码 {:?}；若系统弹窗请点「允许」后重试）",
                status.code()
            ));
        }
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        return Err("当前平台不支持证书安装".into());
    }
    // 3. 安装后复查根存储（吸收 main a3301c7）：安装命令报成功但证书未实际
    //    入库（组策略拦截/存储重定向）时不能误报「安装成功」。
    let result = cert_status(app, state);
    if !result.installed {
        return Err(
            "证书安装命令已执行，但证书存储中未找到 TraeDeviceProxyCA，请检查系统策略".into(),
        );
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::tail_400;

    #[test]
    fn tail_short_input_unchanged() {
        assert_eq!(tail_400("  hello world \n"), "hello world");
        assert_eq!(tail_400(""), "");
    }

    #[test]
    fn tail_long_input_keeps_last_400_chars() {
        let s = "a".repeat(500) + "TRAILING_MARKER";
        let t = tail_400(&s);
        assert_eq!(t.chars().count(), 400);
        assert!(t.ends_with("TRAILING_MARKER"));
        assert!(t.starts_with("aaa")); // 剩余的尾部 a
    }
}
