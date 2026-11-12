# Contributing to DarkBurn

DarkBurn is pre-release. It has never burned a job on a real machine: the machine drivers have run on the built-in simulators and loopback links only.

## The most useful help right now

Testing on real hardware. [docs/HARDWARE-CHECKS.md](docs/HARDWARE-CHECKS.md) lists what still needs a check on a real controller. If you have one of those machines, try an item, stay at the machine with its own stop button within reach, and open an [issue](https://github.com/phobicdotno/DarkBurn/issues) that says what you did and what the machine did, with the controller and its firmware version.

Bug reports are welcome too. Say what you did, what you expected and what happened, and attach the file if one is involved. Security and safety problems go through [SECURITY.md](SECURITY.md), not a public issue.

## Build and test

You need Rust 1.95 or newer and a checkout of [SolidCraft](https://github.com/phobicdotno/solidcraft) beside this repository, as `../solidcraft`, at the commit DarkBurn is pinned to. The pin is the `SOLIDCRAFT_PIN` line in `scripts/install.sh`. What else each system needs is in [docs/INSTALL.md](docs/INSTALL.md#building-from-source).

```sh
git clone https://github.com/phobicdotno/DarkBurn
git clone https://github.com/phobicdotno/solidcraft
git -C solidcraft checkout ae7da14907e00a11ed04656385cc0c2db386dc55
cd DarkBurn
cargo build --release -p darkburn -p darkburn-mcp
target/release/darkburn
```

Before you open a pull request, run the three checks the CI workflow runs:

```sh
cargo fmt -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The UI tests draw the real app offscreen and need a graphics adapter. A software Vulkan driver such as lavapipe will do.

The workspace builds and the tests run on Linux. macOS is untested, and the CI workflow has not had a green run yet.

## Rules for every contribution

The full text is in [docs/BUILD-RULES.md](docs/BUILD-RULES.md). Read it before you write code. In short:

- **No LightBurn material.** No LightBurn files, text, icons or assets, and do not open LightBurn to write code. Work from [docs/PARITY-CHECKLIST.md](docs/PARITY-CHECKLIST.md) and `docs/research/`, which describe what a feature does.
- **No GPL, LGPL or AGPL code.** Do not copy or port it. Their documents may be read for facts.
- **Credit what you port.** Code from MIT, Apache-2.0, BSD or zlib projects may be ported, with credit in [ATTRIBUTION.md](ATTRIBUTION.md).
- **Never connect to a real machine** in code, tests or demos. No serial port, USB device or network address of a controller. Every driver talks through a transport trait, and tests use the simulators or loopback.
- **Every behaviour gets a test.** A bug fix comes with a test that fails without the fix.
- **Formatted and lint-free.** Rust edition 2024, `cargo fmt` with the repository's `rustfmt.toml`, clippy clean. `unsafe` is forbidden in the crates; the app allows it in one Windows file.
- **Geometry** is in millimetres, Y up, origin at the machine origin.
- **Every action is a command.** What the user can do in the app is a registered command that the UI calls. See [docs/WIRING-RULES.md](docs/WIRING-RULES.md).

## UI work

Follow [docs/UI-TESTING.md](docs/UI-TESTING.md). Windows, menus, the ribbon and dialogs are tested by driving the real app in a headless harness, and a UI change comes with such a test.

```sh
cargo test -p darkburn ui_smoke_tests::
```

Pictures of what each test saw land in `target/ui-snapshots/`. Look at them.

## Changelog and version

Do not edit `CHANGELOG.md`, the version in the root `Cargo.toml` or the ticks in `docs/PARITY-CHECKLIST.md`. A change the user can see gets one line in a new file `docs/changelog-fragments/<short-name>.md`, written like the lines in [CHANGELOG.md](CHANGELOG.md): short, casual, no technical detail, starting `- Fix: ` or `- New: `. The maintainer folds the fragments into `CHANGELOG.md` and sets the version.

## Commits and pull requests

- One subject per pull request. Say what you ran and what you could not run.
- No `Co-Authored-By` lines in commit messages.
- If the change finishes checklist items, name them in the commit message: `Done: FILE-03, EDIT-01`.

## Licence

DarkBurn is MIT or Apache-2.0, at your option ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)). What you contribute goes in under the same two licences.
