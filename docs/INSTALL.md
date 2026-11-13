# Installing DarkBurn

The short version is in the [README](../README.md#install). This page has the rest.

**Status: pre-release.** No release has been published yet. So today there is nothing to download from the Releases page, and the one-line installers build DarkBurn from source.

Where this page describes a release file (the MSI, the portable zip, the archives for Linux and macOS), it describes what the packaging scripts and the release workflow are written to make. [Where each platform stands](#where-each-platform-stands) says what has been run and what has not.

Find your case:

- Windows, once a release is out: [the MSI](#windows-the-msi) or [the portable zip](#windows-the-portable-zip).
- Linux, macOS, or Windows without the MSI: [the one-line installers](#the-one-line-installers), their [options](#options-for-the-one-line-installers) and their [settings](#settings).
- [What gets installed, and where](#what-gets-installed-and-where), and [what Linux needs](#what-linux-needs) to build and to run.
- [Where each platform stands](#where-each-platform-stands).
- [First start](#first-start), [updating](#updating), [uninstalling](#uninstalling) and [where settings are kept](#where-settings-are-kept).
- [Read the script first](#read-the-script-first).
- [Building from source](#building-from-source) by hand.

## Windows: the MSI

The file is `darkburn-<version>-windows-x64.msi`. Once a release is out it is on the [Releases](https://github.com/phobicdotno/DarkBurn/releases) page: download it and open it.

- **For you alone, by default.** It installs into `%LocalAppData%\Apps\DarkBurn` and needs no admin rights.
- **For everyone on the computer:** choose "Install for all users of this machine" in the wizard, or give `msiexec` the setting `ALLUSERS=1` as shown below. That installs into `Program Files\DarkBurn` and asks for admin rights.
- **Shortcuts:** a DarkBurn entry in the Start menu, and a shortcut on the desktop. The desktop shortcut is optional.
- **File types:** `.dbrn` projects open with DarkBurn. So do `.dbdev` (device profile), `.dbclb` (cut library), `.dbart` (art library) and `.dbhk` (hotkeys) files.
- **What it holds:** `darkburn.exe`, `darkburn-mcp.exe` (the MCP server for [AI agents](../README.md#ai-agents)), the licences and the README. The C runtime is linked in, so no Visual C++ redistributable is needed.
- **Not on your PATH.** See [first start](#first-start).
- **Not code-signed,** so SmartScreen may warn on first run.
- **Updating:** open a newer MSI. It replaces the older install in place. An older MSI stops with "A newer version of DarkBurn is already installed."
- **Uninstalling:** in Windows Settings, under Apps, or with `msiexec /x`. That removes the program, the shortcuts and the file types, and keeps your settings in `%AppData%\DarkBurn`.

From a command prompt, with the version number in place of `<version>`:

Install without the wizard, for you alone.

```text
msiexec /i darkburn-<version>-windows-x64.msi /qn
```

Install without the wizard, for all users. This one needs an elevated prompt.

```text
msiexec /i darkburn-<version>-windows-x64.msi ALLUSERS=1 /qn
```

Install into a folder of your choice.

```text
msiexec /i darkburn-<version>-windows-x64.msi APPLICATIONFOLDER=D:\Tools\DarkBurn
```

Uninstall, and remove your settings as well.

```text
msiexec /x darkburn-<version>-windows-x64.msi REMOVE_SETTINGS=1
```

## Windows: the portable zip

The file is `darkburn-<version>-windows-x64-portable.zip`, from the same Releases page once a release is out.

- Unzip it anywhere (a USB stick works) and run `darkburn.exe`. Nothing is registered with Windows: no Start menu entry and no file types.
- The zip holds one folder, `darkburn-<version>-windows-x64-portable`, with `darkburn.exe`, `darkburn-mcp.exe`, the licences, the README and `portable.txt`.
- `portable.txt` is what makes it portable. While it sits next to `darkburn.exe`, DarkBurn keeps its settings, auto-save and its other files in a `settings` folder beside the program, not in `%AppData%\DarkBurn`. Delete `portable.txt` to use the normal folder again.
- **Updating:** unzip the new zip. It makes a folder of its own, because the folder name carries the version, so copy the `settings` folder across to keep your settings.
- **Uninstalling:** delete the folder.

## The one-line installers

Both install for you alone, with no admin rights and no `sudo`. This is what they do:

1. Look in the latest GitHub release for a ready-made file for your computer. No release exists yet, so today this step finds nothing.
2. Build from source. The script fetches DarkBurn and SolidCraft (its small UI crates are path dependencies), checks that the build tools are there, and runs `cargo build --release`. When a tool is missing, it names what is missing, says how to install it, and stops.
3. Install the programs `darkburn` and `darkburn-mcp` for the current user.

A build from source needs Rust 1.95 or newer and a C toolchain, about 3.5 GB of disk, and time: the first build took 13 minutes on a four-core computer.

If a line trips, please [say what happened](https://github.com/phobicdotno/DarkBurn/issues). [Building from source](#building-from-source) by hand is six commands.

### Linux and macOS

```sh
curl -fsSL https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.sh | sh
```

**macOS has only been tried on a build server.** DarkBurn builds there, its tests pass and both programs start. Nobody has used it on a Mac yet, and this installer has not been run on one. The script says so when it starts.

Once a release is out, the script downloads the archive for your computer, one of these four, and installs from it:

- `darkburn-<version>-linux-x86_64.tar.gz`
- `darkburn-<version>-linux-aarch64.tar.gz`
- `darkburn-<version>-macos-arm64.tar.gz`
- `darkburn-<version>-macos-x86_64.tar.gz`

Before it installs from an archive, the script starts both programs once. When that fails, or the release has no archive for your computer, it builds from source. The Linux archives are built on Ubuntu 22.04, so they need glibc 2.35 or newer.

### Windows

In PowerShell (Windows PowerShell 5.1 or PowerShell 7):

```powershell
irm https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.ps1 | iex
```

**This script has not been run on Windows yet.** PowerShell 7.7 on Linux has run it against stand-ins for the build tools, the Windows installer, the registry, the Start menu shortcut and GitHub. Windows PowerShell 5.1 has not run it. The build server only checks, with Windows PowerShell 5.1, that it parses.

It looks for a release first. With an MSI in the latest release it installs that, silently and for you alone. With only a portable zip it unpacks that, licence texts included, and leaves `portable.txt` out, so your settings live in `%AppData%\DarkBurn`. With no release, which is today's case, it builds from source.

It stops, and changes nothing, in these cases:

- DarkBurn or `darkburn-mcp` is running. Close it and run the line again. That holds for the uninstall too: its setting stays until DarkBurn has been removed.
- DarkBurn is installed for all users. Update that copy with a newer MSI from an administrator prompt, or remove it in Settings > Apps first.
- DarkBurn is installed from an MSI and the script would have to build from source, or the release has only a zip. Uninstall first, then run the line again.
- The PowerShell window is the 32-bit one on 64-bit Windows. Open the ordinary PowerShell from the Start menu and run the line there.

## Options for the one-line installers

Each block is one command: copy it whole. All the settings are in the [table below](#settings).

### Options on Linux and macOS

A setting goes in front of `sh`. An option goes after `sh -s --`.

Let the script install Rust with rustup when it is missing or too old. A Rust that is too old and came from a system package is left as it is: rustup goes in beside it, and the build uses rustup's.

```sh
curl -fsSL https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.sh \
  | DARKBURN_INSTALL_RUST=1 sh
```

Install a branch, tag or commit of your choice. Here it is the `main` branch.

```sh
curl -fsSL https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.sh \
  | sh -s -- --version main
```

Build from source even when a release exists.

```sh
curl -fsSL https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.sh \
  | DARKBURN_FROM_SOURCE=1 sh
```

Remove DarkBurn and what the installer built. Your settings are kept. More under [uninstalling](#uninstalling).

```sh
curl -fsSL https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.sh \
  | sh -s -- --uninstall
```

Print the options and install nothing.

```sh
curl -fsSL https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.sh \
  | sh -s -- --help
```

### Options on Windows

A setting is set on the line before, and stays for that PowerShell window.

Let the script install Rust with rustup when it is missing, and update it when it is too old and came from rustup.

```powershell
$env:DARKBURN_INSTALL_RUST = '1'
irm https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.ps1 | iex
```

Install a branch, tag or commit of your choice. Here it is the `main` branch.

```powershell
$env:DARKBURN_REF = 'main'
irm https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.ps1 | iex
```

Build from source even when a release exists.

```powershell
$env:DARKBURN_FROM_SOURCE = '1'
irm https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.ps1 | iex
```

Remove DarkBurn and what the installer built. Your settings are kept, and the script clears this setting once DarkBurn is removed. More under [uninstalling](#uninstalling).

```powershell
$env:DARKBURN_UNINSTALL = '1'
irm https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.ps1 | iex
```

## Settings

The settings are environment variables. On Linux and macOS they go in front of `sh`. On Windows they are set first and stay for that PowerShell window, except `DARKBURN_UNINSTALL`, which the script clears once DarkBurn is removed. To clear another one, open a new PowerShell window, or remove it by name, for example `Remove-Item Env:\DARKBURN_FROM_SOURCE`.

| Setting | What it does |
|---|---|
| `DARKBURN_REF` | The branch, tag or commit to install. A bare version number means its tag. A branch or a commit is always built from source. Left out, it is the latest release when that has a file for this computer, else `main`. |
| `DARKBURN_FROM_SOURCE=1` | Build from source even when a release exists. |
| `DARKBURN_INSTALL_RUST=1` | Install Rust with rustup when it is missing or too old. Left out, the script says what to install and stops. On Windows a Rust that is too old and did not come from rustup has to be updated by hand. |
| `DARKBURN_PREFIX` | Where the programs go: `<prefix>/bin` on Linux and macOS (`~/.local` when left out), the folder itself on Windows (`%LocalAppData%\Apps\DarkBurn`). On macOS the app always goes to `~/Applications`. |
| `DARKBURN_BUILD_DIR` | Where the source is kept and built (`~/.cache/darkburn-build`, `%LocalAppData%\darkburn-build`). |
| `DARKBURN_REPO` | The git address or local path of another copy of DarkBurn. With it set, the script always builds from source. |
| `SOLIDCRAFT_REPO` | The git address or local path of another copy of SolidCraft. |
| `SOLIDCRAFT_REF` | The SolidCraft branch, tag or commit to build against. Left out, it is the commit DarkBurn is pinned to. |
| `DARKBURN_UNINSTALL=1` | Windows only: remove DarkBurn. |

`install.sh` also takes `--help`, `--uninstall` and `--version <ref>`. `--ref` is the same as `--version`, and both win over `DARKBURN_REF`.

A release is to have a `.sha256` file beside each download. When it has one, the scripts check the download against it and stop on a mismatch.

## What gets installed, and where

### Linux

- **Installs** `darkburn` and `darkburn-mcp` into `~/.local/bin`, with a menu entry, an icon and the `.dbrn` project file type. It never calls `sudo` and installs nothing outside your home folder. A build from source carries line tables for crash reports, so the two programs take about 300 MB.
- **The files:** `~/.local/bin/darkburn` and `~/.local/bin/darkburn-mcp`, and under `~/.local/share` the menu entry `applications/no.phobic.darkburn.desktop`, the icon `icons/hicolor/256x256/apps/darkburn.png` and the file type `mime/packages/no.phobic.darkburn.xml`.
- **The build** stays in `~/.cache/darkburn-build` (about 3 GB), so the next update has less to do. Uninstalling removes it.
- **Settings** live in `~/.local/share/darkburn`. Uninstalling keeps them.
- **PATH:** when `~/.local/bin` is not on your PATH, the script says so and prints the line to add.
- **Other folders:** when `XDG_DATA_HOME` or `XDG_CACHE_HOME` is set, the script uses it in place of `~/.local/share` or `~/.cache`.

### macOS (not yet used on a Mac)

- **Installs** `DarkBurn.app` into `~/Applications`, with links to `darkburn` and `darkburn-mcp` in `~/.local/bin`. No `sudo`.
- **Building needs** the Xcode command line tools (`xcode-select --install`) and Rust 1.95 or newer.
- **The build** stays in `~/.cache/darkburn-build`. Uninstalling removes it.
- **Settings** live in `~/Library/Application Support/DarkBurn`. Uninstalling keeps them.

### Windows, with the one-line installer

- **Installs** `darkburn.exe` and `darkburn-mcp.exe` into `%LocalAppData%\Apps\DarkBurn`, with a DarkBurn entry in the Start menu. The folder is not put on your PATH.
- **Building needs** Git, Rust with the MSVC toolchain, and the Visual Studio Build Tools with the "Desktop development with C++" workload. When something is missing, the script lists it with its download address and `winget` line, and stops.
- **File types:** the MSI makes `.dbrn` projects open with DarkBurn. A build from source does not.
- **The build** stays in `%LocalAppData%\darkburn-build`. Uninstalling removes it.
- **Settings** live in `%AppData%\DarkBurn`. Uninstalling keeps them.

The MSI and the portable zip are described under [Windows: the MSI](#windows-the-msi) and [Windows: the portable zip](#windows-the-portable-zip).

## What Linux needs

- **Building needs** git, a C compiler, libclang (the camera crate generates its bindings while building) and Rust 1.95 or newer. When something is missing, the script prints the line that installs it (for apt, dnf, pacman, zypper or apk) and stops. It installs no system packages itself. On Debian and Ubuntu the line is `sudo apt install git curl build-essential libclang-dev`.
- **Running needs** a desktop session (X11 or Wayland), a Vulkan or OpenGL ES graphics driver, and libxkbcommon with its X11 part. An ordinary desktop install has all of these. A minimal system may not: on Debian and Ubuntu `sudo apt install libxkbcommon-x11-0 libvulkan1 mesa-vulkan-drivers` covers it (`libegl1` in place of the Vulkan pair for OpenGL ES).
- Without libxkbcommon-x11, DarkBurn stops at start in an X11 session with `Library libxkbcommon-x11.so could not be loaded`. The installer warns when it does not find that library. Under Wayland it is not needed.
- **The wireless pendant** (optional, off until you turn it on in Settings) needs a udev rule, installed once with root rights, before your user may open its USB receiver. The installer does not do this for you. Run `sh install-pendant-rule.sh` from the Linux download (or from `packaging/linux` in the source), or the command that Settings shows, then unplug the receiver and plug it in again. The rule gives the user at the desktop access through systemd-logind or elogind; on a system with neither, add `GROUP="plugdev"` (or another group you are in) to the rule. Windows and macOS need nothing for the pendant.

## Where each platform stands

| Platform | Today |
|---|---|
| **Windows** | The lints and tests pass on a Windows build server. The MSI and the portable zip were built there once, and both programs started. Nobody has opened that MSI by hand yet. |
| **Linux** | Builds from source, and the lints and tests pass on a Linux build server. The x86_64 and arm64 archives were built there once, and both programs started. |
| **macOS** | Builds, and the lints and tests pass on an Apple silicon build server. The Apple silicon and Intel archives were built there once, and both programs started, the Intel ones under Rosetta. Nobody has used DarkBurn on a Mac yet. |

The CI workflow runs the lints and tests on Windows, Linux and macOS, and has passed on all three for the same code. The release workflow builds an MSI and a portable zip for Windows and a tar.gz for Linux and macOS. It has built every package once, each system in a run of its own; it has not yet run for a version tag, and no package has been published.

Both install scripts are new.

- `install.sh` was run on Linux (Ubuntu 24.04, x86_64) from start to finish: the real build, a second run as an update, and the uninstall. It has had fixes since. Those were run with the compile step replaced by a stand-in, and the one that installs rustup beside an old Rust with a stand-in for rustup as well.
- Its macOS half has been run only against stand-ins for the macOS tools, and its ready-made-build path only against a stand-in release, because no release exists.
- `install.ps1` has not been run on Windows. Windows PowerShell 5.1 has not run it either; the build server only checks that it parses there. PowerShell 7.7 on Linux has run it against stand-ins: the build from source, an update, the release paths with their checksums, and the uninstall.

## First start

Start DarkBurn from the applications menu or the Start menu. On macOS it is in `~/Applications`. A fresh install is set to the built-in GRBL simulator, so you can connect and run a job with no laser attached.

On Linux and macOS you can also run `darkburn` (when `~/.local/bin` is on your PATH). On Windows the programs are in `%LocalAppData%\Apps\DarkBurn` (`Program Files\DarkBurn` after an install for all users), which is not on your PATH: where the README types `darkburn` or `darkburn-mcp` as a command, type the full path there. In PowerShell that is:

```powershell
& "$env:LOCALAPPDATA\Apps\DarkBurn\darkburn-mcp.exe" --print-config
```

`darkburn --safe-mode` starts without your saved settings.

## Updating

- **One-line installers:** run the same line again. On Linux and macOS a running DarkBurn keeps working until you restart it. On Windows close DarkBurn first.
- **MSI:** open a newer MSI. It replaces the older install in place.
- **Portable zip:** unzip the new one and copy the `settings` folder across.

DarkBurn does not update itself. Help > Check for Updates compares version numbers and never downloads or installs anything. So far it has worked against a test server only.

## Uninstalling

Uninstalling keeps your settings, with two exceptions: an MSI uninstall with `REMOVE_SETTINGS=1`, and a portable copy, whose settings are inside the folder you delete.

**Linux and macOS**, with the one-line installer.

```sh
curl -fsSL https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.sh \
  | sh -s -- --uninstall
```

This removes the two programs, the build folder, and the menu entry, icon and file type on Linux or `DarkBurn.app` on macOS. It removes only what the installer made: a build folder or a `DarkBurn.app` that it did not make is kept. Rust and the system packages are left as they are. The desktop's lookup tables under `~/.local/share/mime` and `~/.local/share/applications` stay; other programs share them. If you installed with `DARKBURN_PREFIX` or `DARKBURN_BUILD_DIR`, set the same ones here.

**Windows**, with the one-line installer.

```powershell
$env:DARKBURN_UNINSTALL = '1'
irm https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.ps1 | iex
```

This removes the files the installer put in place, its Start menu entry and the build folder. A DarkBurn installed from the MSI for you alone is removed as well. One installed for all users is not: remove that in Settings > Apps, which needs admin rights. Git, Rust and the Visual Studio Build Tools are left as they are.

**Windows, MSI:** in Windows Settings, under Apps, or with `msiexec /x` as shown under [Windows: the MSI](#windows-the-msi).

**Windows, portable zip:** delete the folder. Its `settings` folder goes with it.

### Where settings are kept

| System | Folder |
|---|---|
| Linux | `~/.local/share/darkburn` |
| macOS | `~/Library/Application Support/DarkBurn` |
| Windows | `%AppData%\DarkBurn\data` |
| Windows, portable zip | `settings`, beside `darkburn.exe` |

Delete the folder if you want the settings gone too. On Windows delete the one above it, `%AppData%\DarkBurn`.

- On Linux a start with `--safe-mode` leaves a folder of its own, `~/.local/share/no.phobic.darkburn`. Uninstalling keeps that too.
- While MCP control is switched on, the channel's port and token are in `control.json`: `%AppData%\DarkBurn\control.json` on Windows, `~/.config/DarkBurn/control.json` on Linux and macOS.

## Read the script first

Piping a download into a shell is a matter of trust. Both scripts are plain text in this repository: [scripts/install.sh](../scripts/install.sh) and [scripts/install.ps1](../scripts/install.ps1).

A download that is cut off runs nothing. `install.sh` is one group in braces, and a shell runs none of such a group before it has read all of it; this was tried at every cut-off point in dash, bash and busybox sh. `install.ps1` is one block in braces, and PowerShell reads all of a block before it runs any of it; every cut-off point was checked with PowerShell 7.7's parser.

To read before you run on Linux and macOS, download the script.

```sh
curl -fsSLO https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.sh
```

Read it, with `less install.sh` or any editor. `sh install.sh --help` prints the options and installs nothing. Then run it.

```sh
sh install.sh
```

On Windows, download the script.

```powershell
irm https://raw.githubusercontent.com/phobicdotno/DarkBurn/main/scripts/install.ps1 `
  -OutFile install.ps1
```

Read it, with `notepad install.ps1` or any editor. Then run it.

```powershell
Get-Content -Raw install.ps1 | iex
```

## Building from source

You need:

- Rust 1.95 or newer.
- A checkout of [SolidCraft](https://github.com/phobicdotno/solidcraft) next to this repository, as `../solidcraft`. Its small UI crates are path dependencies. It has to be at the commit DarkBurn is pinned to, which is on SolidCraft's `params-resize` branch. SolidCraft's `main` branch does not have these crates.
- On Linux: git, a C compiler and libclang (the camera crate generates its bindings while building). On Debian and Ubuntu that is `sudo apt install git build-essential libclang-dev`. To run the program you also need what is listed as "Running needs" under [What Linux needs](#what-linux-needs).
- On Windows: Git, Rust with the MSVC toolchain, and the Visual Studio Build Tools with the C++ workload.

```sh
git clone https://github.com/phobicdotno/DarkBurn
git clone https://github.com/phobicdotno/solidcraft
git -C solidcraft checkout ae7da14907e00a11ed04656385cc0c2db386dc55
cd DarkBurn
cargo build --release -p darkburn -p darkburn-mcp
target/release/darkburn
```

- `cargo test --workspace` runs the tests. The UI tests draw the real app offscreen and need a graphics adapter; a software Vulkan driver such as lavapipe will do. See [UI-TESTING.md](UI-TESTING.md).
- `darkburn --safe-mode` starts without your saved settings.
- The Windows packages are made by `scripts/build-installer.ps1` and `scripts/build-portable.ps1`, and the Linux and macOS archives by `scripts/package-unix.sh`, with output in `dist/`. The tools they need, the release steps and the version rules are in [RELEASING.md](RELEASING.md).
- Rules for anyone who writes code here: [BUILD-RULES.md](BUILD-RULES.md).
