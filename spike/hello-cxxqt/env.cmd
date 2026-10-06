@echo off
rem Build/run wrapper for the M0 spike.
rem Qt 6.10.1 ships MSVC objects, so the Rust host must be x86_64-pc-windows-msvc
rem and cl.exe/link.exe must be on PATH -- which only vcvarsall guarantees.
setlocal
call "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvarsall.bat" x64 || exit /b 1
set "QMAKE=C:\Users\baiyl3\dev\qt\6.10.1\msvc2022_64\bin\qmake.exe"
set "PATH=C:\Users\baiyl3\dev\qt\6.10.1\msvc2022_64\bin;%PATH%"
cd /d "%~dp0"
%*
