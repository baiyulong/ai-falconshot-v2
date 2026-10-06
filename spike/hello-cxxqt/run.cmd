@echo off
rem Runs the spike binary under the MSVC + Qt environment and reports the exit code.
rem Usage: run.cmd [path\to\binary]   (defaults to the debug build)
rem Without this, a QML load failure is printed to the debugger channel only and the
rem redirect to a file looks like the app simply never said anything.
set "QT_FORCE_STDERR_LOGGING=1"
set "QT_ASSUME_STDERR_HAS_CONSOLE=1"
if "%~1"=="" (set "EXE=target\debug\hello-cxxqt.exe") else (set "EXE=%~1")
call "%~dp0env.cmd" "%EXE%"
echo EXITCODE=%errorlevel%
