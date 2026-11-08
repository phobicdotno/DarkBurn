# DarkBurn

A clean-room, open laser cutter and engraver program with full LightBurn feature parity,
built in Rust + egui with the SolidCraft look and feel.

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

## Clean-room rule

Nothing in this repository is derived from LightBurn's code or assets. The parity checklist
describes *what* LightBurn does; every implementation here is written fresh, and controller
protocols come from public documentation and permissively-licensed open-source implementations.
