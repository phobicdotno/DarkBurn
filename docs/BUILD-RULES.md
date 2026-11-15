# Build rules for everyone working on DarkBurn

Read this before writing code. These rules are not optional.

## Clean room
- Never open, run, decompile or copy anything from `C:\Program Files\LightBurn` or any LightBurn file, icon, text or asset.
- Work from `docs/PARITY-CHECKLIST.md` and `docs/research/*.md`, which describe what features do, not how LightBurn builds them.
- Code from MIT, Apache-2.0, BSD or zlib projects may be read and ported with credit in `ATTRIBUTION.md` (for example MeerK40t, Rayforge, SolidCraft).
- Never copy or port GPL, LGPL or AGPL code (for example potrace, LaserGRBL, LibLaserCut, OpenLaser, GRBL firmware). Read their docs only.
- Never write anything in the repo about why DarkBurn exists, licence costs, or LightBurn pricing.

## Hardware safety
- Tests, demos and CI never open a real serial port, USB device or network connection to a machine. The app connects to a real machine only when the user tells it to. In tests that means no COM ports, no USB laser devices, no UDP or TCP to 10.1.1.x, 192.168.x.x lasers or any controller.
  - Exception (owner approved 2026-11-12): the `darkburn-pendant` crate may open the XHC wireless pendant receiver (USB 10ce:eba0, HID) when the user turns the pendant on. It only reads keys and sets LEDs; all actions go through the normal gated app paths.
- Every driver talks through a transport trait. Tests and demos use an in-memory emulator or loopback.
- Real-hardware tests are done by the owner by hand. Leave the TARGET-* and MCC-06 items for that.
- Gweike control (owner decisions Q046 and Q063): DarkBurn controls Gweike machines natively and may send what Mlaser sends, including head calibration, follower move / follow / stop, pierce and follower bookkeeping, gas writes and extended outputs. The safety check is **licence + limits only**:
  - Licence: never write registers 150 or 151, the licence / clock area 59500..59599, hardware parameters, follower properties, register 100 or block 5000..5012.
  - Limits: refuse impossible or unsafe numbers (speed above the ceiling, a jog too long, jerk not matching acceleration, stop deceleration out of range, go-to without the absolute bit, targets outside the soft limits, empty vectors).
  - A vector that is not decoded stays refused only with a stated reason, kept next to the classifier in `crates/darkburn-mcc/src/command.rs`.
- Any Gweike remote-monitor or cloud connection is off by default and needs the user to turn it on.

## Code
- Rust edition 2024, `cargo fmt` (repo rustfmt.toml), `cargo clippy --workspace` clean, tests for every behaviour.
- `unsafe` is forbidden in DarkBurn crates.
- A library crate lives in `crates/<name>/` and lists its dependencies in its own `Cargo.toml`.
  Only the integrator edits the root `Cargo.toml`, `CHANGELOG.md` and `docs/PARITY-CHECKLIST.md`.
- Geometry is in millimetres, Y up, origin at the machine origin, using `kurbo` types at crate boundaries.

## Commits
- No `Co-Authored-By` lines and no attribution to tools or assistants in commits or files.
- Changelog lines are user-facing, short and casual, with no technical detail. Each new line bumps the patch version; each pushed batch bumps the build number.
- Name finished checklist IDs in the commit message: `Done: FILE-03, EDIT-01`.
- Tick a checklist item only when it works in the app, not when a library alone supports it.
