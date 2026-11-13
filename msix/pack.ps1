# Packages FalconShot into an unsigned MSIX for Microsoft Store submission
# (the Store re-signs packages during ingestion).
#
# Usage:
#   .\pack.ps1 -IdentityName "<Package/Identity Name from Partner Center>" `
#              -Publisher "<Publisher from Partner Center>" `
#              [-Version "0.2.0.0"]
param(
  [Parameter(Mandatory = $true)][string]$IdentityName,
  [Parameter(Mandatory = $true)][string]$Publisher,
  [string]$Version = "0.2.1.0"
)
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

# Build the exe with the custom-protocol feature: without it the app tries
# to load the vite dev server (127.0.0.1:5173) even in release mode and the
# main window shows ERR_CONNECTION_REFUSED on machines without the dev server.
cargo build --release -p falconshot-desktop --features custom-protocol
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
$ExePath = "..\target\release\falconshot-desktop.exe"

$stage = Join-Path $PSScriptRoot "stage"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path (Join-Path $stage "assets") | Out-Null

Copy-Item $ExePath $stage
Copy-Item (Join-Path $PSScriptRoot "assets\*.png") (Join-Path $stage "assets")

(Get-Content (Join-Path $PSScriptRoot "AppxManifest.template.xml")) `
  -replace "__IDENTITY_NAME__", $IdentityName `
  -replace "__PUBLISHER__", $Publisher `
  -replace "__VERSION__", $Version |
  Set-Content (Join-Path $stage "AppxManifest.xml") -Encoding UTF8

$makeappx = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin\*\x64\makeappx.exe" |
  Sort-Object FullName -Descending | Select-Object -First 1
$out = Join-Path $PSScriptRoot ("FalconShot_" + $Version + "_x64.msix")
& $makeappx.FullName pack /o /d $stage /p $out
Write-Output ""
Write-Output "Package ready: $out"
