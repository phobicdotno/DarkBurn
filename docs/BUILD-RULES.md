# Build rules for everyone working on DarkBurn

Read this before writing code. These rules are not optional.

## Clean room
- Never open, run, decompile or copy anything from `C:\Program Files\LightBurn` or any LightBurn file, icon, text or asset.
- Work from `docs/PARITY-CHECKLIST.md` and `docs/research/*.md`, which describe what features do, not how LightBurn builds them.
- Code from MIT, Apache-2.0, BSD or zlib projects may be read and ported with credit in `ATTRIBUTION.md` (for example MeerK40t, Rayforge, SolidCraft).
- Never copy or port GPL, LGPL or AGPL code (for example potrace, LaserGRBL, LibLaserCut, OpenLaser, GRBL firmware). Read their docs only.
- Never write anything in the repo about why DarkBurn exists, licence costs, or LightBurn pricing.

## Hardware safety
- Never open a real serial port, USB device or network connection to a machine. That means no COM ports, no USB laser devices, no UDP or TCP to 10.1.1.x, 192.168.x.x lasers or any controller.
- Every driver talks through a transport trait. Tests and demos use an in-memory emulator or loopback.
- Real-hardware tests are done by the owner by hand. Leave the TARGET-* and MCC-06 items for that.
- The Gweike M3 Ultra is a 1200 W fibre cutter. No code may ever bypass its height follower, piercing or gas logic.

## Code
- Rust edition 2024, `cargo fmt` (repo rustfmt.toml), `cargo clippy --workspace` clean, tests for every behaviour.
- `unsafe` is forbidden in DarkBurn crates.
- A library crate lives in `crates/<name>/` and lists its dependencies in its own `Cargo.toml`.
  Only the integrator edits the root `Cargo.toml`, `CHANGELOG.md` and `docs/PARITY-CHECKLIST.md`.
- Geometry is in millimetres, Y up, origin at the machine origin, using `kurbo` types at crate boundaries.

## Commits
- No `Co-Authored-By` lines.
- Changelog lines are user-facing, short and casual, with no technical detail. Each new line bumps the patch version; each pushed batch bumps the build number.
- Name finished checklist IDs in the commit message: `Done: FILE-03, EDIT-01`.
- Tick a checklist item only when it works in the app, not when a library alone supports it.
