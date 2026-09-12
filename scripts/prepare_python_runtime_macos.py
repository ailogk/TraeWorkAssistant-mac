#!/usr/bin/env python3
"""macOS 版 Python 资源准备脚本。

Windows 版（prepare_python_runtime.py）会下载 embeddable Python 并装配运行时；
macOS 无此机制——直接把业务脚本装配到 build/python-bundle/
（tauri.conf.json resources 的实际来源），运行时使用系统 python3。
依赖（cryptography）由应用启动时检查并提示用户安装（见 proxy.rs）。

用法:
  python3 scripts/prepare_python_runtime_macos.py
"""

import shutil
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
SRC_PYTHON = REPO_ROOT / "src-python"
BUNDLE_STAGE = REPO_ROOT / "build" / "python-bundle"

# 与业务一同发布的脚本（同 Windows 版 STAGE_SCRIPTS）
STAGE_SCRIPTS = ("auto_checkin.py", "device_proxy.py")


def main() -> int:
    missing = [s for s in STAGE_SCRIPTS if not (SRC_PYTHON / s).is_file()]
    if missing:
        print(f"[错误] src-python 缺少脚本: {missing}", file=sys.stderr)
        return 1

    BUNDLE_STAGE.mkdir(parents=True, exist_ok=True)
    # 清空重建，保证幂等
    for f in BUNDLE_STAGE.iterdir():
        if f.is_dir():
            shutil.rmtree(f)
        else:
            f.unlink()

    for name in STAGE_SCRIPTS:
        shutil.copy2(SRC_PYTHON / name, BUNDLE_STAGE / name)
        print(f"已装配: {name}")

    print(f"macOS python-bundle 装配完成: {BUNDLE_STAGE}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
