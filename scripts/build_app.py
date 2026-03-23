#!/usr/bin/env python3
"""
构建 macOS .app bundle for ATT Tray Application

This script creates a proper macOS application bundle that can display
system tray icons, which is not possible when running directly from terminal.

Usage:
    python scripts/build_app.py

Then run with:
    open dist/ATT.app
"""
import os
import shutil
import subprocess
import sys
from pathlib import Path

APP_NAME = "ATT"
PROJECT_ROOT = Path(__file__).parent.parent
DIST_DIR = PROJECT_ROOT / "dist"
APP_BUNDLE = DIST_DIR / f"{APP_NAME}.app"


def build_app():
    """创建 .app bundle"""
    print(f"Building {APP_NAME}.app...")

    # 清理旧的 .app
    if APP_BUNDLE.exists():
        print(f"  Removing old {APP_BUNDLE}...")
        shutil.rmtree(APP_BUNDLE)

    # 创建目录结构
    contents = APP_BUNDLE / "Contents"
    macos = contents / "MacOS"
    resources = contents / "Resources"

    macos.mkdir(parents=True, exist_ok=True)
    resources.mkdir(parents=True, exist_ok=True)

    # 创建启动脚本
    launcher = macos / APP_NAME

    # 使用更可靠的启动方式
    launcher_content = f'''#!/bin/bash
# ATT Tray Application Launcher

# Get the directory where this script is located
SCRIPT_DIR="$( cd "$( dirname "${{BASH_SOURCE[0]}}" )" && pwd )"
PROJECT_ROOT="{PROJECT_ROOT}"

# Change to project directory
cd "$PROJECT_ROOT"

# Check if virtual environment exists
if [ -d ".venv" ]; then
    source .venv/bin/activate
fi

# Run the tray application
exec python -m tray.main "$@"
'''

    launcher.write_text(launcher_content)
    launcher.chmod(0o755)
    print(f"  Created launcher: {launcher}")

    # 创建 Info.plist
    info_plist = contents / "Info.plist"
    info_plist_content = f'''<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>zh_CN</string>
    <key>CFBundleExecutable</key>
    <string>{APP_NAME}</string>
    <key>CFBundleIconFile</key>
    <string>app</string>
    <key>CFBundleIdentifier</key>
    <string>com.att.tray</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>{APP_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>ATT Tray</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>Copyright 2024. All rights reserved.</string>
    <key>NSAppleEventsUsageDescription</key>
    <string>ATT needs to control media playback during recording.</string>
</dict>
</plist>
'''
    info_plist.write_text(info_plist_content)
    print(f"  Created Info.plist: {info_plist}")

    # 创建 PkgInfo
    pkg_info = contents / "PkgInfo"
    pkg_info.write_text("APPL????")
    print(f"  Created PkgInfo: {pkg_info}")

    # 复制图标（如果存在）
    icns_source = PROJECT_ROOT / "tray" / "icons" / "app.icns"
    if icns_source.exists():
        icns_dest = resources / "app.icns"
        shutil.copy(icns_source, icns_dest)
        print(f"  Copied icon: {icns_dest}")
    else:
        print(f"  Warning: Icon file not found at {icns_source}")
        print(f"  Run 'python scripts/build_app.py --icon' to generate icons")

    print()
    print(f"✅ Successfully created {APP_BUNDLE}")
    print()
    print("To run the app:")
    print(f"    open {APP_BUNDLE}")
    print()
    print("Or double-click the app in Finder:")
    print(f"    {APP_BUNDLE}")


def generate_icons():
    """生成应用图标"""
    print("Generating application icons...")

    icons_dir = PROJECT_ROOT / "tray" / "icons"
    iconset_dir = icons_dir / "app.iconset"

    # 确保 iconset 目录存在
    iconset_dir.mkdir(parents=True, exist_ok=True)

    # 源图标（使用 idle.png 作为基础）
    source_icon = icons_dir / "idle.png"

    if not source_icon.exists():
        print(f"  Error: Source icon not found: {source_icon}")
        print("  Please run the tray app first to generate icons")
        return False

    # 生成不同尺寸的图标
    sizes = [
        (16, "icon_16x16.png"),
        (32, "icon_16x16@2x.png"),
        (32, "icon_32x32.png"),
        (64, "icon_32x32@2x.png"),
        (128, "icon_128x128.png"),
        (256, "icon_128x128@2x.png"),
        (256, "icon_256x256.png"),
        (512, "icon_256x256@2x.png"),
        (512, "icon_512x512.png"),
        (1024, "icon_512x512@2x.png"),
    ]

    for size, filename in sizes:
        output_path = iconset_dir / filename
        cmd = ["sips", "-z", str(size), str(size), str(source_icon), "--out", str(output_path)]
        result = subprocess.run(cmd, capture_output=True)
        if result.returncode == 0:
            print(f"  Generated {filename}")
        else:
            print(f"  Error generating {filename}: {result.stderr.decode()}")

    # 生成 .icns 文件
    icns_output = icons_dir / "app.icns"
    cmd = ["iconutil", "-c", "icns", str(iconset_dir), "-o", str(icns_output)]
    result = subprocess.run(cmd, capture_output=True)

    if result.returncode == 0:
        print(f"  ✅ Created {icns_output}")
        return True
    else:
        print(f"  Error creating .icns: {result.stderr.decode()}")
        return False


def main():
    """主入口"""
    if len(sys.argv) > 1 and sys.argv[1] == "--icon":
        generate_icons()
    else:
        build_app()


if __name__ == "__main__":
    main()
