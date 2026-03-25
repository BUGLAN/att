# -*- mode: python ; coding: utf-8 -*-
"""
PyInstaller spec file for Test Tray Application

Build: pyinstaller a.spec
Run: open dist/ATest.app
"""
import sys
from pathlib import Path

# Get the project root directory
PROJECT_ROOT = Path(SPECPATH)

block_cipher = None

a = Analysis(
    [str(PROJECT_ROOT / 'a.py')],
    pathex=[str(PROJECT_ROOT)],
    binaries=[],
    datas=[
        # Include icon file
        (str(PROJECT_ROOT / 'idle.png'), '.'),
    ],
    hiddenimports=[
        # PySide6 modules
        'PySide6.QtWidgets',
        'PySide6.QtGui',
        'PySide6.QtCore',
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
    name='ATest',
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    console=True,
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
    name='ATest',
)

app = BUNDLE(
    coll,
    name='ATest.app',
    icon=str(PROJECT_ROOT / 'app.icns'),
    bundle_identifier='com.att.test',
    version='1.0.0',
    info_plist={
        # Key configuration: hide Dock icon, show only menu bar icon
        'LSUIElement': True,
        # Support Retina displays
        'NSHighResolutionCapable': True,
        # Basic app info
        'CFBundleName': 'ATest',
        'CFBundleDisplayName': 'ATest Tray',
        'CFBundleShortVersionString': '1.0.0',
        'CFBundleVersion': '1',
        'NSHumanReadableCopyright': 'Copyright 2024. All rights reserved.',
    },
)
