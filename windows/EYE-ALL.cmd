@echo off
rem Crystal Super eye tracking: every capture in one run (see README.md, Crystal Super eye tracking). Starts as Administrator.
powershell -NoProfile -Command "Start-Process powershell -Verb RunAs -ArgumentList \"-NoProfile -ExecutionPolicy Bypass -File '%~dp0crystal-eye-capture.ps1'\""
