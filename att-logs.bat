@echo off
chcp 65001 >nul
setlocal
rem Follow container logs (Ctrl+C to exit).

set "PS1=%~dp0scripts\docker\att.ps1"
set "SHELL_EXE=pwsh"
where pwsh >nul 2>nul || set "SHELL_EXE=powershell"

"%SHELL_EXE%" -NoProfile -ExecutionPolicy Bypass -File "%PS1%" -Action logs %*
exit /b %ERRORLEVEL%
