@echo off
rem P7 gauge: run a deployed exe in an environment that CANNOT see the Qt install.
rem Only the candidate directory plus system paths survive, so any Qt plugin or
rem qml module it loads must come from that directory itself.
rem usage: run_clean.cmd <subdir-with-trailing-or-not> [args...]
setlocal
set "CAND=%~dp0%~1"
set "EXE=%CAND%\hello-cxxqt.exe"
rem %* ignores SHIFT in cmd.exe, so collect the tail from the positional slots.
set "ARGS=%2 %3 %4 %5 %6 %7 %8 %9"
set "PATH=%CAND%;%SystemRoot%\System32;%SystemRoot%;%SystemRoot%\System32\WindowsPowerShell\v1.0"
set "QT_PLUGIN_PATH="
set "QT_QPA_PLATFORM_PLUGIN_PATH="
set "QML2_IMPORT_PATH="
set "QML_IMPORT_PATH="
set "QT_QML_IMPORT_PATH="
set "QT_DIR="
set "QT_PLUGIN_CONF="
set "QMAKE="
set "INCLUDE="
set "LIB="
rem Without a console attached Qt sends qWarning to OutputDebugString, so the
rem module-not-found text vanishes from a redirected log. QT_LOGGING_TO_CONSOLE
rem still works on 6.10.1 but prints a deprecation warning; these are the
rem current spellings.
set "QT_FORCE_STDERR_LOGGING=1"
set "QT_ASSUME_STDERR_HAS_CONSOLE=1"
echo [P7] candidate=%CAND%
echo [P7] exe=%EXE%
echo [P7] cwd=%CD%
echo [P7] PATH=%PATH%
"%EXE%" %ARGS%
echo [P7] exitlevel=%ERRORLEVEL%
endlocal
