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
