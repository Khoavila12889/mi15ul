@echo off
chcp 65001 >nul
cd /d "%~dp0"
where python >nul 2>&1
if errorlevel 1 (
    echo [LOI] Khong tim thay Python trong PATH. Vui long cai Python 3.10+ va thu lai.
    pause
    exit /b 1
)
python main.py
if errorlevel 1 (
    echo.
    echo [LOI] Chuong trinh dung voi ma loi %ERRORLEVEL%.
    pause
)
