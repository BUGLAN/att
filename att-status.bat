@echo off
chcp 65001 >nul
setlocal
rem Show container status, health, streaming readiness and host volume paths.

set "PS1=%~dp0scripts\docker\att.ps1"
set "SHELL_EXE=pwsh"
where pwsh >nul 2>nul || set "SHELL_EXE=powershell"

"%SHELL_EXE%" -NoProfile -ExecutionPolicy Bypass -File "%PS1%" -Action status %*
set "CODE=%ERRORLEVEL%"

echo.
pause >nul
exit /b %CODE%
