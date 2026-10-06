@echo off
rem P7 packaging gauge: the pinned windeployqt line for the M0 spike binary, kept as a
rem script so the size/warning numbers are reproducible rather than recollected.
rem usage: deploy.cmd <out-dir-name> [with-vcvars]
rem The optional second argument re-creates dist4's provenance: that candidate was made
rem from inside env.cmd, so vcvars was active. Whether the result depends on it is a
rem question this script is meant to answer, not to hide.
setlocal
set "QT=C:\Users\baiyl3\dev\qt\6.10.1\msvc2022_64"
set "SPIKE=%~dp0..\hello-cxxqt"
set "EXE=%SPIKE%\target\release\hello-cxxqt.exe"
if /i "%~2"=="with-vcvars" (
  call "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvarsall.bat" x64 || exit /b 1
)
set "OUT=%~dp0%~1"
rem windeployqt never copies the target binary itself; --dir only lays the Qt payload
rem around it. The copy is part of packaging, so it is pinned here rather than remembered.
"%QT%\bin\windeployqt.exe" --release --dir "%OUT%" --qmldir "%SPIKE%\qml" --no-compiler-runtime --no-system-dxc-compiler --no-opengl-sw --no-translations --skip-plugin-types qmltooling "%EXE%"
if errorlevel 1 (
  echo [P7 deploy] windeployqt failed level=%ERRORLEVEL%
  exit /b 1
)
copy /y "%EXE%" "%OUT%\" >nul || exit /b 1
echo [P7 deploy] out=%OUT% exe_copied=1
echo [P7 deploy] exitlevel=0
endlocal
