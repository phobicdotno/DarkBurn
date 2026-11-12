# darkburn-grbl attribution

> The root [`ATTRIBUTION.md`](../../ATTRIBUTION.md) is newer than this file. It lists where the two
> differ, and where they differ the root file holds.

This crate is DarkBurn's own code, licensed MIT OR Apache-2.0 like the rest of DarkBurn.

## Protocol documentation (read, not copied)

The driver and the emulator were written from the published descriptions of the GRBL serial
interface. Only interface facts were taken from them (command bytes, response and status-report
grammar, setting numbers, error and alarm numbers); all code and all message wording are
DarkBurn's own.

| Source | Used for |
|---|---|
| GRBL v1.1 wiki (gnea/grbl): "Interface", "Commands", "Configuration", "Jogging", "Laser Mode" | Real-time bytes, `ok`/`error:N`, status report fields, `$` commands, `$$` setting numbers, error and alarm numbers, `$J=` rules, laser mode behaviour |
| GRBL v1.1 wiki description of character-counting streaming | Re-implemented from the description (sum of unacknowledged line lengths within the RX buffer); `stream.py` itself was not used |
| grblHAL wiki ("For sender developers", "Report extensions", "Changes from grbl 1.1") | Banner, `[FIRMWARE:grblHAL]`, larger RX buffer, extra error and alarm numbers |
| FluidNC wiki ("Grbl compatibility") | Banner and `[VER:...]` format |

GRBL, grblHAL and FluidNC are GPL-3.0. No firmware source, and no source from GPL senders
(LaserGRBL, rCandle), was opened, copied or ported. `raydriver` and gcodekit5 (MIT/Apache) were
not used either.

## Dependencies

| Crate | Licence | Notes |
|---|---|---|
| `kurbo` | MIT OR Apache-2.0 | `Rect` / `Point` at the crate boundary |
| `log` | MIT OR Apache-2.0 | |
| `serialport` (optional, feature `serial`) | MPL-2.0 | Used unmodified, only for real USB serial ports; off by default and in tests |
