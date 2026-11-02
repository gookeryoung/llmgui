param(
    [string]$InstallDir = "$env:LOCALAPPDATA\llmgui\bin",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $SkipBuild) {
    Write-Host "building release binary..."
    Push-Location $root
    cargo build --release -p llmgui-cli
    Pop-Location
}
$binary = Join-Path $root "target\release\llmgui.exe"
if (-not (Test-Path $binary)) {
    throw "release binary not found: $binary (run without -SkipBuild)"
}
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item $binary (Join-Path $InstallDir "llmgui.exe") -Force
Write-Host "installed: $(Join-Path $InstallDir 'llmgui.exe')"
Write-Host "Add this directory to PATH, then run: llmgui doctor"
