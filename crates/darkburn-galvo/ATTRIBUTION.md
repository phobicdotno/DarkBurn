# darkburn-galvo attribution

This crate is DarkBurn's own code, licensed MIT OR Apache-2.0 like the rest of DarkBurn.

## Ported with credit (MIT)

| Source | Licence | What was taken |
|---|---|---|
| MeerK40t `meerk40t/balormk` (`controller.py`, `usb_connection.py`, `mock_connection.py`, `device.py`), https://github.com/meerk40t/meerk40t | MIT | Command and list opcode numbers and names; the 12-byte (six little-endian `u16`) command layout and 3072-byte list packets padded with `0x8002`; the status bits (BUSY 0x04, READY 0x20, AXIS 0x40, word 3 of the reply); the USB parameters (VID 0x9588, PID 0x9899, bulk OUT 0x02, bulk IN 0x88); the board bring-up order and default words (control / laser / delay / timing mode, standby, first pulse killer, PWM, FPK2, fly resolution); fibre `QSwitchPeriod = 20000 / kHz` and `MarkCurrent = power * 0xFFF / 100`, CO2 / UV `MarkFreq = 10000 / kHz` and `MarkPowerRatio = 200 * power / kHz`; speed in galvo units per millisecond; signed delay encoding (magnitude plus `0x8000` sign word); port bits (red light 8, foot switch 15 active low, mark 0); the job sequence (reset list, mark port, MO open, packets with `SetEndOfList` after each, `ExecuteList` after three, wait idle, MO close); the `.cor` readers for `LMC1COR_1.0` (f64) and the older i32 layout, including the `0x8000 + abs` negative encoding and the scale position in the header; the lens table upload (`WriteCorTable` then 4225 `WriteCorLine`, no reply); default timing values |

The code is a Rust re-implementation written for DarkBurn, not a transliteration.

## Read only, nothing taken

| Source | Why |
|---|---|
| Balor (Bryce Schroeder, GitLab `bryce15/balor`) | Its licence is ambiguous (SSPL v1 listed, GPL-3 text in the repo). Not read for code and not ported. |
| galvoplotter (MeerK40t project, MIT), glase (BSD-3) | Not used; mentioned in research notes only. |

## Written by DarkBurn without a public specification

Field calibration maths (scale, skew, bulge, trapezoid, negate, is-X), wobble, hatch and flood
fill, auto-rotate, rotary windows, repeat marking with external axis, 3D sliced image passes,
door-protect stages, red-light framing, the EZCad config key matching and the safety rules of the
driver are DarkBurn's own designs. Where they may differ from other software see
`docs/hardware-checks-galvo.md`.

No GPL, LGPL or AGPL code was read, copied or ported.

## Dependencies

| Crate | Licence | Notes |
|---|---|---|
| `kurbo` | MIT OR Apache-2.0 | `BezPath`, `Point`, `Rect` at the crate boundary |
| `nusb` (optional, feature `usb`) | MIT OR Apache-2.0 | Real USB bulk link; off by default and never used by tests |
| `futures-lite` (optional, feature `usb`) | MIT OR Apache-2.0 | Blocks on `nusb` transfers |
