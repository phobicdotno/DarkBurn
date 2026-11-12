<#
.SYNOPSIS
  Shared steps of build-installer.ps1 and build-portable.ps1 (dot-sourced, not run on its own).

  Provides: $Root, $Version, $Target, $Dist, $Stage, and Initialize-Stage, which builds the
  release binaries (unless -SkipBuild) and fills $Stage with the files both packages contain:
  darkburn.exe, darkburn-mcp.exe, LICENSE-MIT, LICENSE-APACHE, ATTRIBUTION.md, README.md,
  THIRD-PARTY-LICENSES.txt and the licences folder (licenses\) that ATTRIBUTION.md points to.

  The binaries link the C runtime statically (+crt-static), so nothing else needs installing.

  The build and package steps are adapted from SolidCraft's packaging\windows\package.ps1, which
  is itself adapted from PhotoCraft's packaging (Apache-2.0, see ATTRIBUTION.md). Changed for
  DarkBurn: split into these shared steps and two package scripts, two programs, x64 only, the
  third-party licence list written by cargo about.
#>
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

function Invoke-Native([string] $What, [scriptblock] $Block) {
  Write-Output "==> $What"
  & $Block
  if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

# The version lives in one place: [workspace.package] version in the root Cargo.toml.
$Version = $null
$inPkg = $false
foreach ($line in Get-Content (Join-Path $Root 'Cargo.toml')) {
  if ($line -match '^\s*\[') { $inPkg = ($line.Trim() -eq '[workspace.package]'); continue }
  if ($inPkg -and $line -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
}
if (-not $Version) { throw 'could not read [workspace.package] version from Cargo.toml' }
# MSI ProductVersion is numeric (major.minor.build, major under 256); a pre-release tag is dropped there.
$MsiVersion = ($Version -split '[-+]')[0]

$Target = 'x86_64-pc-windows-msvc'
$Dist = if ($env:DIST) { $env:DIST } else { Join-Path $Root 'dist' }
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
$Bin = Join-Path $TargetDir "$Target\release"
$Stage = Join-Path $TargetDir 'windows-package\stage'

# Tools installed with `cargo install` or `dotnet tool install --global` live here.
foreach ($p in @("$env:USERPROFILE\.cargo\bin", "$env:USERPROFILE\.dotnet", "$env:USERPROFILE\.dotnet\tools")) {
  if ((Test-Path $p) -and ($env:PATH -notlike "*$p*")) { $env:PATH = "$p;$env:PATH" }
}

# Writes <package>.sha256 beside a finished package: the SHA-256 in hex, two spaces, the file
# name, and a plain line feed. The installers check a download against it, and `sha256sum -c`
# reads it too.
function Write-Sha256File([string] $Path) {
  $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLower()
  $name = Split-Path -Leaf $Path
  Set-Content -LiteralPath "$Path.sha256" -Value "$hash  $name`n" -Encoding ascii -NoNewline
  Write-Output "SHA-256: $hash  $name"
}

# The licence page of the installer wizard takes an RTF file. This writes one from the two
# licence texts DarkBurn is under. Without it the wizard shows WiX's placeholder text.
function New-LicenceRtf([string] $OutFile) {
  $text = "DarkBurn is licensed under the MIT License or the Apache License, Version 2.0, at your option. Both follow.`n`n" +
    (Get-Content -Raw (Join-Path $Root 'LICENSE-MIT')).Trim() + "`n`n`n" +
    (Get-Content -Raw (Join-Path $Root 'LICENSE-APACHE')).Trim() + "`n"
  $rtf = New-Object System.Text.StringBuilder
  [void]$rtf.Append('{\rtf1\ansi\deff0{\fonttbl{\f0\fnil Segoe UI;}}\fs18 ')
  foreach ($ch in $text.ToCharArray()) {
    $code = [int]$ch
    if ($code -eq 13) { continue }
    elseif ($code -eq 10) { [void]$rtf.Append("\par`r`n") }
    elseif ($code -eq 9) { [void]$rtf.Append('\tab ') }
    elseif ($code -eq 92 -or $code -eq 123 -or $code -eq 125) { [void]$rtf.Append('\').Append($ch) }
    elseif ($code -gt 126) {
      # Anything outside plain ASCII as a numbered character (a signed 16-bit number in RTF).
      if ($code -gt 32767) { $code -= 65536 }
      [void]$rtf.Append('\u').Append($code).Append('?')
    }
    else { [void]$rtf.Append($ch) }
  }
  [void]$rtf.Append('}')
  [System.IO.File]::WriteAllText($OutFile, $rtf.ToString(), [System.Text.Encoding]::ASCII)
}

function Get-PeSubsystem([string] $Path) {
  $bytes = [System.IO.File]::ReadAllBytes($Path)
  $pe = [BitConverter]::ToInt32($bytes, 0x3C)
  return @{ Machine = [BitConverter]::ToUInt16($bytes, $pe + 4); Subsystem = [BitConverter]::ToUInt16($bytes, $pe + 0x5C) }
}

function New-ThirdPartyLicences([string] $OutFile) {
  if (Get-Command cargo-about -ErrorAction SilentlyContinue) {
    Push-Location $Root
    try {
      Invoke-Native 'cargo about generate (THIRD-PARTY-LICENSES.txt)' {
        cargo about generate --locked --workspace --config packaging/about.toml packaging/about.hbs -o $OutFile
      }
    } finally { Pop-Location }
  } else {
    throw 'cargo-about is not installed: run `cargo install cargo-about --locked --features cli`'
  }
}

function Initialize-Stage([switch] $SkipBuild) {
  if (-not $SkipBuild) {
    # Static CRT: no VC++ redistributable needed. Scoped to the target so build scripts and
    # proc-macros are unaffected.
    $flagVar = 'CARGO_TARGET_' + ($Target.ToUpper() -replace '-', '_') + '_RUSTFLAGS'
    [Environment]::SetEnvironmentVariable($flagVar, '-C target-feature=+crt-static')
    Push-Location $Root
    try {
      Invoke-Native "cargo build --release ($Target)" { cargo build --release --locked -p darkburn -p darkburn-mcp --target $Target }
    } finally { Pop-Location }
  }
  # darkburn.exe is a GUI program (subsystem 2: no console window), darkburn-mcp.exe a console one (3).
  foreach ($check in @(@('darkburn.exe', 2), @('darkburn-mcp.exe', 3))) {
    $path = Join-Path $Bin $check[0]
    if (-not (Test-Path $path)) { throw "missing $path (build first, or drop -SkipBuild)" }
    $h = Get-PeSubsystem $path
    if ($h.Machine -ne 0x8664) { throw "$($check[0]) is not an x64 program" }
    if ($h.Subsystem -ne $check[1]) { throw "$($check[0]) has PE subsystem $($h.Subsystem), expected $($check[1])" }
    Write-Output "ok $($check[0]): x64, PE subsystem $($h.Subsystem)"
  }
  Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force -Path $Stage | Out-Null
  Copy-Item (Join-Path $Bin 'darkburn.exe'), (Join-Path $Bin 'darkburn-mcp.exe') $Stage
  foreach ($f in 'LICENSE-MIT', 'LICENSE-APACHE', 'ATTRIBUTION.md', 'README.md') { Copy-Item (Join-Path $Root $f) $Stage }
  # The licence texts and notices that ATTRIBUTION.md points to.
  Copy-Item (Join-Path $Root 'licenses') (Join-Path $Stage 'licenses') -Recurse
  # The MSI names each of them. A text added to licenses\ and not named in the .wxs would be
  # left out of the installer without a word, so that stops here.
  $wxs = Get-Content -Raw (Join-Path $Root 'packaging\windows\darkburn.wxs')
  foreach ($f in Get-ChildItem (Join-Path $Stage 'licenses') -File) {
    if (-not $wxs.Contains("\licenses\$($f.Name)`"")) {
      throw "licenses\$($f.Name) is not listed in packaging\windows\darkburn.wxs (component LicenceTexts)"
    }
  }
  New-ThirdPartyLicences (Join-Path $Stage 'THIRD-PARTY-LICENSES.txt')
  New-Item -ItemType Directory -Force -Path $Dist | Out-Null
}
