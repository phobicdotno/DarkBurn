<#
.SYNOPSIS
  DarkBurn installer for Windows.

    irm https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.ps1 | iex

.DESCRIPTION
  Status: pre-release. DarkBurn has not yet run a job on a real machine.

  Runs in Windows PowerShell 5.1 and PowerShell 7, without admin rights, and installs for the
  current user only.

  1. Looks in the latest GitHub release for darkburn-<version>-windows-x64.msi and installs it
     silently (msiexec /i ... /qn, see docs/RELEASING.md). With no MSI but a
     darkburn-<version>-windows-x64-portable.zip, that is unpacked instead. A <name>.sha256 file
     in the release is checked when there is one.
  2. With no release (today's case) it builds from source: fetches DarkBurn and SolidCraft side
     by side, runs cargo, and copies darkburn.exe and darkburn-mcp.exe into place. This needs
     Git, Rust (MSVC) and the Visual Studio Build Tools, and can take ten minutes or more.
     When one of them is missing the script says what to install and stops.
  The programs go to %LocalAppData%\Apps\DarkBurn, with a DarkBurn entry in the Start menu.
  Run it again to update.

  `iex` cannot take arguments, so the settings are environment variables. Set them first:

    $env:DARKBURN_FROM_SOURCE = '1'; irm <address> | iex

    DARKBURN_REPO            DarkBurn git URL or local path     [https://github.com/phobicdotno/DarkBurn]
    DARKBURN_REF             Branch, tag or commit to install   [the latest release, else main]
    SOLIDCRAFT_REPO          SolidCraft git URL or local path   [https://github.com/phobicdotno/solidcraft]
    SOLIDCRAFT_REF           SolidCraft branch, tag or commit   [the commit DarkBurn is pinned to]
    DARKBURN_PREFIX          Folder the programs go into        [%LocalAppData%\Apps\DarkBurn]
    DARKBURN_BUILD_DIR       Where the source is kept and built [%LocalAppData%\darkburn-build]
    DARKBURN_FROM_SOURCE=1   Build from source even when a release exists
    DARKBURN_INSTALL_RUST=1  Install Rust with rustup when it is missing or too old
    DARKBURN_UNINSTALL=1     Remove what this installer (or the MSI) put on the computer.
                             Settings in %AppData%\DarkBurn are kept. The variable is cleared
                             afterwards, so the next run installs again.

  The whole script is one function, called on the last line, so a download that was cut off
  runs nothing. Nothing that is downloaded is ever run as script text.
#>

function Install-DarkBurn {
    # With `irm | iex` this runs in your own PowerShell session. A strict mode set there (some
    # profiles do) would stop this script at the first variable or property that is not set.
    Set-StrictMode -Off
    $ErrorActionPreference = 'Stop'
    # The progress bar makes downloads in Windows PowerShell 5.1 many times slower.
    $ProgressPreference = 'SilentlyContinue'

    # The SolidCraft commit this version of DarkBurn builds against. Keep this line in this exact
    # form: the installer reads it from the DarkBurn checkout it is about to build, so an older
    # tag is built against the SolidCraft commit it was written for. install.sh has the same line.
    $SolidCraftPin = 'ae7da14907e00a11ed04656385cc0c2db386dc55'

    $GitHubRepo = 'phobicdotno/DarkBurn'
    $DefaultDarkBurnRepo = 'https://github.com/phobicdotno/DarkBurn'
    $DefaultSolidCraftRepo = 'https://github.com/phobicdotno/solidcraft'
    $InstallUrl = 'https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.ps1'
    # Used only when the checkout's Cargo.toml cannot be read.
    $MinRustFallback = '1.95'
    $ManifestName = 'installed-by-darkburn-installer.txt'
    $BuildMarker = '.darkburn-installer'

    # ---------------------------------------------------------------------------------------
    # Small helpers
    # ---------------------------------------------------------------------------------------

    function Say([string] $Text) { Write-Host $Text }
    function Step([string] $Text) { Write-Host "==> $Text" }

    function Get-Setting([string] $Name, [string] $Default) {
        $value = [Environment]::GetEnvironmentVariable($Name)
        if ([string]::IsNullOrEmpty($value)) { return $Default }
        return $value
    }

    function Test-Tool([string] $Name) {
        return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
    }

    # True for a plain version number such as 0.0.509.
    function Test-Version([string] $Text) {
        return ($Text -match '^[0-9]+(\.[0-9]+)+$')
    }

    # Environment variables changed for the build are put back at the end: with `irm | iex`
    # this runs in your own PowerShell session, and it should leave it as it found it.
    # An empty value means "not set". The variable is removed, never set to an empty string:
    # PowerShell 7.5 and newer keep an empty variable as an empty variable, and git and cargo
    # both stop on an empty GIT_DIR, GIT_OBJECT_DIRECTORY or CARGO_TARGET_DIR.
    $savedEnv = @{}
    function Set-EnvOrRemove([string] $Name, [string] $Value) {
        if ([string]::IsNullOrEmpty($Value)) {
            Remove-Item -LiteralPath "Env:\$Name" -ErrorAction SilentlyContinue
        } else {
            [Environment]::SetEnvironmentVariable($Name, $Value)
        }
    }
    function Set-TempEnv([string] $Name, [string] $Value) {
        if (-not $savedEnv.ContainsKey($Name)) { $savedEnv[$Name] = [Environment]::GetEnvironmentVariable($Name) }
        Set-EnvOrRemove $Name $Value
    }
    function Restore-Env {
        foreach ($name in @($savedEnv.Keys)) { Set-EnvOrRemove $name $savedEnv[$name] }
    }

    # Runs a program and stops when it fails. git and cargo write their progress to the error
    # stream; Windows PowerShell 5.1 would turn that into an error under 'Stop'. What the program
    # prints goes to the screen (Out-Host), not into the value of the function that called this.
    function Invoke-Tool([string] $What, [scriptblock] $Block) {
        $ErrorActionPreference = 'Continue'
        $global:LASTEXITCODE = 0
        & $Block | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "$What failed (exit code $LASTEXITCODE)." }
    }

    # Runs a program and says whether it worked, without stopping.
    function Test-ToolRun([scriptblock] $Block) {
        $ErrorActionPreference = 'Continue'
        $global:LASTEXITCODE = 0
        & $Block | Out-Host
        return ($LASTEXITCODE -eq 0)
    }

    # The text a program prints, or nothing when it fails or is not there.
    function Get-ToolOutput([scriptblock] $Block) {
        $ErrorActionPreference = 'Continue'
        try { return (& $Block 2>$null) } catch { return $null }
    }

    function Add-CargoToPath {
        $cargoHome = Get-Setting 'CARGO_HOME' (Join-Path $env:USERPROFILE '.cargo')
        $cargoBin = Join-Path $cargoHome 'bin'
        if ((Test-Path (Join-Path $cargoBin 'cargo.exe')) -and (($env:PATH -split ';') -notcontains $cargoBin)) {
            Set-TempEnv 'PATH' "$cargoBin;$env:PATH"
        }
    }

    # A git source as given: a local folder becomes a full path, a URL is kept.
    function Resolve-RepoSource([string] $Source) {
        # https://..., ssh://..., git@host:owner/name
        if ($Source -match '^[A-Za-z][A-Za-z0-9+.-]+://' -or $Source -match '^[^\\/]+@[^\\/]+:') { return $Source }
        if (Test-Path -LiteralPath $Source -PathType Container) { return (Resolve-Path -LiteralPath $Source).Path }
        return $Source
    }

    # The value of `key = "value"` in one [section] of a TOML file, or $null.
    function Get-TomlValue([string] $File, [string] $Section, [string] $Key) {
        if (-not (Test-Path -LiteralPath $File)) { return $null }
        $inside = $false
        foreach ($line in Get-Content -LiteralPath $File) {
            $trimmed = $line.Trim()
            if ($trimmed.StartsWith('[')) { $inside = ($trimmed -eq "[$Section]"); continue }
            if ($inside -and $trimmed -match ('^' + [regex]::Escape($Key) + '\s*=\s*"([^"]*)"')) { return $Matches[1] }
        }
        return $null
    }

    # The SolidCraft pin written in a DarkBurn checkout's copy of this file, or $null.
    function Get-PinFrom([string] $File) {
        if (-not (Test-Path -LiteralPath $File)) { return $null }
        foreach ($line in Get-Content -LiteralPath $File) {
            if ($line -match '^\s*\$SolidCraftPin\s*=\s*''([0-9a-f]{40})''\s*$') { return $Matches[1] }
        }
        return $null
    }

    # The product code of an installed DarkBurn MSI under one registry root, or $null.
    function Get-MsiProduct([string] $Root) {
        $uninstall = "$Root\Software\Microsoft\Windows\CurrentVersion\Uninstall"
        if (-not (Test-Path $uninstall)) { return $null }
        foreach ($key in @(Get-ChildItem $uninstall -ErrorAction SilentlyContinue)) {
            if (-not $key -or $key.PSChildName -notmatch '^\{[0-9A-Fa-f-]{36}\}$') { continue }
            if ($key.GetValue('DisplayName') -eq 'DarkBurn') { return $key.PSChildName }
        }
        return $null
    }

    # Windows does not let a running program be replaced. Asked at the start, and once more
    # just before the files are written: a build takes long enough for DarkBurn to be started.
    function Assert-NotRunning {
        if (Get-Process -Name 'darkburn', 'darkburn-mcp' -ErrorAction SilentlyContinue) {
            throw 'DarkBurn (or its MCP server, darkburn-mcp) is running. Close it and run the installer again.'
        }
    }

    # A folder setting as a full path. A relative path is taken from PowerShell's current
    # folder, which is not always the folder Windows has for the process.
    function Resolve-Folder([string] $Path) {
        return [IO.Path]::GetFullPath($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Path))
    }

    function Get-StartMenuLink {
        return (Join-Path ([Environment]::GetFolderPath('Programs')) 'DarkBurn.lnk')
    }

    function New-StartMenuLink([string] $Exe) {
        $link = Get-StartMenuLink
        $folder = Split-Path -Parent $link
        if (-not (Test-Path $folder)) { New-Item -ItemType Directory -Force -Path $folder | Out-Null }
        $shell = New-Object -ComObject WScript.Shell
        $shortcut = $shell.CreateShortcut($link)
        $shortcut.TargetPath = $Exe
        $shortcut.WorkingDirectory = (Split-Path -Parent $Exe)
        $shortcut.Description = 'DarkBurn laser cutter and engraver'
        $shortcut.IconLocation = "$Exe,0"
        $shortcut.Save()
    }

    # ---------------------------------------------------------------------------------------
    # Step A: a release
    # ---------------------------------------------------------------------------------------

    # Looks up the release to install. Returns a table with Release (the release as an object,
    # or $null) and Why (when there is none: a sentence that says why). GitHub answers 404 when
    # no release exists, 403 or 429 when this address has asked too often.
    function Get-Release([string] $Tag) {
        if ($Tag) { $api = "https://api.github.com/repos/$GitHubRepo/releases/tags/$Tag" }
        else { $api = "https://api.github.com/repos/$GitHubRepo/releases/latest" }
        try {
            $release = Invoke-RestMethod -UseBasicParsing -TimeoutSec 30 -Uri $api -Headers @{ 'User-Agent' = 'darkburn-install'; 'Accept' = 'application/vnd.github+json' }
            return @{ Release = $release; Why = '' }
        } catch {
            $status = 0
            try { if ($_.Exception.Response) { $status = [int]$_.Exception.Response.StatusCode } } catch { $status = 0 }
            if ($status -eq 404) {
                if ($Tag) { $why = "There is no release $Tag." } else { $why = 'No release has been published yet.' }
            } elseif ($status -gt 0) {
                $why = "GitHub answered the release lookup with HTTP $status, so it could not be read."
            } else {
                $why = 'The GitHub release list did not answer.'
            }
            return @{ Release = $null; Why = $why }
        }
    }

    function Get-Asset($Release, [string] $Name) {
        foreach ($asset in @($Release.assets)) {
            if ($asset -and $asset.name -eq $Name) { return $asset }
        }
        return $null
    }

    # Downloads a release file into $Folder and checks it against <name>.sha256 when the release
    # has one. Returns the path, or $null when the release has no file of that name.
    function Get-ReleaseFile($Release, [string] $Name, [string] $Folder) {
        $asset = Get-Asset $Release $Name
        if (-not $asset) { return $null }
        if ($asset.browser_download_url -notmatch '^https://') { return $null }
        $file = Join-Path $Folder $Name
        Step "Downloading $Name"
        Invoke-WebRequest -UseBasicParsing -Uri $asset.browser_download_url -OutFile $file
        $sumAsset = Get-Asset $Release "$Name.sha256"
        if ($sumAsset -and $sumAsset.browser_download_url -match '^https://') {
            $sumFile = "$file.sha256"
            Invoke-WebRequest -UseBasicParsing -Uri $sumAsset.browser_download_url -OutFile $sumFile
            $first = [string](Get-Content -LiteralPath $sumFile -TotalCount 1)
            $want = ($first.Trim() -split '\s+')[0].ToLower()
            $have = (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash.ToLower()
            if ($want -ne $have) {
                throw ("The SHA-256 of $Name does not match the one published with the release.`n" +
                    "  published  $want`n  downloaded $have`n" +
                    "Nothing was installed. Try again; if it keeps happening, please report it at`n" +
                    "https://github.com/$GitHubRepo/issues. DARKBURN_FROM_SOURCE=1 builds from source instead.")
            }
            Say '    checksum ok'
        }
        return $file
    }

    function Install-Msi([string] $Msi, [string] $InstallDir, [bool] $CustomDir) {
        $log = Join-Path $env:TEMP 'darkburn-install-msi.log'
        $arguments = @('/i', "`"$Msi`"", '/qn', '/norestart', '/l*v', "`"$log`"")
        if ($CustomDir) { $arguments += "APPLICATIONFOLDER=`"$InstallDir`"" }
        Step 'Installing for this user (no admin rights needed)'
        $process = Start-Process -FilePath 'msiexec.exe' -ArgumentList $arguments -Wait -PassThru
        if ($process.ExitCode -ne 0 -and $process.ExitCode -ne 3010) {
            throw "The installer (msiexec) stopped with exit code $($process.ExitCode). Its log is in $log"
        }
    }

    # Copies files into the install folder and writes down their names, so that uninstalling
    # removes exactly these.
    function Install-Files([string[]] $Files, [string] $InstallDir) {
        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
        $names = @()
        foreach ($file in $Files) {
            Copy-Item -LiteralPath $file -Destination $InstallDir -Force
            $names += (Split-Path -Leaf $file)
        }
        $manifest = @('# Files put here by the DarkBurn installer (scripts/install.ps1). Uninstalling removes them.') + $names
        Set-Content -LiteralPath (Join-Path $InstallDir $ManifestName) -Value $manifest -Encoding ASCII
        New-StartMenuLink (Join-Path $InstallDir 'darkburn.exe')
    }

    function Install-Portable([string] $Zip, [string] $Folder, [string] $InstallDir) {
        $unpacked = Join-Path $Folder 'portable'
        Expand-Archive -LiteralPath $Zip -DestinationPath $unpacked -Force
        $exe = Get-ChildItem -Path $unpacked -Recurse -Filter 'darkburn.exe' | Select-Object -First 1
        if (-not $exe) { throw 'The portable zip has no darkburn.exe in it.' }
        if (-not (Test-Path (Join-Path $exe.DirectoryName 'darkburn-mcp.exe'))) { throw 'The portable zip has no darkburn-mcp.exe in it.' }
        # portable.txt is left out on purpose: with it DarkBurn would keep its settings inside the
        # install folder. Without it they live in %AppData%\DarkBurn, as with the MSI.
        $files = @(Get-ChildItem -LiteralPath $exe.DirectoryName -File | Where-Object { $_.Name -ne 'portable.txt' } | ForEach-Object { $_.FullName })
        Step "Installing into $InstallDir"
        Install-Files $files $InstallDir
    }

    # ---------------------------------------------------------------------------------------
    # Step B: build from source
    # ---------------------------------------------------------------------------------------

    # True when Visual Studio (or its Build Tools) with the C++ compiler and linker is installed.
    function Test-BuildTools {
        $programFiles86 = [Environment]::GetEnvironmentVariable('ProgramFiles(x86)')
        if (-not $programFiles86) { $programFiles86 = $env:ProgramFiles }
        $vswhere = Join-Path $programFiles86 'Microsoft Visual Studio\Installer\vswhere.exe'
        if (-not (Test-Path -LiteralPath $vswhere)) { return $false }
        $found = Get-ToolOutput {
            & $vswhere -latest -products * -requiresAny -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 Microsoft.VisualStudio.Component.VC.Tools.ARM64 -property installationPath
        }
        return [bool]$found
    }

    function Install-Rust([string] $Folder) {
        Step 'Installing Rust with rustup (stable toolchain, for this user only)'
        if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { $rustupUrl = 'https://win.rustup.rs/aarch64' } else { $rustupUrl = 'https://win.rustup.rs/x86_64' }
        $rustupInit = Join-Path $Folder 'rustup-init.exe'
        Invoke-WebRequest -UseBasicParsing -Uri $rustupUrl -OutFile $rustupInit
        Invoke-Tool 'rustup-init' { & $rustupInit -y --profile minimal --default-toolchain stable }
        Add-CargoToPath
    }

    # True when the folder $Dir is a git checkout of its own (not merely inside one).
    function Test-Checkout([string] $Dir) {
        $ErrorActionPreference = 'Continue'
        $global:LASTEXITCODE = 0
        try { & git "--git-dir=$(Join-Path $Dir '.git')" rev-parse --git-dir 2>$null | Out-Null } catch { return $false }
        return ($LASTEXITCODE -eq 0)
    }

    # Brings the checkout in $Dir to $Ref of the repository $Url. Shallow when the server allows
    # it (GitHub does, also for a commit id), full history otherwise.
    #
    # Every git call names the checkout outright (--git-dir and --work-tree). With `git -C` git
    # would look for a repository by itself, and when the checkout is not one (an init that was
    # cut off) it would find the one around it, for example a user folder kept in git, and check
    # DarkBurn out over that.
    function Sync-Repo([string] $Name, [string] $Dir, [string] $Url, [string] $Ref, [string] $Setting) {
        $gitDir = "--git-dir=$(Join-Path $Dir '.git')"
        $workTree = "--work-tree=$Dir"
        # A checkout that is not a repository is made again. It is inside the marked build folder.
        if ((Test-Path -LiteralPath $Dir) -and -not (Test-Checkout $Dir)) {
            Remove-Item -LiteralPath $Dir -Recurse -Force
        }
        if (-not (Test-Path -LiteralPath (Join-Path $Dir '.git'))) {
            New-Item -ItemType Directory -Force -Path $Dir | Out-Null
            Invoke-Tool "git init ($Name)" { git $gitDir $workTree init -q }
        }
        Step "Fetching $Name ($Ref)"
        $retry = "If it keeps happening, delete $Dir and run the installer again."
        if (Test-ToolRun { git $gitDir $workTree -c core.longpaths=true fetch --depth 1 $Url $Ref }) {
            if (-not (Test-ToolRun { git $gitDir $workTree -c core.longpaths=true checkout -q -f --detach FETCH_HEAD })) {
                throw "Could not check out $Name $Ref.`n$retry"
            }
            return
        }
        # No shallow fetch of that ref (a short commit id, or a server that refuses): take it all.
        # The depth is git's "everything", which also completes a checkout that was shallow before.
        Say "    $Ref cannot be fetched on its own. Fetching the full history."
        if (-not (Test-ToolRun { git $gitDir $workTree -c core.longpaths=true fetch --depth 2147483647 --tags $Url '+refs/heads/*:refs/remotes/origin/*' })) {
            throw ("Could not fetch $Name from $Url.`n" +
                "Check the network. If the repository is private or has moved, point $Setting at a copy you can reach.`n" +
                "If the lines above name a lock file or a broken repository, delete $Dir and run the installer again.")
        }
        foreach ($candidate in @("refs/remotes/origin/$Ref", "refs/tags/$Ref", $Ref)) {
            $commit = ([string](Get-ToolOutput { git $gitDir $workTree rev-parse -q --verify "$candidate^{commit}" })).Trim()
            if ($commit -match '^[0-9a-f]{40,64}$') {
                if (-not (Test-ToolRun { git $gitDir $workTree -c core.longpaths=true checkout -q -f --detach $commit })) {
                    throw "Could not check out $Name $Ref.`n$retry"
                }
                return
            }
        }
        throw "$Name has no branch, tag or commit named $Ref."
    }

    # Builds both programs and returns the folder they are in.
    function Build-FromSource([string] $Folder, [string] $BuildDir, [string] $Ref) {
        Add-CargoToPath
        $missing = @()
        if (-not (Test-Tool 'git')) {
            $missing += "Git for Windows`n      https://git-scm.com/download/win    or: winget install --id Git.Git -e"
        }
        $haveBuildTools = Test-BuildTools
        if (-not $haveBuildTools) {
            $missing += ("Visual Studio Build Tools with the workload `"Desktop development with C++`"`n" +
                "      https://visualstudio.microsoft.com/visual-cpp-build-tools/`n" +
                "      or: winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override `"--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended`"")
        }
        $haveRust = (Test-Tool 'cargo') -and (Test-Tool 'rustc')
        if (-not $haveRust -and $env:DARKBURN_INSTALL_RUST -ne '1') {
            $missing += ("Rust (MSVC toolchain), from rustup`n" +
                "      https://rustup.rs    or: winget install --id Rustlang.Rustup -e`n" +
                "      (with DARKBURN_INSTALL_RUST=1 set, this installer does that step itself)")
        }
        if ($missing.Count -gt 0) {
            throw ("Building DarkBurn from source needs these, and they were not found:`n`n  - " +
                ($missing -join "`n  - ") +
                "`n`nInstall them, open a new PowerShell window and run the installer again.`n" +
                "It does not install them itself, and nothing was changed.")
        }
        if (-not $haveRust) {
            # Only reached with DARKBURN_INSTALL_RUST=1, and with the Build Tools in place.
            Install-Rust $Folder
            if (-not ((Test-Tool 'cargo') -and (Test-Tool 'rustc'))) {
                throw 'Rust was installed but cargo is still not found. Open a new PowerShell window and run the installer again.'
            }
        }

        # The build folder is reused between runs. It is marked so that neither a re-run nor an
        # uninstall ever touches a folder this installer did not make.
        if ((Test-Path -LiteralPath $BuildDir) -and -not (Test-Path -LiteralPath (Join-Path $BuildDir $BuildMarker))) {
            if (-not (Test-Path -LiteralPath $BuildDir -PathType Container) -or @(Get-ChildItem -LiteralPath $BuildDir -Force).Count -gt 0) {
                throw "$BuildDir exists and was not made by this installer. Set DARKBURN_BUILD_DIR to an empty or new folder."
            }
        }
        New-Item -ItemType Directory -Force -Path $BuildDir | Out-Null
        Set-Content -LiteralPath (Join-Path $BuildDir $BuildMarker) -Encoding ASCII -Value 'Made by the DarkBurn installer (scripts/install.ps1). Safe to delete; uninstalling removes it.'

        # Never wait for a password: a repository that cannot be read should fail at once.
        Set-TempEnv 'GIT_TERMINAL_PROMPT' '0'
        Set-TempEnv 'GCM_INTERACTIVE' 'never'
        # Git settings of this PowerShell session must not point the git calls at another
        # repository. Cleared here, put back at the end with the rest.
        foreach ($name in 'GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE', 'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES', 'GIT_COMMON_DIR', 'GIT_NAMESPACE') {
            Set-TempEnv $name ''
        }

        $source = Join-Path $BuildDir 'DarkBurn'
        $darkBurnUrl = Resolve-RepoSource (Get-Setting 'DARKBURN_REPO' $DefaultDarkBurnRepo)
        Sync-Repo 'DarkBurn' $source $darkBurnUrl $Ref 'DARKBURN_REPO'
        if (-not (Test-Path -LiteralPath (Join-Path $source 'Cargo.toml'))) {
            throw "$darkBurnUrl at $Ref is not a DarkBurn checkout (no Cargo.toml)."
        }

        # rustc must be the MSVC kind and at least the workspace's rust-version.
        Push-Location -LiteralPath $source
        try { $rustInfo = @(Get-ToolOutput { rustc -vV }) } finally { Pop-Location }
        $hostTriple = $null
        $rustHave = $null
        foreach ($line in $rustInfo) {
            if ($line -match '^host:\s*(\S+)') { $hostTriple = $Matches[1] }
            if ($line -match '^release:\s*([0-9]+(\.[0-9]+)*)') { $rustHave = $Matches[1] }
        }
        if (-not $hostTriple -or -not $rustHave) {
            throw "rustc is installed but does not run. With rustup, pick a toolchain with: rustup default stable"
        }
        if ($hostTriple -notmatch '-pc-windows-msvc$') {
            throw ("This Rust builds for $hostTriple. DarkBurn is built with the MSVC toolchain.`n" +
                "Switch with: rustup default stable-msvc")
        }
        $rustNeed = Get-TomlValue (Join-Path $source 'Cargo.toml') 'workspace.package' 'rust-version'
        if (-not $rustNeed) { $rustNeed = $MinRustFallback }
        if ([version]$rustHave -lt [version]$rustNeed) {
            if ($env:DARKBURN_INSTALL_RUST -eq '1' -and (Test-Tool 'rustup')) {
                Step "Rust $rustHave is older than ${rustNeed}: installing the stable toolchain with rustup"
                Invoke-Tool 'rustup toolchain install' { rustup toolchain install stable --profile minimal }
                Set-TempEnv 'RUSTUP_TOOLCHAIN' 'stable'
            } else {
                throw ("Rust $rustHave is too old: DarkBurn needs $rustNeed or newer.`n" +
                    "Update it with: rustup update stable`n" +
                    "and run the installer again (or set DARKBURN_INSTALL_RUST=1 and it does that for you).")
            }
        } else {
            Say "    Rust $rustHave (DarkBurn needs $rustNeed or newer)"
        }

        # DarkBurn's Cargo.toml reaches for ..\solidcraft, so the two checkouts sit side by side.
        $solidCraftRef = Get-Setting 'SOLIDCRAFT_REF' ''
        if (-not $solidCraftRef) { $solidCraftRef = Get-PinFrom (Join-Path $source 'scripts\install.ps1') }
        if (-not $solidCraftRef) { $solidCraftRef = $SolidCraftPin }
        $solidCraftUrl = Resolve-RepoSource (Get-Setting 'SOLIDCRAFT_REPO' $DefaultSolidCraftRepo)
        Sync-Repo 'SolidCraft' (Join-Path $BuildDir 'solidcraft') $solidCraftUrl $solidCraftRef 'SOLIDCRAFT_REPO'

        # As scripts/package-common.ps1 builds a release: the C runtime linked in (no Visual C++
        # redistributable needed), only for the target, so build scripts are unaffected.
        $targetDir = Join-Path $BuildDir 'target'
        Set-TempEnv 'CARGO_TARGET_DIR' $targetDir
        Set-TempEnv ('CARGO_TARGET_' + ($hostTriple.ToUpper() -replace '-', '_') + '_RUSTFLAGS') '-C target-feature=+crt-static'
        Step 'Building DarkBurn (cargo build --release). The first build can take ten minutes or more.'
        Push-Location -LiteralPath $source
        try {
            if (-not (Test-ToolRun { cargo build --release --locked -p darkburn -p darkburn-mcp --target $hostTriple })) {
                throw ("The build failed; the lines above say why.`n" +
                    "Nothing was installed or changed. The source and what was built so far stay in`n  $BuildDir`n" +
                    "so running the installer again continues from there.")
            }
        } finally {
            Pop-Location
        }
        $bin = Join-Path $targetDir "$hostTriple\release"
        foreach ($name in 'darkburn.exe', 'darkburn-mcp.exe') {
            if (-not (Test-Path -LiteralPath (Join-Path $bin $name))) { throw "The build finished but $name is not in $bin." }
        }
        return $bin
    }

    # ---------------------------------------------------------------------------------------
    # Uninstall
    # ---------------------------------------------------------------------------------------

    function Uninstall-DarkBurn([string] $InstallDir, [string] $BuildDir) {
        Step 'Removing DarkBurn'
        Assert-NotRunning
        $removed = $false

        # An install made by the MSI: Windows Installer removes it (program, shortcuts, file types).
        $product = Get-MsiProduct 'HKCU:'
        if ($product) {
            $process = Start-Process -FilePath 'msiexec.exe' -ArgumentList @('/x', $product, '/qn', '/norestart') -Wait -PassThru
            if ($process.ExitCode -ne 0 -and $process.ExitCode -ne 3010) {
                throw "Windows Installer (msiexec /x) stopped with exit code $($process.ExitCode). Remove DarkBurn in Settings > Apps instead."
            }
            Say '  removed the installed DarkBurn (MSI)'
            $removed = $true
        }

        # An install made by this script from the zip or from source: the files it wrote down.
        $manifest = Join-Path $InstallDir $ManifestName
        if (Test-Path -LiteralPath $manifest) {
            foreach ($name in @(Get-Content -LiteralPath $manifest)) {
                $name = $name.Trim()
                if (-not $name -or $name.StartsWith('#') -or $name -match '[\\/:]' -or $name -eq '..') { continue }
                $file = Join-Path $InstallDir $name
                if (Test-Path -LiteralPath $file -PathType Leaf) {
                    Remove-Item -LiteralPath $file -Force
                    Say "  removed $file"
                }
            }
            Remove-Item -LiteralPath $manifest -Force
            if (@(Get-ChildItem -LiteralPath $InstallDir -Force).Count -eq 0) { Remove-Item -LiteralPath $InstallDir -Force }
            $link = Get-StartMenuLink
            if (Test-Path -LiteralPath $link) {
                $target = (New-Object -ComObject WScript.Shell).CreateShortcut($link).TargetPath
                if ($target -and $target.StartsWith($InstallDir, [StringComparison]::OrdinalIgnoreCase)) {
                    Remove-Item -LiteralPath $link -Force
                    Say "  removed $link"
                }
            }
            $removed = $true
        }

        # The build folder: the two checkouts, the build output and the marker go. The folder
        # itself goes only when nothing else is in it, so files of your own that ended up there
        # are kept.
        if (Test-Path -LiteralPath $BuildDir) {
            $marker = Join-Path $BuildDir $BuildMarker
            if (Test-Path -LiteralPath $marker) {
                foreach ($name in 'DarkBurn', 'solidcraft', 'target') {
                    $folder = Join-Path $BuildDir $name
                    if (Test-Path -LiteralPath $folder) {
                        Remove-Item -LiteralPath $folder -Recurse -Force
                        Say "  removed $folder"
                        $removed = $true
                    }
                }
                Remove-Item -LiteralPath $marker -Force
                if (@(Get-ChildItem -LiteralPath $BuildDir -Force).Count -eq 0) {
                    Remove-Item -LiteralPath $BuildDir -Force
                    Say "  removed $BuildDir"
                    $removed = $true
                } else {
                    Say "  kept $BuildDir (it holds other files)"
                }
            } else {
                Say "  kept $BuildDir (it was not made by this installer)"
            }
        }

        if (-not $removed) { Say '  nothing found to remove (was it installed with another DARKBURN_PREFIX or DARKBURN_BUILD_DIR?)' }
        if (Get-MsiProduct 'HKLM:') {
            Say '  DarkBurn is also installed for all users. Remove that in Settings > Apps (it needs admin rights).'
        }
        Say ''
        $settings = Join-Path $env:APPDATA 'DarkBurn'
        if (Test-Path -LiteralPath $settings) {
            Say "Your settings are kept in $settings"
            Say 'Delete that folder too if you want them gone.'
        } else {
            Say "Settings would be in $settings; there are none."
        }
        Say 'Git, Rust and the Visual Studio Build Tools are left as they are.'
    }

    # ---------------------------------------------------------------------------------------
    # Main
    # ---------------------------------------------------------------------------------------

    if ($env:OS -ne 'Windows_NT') {
        throw 'This script is for Windows. On Linux and macOS use: curl -fsSL https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.sh | sh'
    }
    # Windows PowerShell 5.1 on older systems does not offer TLS 1.2 by itself; GitHub needs it.
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch { }

    $customDir = -not [string]::IsNullOrEmpty($env:DARKBURN_PREFIX)
    $installDir = Resolve-Folder (Get-Setting 'DARKBURN_PREFIX' (Join-Path $env:LOCALAPPDATA 'Apps\DarkBurn'))
    $buildDir = Resolve-Folder (Get-Setting 'DARKBURN_BUILD_DIR' (Join-Path $env:LOCALAPPDATA 'darkburn-build'))

    if ($env:DARKBURN_UNINSTALL -eq '1') {
        # One shot: cleared first, so that the next run in this window installs again.
        Remove-Item Env:\DARKBURN_UNINSTALL -ErrorAction SilentlyContinue
        Uninstall-DarkBurn $installDir $buildDir
        return
    }

    $arch = $env:PROCESSOR_ARCHITECTURE
    if ($env:PROCESSOR_ARCHITEW6432) { $arch = $env:PROCESSOR_ARCHITEW6432 }
    Say "DarkBurn installer (pre-release) for Windows $arch"
    if ($arch -ne 'AMD64' -and $arch -ne 'ARM64') { throw 'DarkBurn needs 64-bit Windows.' }
    if ($arch -eq 'ARM64') { Say 'Windows on ARM is untested. The release is an x64 program, which Windows 11 runs through emulation.' }
    Assert-NotRunning
    if (Get-MsiProduct 'HKLM:') {
        throw ("DarkBurn is already installed for all users of this computer.`n" +
            "Update that copy with a newer MSI from an administrator prompt, or remove it in Settings > Apps`n" +
            "and run this installer again to install it for you alone.")
    }

    # What to install. A bare version number means its tag.
    $ref = Get-Setting 'DARKBURN_REF' ''
    if (Test-Version $ref) { $ref = "v$ref" }
    # A release is used unless a build was asked for: DARKBURN_FROM_SOURCE=1, a repository of
    # your own, or a branch or commit (those have no release).
    $wantRelease = ($env:DARKBURN_FROM_SOURCE -ne '1') -and [string]::IsNullOrEmpty($env:DARKBURN_REPO) -and
        ((-not $ref) -or (Test-Version ($ref -replace '^v', '')))

    $work = Join-Path ([IO.Path]::GetTempPath()) ('darkburn-install-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $work | Out-Null
    try {
        $done = $null
        $builtHere = $false
        # Why no release was installed, for the screen. Empty when none was asked for.
        $why = ''
        if ($wantRelease) {
            Step 'Looking for a release on GitHub'
            $found = Get-Release $ref
            $release = $found.Release
            $why = $found.Why
            $version = $null
            if ($release -and $release.tag_name) { $version = ([string]$release.tag_name) -replace '^v', '' }
            if ($version -and (Test-Version $version)) {
                $msi = Get-ReleaseFile $release "darkburn-$version-windows-x64.msi" $work
                if ($msi) {
                    Install-Msi $msi $installDir $customDir
                    $done = "release v$version, MSI"
                    $registered = Get-ItemProperty -Path 'HKCU:\Software\DarkBurn' -ErrorAction SilentlyContinue
                    if ($registered -and $registered.InstallDir) { $installDir = ([string]$registered.InstallDir).TrimEnd('\') }
                } else {
                    $zip = Get-ReleaseFile $release "darkburn-$version-windows-x64-portable.zip" $work
                    if ($zip) {
                        if (Get-MsiProduct 'HKCU:') {
                            throw 'DarkBurn is installed from an MSI, and this release has only a zip. Uninstall first (DARKBURN_UNINSTALL=1), then run the installer again.'
                        }
                        Assert-NotRunning
                        Install-Portable $zip $work $installDir
                        $done = "release v$version, portable zip"
                    } else {
                        $why = "Release v$version has no MSI and no portable zip."
                    }
                }
            } elseif ($release) {
                $why = 'The reply from the GitHub release list could not be read.'
            }
        }

        if (-not $done) {
            if ($why) { Step "$why Building from source." } else { Step 'Building from source.' }
            if (Get-MsiProduct 'HKCU:') {
                throw 'DarkBurn is installed from an MSI. Uninstall it first (DARKBURN_UNINSTALL=1), then run the installer again to build from source.'
            }
            $sourceRef = $ref
            if (-not $sourceRef) { $sourceRef = 'main' }
            $bin = Build-FromSource $work $buildDir $sourceRef
            Assert-NotRunning
            Step "Installing into $installDir"
            Install-Files @((Join-Path $bin 'darkburn.exe'), (Join-Path $bin 'darkburn-mcp.exe')) $installDir
            $commit = Get-ToolOutput { git "--git-dir=$(Join-Path $buildDir 'DarkBurn\.git')" rev-parse --short HEAD }
            $done = "built from $sourceRef at $commit"
            $builtHere = $true
        }

        $installed = Get-ToolOutput { & (Join-Path $installDir 'darkburn-mcp.exe') --version }
        $installedVersion = ''
        if ($installed -and ([string]@($installed)[0]) -match '^darkburn-mcp\s+(\S+)') { $installedVersion = $Matches[1] + ' ' }
        Say ''
        Say "DarkBurn ${installedVersion}is installed ($done)."
        Say "  folder      $installDir"
        Say '  Start menu  DarkBurn'
        Say "  settings    $(Join-Path $env:APPDATA 'DarkBurn')"
        if ($builtHere) {
            Say ''
            Say "The source and the build stay in $buildDir"
            Say 'so the next update is quick. Delete that folder to get the disk space back.'
        }
        Say ''
        Say 'Pre-release: DarkBurn has not yet run a job on a real machine.'
        Say 'Update: run this installer again.'
        Say "Remove: `$env:DARKBURN_UNINSTALL = '1'; irm $InstallUrl | iex"
    } finally {
        Restore-Env
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Install-DarkBurn
