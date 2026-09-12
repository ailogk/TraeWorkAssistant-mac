#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Trae Work 账号切换集成桥 - macOS 版（非交互模式）

供 Trae Work 助手（Tauri）调用的非交互切换层，等价于 Windows 版
trae-switch-bridge.ps1。封装「关闭 TRAE → 恢复目标账号登录态 → 重置机器码
→ 启动 TRAE」流程，并以 NDJSON 逐行输出进度，供桌面端渲染步骤条。

用法:
    python3 trae-switch-bridge.py --action Switch --user-id 1234567890123456 --json
    action ∈ {Switch, ResetMachineId, BackupCurrent, RestoreOnly,
              ResetDeviceIds, SaveCurrentLogin}

macOS 与 Windows 的差异:
- TRAE 数据目录: ~/Library/Application Support/TRAE SOLO CN（Electron userData）
- 机器码 MachineGuid 注册表项不存在，跳过（不影响账号切换）
- 进程管理: pgrep/pkill 按精确命令行匹配，绝不误杀 Trae CN IDE（进程名 "Trae"）
"""

import argparse
import json
import os
import random
import shutil
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path

HOME = Path.home()
APP_SUPPORT = HOME / "Library" / "Application Support"

# TRAE SOLO 系列数据目录候选（按优先级）
TRAE_DATA_DIRS = [APP_SUPPORT / "TRAE SOLO CN", APP_SUPPORT / "TRAE SOLO"]
# 助手自身数据目录（与 Rust state.rs 保持一致）
APP_DATA_DIR = APP_SUPPORT / "TraeWorkAssistant"
PROFILES_DIR = APP_DATA_DIR / "data" / "profiles"
CURRENT_ACCOUNT_FILE = PROFILES_DIR / "current_account.txt"
LOG_FILE = APP_DATA_DIR / "logs" / "switcher.log"

# 精确命令行匹配片段：只命中 TRAE SOLO 系列进程。
# 严禁用 "Trae" 通配 —— 用户可能同时开着 Trae CN IDE（进程名 "Trae"），
# 通配会在切换/续期/保存时把无关的 CN IDE 一起杀掉。
TARGET_PROC_PATTERNS = ["TRAE SOLO CN.app", "TRAE SOLO.app"]

# .app 候选路径（与 Rust env.rs 保持一致）
APP_CANDIDATES = [
    Path("/Applications/TRAE SOLO CN.app"),
    HOME / "Applications/TRAE SOLO CN.app",
    Path("/Applications/TRAE SOLO.app"),
    HOME / "Applications/TRAE SOLO.app",
]

_OUTPUT_JSON = False


def now_str():
    return datetime.now().strftime("%Y-%m-%d %H:%M:%S")


def write_step(stage, message, status="info"):
    """输出进度（stdout NDJSON + 日志文件）"""
    global _OUTPUT_JSON
    obj = {"stage": stage, "status": status, "message": message, "time": now_str()}
    if _OUTPUT_JSON:
        sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
        sys.stdout.flush()
    else:
        print(f"[{stage}] {message}", flush=True)
    try:
        LOG_FILE.parent.mkdir(parents=True, exist_ok=True)
        with open(LOG_FILE, "a", encoding="utf-8") as f:
            f.write(f"[{now_str()}] [{stage}] {message}\n")
    except OSError:
        pass


def trae_data_dir():
    for d in TRAE_DATA_DIRS:
        if d.is_dir():
            return d
    return TRAE_DATA_DIRS[0]


def find_trae_app():
    """定位 TRAE 安装的 .app 路径：用户配置 > 标准候选路径"""
    # 1. 用户显式配置（与 Rust 侧 conf/app_settings.json 一致）
    settings_file = APP_DATA_DIR / "conf" / "app_settings.json"
    try:
        if settings_file.is_file():
            settings = json.loads(settings_file.read_text(encoding="utf-8"))
            p = settings.get("trae_path")
            if p and Path(p).exists():
                return Path(p)
    except (OSError, ValueError):
        pass

    # 2. 标准候选路径
    for c in APP_CANDIDATES:
        if c.exists():
            return c
    return None


def get_current_account():
    try:
        if CURRENT_ACCOUNT_FILE.is_file():
            aid = CURRENT_ACCOUNT_FILE.read_text(encoding="utf-8").strip()
            if aid:
                return aid
    except OSError:
        pass
    return None


def set_current_account(account_id):
    try:
        CURRENT_ACCOUNT_FILE.parent.mkdir(parents=True, exist_ok=True)
        CURRENT_ACCOUNT_FILE.write_text(account_id, encoding="utf-8")
    except OSError:
        pass


def trae_pids():
    """列出 TRAE SOLO 系列进程 PID（精确命令行匹配，不误伤 Trae CN IDE / 本助手）"""
    pids = []
    for pattern in TARGET_PROC_PATTERNS:
        try:
            out = subprocess.run(
                ["pgrep", "-f", pattern],
                capture_output=True, text=True, timeout=10,
            ).stdout
            pids.extend(int(p) for p in out.split() if p.isdigit())
        except (OSError, subprocess.TimeoutExpired):
            pass
    return sorted(set(pids))


def stop_trae():
    """关闭 TRAE：先优雅退出（osascript quit，让 Electron 正常落盘避免
    leveldb/vscdb 文件锁），最长等 5 秒；仍未退出再 kill -9。"""
    pids = trae_pids()
    if not pids:
        write_step("stop", "Trae Work 未运行", "skip")
        return

    write_step("stop", "正在关闭 Trae Work", "running")
    app = find_trae_app()
    if app is not None:
        # 优雅关闭：向应用发送 quit AppleEvent
        try:
            subprocess.run(
                ["osascript", "-e", f'quit app "{app.stem}"'],
                capture_output=True, timeout=10,
            )
        except (OSError, subprocess.TimeoutExpired):
            pass
        write_step("stop", "已发送优雅关闭请求，等待进程退出（最长 5 秒）", "running")
    else:
        write_step("stop", "未定位到 TRAE 安装路径，将直接结束进程", "warn")

    for _ in range(5):
        time.sleep(1)
        if not trae_pids():
            return

    # 第二级：仍有存活进程 → 强制结束
    alive = trae_pids()
    if alive:
        write_step("stop", "优雅关闭超时，强制结束进程", "warn")
        for pid in alive:
            try:
                os.kill(pid, 9)
            except OSError:
                pass

    # 等待进程完全退出，最多再等 3 秒
    for _ in range(3):
        time.sleep(1)
        if not trae_pids():
            return
    raise RuntimeError("Trae Work 进程未在预期时间内退出，可能仍有文件锁，请手动关闭后重试")


def start_trae():
    app = find_trae_app()
    if app is None:
        write_step("start", "未找到 TRAE 安装路径，请在设置中指定", "error")
        raise RuntimeError("未找到 TRAE 可执行文件")
    write_step("start", f"正在启动 Trae Work: {app}", "running")
    try:
        subprocess.Popen(
            ["open", "-a", str(app)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
    except OSError as e:
        write_step("start", f"启动 TRAE 失败: {e}", "error")
        raise


# ---------------- 备份 / 恢复（与 Windows 版逐项对称） ----------------

def _copy_file(src: Path, dest: Path) -> bool:
    try:
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dest)
        return True
    except OSError:
        return False


def _copy_tree(src: Path, dest: Path) -> bool:
    try:
        if dest.exists():
            shutil.rmtree(dest, ignore_errors=True)
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(src, dest)
        return True
    except OSError:
        return False


def backup_current_profile(slot):
    src = trae_data_dir()
    dest = PROFILES_DIR / slot
    if not src.is_dir():
        write_step("backup", "当前数据目录不存在，跳过备份", "skip")
        return
    dest.mkdir(parents=True, exist_ok=True)
    copied = 0

    # 1. storage.json — 设备标识、遥测、认证信息
    if _copy_file(src / "User/globalStorage/storage.json", dest / "User/globalStorage/storage.json"):
        copied += 1

    # 2. state.vscdb — 登录令牌数据库
    if _copy_file(src / "User/globalStorage/state.vscdb", dest / "User/globalStorage/state.vscdb"):
        copied += 1
    if (src / "User/globalStorage/state.vscdb.backup").is_file():
        if _copy_file(src / "User/globalStorage/state.vscdb.backup", dest / "User/globalStorage/state.vscdb.backup"):
            copied += 1

    # 3. machineid — 机器标识
    if (src / "machineid").is_file():
        if _copy_file(src / "machineid", dest / "machineid"):
            copied += 1

    # 4. aha/ — 设备认证数据
    if (src / "aha").is_dir():
        if _copy_tree(src / "aha", dest / "aha"):
            copied += 1

    # 5. Preferences / Local State
    if (src / "Preferences").is_file():
        if _copy_file(src / "Preferences", dest / "Preferences"):
            copied += 1
    if (src / "Local State").is_file():
        if _copy_file(src / "Local State", dest / "Local State"):
            copied += 1

    # 6. Local Storage/leveldb + config.db
    if (src / "Local Storage/leveldb").is_dir():
        try:
            ls_dest = dest / "Local Storage/leveldb"
            ls_dest.mkdir(parents=True, exist_ok=True)
            for f in (src / "Local Storage/leveldb").iterdir():
                shutil.copy2(f, ls_dest / f.name)
            copied += 1
        except OSError:
            pass
    if (src / "Local Storage/config.db").is_file():
        if _copy_file(src / "Local Storage/config.db", dest / "Local Storage/config.db"):
            copied += 1

    # 7. Network/
    if (src / "Network").is_dir():
        if _copy_tree(src / "Network", dest / "Network"):
            copied += 1

    # 8. Partitions/trae-webview + icube-web-crawler
    if (src / "Partitions/trae-webview").is_dir():
        if _copy_tree(src / "Partitions/trae-webview", dest / "Partitions/trae-webview"):
            copied += 1
    if (src / "Partitions/icube-web-crawler-shared-session-v1.0").is_dir():
        if _copy_tree(src / "Partitions/icube-web-crawler-shared-session-v1.0",
                      dest / "Partitions/icube-web-crawler-shared-session-v1.0"):
            copied += 1

    # 9. Session Storage/
    if (src / "Session Storage").is_dir():
        if _copy_tree(src / "Session Storage", dest / "Session Storage"):
            copied += 1

    write_step("backup", f"已备份当前登录态到 {slot} ({copied} 项)", "ok")


def restore_profile(slot):
    """恢复登录态，返回恢复项数（0 = 快照空/损坏）"""
    src = PROFILES_DIR / slot
    if not src.is_dir():
        write_step("restore", f"目标账号 {slot} 无快照，请先登录该账号并保存登录态", "error")
        raise RuntimeError(f"目标账号 {slot} 无快照")
    dest = trae_data_dir()
    dest.mkdir(parents=True, exist_ok=True)
    restored = 0

    # 删除 code.lock 防止启动冲突
    try:
        (dest / "code.lock").unlink(missing_ok=True)
    except OSError:
        pass

    # 与 backup_current_profile 逐项对称
    if _copy_file(src / "User/globalStorage/storage.json", dest / "User/globalStorage/storage.json"):
        restored += 1
    if _copy_file(src / "User/globalStorage/state.vscdb", dest / "User/globalStorage/state.vscdb"):
        restored += 1
    if (src / "User/globalStorage/state.vscdb.backup").is_file():
        if _copy_file(src / "User/globalStorage/state.vscdb.backup", dest / "User/globalStorage/state.vscdb.backup"):
            restored += 1
    if (src / "machineid").is_file():
        if _copy_file(src / "machineid", dest / "machineid"):
            restored += 1
    if (src / "aha").is_dir():
        if _copy_tree(src / "aha", dest / "aha"):
            restored += 1
    if (src / "Preferences").is_file():
        if _copy_file(src / "Preferences", dest / "Preferences"):
            restored += 1
    if (src / "Local State").is_file():
        if _copy_file(src / "Local State", dest / "Local State"):
            restored += 1
    if (src / "Local Storage/leveldb").is_dir():
        try:
            target = dest / "Local Storage/leveldb"
            target.mkdir(parents=True, exist_ok=True)
            for f in (target).iterdir():
                f.unlink(missing_ok=True)
            for f in (src / "Local Storage/leveldb").iterdir():
                shutil.copy2(f, target / f.name)
            restored += 1
        except OSError:
            pass
    if (src / "Local Storage/config.db").is_file():
        if _copy_file(src / "Local Storage/config.db", dest / "Local Storage/config.db"):
            restored += 1
    if (src / "Network").is_dir():
        if _copy_tree(src / "Network", dest / "Network"):
            restored += 1
    if (src / "Partitions/trae-webview").is_dir():
        if _copy_tree(src / "Partitions/trae-webview", dest / "Partitions/trae-webview"):
            restored += 1
    if (src / "Partitions/icube-web-crawler-shared-session-v1.0").is_dir():
        if _copy_tree(src / "Partitions/icube-web-crawler-shared-session-v1.0",
                      dest / "Partitions/icube-web-crawler-shared-session-v1.0"):
            restored += 1
    if (src / "Session Storage").is_dir():
        if _copy_tree(src / "Session Storage", dest / "Session Storage"):
            restored += 1

    write_step("restore", f"已恢复账号 {slot} 的登录态 ({restored} 项)", "ok")
    return restored


# ---------------- 设备标识重置 ----------------

def reset_device_ids():
    """macOS 版设备标识重置（与 Windows 6 层对应的可移植子集）：
    1. machineid 文件 → 新 hex32
    2. storage.json telemetry.machineId/sqmId + aha.device.device_id → 替换
    3. aha/TinyStorage device_id → 清除
    4. Partitions/trae-webview 追踪数据 → 清除
    （Windows 注册表 MachineGuid 在 macOS 不存在，跳过）"""
    trae_dir = trae_data_dir()
    if not trae_dir.is_dir():
        write_step("device", f"TRAE 数据目录不存在: {trae_dir}", "error")
        return

    new_machine_id = "".join(random.choice("0123456789abcdef") for _ in range(32))
    new_device_id = "".join(random.choice("0123456789") for _ in range(15))
    reset_count = 0

    # 1. machineid 文件
    machine_id_file = trae_dir / "machineid"
    if machine_id_file.is_file():
        try:
            machine_id_file.write_text(new_machine_id, encoding="utf-8")
            write_step("device", "[1/4] machineid 已重置", "ok")
            reset_count += 1
        except OSError as e:
            write_step("device", f"[1/4] machineid 重置失败: {e}", "skip")
    else:
        write_step("device", "[1/4] machineid 文件不存在，跳过", "skip")

    # 2. storage.json（点号键名，非嵌套对象）
    storage_file = trae_dir / "User/globalStorage/storage.json"
    if storage_file.is_file():
        try:
            raw = storage_file.read_text(encoding="utf-8")
            storage = json.loads(raw)
            changed = False
            if "telemetry.machineId" in storage:
                storage["telemetry.machineId"] = new_machine_id
                changed = True
            if "telemetry.sqmId" in storage:
                storage["telemetry.sqmId"] = new_machine_id
                changed = True
            if "aha.device.device_id" in storage:
                storage["aha.device.device_id"] = new_device_id
                changed = True
            storage.pop("has_device_id_updated_to_aha", None)
            if changed:
                storage_file.write_text(
                    json.dumps(storage, ensure_ascii=False, indent=2), encoding="utf-8")
                write_step("device", "[2/4] storage.json 设备标识已重置", "ok")
                reset_count += 1
            else:
                write_step("device", "[2/4] storage.json 无需修改", "skip")
        except (OSError, ValueError) as e:
            write_step("device", f"[2/4] storage.json 重置失败: {e}", "skip")
    else:
        write_step("device", "[2/4] storage.json 不存在，跳过", "skip")

    # 3. aha/TinyStorage device_id — 清除
    tiny_dir = trae_dir / "aha/TinyStorage"
    if tiny_dir.is_dir():
        try:
            for f in tiny_dir.rglob("*"):
                if f.is_file():
                    try:
                        content = f.read_text(encoding="utf-8", errors="ignore")
                        if "device_id" in content:
                            f.unlink()
                    except OSError:
                        pass
            write_step("device", "[3/4] aha/TinyStorage device_id 已清除", "ok")
            reset_count += 1
        except OSError:
            write_step("device", "[3/4] aha/TinyStorage 清除失败", "skip")
    else:
        write_step("device", "[3/4] aha/TinyStorage 目录不存在，跳过", "skip")

    # 4. trae-webview 追踪数据
    webview_dir = trae_dir / "Partitions/trae-webview"
    if webview_dir.is_dir():
        try:
            for sub in ["Network", "Local Storage", "Session Storage"]:
                target = webview_dir / sub
                if target.exists():
                    shutil.rmtree(target, ignore_errors=True)
            write_step("device", "[4/4] trae-webview 追踪数据已清除", "ok")
            reset_count += 1
        except OSError:
            write_step("device", "[4/4] trae-webview 清除失败", "skip")
    else:
        write_step("device", "[4/4] trae-webview 目录不存在，跳过", "skip")

    write_step(
        "device",
        f"4 层设备标识重置完成（{reset_count}/4 层成功）",
        "ok" if reset_count >= 3 else "info",
    )


def main():
    global _OUTPUT_JSON
    parser = argparse.ArgumentParser(description="Trae Work 账号切换集成桥（macOS）")
    parser.add_argument("--action", required=True,
                        choices=["Switch", "ResetMachineId", "BackupCurrent",
                                 "RestoreOnly", "ResetDeviceIds", "SaveCurrentLogin"])
    parser.add_argument("--user-id", dest="user_id", default="")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    _OUTPUT_JSON = args.json

    try:
        if not args.user_id and args.action not in ("ResetMachineId", "ResetDeviceIds"):
            write_step("init", "缺少 --user-id 参数", "error")
            sys.exit(1)
        write_step("init", f"开始操作: {args.action} (userId={args.user_id})", "info")

        if args.action == "Switch":
            target = PROFILES_DIR / args.user_id
            if not target.is_dir():
                write_step("fatal",
                           f"目标账号 {args.user_id} 无快照，请先登录该账号并点击「保存当前登录态」",
                           "error")
                sys.exit(1)
            stop_trae()
            # 当前登录态安全备份到 last 槽位
            backup_current_profile("last")
            current = get_current_account()
            if current and current != args.user_id:
                backup_current_profile(current)
                write_step("backup", f"当前账号 {current} 的登录态已备份", "ok")
            restored = restore_profile(args.user_id)
            # 恢复后校验（与 Windows 版 issue #9 对齐）
            missing = []
            if restored <= 0:
                missing.append("（快照为空或损坏，0 项恢复）")
            else:
                for rel in ["User/globalStorage/storage.json", "User/globalStorage/state.vscdb"]:
                    if not (trae_data_dir() / rel).is_file():
                        missing.append(rel)
            if missing:
                write_step("restore",
                           f"目标快照无效（{'；'.join(missing)}），正在从 last 槽回滚到切换前状态…",
                           "warn")
                restore_profile("last")
                start_trae()
                write_step("fatal",
                           f"账号 {args.user_id} 的快照无效，已回滚到切换前状态。"
                           "请登录该账号后重新「保存当前登录态」；若重新保存后仍报此错，"
                           "可能是 TRAE 新版登录态布局变化，请携带日志反馈", "error")
                sys.exit(1)
            set_current_account(args.user_id)
            start_trae()
            write_step("done", f"已切换至账号 {args.user_id}", "ok")

        elif args.action == "SaveCurrentLogin":
            stop_trae()
            backup_current_profile(args.user_id)
            set_current_account(args.user_id)
            start_trae()
            write_step("done", f"已保存账号 {args.user_id} 的当前登录态", "ok")

        elif args.action == "ResetMachineId":
            # macOS 无注册表 MachineGuid，直接做文件级设备标识重置
            reset_device_ids()
            write_step("done", "设备标识已重置", "ok")

        elif args.action == "ResetDeviceIds":
            reset_device_ids()
            write_step("done", "设备标识重置完成", "ok")

        elif args.action == "BackupCurrent":
            backup_current_profile(args.user_id)
            set_current_account(args.user_id)
            write_step("done", "备份完成", "ok")

        elif args.action == "RestoreOnly":
            stop_trae()
            restore_profile(args.user_id)
            set_current_account(args.user_id)
            start_trae()
            write_step("done", f"已恢复账号 {args.user_id} 的登录态", "ok")

        sys.exit(0)
    except Exception as e:  # noqa: BLE001 - 顶层兜底，向前端报 fatal
        write_step("fatal", f"失败: {e}", "error")
        sys.exit(1)


if __name__ == "__main__":
    main()
