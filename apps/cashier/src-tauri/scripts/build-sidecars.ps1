# Build the bouncer-mock sidecar binary and copy it to the location Tauri
# expects (binaries/bouncer-mock-<target-triple>.exe).

$ErrorActionPreference = "Stop"

$ScriptDir     = Split-Path -Parent $MyInvocation.MyCommand.Definition
$SrcTauriDir   = Resolve-Path (Join-Path $ScriptDir "..")
$WorkspaceRoot = Resolve-Path (Join-Path $SrcTauriDir "..\..\..")

$Triple = (& rustc -vV) | Select-String "^host:" | ForEach-Object { ($_ -replace "host: ", "").Trim() }
if (-not $Triple) {
    Write-Error "build-sidecars.ps1: failed to detect host target triple via rustc"
    exit 1
}

$Ext = ""
if ($Triple -like "*windows*") { $Ext = ".exe" }

$BinName = "bouncer-mock$Ext"
$OutName = "bouncer-mock-$Triple$Ext"
$OutDir  = Join-Path $SrcTauriDir "binaries"

Write-Host "build-sidecars.ps1: building bouncer-mock for $Triple"
Push-Location $WorkspaceRoot
try {
    & cargo build --release -p bouncer-mock
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
    Pop-Location
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$SrcPath = Join-Path $WorkspaceRoot "target\release\$BinName"
$DstPath = Join-Path $OutDir $OutName
Copy-Item -Force $SrcPath $DstPath
Write-Host "build-sidecars.ps1: copied -> $DstPath"
