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

The workspace builds and the tests pass on Windows, Linux and macOS, on GitHub's build servers. Nobody has used the program on a Mac yet.

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

## The web page

`site/` is the page for https://darkburn.phobic.no. It says about DarkBurn what the [README](README.md) says, in fewer words: change the README first, then the page.

```sh
sh scripts/build-site.sh
python3 -m http.server -d target/site
```

The script puts the page together in `target/site` with copies of the pictures in `docs/img`, the fonts and the icon, and checks that every file the page points to is there, that nothing is fetched from another host and that every page carries its Content-Security-Policy line. That line allows only the page's own files, so script and style go in `site/site.js` and `site/style.css`, never inside the HTML. The script does not judge what those two files do: both can change what a visitor reads, the install commands too, so a change to either is read like a change to a program. The script also holds the two install commands the page shows and refuses the page when they differ, so a change to a command is made in two places. `site/` holds five files and the script publishes a fixed list of files: a new file, a new font among them, is named in `scripts/build-site.sh` first. A new picture in `docs/img` needs no such line.

The fifth file, `site/.htaccess`, holds the rules for the web host: the same policy as a header, that a browser asks again for every file, the "not found" page, https, and one name for the page. The host runs PHP, and one line in that file could make it run an uploaded file as a program, so the script holds the same rules and refuses the file when a line differs. A rule is changed in `scripts/build-site.sh` first, then in the file.

The page is hosted on the owner's own web host, the way the other pages on phobic.no are: a folder of plain files, uploaded over SFTP. Nothing publishes it by itself. After a change to the page is on `main`, the owner uploads it, with this script and nothing else (`site/` is not the folder that goes up, and a program that skips files whose names start with a dot would leave `.htaccess` out):

```sh
sh scripts/deploy-site.sh USER
```

`USER` is the SFTP user of the host; the password is typed each time and is stored nowhere in the repository or its settings. The script uploads from `main` with nothing uncommitted: it builds the folder, uploads it, names what the folder on the host holds besides the page and says what the page then answers. `.github/workflows/site.yml` builds and checks the page on pull requests and on `main`. Once a day it also compares the page on the host with what `main` builds, so it is red while a change to the page waits for its upload, and when a file was changed on the host.

The picture at the top of the README, `docs/readme-banner.png`, shows the top of the page: status, headline and the line under it. When one of those changes on the page, the picture is drawn again from `tools/readme-banner.html`; the two commands are at the top of that file.

## Changelog and version

Do not edit `CHANGELOG.md`, the version in the root `Cargo.toml` or the ticks in `docs/PARITY-CHECKLIST.md`. A change the user can see gets one line in a new file `docs/changelog-fragments/<short-name>.md`, written like the lines in [CHANGELOG.md](CHANGELOG.md): short, casual, no technical detail, starting `- Fix: ` or `- New: `. The maintainer folds the fragments into `CHANGELOG.md` and sets the version.

## Commits and pull requests

- One subject per pull request. Say what you ran and what you could not run.
- No `Co-Authored-By` lines in commit messages.
- If the change finishes checklist items, name them in the commit message: `Done: FILE-03, EDIT-01`.

## Licence

DarkBurn is MIT or Apache-2.0, at your option ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)). What you contribute goes in under the same two licences.
