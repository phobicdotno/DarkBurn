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

## Device layer in the app (built against the simulators only)
- [ ] GRBL: Connect, Start, Pause, Resume, Stop and Fire on a real machine through the Laser panel (the human clicks Connect; no builder does)
- [ ] GRBL: Stop right after Resume can reset while the head still moves and raise ALARM 3 (seen with the emulator); check on hardware
- [ ] GRBL: jog arrow directions and Frame for each origin corner (the arrows are mapped from the screen to the machine axes by the device's origin corner)
- [ ] GRBL: Machine Settings write of `$N=value` and restore of a `.dbset` backup, including the protection-settings warning ($20, $21, $22, $30, $31, $32)
- [ ] Ruida: the Fire button sends the panel Pulse key down and up on the jog port (UDP only); confirm it fires and stops
- [ ] Ruida: bed size read on connect from registers 0x26 and 0x36 (two read-only register reads)
- [ ] Ruida: Home sends HomeXY; Go to origin uses the rapid with the origin option (see "Go to origin" above)
- [ ] Ruida: set and clear user origin are not offered because the panel Origin key is unconfirmed
- [ ] Ruida: laser 2 and U axis read and write through Machine Settings and the Move panel (addresses above)
- [ ] Ruida: file list, upload with the overwrite question, run, delete and calculate time on a real controller
- [ ] Find My Laser: the USB VID/PID table (CH340, CP210x, FTDI, Arduino, ESP32, STM32) only describes ports; confirm names on real hardware

## Connections and GRBL flavours (built against the simulators and loopback listeners only)
- [ ] GRBL-M3 device type on a GRBL 1.1e or 0.9 board: the job runs with M3 only, and Stop leaves the board in an alarm that Unlock clears
- [ ] GRBL-LPC on a Smoothieboard: banner text, the `[OPT:...]` buffer size (the simulator assumes 8192) and that opening the port does not reset the board
- [ ] grblHAL and FluidNC: a full job with the buffer size each reports
- [ ] USB serial at baud rates other than 115200
- [ ] TCP/IP to FluidNC telnet (port 23) and to a grblHAL Ethernet board
- [ ] Bluetooth serial: the paired port is marked as Bluetooth by the operating system and a job streams over it
- [ ] DTR: "Pulse DTR when connecting" resets an Arduino board; "Keep DTR on while connected" off lets an ESP32 board start
- [ ] HTTP: what FluidNC (and other web interfaces) answer to `/command?plain=`, and whether G-code lines are accepted there at all
- [ ] Raw USB: open a controller by vendor and product ID, find its bulk endpoints, stream a job
- [ ] Network bridge: the TCP framing (0x4C, two length bytes, then the UDP packet with its checksum, port 5005) against a real bridge, and whether answers come back framed the same way
- [ ] Automation socket: START from another computer with "Accept commands from other computers" on
- [ ] Diode laser names: Ortur and xTool strings match what owners posted; whether Atomstack and Sculpfun boards print their name at all (the simulator assumes `[MSG:Using machine:...]`)
