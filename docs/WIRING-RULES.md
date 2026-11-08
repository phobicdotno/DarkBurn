# Rules for builders who wire libraries into the app

Read `docs/BUILD-RULES.md` first. These rules add to it.

## Where you work
- You work in your own git worktree on your own branch. Never push to `main`. The integrator merges branches one at a time.
- Put new code in new files under `apps/darkburn/src/` (for example `shape_ops.rs`, `preview.rs`). Shared files (`shell.rs`, `commands.rs`, `panels.rs`, `ribbon.rs`, `canvas.rs`) get only small, additive edits: a `mod` line, one registration call, one menu or panel entry. This keeps merges clean when six builders work at once.
- Every user-visible action is a registered command (see `apps/darkburn/src/commands.rs`: stable dotted id, title, group, JSON-schema args, `hardware` flag, checklist ids, `implemented`). The UI calls the command; it does not hold its own copy of the logic. The command handlers must work without a window (`api::execute_headless`) wherever the action is a document edit. Never mark a command `hardware: false` if it moves an axis, fires the laser, sends a job, writes controller settings or connects to a device.
- Add tests for every behaviour, including headless command tests.

## Checklist and changelog
- Tick `[x]` in `docs/PARITY-CHECKLIST.md` only for items that work in the running app. Use `[~]` for partial. Do not edit the `Progress` line; the integrator recomputes it.
- Do not edit `CHANGELOG.md` or the workspace version. Write user-facing lines (short, casual, no technical detail) into `docs/changelog-fragments/<your-area>.md`, one line per feature.
- Commit often with `Done: ID, ID` in the message. No Co-Authored-By lines.

## Before you report
- `cargo fmt`, `cargo clippy --workspace --all-targets` clean, `cargo test --workspace` green.
- Build release, launch the app, exercise your features as far as you can (synthetic input through PowerShell is fine), save a screenshot to `docs/research/img/<area>.png`, look at it, close the app.
- Final report: IDs ticked, IDs partial with reasons, what bypasses the command registry, commits, and anything you could not verify.

## Never
- No connection to any real machine. Devices in the app are the in-memory emulators only; real ports stay unopened unless the human clicks Connect in the app, and no builder clicks it.
- No code from LightBurn or GPL projects; no phrases about why DarkBurn exists; never write "look and feel".
