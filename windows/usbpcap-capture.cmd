@echo off
rem Starts usbpcap-capture.ps1 as Administrator. Arguments are passed on, e.g.  usbpcap-capture.cmd -Name coldstart1 -Seconds 90
set ARGS=%*
powershell -NoProfile -Command "Start-Process powershell -Verb RunAs -ArgumentList \"-NoProfile -ExecutionPolicy Bypass -File '%~dp0usbpcap-capture.ps1' %ARGS%\""
