@echo off
rem Double-click this file. It installs everything Kani needs, builds it and
rem makes a folder you can run (dist\Kani\Kani.bat) and a zip of it.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup-windows.ps1" %*
echo.
pause
