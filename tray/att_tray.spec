# -*- mode: python ; coding: utf-8 -*-
"""
PyInstaller spec file for ATT Tray Application

Build: pyinstaller tray/att_tray.spec
Run: open dist/ATT.app
"""
import sys
from pathlib import Path

# Get the project root directory
PROJECT_ROOT = Path(SPECPATH).parent

block_cipher = None

a = Analysis(
    [str(PROJECT_ROOT / 'tray' / 'main.py')],
    pathex=[str(PROJECT_ROOT)],
    binaries=[],
    datas=[
        # Include icon files
        (str(PROJECT_ROOT / 'tray' / 'icons' / '*.png'), 'tray/icons'),
    ],
    hiddenimports=[
        # PySide6 modules
        'PySide6.QtWidgets',
        'PySide6.QtGui',
        'PySide6.QtCore',
        # Application modules
        'tray.websocket_client',
        'tray.hotkey',
        'tray.audio_controller',
        # Config
        'app.config',
    ],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[],
    win_no_prefer_redirects=False,
    win_private_assemblies=False,
    cipher=block_cipher,
    noarchive=False,
)

pyz = PYZ(a.pure, a.zipped_data, cipher=block_cipher)

exe = EXE(
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name='ATT',
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    console=True,  # Show terminal window for debugging
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
)

coll = COLLECT(
    exe,
    a.binaries,
    a.zipfiles,
    a.datas,
    strip=False,
    upx=True,
    upx_exclude=[],
    name='ATT',
)

app = BUNDLE(
    coll,
    name='ATT.app',
    icon=str(PROJECT_ROOT / 'tray' / 'icons' / 'app.icns'),
    bundle_identifier='com.att.tray',
    version='1.0.0',
    info_plist={
        # Key configuration: hide Dock icon, show only menu bar icon
        'LSUIElement': True,
        # Support Retina displays
        'NSHighResolutionCapable': True,
        # Permission description for media control
        'NSAppleEventsUsageDescription': 'ATT needs to control media playback during recording.',
        # Basic app info
        'CFBundleName': 'ATT',
        'CFBundleDisplayName': 'ATT Tray',
        'CFBundleShortVersionString': '1.0.0',
        'CFBundleVersion': '1',
        'NSHumanReadableCopyright': 'Copyright 2024. All rights reserved.',
    },
)
