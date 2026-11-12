<#
.SYNOPSIS
  Build the portable zip: dist/darkburn-<version>-windows-x64-portable.zip (and its .sha256)

.DESCRIPTION
  darkburn.exe, darkburn-mcp.exe, the licences, and a portable.txt marker. With portable.txt
  beside darkburn.exe the program keeps its settings in a `settings` folder beside the exe
  instead of %AppData%\DarkBurn (apps/darkburn/src/startup.rs). Nothing is registered with
  Windows. Unzip anywhere (a USB stick works) and run darkburn.exe.

.EXAMPLE
  scripts/build-portable.ps1
  scripts/build-portable.ps1 -SkipBuild
#>
param([switch] $SkipBuild)
. (Join-Path $PSScriptRoot 'package-common.ps1')

Write-Output "DarkBurn $Version portable for Windows x64"
Initialize-Stage -SkipBuild:$SkipBuild

$Name = "darkburn-$Version-windows-x64-portable"
$Folder = Join-Path $TargetDir "windows-package\$Name"
Remove-Item -Recurse -Force $Folder -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Folder | Out-Null
Copy-Item (Join-Path $Stage '*') $Folder -Recurse -Exclude 'msi-validation'
Set-Content -Path (Join-Path $Folder 'portable.txt') -Encoding ascii -Value @(
  'This file makes DarkBurn portable.',
  'While it sits next to darkburn.exe, settings, auto-save and the other files DarkBurn',
  'keeps are stored in the "settings" folder beside the program instead of in',
  '%AppData%\DarkBurn. Delete this file to use the normal folder again.'
)
$Zip = Join-Path $Dist "$Name.zip"
Remove-Item -Force $Zip -ErrorAction SilentlyContinue
# Written entry by entry so the names use forward slashes (Windows PowerShell 5.1's Compress-Archive
# writes backslashes, which other unzip tools mishandle).
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::Open($Zip, 'Create')
try {
  foreach ($file in Get-ChildItem $Folder -Recurse -File) {
    $entry = "$Name/" + $file.FullName.Substring($Folder.Length + 1).Replace('\', '/')
    [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $file.FullName, $entry, 'Optimal')
  }
} finally { $archive.Dispose() }
Write-Sha256File $Zip
Get-Item $Zip | Format-Table Name, Length
Write-Output "Zip: $Zip"
