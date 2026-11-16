# Builds the Windows installer from an existing release build.
#   pwsh tools/build-windows-installer.ps1 0.2.1
# Writes feather-windows-x86_64-setup.exe to the repository root.
param([Parameter(Mandatory = $true)][string]$Version)
$ErrorActionPreference = 'Stop'

$iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
if (-not (Test-Path $iscc)) {
    choco install innosetup -y --no-progress
    if ($LASTEXITCODE -ne 0) { throw 'Could not install Inno Setup' }
}
& $iscc "/DAppVersion=$Version" "$PSScriptRoot\..\packaging\windows\feather.iss"
if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed ($LASTEXITCODE)" }
