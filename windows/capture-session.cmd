@echo off
rem Guided Windows capture session for the Pimax Dream Air. Double-click this file.
rem Asks for Administrator (needed by USBPcap). Capture only: nothing is written to the headset.
setlocal
set PS=%SystemRoot%\Sysnative\WindowsPowerShell\v1.0\powershell.exe
if not exist "%PS%" set PS=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe
"%PS%" -NoProfile -Command "Start-Process '%PS%' -Verb RunAs -ArgumentList '-NoProfile -ExecutionPolicy Bypass -File \"%~dp0capture-session.ps1\" %*'"
endlocal
