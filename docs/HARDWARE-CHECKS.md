# Things to confirm on real machines

These parts of DarkBurn were built from public documents and tested against simulators only.
Each line needs a check on real hardware before the matching checklist item is ticked.

## Ruida controller (no machine on hand yet)
- [ ] Machine status word: which values or bits mean running, idle and paused
- [ ] Firmware version reply format
- [ ] Units of power and time settings (assumed 0.1 % steps and milliseconds)
- [ ] Where the job-time result is stored and its unit
- [ ] Dot mode commands for dot time and spacing (currently a guess)
- [ ] Which command moves Z inside a job, and whether the A command is the A axis or Y
- [ ] Go to origin: rapid to 0,0 or the panel Origin key
- [ ] Job bounds corner and X mirroring
- [ ] File name length, overwrite behaviour, reply for a missing file
- [ ] Query replies without checksum; position report on jog port connect
- [ ] Jog direction for each origin corner
- [ ] Card IDs for boards other than RDC6442S
- [ ] USB (FTDI) transport end to end
- [ ] Laser 2 offset and dual-tube enable addresses
- [ ] Job start and end delays

## G-code machines
- [ ] Atomstack GRBL: full job end to end at 115200 baud (TARGET-01)
- [ ] Smoothieware powered dwell for dot mode
- [ ] LinuxCNC powered dwell with M67 and G4
- [ ] Marlin inline rapids with S0 on G0

## Gweike M3 Ultra
- [ ] Read-only capture on the real card to close the gaps in `docs/research/mlaser-rea-findings.md`
- [ ] Exported DXF and G-code files open and run in Gweike's own software

## Ruida corrections from public sources (apply to darkburn-ruida, then verify on a controller)
Source: `docs/research/other-controllers.md`.
- [ ] Status word: bit 24 = moving, bit 0 = job running, bit 1 = paused or part end. MeerK40t's emulator uses 21/22/23, which should not be copied
- [ ] Power is 14-bit: raw / 163.84 gives percent (about 0.006 % steps), not 0.1 % steps
- [ ] Time values are microseconds
- [ ] Hardware overscan modes are `CA 01 00..05`; which axis each mode uses is unsettled
- [ ] Known board IDs: 0x65106510 (RDC6442S) and 0x90109010 (RDC8445S); no source names a job start or end delay register

## Controllers that need a real capture first
- [ ] Trocen / AWC / LO-X7: no public spec; `.ud5` is reported as scrambled; USB chip unresolved
- [ ] TopWisdom: USB serial per a developer note; `.out` format not public
- [ ] Cubiio 2 HTTP upload, Emblaser Wi-Fi, Snapmaker 2.0 HTTP
