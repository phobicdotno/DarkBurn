# Hardware checks: G-code machines (darkburn-gcodebots)

Everything below was built and tested against emulators written from public documentation.
Each line is something an emulator cannot prove. Confirm it on a real machine, by hand, with
the laser off or the lens covered, before the matching checklist item is ticked.

## G-code machines

### Smoothieware
- [ ] `version` replies with a `Build version: ... MCU: ...` line followed by `ok` (used for identification).
- [ ] `?` returns a report and no `ok`; confirm the exact form of the report (`<Idle,MPos:...,WPos:...>` or `<Idle|MPos:...|WPos:...|F:...>`) on the firmware build in use. Both are parsed.
- [ ] `M114` replies `ok C: X:... Y:... Z:...`.
- [ ] `ok` is sent once a line is accepted into the planner, and a full planner holds the `ok` back (no overrun at a window of 1 and of 2 lines).
- [ ] Which window (1 or 2 lines) is fast enough for fine raster lines over USB without starving the planner.
- [ ] `fire <percent>` and `fire off` exist as written, and `fire` power is a percentage (0 to 100).
- [ ] The laser `S` word runs 0.0 to 1.0 on the machine's config (`laser_module_max_power`); the profile's S-max must match.
- [ ] `suspend` / `resume` pause and continue a streamed job, and how long `suspend` takes to act with 1 or 2 lines in flight (the "reduce pause delay" option sets 1).
- [ ] Byte `0x18` as a reset while a job runs: whether it halts (needs `M999`), resets quietly, or is ignored when grbl mode is off. The stop sequence sends `0x18`, then `M999`, then `M5`.
- [ ] `M999` and `$X` clear a halt; the text printed on `M112` / `0x18` (`ALARM: ...` or `!!`) is recognised as an alarm.
- [ ] `$H` homes and answers `ok` when finished; `G28.2` is not needed.
- [ ] `cat /sd/config.txt` prints the file, and `config-set sd <key> <value>` changes it and what it prints back; whether a reset is needed before the new value applies.
- [ ] Air assist on `M106` / `M107` (the profile default) versus `M7` / `M8` / `M9`, and `M108` / `M109` on the board in use.
- [ ] `M220 S<percent>` works as feed override while a job runs.
- [ ] `G53 G0` machine-coordinate moves work.
- [ ] G-code clustering (collinear same-power moves merged) gives identical burn results and a measurable raster speed gain.

### Marlin
- [ ] `M115` prints `FIRMWARE_NAME:Marlin ...` on the laser build in use (identification and the version shown).
- [ ] `M114` reply form and which coordinates it reports (work or machine) with `G92` active.
- [ ] `ok` flow control with a window of 1 and of 4 lines (96 bytes) on the board's serial buffer, at 115200 and 250000 baud.
- [ ] `M3` / `M4` with `S`, inline mode (`M3 I` / `M4 I` / `M5 I`), and the fan mode (`M106 P<tool> S<0-255>` / `M107 P<tool>`) on the actual firmware configuration; which of them the board's `LASER_FEATURE` / `SPINDLE_FEATURE` accepts.
- [ ] Fire button command per mode (`M3 S<n>` then `M5`, or `M106` / `M107`); the laser really goes off on `M5`, also in inline mode.
- [ ] `M410` quick stop clears the planner without halting and the laser goes off afterwards.
- [ ] `M112`: the board prints `Error:Printer halted. kill() called!` and stops answering until reset; the driver then refuses everything until reconnect.
- [ ] `M503` output matches the parsed form (`M92`, `M203`, `M201`, `M206`, `M211`); `M500` stores, `M501` loads; settings writes such as `M92 X100` apply at once.
- [ ] `M211 S0/S1` soft endstops: the travel check in the driver agrees with the board's limits.
- [ ] Homing: `G28 X Y` (the default; Z is never homed) leaves the laser safe, and `G28` without axes is not used.
- [ ] `G92.1` clears the work offset (used by "clear origin"); some builds lack it.
- [ ] `M400` at the end of a job returns only after the last move.
- [ ] Unknown commands print `echo:Unknown command` and still send `ok` (reported as a job line warning, job continues).
- [ ] `M220 S<percent>` feed override.
- [ ] `start` printed after a board reset is detected and aborts the job.

### Snapmaker
- [ ] `M115` identifies a Snapmaker; the accepted names (`Snapmaker`, `Marlin`).
- [ ] The `;Header Start` field names and units written by `snapmaker::header` are accepted by the touchscreen (work speed, estimated time, bounds, power).
- [ ] Laser power command: `M106 P0 S<0-255>` / `M107 P0` (the default here) versus `M3 P<percent>` / `M5` on the 2.0 laser modules.
- [ ] Focus: the Z position (`focus_height + material_height`) that puts the 1.6 W, 10 W and 40 W modules at focus.
- [ ] Whether serial control needs a handshake or line numbers and checksums before it accepts G-code.
- [ ] Homing with `G28 X Y` is allowed with the laser head fitted.

### LinuxCNC (file output only)
- [ ] The `.ngc` file written by `linuxcnc::write_program` runs on a LinuxCNC machine with `motion.analog-out-00` wired to the laser driver.
- [ ] `M67 E0 Q<0-100>` scaling matches the HAL configuration; the `Q` range of the real wiring.
- [ ] `M64 P0` / `M65 P0` air assist output number.
- [ ] `G64 P0.05` blending does not round the corners of fine work.
- [ ] No live linuxcncrsh driver exists. If one is wanted later, the telnet handshake needs a real session to test.

### Custom G-code device
- [ ] A real controller that answers `ok` per line streams correctly at window 1 and 4.
- [ ] The `status` template: that the reply is a `<State,MPos:x,y,z>` report and is not followed by `ok`.
- [ ] `M400` at the end of a job is understood; clear `finish_sync` where it is not.

### Common
- [ ] Dead-reckoned position (commanded, not measured) on devices without status polling is shown as such in the UI.
- [ ] Opening a real port and TCP socket: `Transport` implementations for serial and TCP come from `darkburn-grbl` and have only been exercised there.
