<#
.SYNOPSIS
  Build the DarkBurn Windows installer: dist/darkburn-<version>-windows-x64.msi

.DESCRIPTION
  Builds darkburn.exe and darkburn-mcp.exe in release (static C runtime), generates
  THIRD-PARTY-LICENSES.txt, and wraps them in an MSI with WiX v5 (packaging/windows/darkburn.wxs).
  The version is the workspace version in the root Cargo.toml.

  The MSI installs per user by default (no admin rights) and for all users with ALLUSERS=1 or the
  wizard's "Anyone who uses this computer" choice. Upgrades replace an earlier install (stable
  UpgradeCode). Uninstall keeps the settings folder; REMOVE_SETTINGS=1 removes it too.

  Needs (all user-level, no admin):
    Rust (MSVC toolchain)
    cargo install cargo-about --locked --features cli
    a .NET 6+ SDK (https://dot.net/v1/dotnet-install.ps1 -Channel 8.0), then
    dotnet tool install --global wix --version 5.0.2
    wix extension add -g WixToolset.UI.wixext/5.0.2
    wix extension add -g WixToolset.Util.wixext/5.0.2

.EXAMPLE
  scripts/build-installer.ps1              # build everything and make the MSI
  scripts/build-installer.ps1 -SkipBuild   # reuse target/x86_64-pc-windows-msvc/release
#>
param([switch] $SkipBuild)
. (Join-Path $PSScriptRoot 'package-common.ps1')

if (-not (Get-Command wix -ErrorAction SilentlyContinue)) {
  throw 'wix is not installed: dotnet tool install --global wix --version 5.0.2 (see the header of this script)'
}
Write-Output "DarkBurn $Version (MSI version $MsiVersion) for Windows x64"
Initialize-Stage -SkipBuild:$SkipBuild

$Icon = Join-Path $Root 'assets\icons\app\darkburn.ico'
if (-not (Test-Path $Icon)) { throw "missing the app icon: $Icon" }
$Msi = Join-Path $Dist "darkburn-$Version-windows-x64.msi"
Remove-Item -Force $Msi -ErrorAction SilentlyContinue

Invoke-Native 'wix build' {
  wix build (Join-Path $Root 'packaging\windows\darkburn.wxs') -arch x64 `
    -ext WixToolset.UI.wixext -ext WixToolset.Util.wixext `
    -d "Version=$MsiVersion" -d "BinDir=$Stage" -d "IconPath=$Icon" `
    -o $Msi
}
# wix writes its debug symbols next to the MSI; keep them out of the release assets.
Remove-Item -Force -ErrorAction SilentlyContinue ([IO.Path]::ChangeExtension($Msi, '.wixpdb'))

Invoke-Native 'MSI validation (ICE)' {
  wix msi validate $Msi -intermediateFolder (Join-Path $Stage 'msi-validation')
}

Get-Item $Msi | Format-Table Name, Length
Write-Output "MSI: $Msi"
