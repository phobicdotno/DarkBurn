<#
.SYNOPSIS
  Shared steps of build-installer.ps1 and build-portable.ps1 (dot-sourced, not run on its own).

  Provides: $Root, $Version, $Target, $Dist, $Stage, and Initialize-Stage, which builds the
  release binaries (unless -SkipBuild) and fills $Stage with the files both packages contain:
  darkburn.exe, darkburn-mcp.exe, LICENSE-MIT, LICENSE-APACHE, ATTRIBUTION.md, README.md and
  THIRD-PARTY-LICENSES.txt.

  The binaries link the C runtime statically (+crt-static), so nothing else needs installing.
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
  New-ThirdPartyLicences (Join-Path $Stage 'THIRD-PARTY-LICENSES.txt')
  New-Item -ItemType Directory -Force -Path $Dist | Out-Null
}
