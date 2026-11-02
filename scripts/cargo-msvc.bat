@echo off
rem Windows helper: enter the VS2022 x64 developer environment, then run cargo.
rem Set LIBCLANG_PATH to the directory containing libclang.dll before building
rem with --features llama (e.g. pip install libclang, then point at its native/).
if not exist "%ProgramFiles%\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" (
  echo vcvars64.bat not found; install Visual Studio 2022 C++ build tools.
  exit /b 1
)
call "%ProgramFiles%\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" >nul
if not defined LIBCLANG_PATH echo [llmgui] LIBCLANG_PATH is not set; llama feature will fail.
cargo %*
