@echo off
chcp 65001 >nul
setlocal
rem One-click start for real-time ASR, then launch the Electron desktop app.
rem The app reuses the containerized backend via ATT_BACKEND_PORT (default 8000).

set "PS1=%~dp0scripts\docker\att.ps1"
set "SHELL_EXE=pwsh"
where pwsh >nul 2>nul || set "SHELL_EXE=powershell"

"%SHELL_EXE%" -NoProfile -ExecutionPolicy Bypass -File "%PS1%" -Action start -WithApp %*
set "CODE=%ERRORLEVEL%"

if not "%CODE%"=="0" (
    echo.
    echo [FAILED] exit code %CODE%. Press any key to close.
    pause >nul
)

exit /b %CODE%
