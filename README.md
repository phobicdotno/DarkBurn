# DarkBurn

A clean-room, open laser cutter and engraver program with full LightBurn feature parity,
built in Rust + egui.

- Targets: GRBL (USB serial), Ruida (Ethernet/USB), Gweike M3 Ultra, and more.
- Status: planning. See `docs/PARITY-CHECKLIST.md` for the feature, GUI and menu checklist
  that is ticked off as features land.

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
