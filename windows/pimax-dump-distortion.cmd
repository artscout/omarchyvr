@echo off
rem Runs the Pimax dump tool (read-only). Double-click this file.
setlocal
cd /d "%~dp0"
rem 64-bit PowerShell is required (the PVR client DLL is 64-bit)
set PS=%SystemRoot%\Sysnative\WindowsPowerShell\v1.0\powershell.exe
if not exist "%PS%" set PS=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe
"%PS%" -NoProfile -ExecutionPolicy Bypass -File "%~dp0pimax-dump-distortion.ps1" %*
if errorlevel 1 pause
endlocal
