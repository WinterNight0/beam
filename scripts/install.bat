@echo off
rem Double-click to install beam for this Windows user (see install.ps1).
rem   install.bat              install, or update to the latest build
rem   install.bat -Uninstall   remove it again (your keys in ~/.beam stay)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
if "%~1"=="" pause
