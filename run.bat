@echo off
rem Run Seoul in release mode. Extra arguments pass through, e.g.
rem   run.bat --synth --preset Vortex
rem   run.bat --help
cd /d "%~dp0"
cargo run --release -- %*
if errorlevel 1 pause
