# DarkBurn

A clean-room, open laser cutter and engraver program with full LightBurn feature parity,
built in Rust + egui with the SolidCraft look and feel.

- Targets: GRBL (USB serial), Ruida (Ethernet/USB), Gweike M3 Ultra, and more.
- Status: planning. See `docs/PARITY-CHECKLIST.md` for the feature, GUI and menu checklist
  that is ticked off as features land.

## Clean-room rule

Nothing in this repository is derived from LightBurn's code or assets. The parity checklist
describes *what* LightBurn does; every implementation here is written fresh, and controller
protocols come from public documentation and permissively-licensed open-source implementations.
