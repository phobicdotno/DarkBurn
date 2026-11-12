# Reads every PowerShell script under scripts/ with the parser of the PowerShell that runs this
# file, and fails when one does not parse. Nothing is executed.
#
# ci.yml runs it twice: under PowerShell 7 and under Windows PowerShell 5.1. The second is what
# "irm ... | iex" meets on a fresh Windows, and it does not know the newer syntax (&&, ?:, ?.).
#
# A script must also be plain ASCII: Windows PowerShell 5.1 reads a script file without a
# byte-order mark in the system's code page, so any other character can come out wrong.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$bad = 0
Write-Output "PowerShell $($PSVersionTable.PSVersion)"
foreach ($f in Get-ChildItem (Join-Path $root 'scripts') -Filter *.ps1) {
    $tokens = $null
    $errors = $null
    [void][System.Management.Automation.Language.Parser]::ParseFile($f.FullName, [ref]$tokens, [ref]$errors)
    foreach ($e in $errors) {
        Write-Output "::error file=scripts/$($f.Name),line=$($e.Extent.StartLineNumber)::$($e.Message)"
        $bad++
    }
    $line = 0
    foreach ($text in [System.IO.File]::ReadAllLines($f.FullName)) {
        $line++
        if ($text -cmatch '[^\x00-\x7F]') {
            Write-Output "::error file=scripts/$($f.Name),line=$line::a character outside ASCII; Windows PowerShell 5.1 may read it wrong"
            $bad++
        }
    }
    Write-Output "$($f.Name): $(@($errors).Count) parse errors"
}
if ($bad -gt 0) { exit 1 }
