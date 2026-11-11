# darkburn-gcodebots attribution

> The root [`ATTRIBUTION.md`](../../ATTRIBUTION.md) is newer than this file. It lists where the two
> differ, and where they differ the root file holds.

This crate is DarkBurn's own code, licensed MIT OR Apache-2.0 like the rest of DarkBurn.

## Protocol documentation (read, not copied)

The drivers, the emulators and the translator were written from published descriptions of the
serial interfaces. Only interface facts were taken from them (command names, reply shapes,
which commands exist); all code and all message wording are DarkBurn's own.

| Source | Used for |
|---|---|
| Smoothieware documentation (smoothieware.org: "console commands", "laser", "grbl-mode", "configuring smoothie", "supported g-codes") | `ok` per line and no character counting, `?` status report, `M114` reply, `fire` / `fire off`, `suspend` / `resume`, `config-set`, `cat /sd/config.txt`, `M112` / `M999`, `version`, laser `S` range |
| Marlin documentation (marlinfw.org G-code index: M3-M5 laser/spindle, M106/M107, M114, M115, M211, M220, M400, M410, M500-M503, M112, G28, G92) | Power modes and inline mode, position and firmware replies, settings commands, quick stop, emergency stop behaviour |
| LinuxCNC documentation (G-code reference: M62-M68 digital and analog I/O, M2, G64) | `M67 E Q` analog power, `M64`/`M65` air output, program layout |
| Snapmaker public wiki and forum posts about the laser file header and the 2.0 serial interface | Header block fields, focus and material height |

Smoothieware, Marlin and LinuxCNC firmware are GPL. No firmware source and no source from GPL
senders (LaserGRBL, Universal G-code Sender, Pronterface, Luban) was opened, copied or ported.
Snapmaker Luban is AGPL; only the written public descriptions of the header block were used.

## Code

`darkburn-grbl` (DarkBurn, same licence) supplies the `Transport` trait, the GRBL driver that
`GrblAdapter` wraps, and the shared value types (`Job`, `JogStep`, `MachineState`, ...).

Dependencies: `kurbo` (MIT/Apache-2.0), `serde` and `serde_json` (MIT/Apache-2.0), `log`
(MIT/Apache-2.0).
