# Things to confirm on real machines

These parts of DarkBurn were built from public documents and tested against simulators only.
Each line needs a check on real hardware before the matching checklist item is ticked.

## Ruida controller (no machine on hand yet)
- [ ] Machine status word: which values or bits mean running, idle and paused
- [ ] Firmware version reply format
- [ ] Units of the laser power registers (0x12, 0x13, 0x18, 0x19: shown as 0.1 % steps, no source gives the unit) and of the time registers
- [ ] Where the job-time result is stored and its unit
- [ ] Dot mode commands for dot time and spacing (currently a guess)
- [ ] Which command moves Z inside a job, and whether the A command is the A axis or Y
- [ ] Go to origin: rapid to 0,0 or the panel Origin key
- [ ] Job bounds corner and X mirroring
- [ ] File name length, overwrite behaviour, reply for a missing file
- [ ] Query replies without checksum; position report on jog port connect
- [ ] Jog direction for each origin corner
- [ ] Card IDs for boards other than RDC6442S
- [ ] USB end to end: the controller's FTDI chip (0403:6001) opened as a serial port (a COM port) at 115200 baud with DTR and RTS on; replies that arrive in pieces; whether a board with a serial FT232R needs another baud rate. The raw FTDI path (build feature `usb`) has never run against anything
- [ ] Magic detection: a controller acknowledges a datagram scrambled with the wrong magic too, with one byte scrambled with its own magic (the emulator does, after MeerK40t). DarkBurn reads the magic from that byte and from the card ID reply, then confirms it. If a real controller stays silent on a wrong magic, detection still walks through all values
- [ ] Ethernet end to end: DarkBurn sends from UDP 40200 (data) and 40207 (jog) to 50200 and 50207 and expects the answers on 40200 and 40207; the tests use a loopback emulator that answers to whatever port asked
- [ ] RD file saved by DarkBurn: runs from the controller's file list and from a USB stick, and opens in the controller maker's own software
- [ ] RD file reference point: `D8 10` for bed coordinates, `D8 12` for a job that starts from the current position (the app never writes `D8 11`)
- [ ] Hardware overscan: with `CA 01 01..04` in a layer and no overscan moves in the job, the controller adds the run-up and run-out itself, by the distance set on its panel
- [ ] Laser 2 offset and dual-tube enable addresses
- [ ] Job start and end delays

## G-code machines
- [ ] Atomstack GRBL: full job end to end at 115200 baud (TARGET-01)
- [ ] Smoothieware powered dwell for dot mode
- [ ] LinuxCNC powered dwell with M67 and G4
- [ ] Marlin inline rapids with S0 on G0

## Gweike M3 Ultra
- [ ] Read-only capture on the real card to close the gaps in `docs/research/mlaser-rea-findings.md`
- [ ] CO2 water input (`docs/hardware-checks-mcc.md` C22): IN11 read 0 with the chiller off and 1 with it on (owner, 2026-11-12). Still to see: the level during a real water alarm with the chiller running
- [ ] Exported DXF and G-code files open and run in Gweike's own software

## Ruida corrections from public sources (applied to darkburn-ruida and its emulator; verify on a controller)
Source: `docs/research/other-controllers.md`, MeerK40t and ruida-pa (both MIT, see `crates/darkburn-ruida/ATTRIBUTION.md`).
The driver, the `.rd` writer and the emulator all use these now. No real controller has confirmed any of them.
- [ ] Status word: bit 24 = moving, bit 0 = job running, bit 1 = paused or part end. MeerK40t's emulator uses 21/22/23, which should not be copied. Verify on a controller: read the word while idle, running, paused and jogging
- [ ] Power is 14-bit: raw / 163.84 gives percent (about 0.006 % steps), not 0.1 % steps. 100 % is sent as 0x3FFF. Verify on a controller: a 50 % layer shows 50.0 on the panel
- [ ] Time values are microseconds. Verify on a controller: a laser-on delay or dot time of 3 ms behaves as 3 ms
- [ ] Hardware overscan modes are `CA 01 00..05`; which axis each mode uses is unsettled. DarkBurn sends 01/02 for scan lines along X (both ways / one way) and 03/04 for lines along Y, after ruida-pa's names; MeerK40t's use suggests the opposite. Verify on a controller, and swap them if a fill runs its overscan on the wrong axis
- [ ] Known board IDs: 0x65106510 (RDC6442S) and 0x90109010 (RDC8445S); 0x65006500 is only what MeerK40t's emulator answers. No source names a job start or end delay register, so RUIDA-11 stays open. Verify on a controller: read the card ID of every board at hand and add it to the list
- [ ] Job reference point: `D8 10` machine zero, `D8 11` user origin, `D8 12` current position (the writer used `D8 12` for every job before; it now writes `D8 10` for bed coordinates). Verify on a controller: a job in bed coordinates lands where it is drawn wherever the head stands

## Controllers that need a real capture first
- [ ] Trocen / AWC / LO-X7: no public spec; `.ud5` is reported as scrambled; USB chip unresolved. DarkBurn lists the controller as not supported yet and writes no `.ud5` file
- [ ] TopWisdom: USB serial per a developer note; `.out` format not public. DarkBurn lists the controller as not supported yet and writes no `.out` file
- [ ] Cubiio 2 HTTP upload, Emblaser Wi-Fi, Snapmaker 2.0 HTTP

## Device layer in the app (built against the simulators only)
- [ ] GRBL: Connect, Start, Pause, Resume, Stop and Fire on a real machine through the Laser panel (the human clicks Connect; no builder does)
- [ ] GRBL: Stop right after Resume can reset while the head still moves and raise ALARM 3 (seen with the emulator); check on hardware
- [ ] GRBL: jog arrow directions and Frame for each origin corner (the arrows are mapped from the screen to the machine axes by the device's origin corner)
- [ ] GRBL: Machine Settings write of `$N=value` and restore of a `.dbset` backup, including the protection-settings warning ($20, $21, $22, $30, $31, $32)
- [ ] Ruida: the Fire button sends the panel Pulse key down and up on the jog port (UDP only); confirm it fires and stops. Over USB the button is refused with a message
- [ ] Ruida: Connect over Ethernet and over USB on a real controller (the human clicks Connect; no builder does), then Start, Pause, Resume, Stop, Frame, Home and jog
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
