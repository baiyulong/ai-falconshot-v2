@echo off
rem P8: the M0 exit criterion's other half -- deps -> build -> deploy -> gate -> artifact,
rem as ONE command that returns non-zero if any link is broken. This is the local twin of
rem the CI job; ci.yml is a wrapper around these same steps.
rem usage: ci_local.cmd [out-dir-name]        (default: dist-ci)
rem
rem Why the GUI step cannot be a plain "run it and look at the exit code": measured on
rem 2026-10-06, the spike finishes its P1+P4 suites ~30s after launch and then sits in
rem exec() forever, and EVERY gui-mode log -- healthy deployed package included -- ends
rem with "exitlevel=1" because the harness is what ends the run. So the gate is log content
rem plus an explicit completion marker, bounded by a wall-clock ceiling.
setlocal enabledelayedexpansion

set "QT=C:\Users\baiyl3\dev\qt\6.10.1\msvc2022_64"
rem QT_HOME lets a hosted runner point at its own kit without editing this file; the
rem default stays the shared local install so the spike keeps working as-is.
if not "%QT_HOME%"=="" set "QT=%QT_HOME%"
set "HERE=%~dp0"
set "SPIKE=%HERE%..\hello-cxxqt"
set "DIST=%~1"
if "%DIST%"=="" set "DIST=dist-ci"
set "MODE=%~2"
set "OUT=%HERE%%DIST%"
set "LOGDIR=%HERE%logs-p8"
set "FAIL=0"
set "STEPS=0"
pushd "%HERE%"

if not exist "%LOGDIR%" mkdir "%LOGDIR%"
set "RUNID=%TIME%"
echo [P8] run_id=%DATE% %TIME% dist=%DIST%
echo [P8] qt=%QT%
echo [P8] vcvars_in_scope=build-step-only deploy-runs-without-it
if /i "%MODE%"=="gate-only" (
  echo [P8] mode=gate-only skipped=deps,build,deploy target=%DIST%
  goto gate
)

rem ---------------------------------------------------------------- step 1: deps
rem "aqt installed the module" is not evidence; the files are. Same rule as sec 9.1.
for %%f in (bin\qmake.exe bin\windeployqt.exe bin\qsb.exe plugins\platforms\qwindows.dll plugins\imageformats\qtiff.dll) do (
  if exist "%QT%\%%f" (
    echo [P8] step=deps file=%%f status=present
  ) else (
    echo [P8] step=deps file=%%f status=MISSING
    set "FAIL=1"
  )
)
set /a STEPS+=1

rem ---------------------------------------------------------------- step 2: build
echo [P8] step=build cmd=cargo build --release
call "%SPIKE%\env.cmd" cargo build --release > "%LOGDIR%\p8-build.log" 2>&1
if errorlevel 1 (
  echo [P8] step=build status=FAIL log=%LOGDIR%\p8-build.log
  set "FAIL=1"
  goto report
)
if not exist "%SPIKE%\target\release\hello-cxxqt.exe" (
  echo [P8] step=build status=FAIL reason=no-exe
  set "FAIL=1"
  goto report
)
echo [P8] step=build status=pass
set /a STEPS+=1

rem ------------------------------------------------------------- step 3: deploy
rem The purge is scoped to a scratch name on purpose: dist2..dist9 are cited evidence in
rem the plan, so an argument that names one of them must be refused, not cleaned away.
echo [P8] step=deploy out=%OUT%
if exist "%OUT%" (
  echo.%DIST%|findstr /b /i /c:"dist-ci" >nul 2>&1
  if errorlevel 1 (
    echo [P8] step=deploy status=FAIL reason=out-dir-exists-and-not-disposable dir=%DIST%
    set "FAIL=1"
    goto report
  )
  rmdir /s /q "%OUT%"
)
call "%HERE%deploy.cmd" "%DIST%" > "%LOGDIR%\p8-deploy.log" 2>&1
if errorlevel 1 (
  echo [P8] step=deploy status=FAIL log=%LOGDIR%\p8-deploy.log
  set "FAIL=1"
  goto report
)
findstr /c:"Warning: Cannot find" "%LOGDIR%\p8-deploy.log" >nul 2>&1
if errorlevel 1 (
  echo [P8] step=deploy status=pass wdeployqt_warnings=0
) else (
  echo [P8] step=deploy status=FAIL reason=wdeployqt-warnings log=%LOGDIR%\p8-deploy.log
  set "FAIL=1"
  goto report
)
set /a STEPS+=1

:gate
rem ----------------------------------------------------- step 4: headless audit gate
rem Proves plugins and codecs resolved from inside the package. NOT sufficient alone --
rem the crippled default package passes this and loads no UI (sec 7.2 ).
call "%HERE%run_clean.cmd" "%DIST%" --audit > "%LOGDIR%\p8-audit-%DIST%.log" 2>&1
findstr /c:"reader_formats=" "%LOGDIR%\p8-audit-%DIST%.log" >nul 2>&1
if errorlevel 1 (
  echo [P8] step=audit status=FAIL log=%LOGDIR%\p8-audit-%DIST%.log
  set "FAIL=1"
) else (
  echo [P8] step=audit status=pass log=%LOGDIR%\p8-audit-%DIST%.log
)
set /a STEPS+=1

rem ---------------------------------------------------------- step 5: GUI load gate
rem Nested quotes inside a start command are a known cmd trap, so this relies on pushd
rem above and keeps every path relative.
start "p8gate" /min %COMSPEC% /c "call run_clean.cmd %DIST% > logs-p8\p8-gui-%DIST%.log 2>&1"
set "WAITED=0"
:waitgui
rem ping is the sleep that survives a redirected stdin; timeout does not.
ping -n 4 127.0.0.1 >nul
findstr /c:"[P4] provider calls=" "%LOGDIR%\p8-gui-%DIST%.log" >nul 2>&1
if not errorlevel 1 goto gui_done
set /a WAITED+=3
if %WAITED% LSS 120 goto waitgui
echo [P8] step=gui status=FAIL reason=no-completion-marker waited=%WAITED%s
:gui_done
taskkill /f /im hello-cxxqt.exe >nul 2>&1
ping -n 2 127.0.0.1 >nul

rem Four independent assertions. Any one missing fails the gate.
call :assert "%LOGDIR%\p8-gui-%DIST%.log" "[P1] suite start"                qml_loaded
call :assert "%LOGDIR%\p8-gui-%DIST%.log" "[P1] DIM rects shaderStatus=0"   shader_chain
call :assert "%LOGDIR%\p8-gui-%DIST%.log" "[P4] provider calls="            suite_completed
call :assertabsent "%LOGDIR%\p8-gui-%DIST%.log" "is not installed"          no_module_gap
set /a STEPS+=1

rem ------------------------------------------------------------ step 6: neg control
rem Without this, "gate passed" may only mean the scrub never isolated anything.
if exist "%HERE%dist-bare\hello-cxxqt.exe" (
  call "%HERE%run_clean.cmd" "dist-bare" --audit > "%LOGDIR%\p8-bare.log" 2>&1
  findstr /c:"exitlevel=-1073741515" "%LOGDIR%\p8-bare.log" >nul 2>&1
  if errorlevel 1 (
    echo [P8] step=neg-control status=FAIL reason=bare-package-started isolation-suspect
    set "FAIL=1"
  ) else (
    echo [P8] step=neg-control status=pass reason=dll-not-found-as-expected
  )
) else (
  echo [P8] step=neg-control status=SKIP reason=dist-bare-missing
)
set /a STEPS+=1

rem ------------------------------------------------------------- step 7: artifact
rem A for /f backquote command loses its inner quotes to cmd's own parsing, so the
rem numbers land in a file first and are read back with set /p.
powershell -NoProfile -Command "$f=Get-ChildItem -LiteralPath '%OUT%' -Recurse -File; ('{0} {1:F2}' -f $f.Count, (($f|Measure-Object -Sum Length).Sum/1MB)) | Out-File -Encoding ascii '%LOGDIR%\p8-artifact.txt'"
set /p ART=<"%LOGDIR%\p8-artifact.txt"
for /f "tokens=1,2" %%a in ("%ART%") do (set "NF=%%a"&set "MB=%%b")
echo [P8] step=artifact files=%NF% MiB=%MB%
if %NF% LSS 1300 (
  echo [P8] step=artifact status=FAIL reason=file-count-below-floor floor=1300
  set "FAIL=1"
) else (
  echo [P8] step=artifact status=pass
)
set /a STEPS+=1

:report
echo [P8] finish=%TIME% started=%RUNID%
if "%FAIL%"=="0" (
  echo [P8] verdict=PASS steps=%STEPS% logs=%LOGDIR%
) else (
  echo [P8] verdict=FAIL steps=%STEPS% logs=%LOGDIR%
)
endlocal & exit /b %FAIL%

rem ------------------------------------------------------------- helpers
:assert
findstr /c:"%~2" "%~1" >nul 2>&1
if errorlevel 1 (
  echo [P8] assert=%~3 pattern=%2 status=MISSING file=%~1
  set "FAIL=1"
) else (
  echo [P8] assert=%~3 pattern=%2 status=found
)
goto :eof

:assertabsent
findstr /c:"%~2" "%~1" >nul 2>&1
if errorlevel 1 (
  echo [P8] assert=%~3 pattern=%2 status=absent-as-expected
) else (
  echo [P8] assert=%~3 pattern=%2 status=PRESENT file=%~1
  set "FAIL=1"
)
goto :eof
