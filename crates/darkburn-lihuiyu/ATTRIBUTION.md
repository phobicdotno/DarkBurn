# darkburn-lihuiyu attribution

> The root [`ATTRIBUTION.md`](../../ATTRIBUTION.md) is newer than this file. It lists where the two
> differ, and where they differ the root file holds.

This crate is DarkBurn's own code, licensed MIT OR Apache-2.0 like the rest of DarkBurn.

## Ported with credit (MIT)

All from MeerK40t, https://github.com/meerk40t/meerk40t, Copyright (c) 2021 meerk40t, MIT
(`licenses/MeerK40t-LICENSE-MIT`), read at commit `d511944e08788e88077727f682957759884ff5ca`
(25 September 2026).

| DarkBurn files | Source | What was taken |
|---|---|---|
| `src/distance.rs` | `meerk40t/lihuiyu/driver.py`, `parser.py` | How a distance is written: `z` for each 255 mils, `\|` plus a letter for 26 to 51, three digits from 52, letters `a` to `y` for 1 to 25, and the reading of the same |
| `src/speed.rs`, `tests/speed_codes.rs`, `tests/data/speedcodes.txt` | `meerk40t/lihuiyu/laserspeed.py`, `test/test_laser_speeds.py` | The speed code layout (`CV`, `V`, the 16-bit value as two three-digit numbers, the acceleration digit, the `C` suffix, raster step `G`, diagonal value); the period equation per board and acceleration (M2 with the original software's slope 12120, and 11148 under "fix speeds", M1/M3, B2, B, B1, A, M); acceleration breakpoints; the limits and low-speed clamps. The 3064 speed codes in `speedcodes.txt` and the comparison rules of `tests/speed_codes.rs` come from `test_laser_speeds.py` |
| `src/egv.rs` | `meerk40t/lihuiyu/driver.py`, `test/test_drivers_lihuiyu.py` | The direction and laser letters (`B`, `T`, `L`, `R`, `M`, `D`, `U`); rapid moves (`I` … `S1P`, and `IS2P` when the rail is unlocked); entering program mode (`I`, speed code, `N`, the declared directions, `S1E`) and leaving it (`FNSE-`); raster mode, its row reversal and the 45 degree step correction; the golden rectangle used in the tests comes from `test_drivers_lihuiyu.py` |
| `src/egv.rs` (`octant_runs`) | `meerk40t/core/plotplanner.py` (`grouped`) | Grouping single steps into straight and diagonal runs |
| `src/emulator.rs` | `meerk40t/lihuiyu/parser.py` | How EGV text is read back into moves, laser state and speeds; double `P` homes; text after `FNSE-` is ignored until the next `I` |
| `src/packet.rs`, `src/transport.rs` | `meerk40t/lihuiyu/controller.py`, `meerk40t/ch341/libusb.py`, `ch341/mock.py`, `ch341/__init__.py` | 30-byte payloads padded with `F` (`@` padding with 0), the `0xA6` byte, the CRC-8 table, the meta characters (`-` wait for finish, `*` abort, `!` pause, `&` resume, `#` serial, `\x18` quit), the status bytes (206 OK, 238 busy, 207 CRC error, 236 finished, 239 power, 204 serial OK or M3 finished); the send loop (resend on error, wait after `-`); USB ID `1a86:5512` and endpoint `0x02`, recorded in comments only |
| `src/egv_file.rs` | `meerk40t/lihuiyu/device.py`, `loader.py` | The `.egv` header MeerK40t writes (`Document type : LHYMICRO-GL file`, the creator line, `%0%0%0%0%`) and that a reader skips it |

The code is a Rust re-implementation written for DarkBurn, not a transliteration. One known
difference: in program mode the writer always uses acceleration 1 with the `C` suffix; see
`docs/research/other-controllers.md`, section 7.

## Written by DarkBurn

The toolpath conversion (`src/toolpath.rs`: fitting rows to raster mode, falling back to lines,
the bed check, speed clamps and warnings), the in-memory board, the round-trip tests and the
`.egv` reader's tolerance of other headers are DarkBurn's own.

## Not read

LibLaserCut (LGPL), K40 Whisperer (GPL) and the boards' own software were not opened or read. No
GPL, LGPL or AGPL code was read, copied or ported.

## Dependencies

| Crate | Licence | Notes |
|---|---|---|
| `kurbo` | MIT OR Apache-2.0 | `Point` at the crate boundary |
| `darkburn-job` | MIT OR Apache-2.0 (DarkBurn) | `Toolpath`, `Op`, and the fill helpers used by the tests |

This crate opens no USB, serial or network device.
