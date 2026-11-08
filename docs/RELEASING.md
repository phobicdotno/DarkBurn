# Releasing DarkBurn

## Version rules

- The version lives in one place: `[workspace.package] version` in the root `Cargo.toml`.
  The installer, the zip, the executable's version resource and the update check all read it.
- Every line in `CHANGELOG.md` bumps the patch version by one. Three new lines: `0.0.24` becomes `0.0.27`.
- The build number goes up by one for each deploy (each tagged release), not for each commit.
  Headings in the changelog read `## v0.0.24 (build 5)`.
- Changelog lines are user-facing, short and casual, with no technical detail. They live their own
  life, apart from git commit messages. Builders write theirs into `docs/changelog-fragments/<area>.md`;
  the integrator folds them into `CHANGELOG.md` and sets the version.
- Update the changelog and the version **before** committing a release, never after.

## Cutting a release

1. Fold the fragments into `CHANGELOG.md` under a new heading, bump the workspace version, and
   tick the checklist (`docs/PARITY-CHECKLIST.md`) for what really works.
2. `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test --workspace`.
3. Build the packages locally if you want to look at them first:
   `scripts/build-installer.ps1` and `scripts/build-portable.ps1` (see below). Output goes to `dist/`.
4. Commit (`Release v0.0.24 (build 5)`), then tag with a `v` in front of the workspace version and push the tag:
   ```
   git tag v0.0.24
   git push origin main v0.0.24
   ```
5. `.github/workflows/release.yml` runs on the tag. It checks that the tag equals `v<workspace version>`,
   runs the tests, builds the MSI and the portable zip, uploads them as workflow artifacts and attaches
   them to a **draft** release. Nothing is published by itself.
6. Open the draft on GitHub, paste the changelog lines for the version into it, check the files, press Publish.

`.github/workflows/ci.yml` runs format, clippy and tests on Windows for every push to `main` and every pull
request. Both workflows need a repository secret `SOLIDCRAFT_TOKEN` (read access to the SolidCraft
repository, whose small UI crates are path dependencies); the optional repository variable
`SOLIDCRAFT_REF` picks the branch or tag to use (default `main`).

## What the scripts need

All of these install per user and need no admin rights.

| Tool | Install |
|---|---|
| Rust (MSVC) | https://rustup.rs |
| cargo-about (third-party licence list) | `cargo install cargo-about --locked --features cli` |
| .NET SDK 6 or newer (for the WiX tool) | `winget install Microsoft.DotNet.SDK.8`, or without admin: `dotnet-install.ps1 -Channel 8.0` from https://dot.net/v1/dotnet-install.ps1 |
| WiX Toolset 5 | `dotnet tool install --global wix --version 5.0.2` |
| WiX extensions | `wix extension add -g WixToolset.UI.wixext/5.0.2` and `wix extension add -g WixToolset.Util.wixext/5.0.2` |
| actionlint (optional, checks the workflow files) | `winget install rhysd.actionlint` |

- `scripts/build-installer.ps1` builds `darkburn.exe` and `darkburn-mcp.exe` in release (static C runtime, so
  no Visual C++ redistributable is needed), writes `THIRD-PARTY-LICENSES.txt` with cargo-about
  (`packaging/about.toml` lists the accepted licences; a dependency under any other licence stops the build),
  and makes `dist/darkburn-<version>-windows-x64.msi` with `packaging/windows/darkburn.wxs`.
  `-SkipBuild` reuses the binaries already in `target/x86_64-pc-windows-msvc/release`.
- `scripts/build-portable.ps1` makes `dist/darkburn-<version>-windows-x64-portable.zip`: the two programs, the
  licences and a `portable.txt` marker. With the marker beside `darkburn.exe`, DarkBurn keeps its settings in a
  `settings` folder next to it.

## About the MSI

- **UpgradeCode** `D7B4E1A2-5C3F-4E8A-9B61-2F0A8C4D7E15` is what makes a new MSI replace an older install.
  Never change it. The component GUIDs in `darkburn.wxs` stay put too.
- Installs for the current user by default (into `%LocalAppData%\Apps\DarkBurn`, no admin rights) and for all
  users on request (`ALLUSERS=1`, or "Anyone who uses this computer" in the wizard; into `Program Files\DarkBurn`).
- Silent installs: `msiexec /i darkburn-<version>-windows-x64.msi /qn` (per user),
  `msiexec /i ... ALLUSERS=1 /qn` (all users, from an elevated prompt), `APPLICATIONFOLDER=D:\Tools\DarkBurn`
  to choose the folder.
- Uninstall (Settings, or `msiexec /x`) removes the program, the shortcuts and the file types and keeps the settings
  folder `%AppData%\DarkBurn`. `msiexec /x darkburn-<version>-windows-x64.msi REMOVE_SETTINGS=1` removes that too.
- The MSI is not code-signed, so SmartScreen may warn on first run. Signing is a later step.

## Update check

`Help > Check for Updates` (command `help.check_updates`) asks the address in `Settings > Behaviour > Update
address`, by default the GitHub releases API of this repository, and compares its `tag_name` with the running
version. The setting `Check for updates` (off by default) does the same once when DarkBurn starts. It never
downloads or installs anything and sends only the user agent `DarkBurn`. While the repository is private
GitHub answers 404 and the dialog says there is nothing to compare with. Any server that returns
`{"version": "x.y.z", "url": "https://...", "notes": "..."}` works too.
