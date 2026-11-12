# Releasing DarkBurn

## Version rules

- The version lives in one place: `[workspace.package] version` in the root `Cargo.toml`.
  The installer, the zip, the Linux and macOS archives, the executable's version resource and the update
  check all read it.
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
2. `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
   These are the checks `ci.yml` runs.
3. Build the packages locally if you want to look at them first: `scripts/build-installer.ps1` and
   `scripts/build-portable.ps1` on Windows, `scripts/package-unix.sh` on Linux and macOS (see below).
   Output goes to `dist/`.
4. Commit (`Release v0.0.24 (build 5)`), then tag with a `v` in front of the workspace version and push the tag:
   ```
   git tag v0.0.24
   git push origin main v0.0.24
   ```
5. `.github/workflows/release.yml` runs on the tag. First it checks that the tag equals `v<workspace version>`;
   a tag that does not match stops there, before anything is built. Then it runs the tests and builds the
   packages for Windows, Linux and macOS, each with a `.sha256` file beside it. When the tests and every build
   have passed it attaches the files to a **draft** release. Nothing is published by itself. A failed test or
   build means no draft: run the failed jobs again from the run's page, or fix the cause and tag again.
   The run keeps the packages of its other jobs for 5 days; after that, run all of its jobs again.
6. Open the draft on GitHub, paste the changelog lines for the version into it, check the files, press Publish.
   Leave "Set as a pre-release" unticked and "Set as the latest release" ticked. The installers ask GitHub for
   the latest release, and that is never a draft or a pre-release. Until Publish is pressed, and after a
   pre-release, `scripts/install.sh` and `scripts/install.ps1` keep building from source. After a full release
   they install these files.

## What runs on GitHub

Neither workflow needs a secret. SolidCraft, whose small UI crates are path dependencies, is public. Every job
fetches it next to the checkout at the commit named in the `SOLIDCRAFT_PIN` line of `scripts/install.sh`.
That line is the one place the commit is written for the workflows; `scripts/install.ps1` carries the same
commit for the Windows installer, and a job stops when the two differ.

- `.github/workflows/ci.yml` runs on every push to `main` and on every pull request: `cargo fmt -- --check`
  once, then `cargo clippy --workspace --all-targets --locked -- -D warnings` and
  `cargo test --workspace --locked` on Windows, Linux and macOS. On Linux the UI tests draw with a GPU in
  software (`mesa-vulkan-drivers`); on macOS with the runner's own. The tests are built in one step and run
  in the next, which stops after 30 minutes, so a test that hangs does not hold a runner for long.
- `.github/workflows/release.yml` runs on every push to `main` and on every version tag (`v0.0.24`). Its first
  job, Plan, reads the version and says on the run's page what the run will do. A push to `main` builds the
  six packages and keeps them with the run as artifacts for 7 days. Each one can be downloaded from the run's
  page and used as it is. Only a tag goes further, as described above: tests, then the draft release. The
  packages of a tag are built without the build cache, and the run keeps them for 5 days; they are on the
  draft by then. A release that is already published is never changed.
- A push to `main` that changes only `README.md` or files under `docs/` starts neither workflow: nothing that
  is built or tested reads them. The one exception is `docs/PARITY-CHECKLIST.md`, which is built into
  `darkburn-mcp`. Pull requests and tags always run. A release push (`main` and its tag together) builds the
  packages once, in the run for the tag.
- While the repository is private, the workflows do less by themselves, because a private repository has a
  small monthly allowance of runner time and of room for artifacts:
  a push to `main` builds no packages, a push or a pull request runs no macOS tests, and artifacts are kept
  for 1 day. A tag and a run by hand do everything. The workflows read from GitHub whether the repository is
  private, so nothing has to be changed on the day it becomes public.
- Both can be started by hand: Actions, the workflow, Run workflow. The choice `only` runs one system alone
  (`windows`, `linux` or `macos`). From a terminal, for a branch: `gh workflow run ci.yml --ref <branch> -f only=linux`.
  Runs by hand for different systems do not stop each other. A run by hand of `release.yml` publishes nothing,
  and it drafts a release only when it is started on a tag for all systems.
- `.github/actions/prepare` holds the steps both share: the SolidCraft checkout, the Rust toolchain, the build
  cache and cargo-about. The Rust version is fixed there (input `rust`), so a new Rust release with new lints
  cannot turn a green build red by itself. Raise it there when you move to a newer Rust. The cargo-about
  version is fixed there too (input `cargo-about-version`).
- The Linux packages are built inside Ubuntu 22.04, so they start on systems with glibc 2.35 or newer.
  Both macOS packages are built on an Apple silicon runner; the Intel one is cross-compiled and started
  with Rosetta. A package whose programs could not be started on the runner is not kept.
- The macOS jobs are new. Until they have been seen green, expect to fix things after their first runs.
  Prove a system by hand before the first tag, the tests first and then the packages:
  `gh workflow run ci.yml --ref <branch> -f only=macos`, then `gh workflow run release.yml --ref <branch> -f only=macos`.
- GitHub's own actions are used by major version (`actions/checkout@v7`). The one action from elsewhere, the
  build cache, is pinned to a commit.

Package names, as the installers look for them in a release:

| System | File |
|---|---|
| Windows | `darkburn-<version>-windows-x64.msi`, `darkburn-<version>-windows-x64-portable.zip` |
| Linux | `darkburn-<version>-linux-x86_64.tar.gz`, `darkburn-<version>-linux-aarch64.tar.gz` |
| macOS | `darkburn-<version>-macos-arm64.tar.gz`, `darkburn-<version>-macos-x86_64.tar.gz` |

Each has a `<file>.sha256` beside it: the SHA-256 in hex, two spaces, the file name. `sha256sum -c` reads it.

## What the scripts need

On Windows all of these install per user and need no admin rights.

| Tool | Install |
|---|---|
| Rust (MSVC) | https://rustup.rs |
| cargo-about (third-party licence list) | `cargo install cargo-about --locked --features cli` (the workflows use version 0.9.3: add `--version 0.9.3` for the same list) |
| .NET SDK 6 or newer (for the WiX tool) | `winget install Microsoft.DotNet.SDK.8`, or without admin: `dotnet-install.ps1 -Channel 8.0` from https://dot.net/v1/dotnet-install.ps1 |
| WiX Toolset 5 | `dotnet tool install --global wix --version 5.0.2` |
| WiX extensions | `wix extension add -g WixToolset.UI.wixext/5.0.2` and `wix extension add -g WixToolset.Util.wixext/5.0.2` |
| actionlint (optional, checks the workflow files; with shellcheck installed it checks their scripts too) | `winget install rhysd.actionlint` |

On Linux and macOS: Rust from https://rustup.rs, a C compiler (on Linux also libclang:
`sudo apt install build-essential libclang-dev`; on macOS the Xcode command line tools), and cargo-about as above.
Every build needs a SolidCraft checkout next to this one (`../solidcraft`), at the commit in `scripts/install.sh`.

- `scripts/build-installer.ps1` builds `darkburn.exe` and `darkburn-mcp.exe` in release (static C runtime, so
  no Visual C++ redistributable is needed), writes `THIRD-PARTY-LICENSES.txt` with cargo-about
  (`packaging/about.toml` lists the accepted licences; a dependency under any other licence stops the build),
  and makes `dist/darkburn-<version>-windows-x64.msi` with `packaging/windows/darkburn.wxs`.
  `-SkipBuild` reuses the binaries already in `target/x86_64-pc-windows-msvc/release`.
- `scripts/build-portable.ps1` makes `dist/darkburn-<version>-windows-x64-portable.zip`: the two programs, the
  licences and a `portable.txt` marker. With the marker beside `darkburn.exe`, DarkBurn keeps its settings in a
  `settings` folder next to it.
- Both write `<package>.sha256` beside the package.
- `scripts/package-unix.sh` makes `dist/darkburn-<version>-<linux|macos>-<arch>.tar.gz` and its `.sha256`.
  It builds `darkburn` and `darkburn-mcp` in release for this computer, or for `--target <triple>`
  (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-apple-darwin`).
  `--skip-build` packages what is already built, `--bin-dir <folder>` the two programs in a folder.
  The archive holds one folder with the programs, the icon, a short `README.txt`, the licences and
  `THIRD-PARTY-LICENSES.txt` (the same accepted licences, for the package's own target). Without cargo-about
  the list is left out with a message; `--require-licences`, which the workflow uses, stops instead.
  The debug line tables are taken out of the packed programs. Before it finishes, the script unpacks the
  archive, as the installer does, and starts both programs from it (`darkburn-mcp --version`,
  `darkburn --mcp-headless`) when the computer can run them. When it cannot, they are packed unstarted with
  a message; `--require-start`, which the workflow uses, stops instead. The same input gives the same archive:
  the packed files carry the time of the last commit, read from git. When git cannot give it, the script
  says so and uses the time of now.

## About the MSI

- **UpgradeCode** `D7B4E1A2-5C3F-4E8A-9B61-2F0A8C4D7E15` is what makes a new MSI replace an older install.
  Never change it. The component GUIDs in `darkburn.wxs` stay put too.
- Installs for the current user by default (into `%LocalAppData%\Apps\DarkBurn`, no admin rights) and for all
  users on request (`ALLUSERS=1`, or "Install for all users of this machine" in the wizard; into `Program Files\DarkBurn`).
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
