@echo off
rem Dev-environment wrapper: gives Cargo the two things it cannot discover by
rem itself -- the MSVC host tools (cl.exe/link.exe, only vcvarsall puts them on
rem PATH) and the Qt install cxx-qt-build locates through QMAKE.
rem
rem   scripts\env.cmd cargo clippy --workspace --all-targets
rem
rem Override on another machine with:
rem   set QTDIR=D:\Qt\6.10.1\msvc2022_64
rem   set VCVARS=D:\VS\BuildTools\VC\Auxiliary\Build\vcvarsall.bat
setlocal

if not defined QTDIR set "QTDIR=C:\Users\baiyl3\dev\qt\6.10.1\msvc2022_64"
if not defined VCVARS set "VCVARS=C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvarsall.bat"

if not exist "%QTDIR%\bin\qmake.exe" (
    echo [env] no qmake under %QTDIR% -- set QTDIR to your Qt 6.10.1 msvc2022_64 install 1>&2
    exit /b 1
)
if not exist "%VCVARS%" (
    echo [env] no vcvarsall at %VCVARS% -- set VCVARS to your Visual Studio copy 1>&2
    exit /b 1
)

call "%VCVARS%" x64 >nul || exit /b 1
set "QMAKE=%QTDIR%\bin\qmake.exe"
set "PATH=%QTDIR%\bin;%PATH%"
cd /d "%~dp0.."
%*
