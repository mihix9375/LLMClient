@echo off
setlocal
where pwsh.exe >nul 2>nul
if errorlevel 1 (
  powershell.exe -NoProfile -File "%~dp0qwen.ps1" %*
) else (
  pwsh.exe -NoProfile -File "%~dp0qwen.ps1" %*
)
set "qwen_exit=%errorlevel%"
if %qwen_exit% neq 0 pause
exit /b %qwen_exit%
