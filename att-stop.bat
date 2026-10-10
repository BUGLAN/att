@echo off
chcp 65001 >nul
setlocal
rem Stop the real-time ASR containers. Host volume data is NOT touched.

set "PS1=%~dp0scripts\docker\att.ps1"
set "SHELL_EXE=pwsh"
where pwsh >nul 2>nul || set "SHELL_EXE=powershell"

"%SHELL_EXE%" -NoProfile -ExecutionPolicy Bypass -File "%PS1%" -Action stop %*
set "CODE=%ERRORLEVEL%"

echo.
if not "%CODE%"=="0" (
    echo [FAILED] exit code %CODE%.
)
pause >nul
exit /b %CODE%
