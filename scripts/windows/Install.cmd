@echo off
rem Installs Catbot for this Windows user. Double-click it; no administrator needed.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
pause
