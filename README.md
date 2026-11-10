# DarkBurn

**Free, open laser software that does what LightBurn 1.4.04 does, and then drives the machines it cannot.**

DarkBurn is one program for designing, arranging and burning: diode lasers, CO2 lasers, galvo markers and a
1200 W fibre cutter, all from the same window. No licence key, no trial, no device tiers, no usage reporting.
Written from scratch in Rust.

> **Status: pre-release.** 728 of the 802 items on the [parity checklist](docs/PARITY-CHECKLIST.md) are
> done and 48 more are partly done. Everything is proven against built-in machine simulators by about 2000
> automated tests. **It has not yet burned a job on a real machine**; those checks are listed in
> [docs/HARDWARE-CHECKS.md](docs/HARDWARE-CHECKS.md) and are done by hand.

## Why DarkBurn

- **Everything you know is here.** Every menu, toolbar, panel and dialog on the checklist: drawing and node
  editing, boolean and offset tools, arrays, layers with every cut setting, image engraving with all dither modes,
  preview with time estimate, cut planner, camera, rotary, print and cut, variable text, material and art libraries.
- **Your LightBurn projects open.** `.lbrn2` and `.lbrn` files load, and you can save `.lbrn2` back.
- **Nothing to unlock.** Every device type is included for everyone. MIT or Apache-2.0, your choice.
- **Safety is built in, not bolted on.** One test job is run through the simulated controllers and must come out laser-off on travel moves, stop
  always works, and every machine must burn the same shape in the same place and end with the laser off.
- **An AI agent can run it.** Every button is also a command. Claude can draw, set up layers, preview and measure
  for you, and it cannot touch a machine unless you switch that on. Starting a job always asks you first.
- **Fast and small.** One native program. A 100 mm offset-filled circle plans in under a second.

## Machines

| Family | What works today (on the simulators) |
|---|---|
| **GRBL** | GRBL 1.1, older M3-only GRBL, GRBL-LPC, grblHAL, FluidNC. Jogging, overrides while running, settings editor with backup, alarms in plain words, auto-detect for xTool, Ortur, Atomstack and Sculpfun |
| **Other G-code** | Smoothieware, Marlin, LinuxCNC, custom G-code devices; Snapmaker partly |
| **Ruida** | Ethernet and USB: start, pause, stop, frame, jog, file list on the controller, machine settings with backup, dual tube, rotary, RD files in and out |
| **Galvo** | EZCad2-style boards: fibre, MOPA, UV and CO2 sources, lens correction, red-light framing, wobble, timing settings, EZCad config import |
| **Gweike M3 Ultra** | A fibre plus CO2 flatbed LightBurn cannot drive. Today: DXF and the vendor's own job file, with assist gas, pierce stages and follower height. The direct Ethernet link waits for tests on the real controller card |

Connections: USB serial, TCP, UDP, Bluetooth serial; raw USB, HTTP and network bridges are partly done.
Not supported: Trocen and TopWisdom controllers (their protocols are not public).

## Files

- **Open:** SVG, Adobe Illustrator, PDF, DXF, PLT/HPGL, Ruida RD, PNG, JPG, BMP, GIF, TIFF, WebP, LightBurn projects.
- **Save:** SVG, AI, DXF, G-code, RD, bitmaps, DarkBurn `.dbrn` projects, `.lbrn2`.
- SHX single-line fonts, CSV for variable text, and files for libraries, devices, preferences and hotkeys.

## AI agents

`darkburn-mcp.exe` ships next to the app and connects Claude Code or Claude Desktop to the running window.
It is off until you turn it on, listens on this computer only, and logs every call where you can read it.
Out of the box an agent can design, inspect, measure, take screenshots and preview jobs. Machine control is a
separate switch, stop is always allowed, and firing or starting a job needs your click every time.
See [crates/darkburn-mcp/README.md](crates/darkburn-mcp/README.md).

## What is not there yet

- Real-hardware sign-off on any machine.
- Windows only for now; macOS and Linux builds are open items.
- English and Norwegian; the other 22 languages are open.
- The full list, item by item: [docs/PARITY-CHECKLIST.md](docs/PARITY-CHECKLIST.md). What changed: [CHANGELOG.md](CHANGELOG.md).

## Building

Needs Rust 1.95 or newer and a checkout of SolidCraft next to this repository
(`../solidcraft`), whose small UI crates are used as path dependencies.

```
cargo build --release
target/release/darkburn
```

`cargo test --workspace` runs the tests. Start with `--safe-mode` to ignore saved settings.

Installer and portable zip (Windows, output in `dist/`; tools needed are listed in `docs/RELEASING.md`):

```
scripts/build-installer.ps1    # dist/darkburn-<version>-windows-x64.msi
scripts/build-portable.ps1     # dist/darkburn-<version>-windows-x64-portable.zip
```

## Install

- **MSI**: double-click `darkburn-<version>-windows-x64.msi`. It installs for you alone, without admin
  rights; choose "Anyone who uses this computer" in the wizard (or `msiexec /i darkburn-<version>-windows-x64.msi ALLUSERS=1`)
  to install for everyone. It adds Start menu and desktop shortcuts and opens `.dbrn` projects (and `.dbdev`,
  `.dbclb`, `.dbart`, `.dbhk` files) with DarkBurn. Installing a newer MSI updates in place. Uninstalling keeps your
  settings; add `REMOVE_SETTINGS=1` to `msiexec /x` to remove them as well.
- **Portable zip**: unzip anywhere and run `darkburn.exe`. The `portable.txt` file next to it keeps the settings in
  a `settings` folder beside the program.
- `darkburn-mcp.exe` (in both) is the MCP server for AI agents; see `crates/darkburn-mcp/README.md`.
- Updates: `Help > Check for Updates`. Nothing is downloaded or installed automatically.

Release steps and version rules: `docs/RELEASING.md`.

## Clean-room rule

Nothing in this repository is derived from LightBurn's code or assets. The parity checklist
describes *what* LightBurn does; every implementation here is written fresh, and controller
protocols come from public documentation and permissively-licensed open-source implementations.
DarkBurn is not affiliated with LightBurn Software. See [docs/BUILD-RULES.md](docs/BUILD-RULES.md).
