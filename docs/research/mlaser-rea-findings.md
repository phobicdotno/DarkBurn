# MLaser v0.0.1.51 SP1: REA findings (2026-11-08)

Handed over from the SolidCraft session, where the official MLaser download was fetched and inspected with REA (Reverse Engineer Anything, `rea-agents` 6.8.0). Static analysis only: PE import tables, strings, shipped config and log files. Nothing from the download was executed. No decompiled code was transcribed.

Legend: **observed** = read directly from the shipped files (REA kept an evidence ID for some of them; the IDs and the file hashes are left out of this copy); **inferred** = my reading of observations; **unknown** = not established.

## Files

- Download: `Mlaser-v0.0.1.51_SP1.zip`, from the Gweike documentation page on gweikecloud.com (gweikelaser.com is parked). Manual: `Gweike-M-Series-User-Manual.zip`, from the same page.
- Extracted tree: the folder `Mlaser-v0.0.1.51_SP1\Mlaser-v0.0.1.51_SP1\`, unpacked from that zip.
- Prior full analysis of v0.0.0.52: the owner's phobicMlaserLinux project, `docs\analysis\` (00-overview, 01-hardware-config, 02-layer-params, 03-chf-format, 04-controller-protocol, 05-motion-pipeline, 06-ui-features, 07-exe-internals, 08-runtime-logs, 09-cad-libs, 10-wine-mlaser-prior-work, 11-static-findings, 99-gaps) and a Python protocol implementation in `src/nexcut/mcc/` (framing, crc, commands, registers, dissector).

## Layout (observed)

Top level: `MainApp.exe`, `MotionCtrl.dll`, `PHBX.dll`, `AutoNest.dll`, `SmartNest.dll`, `CircleFitDLL.dll`, `Dxf2Grp.dll`, `DxfParseDllvc100.dll`, `splineAnalyerVc100.dll`, BCGControlBar Pro 22.10 (`BCGCBPRO2210u100.dll` + two style DLLs), VC10 runtime (`mfc100u`, `msvcp100`, `msvcr100`), `VCRedistSetup.exe`, `windowsdesktop-runtime-8.0.17-win-x64.exe`, folders `Module`, `File`, `Lang`, `Log`, `Update`, `Report`, `SVG`, `Graph`, `Help`, `Bitmap`, `dat`, `res`, `Dump`.
`Module\`: `CADModule.dll`, `ControlModule.dll`, `LangModule.dll`, `LogModule.dll`, `NCModule.dll`, `ParaModule.dll`.
Same module layout as the v0.0.0.52 build analysed in phobicMlaserLinux. The .NET 8 desktop runtime installer is new to me; what uses it is **unknown** (no managed module found in the tree so far).

## Import tables (REA `inspect_pe_imports`)

### `Module\NCModule.dll` (observed)
- PE32 x86, 795,648 bytes.
- Imports 8 DLLs, 363 functions, coverage complete.
- `WS2_32.dll`: 18 functions, all by ordinal (19, 52, 8, 11, 111, 20, 17, 2, 115, 23, 9, 4, 21, 116, 3, 18, 151, 16). Inferred from the standard ws2_32 ordinal table: socket, sendto, recvfrom, bind, closesocket, select, setsockopt, ioctlsocket, WSAStartup/WSACleanup, WSAGetLastError, inet_addr, htons/ntohs, gethostbyname, connect, send, recv.
- `KERNEL32.dll` serial API: `GetCommState`, `SetCommState`, `GetCommTimeouts`, `SetCommTimeouts`, `PurgeComm`, `CreateFileA/W`, `ReadFile`, `WriteFile`, `FlushFileBuffers`. Also `GetPrivateProfileStringA/IntA` (reads the INI below), `LoadLibraryW`/`GetProcAddress`, threads and events.
- `RPCRT4.dll`: `UuidCreateSequential` (one function).
- `MSVCR100`/`MSVCP100`/`mfc100u` runtime, `SHELL32`, `SHLWAPI`.
- Inferred: NCModule owns all machine I/O (UDP and serial). Matches the v0.0.0.52 finding.

### `PHBX.dll` (observed)
- PE32 x86, 135,168 bytes. 9 DLLs, 234 functions, complete.
- `HID.DLL`: `HidD_GetHidGuid`, `HidD_GetAttributes`, `HidD_GetPreparsedData`, `HidP_GetCaps`, `HidD_GetFeature`, `HidD_SetFeature`. `SETUPAPI.dll`: `SetupDiGetClassDevsA`, `SetupDiEnumDeviceInterfaces`, `SetupDiGetDeviceInterfaceDetailA`, `SetupDiDestroyDeviceInfoList`. Plus `CreateThread`, `ReadFile`, `CancelIo`, registry read/write, old statically-linked MFC style imports (`GlobalAddAtomA`, `WinHelpA`).
- No network imports.
- Inferred: USB HID pendant driver (Chengdu XHC wireless handwheel). `xhcusb.txt` in the root is its log and is full of `Open HID error5` / `Get HID Error!1168` lines (observed), i.e. the pendant was not present on the vendor's packaging machine.

### `MainApp.exe` (observed in the earlier REA pass, evidence not retained)
- Import inventory has 2001 functions (REA refused the default budget twice; the first pass earlier in the session read the saved JSON). Earlier observation: no `ws2_32`; imports include `HID`/`SETUPAPI`, `WINHTTP` and `SensApi`.
- Inferred: the shell does UI, HTTP (updates or cloud) and pendant handling itself, and delegates machine traffic to NCModule.

### Not inspected in this pass
`MotionCtrl.dll`, `ControlModule.dll`, `ParaModule.dll`, `CADModule.dll`, `LogModule.dll`, `LangModule.dll`, nesting and DXF DLLs. The v0.0.0.52 docs describe `MotionCtrl.dll` as an orphaned planner copy; whether that still holds for 0.0.1.51 is **unknown**.

## Shipped configuration `File\ipAdd.ini` (observed, full file read)

Endpoints (all defaults, the user can change them in the app):

| Key | Address | Port | Inferred meaning |
|---|---|---|---|
| Card | 10.1.1.168 | 502 | MCC100 motion card, proprietary framing over UDP (not real Modbus) |
| ZF | 10.1.1.169 | 502 | external height follower |
| OnBZF | 10.1.1.168 | 999 | on-board height follower |
| OnBLaser | 10.1.1.168 | 888 | on-board laser channel |
| Laser | 10.1.1.170 | 10001 | external laser source (IPG/Raycus ASCII per old docs) |
| AF | 10.1.1.168 | 888 | auto focus head |
| AdvAF | 10.1.1.168 | 666 | advanced auto focus |
| EC | 10.1.1.168 | 888 | EC controller |
| AdvEC | 10.1.1.168 | 666 | advanced EC |
| EC3710 | 10.1.1.170 | 502 | EC3710 card, real Modbus TCP per old docs |
| Monitor | 47.104.17.21 | 9001 | remote monitor. Privacy relevant: 47.104.x.x is Alibaba Cloud space (inferred from the range). Reconnect every 300000 ms, MaxTime 1 |

Timing and protocol limits (observed): `MaxItemPerFrame=60`, `MaxFillItem=2000`, `MCFifoTime=1600`, `MCTimeout=500`, `MCMaxSendTime=2`, `MCMaxRecvTime=3`, `MCSendInterval=1`, `FifoTimeout=600`, `FifoAlarmNum=30`, `MCCore=30`, `OfflineTimeout=5000`, `MaxZFDownItem=512`, `MaxAFDownItem=200`, `MaxECDownItem=200`.
`[Soft]` (observed): `MinHardwareVer=20177` (matches firmware `MCC100_V201.77.mcf` in `Update\`, 102,912 bytes, encrypted per old docs), `OnBZFMinHardwareVer=311`, `NCZFMinHardwareVer=325`, `AFMinHardwareVer=133`, `EnableOvertime=1`, `FollowOvertime=20000`, `AccessType=1`, `EnableLog=0`, `AdvLaserWrite=1`, `CheckUserID=109`, `Custom=1234`.
Correction to what I said earlier in that session: this INI does contain the LaserIP, MonitorIP and `[Soft]` blocks.

## Vendor log `Log\2026-09-16.log` (observed)

Frames logged by the NC module while the card was unreachable (WinSock 10060 timeouts), in its `DataEx` short form `func reg count value`:
- `40 64 01 22b8`: write (0x40) register 0x64 (100), one item, value 0x22B8 (8888). Register 0x64 is not listed in the v0.0.0.52 register tables. Meaning **unknown**; 8888 looks like a port number or a magic handshake value (inferred, weak).
- `30 2710 12`: read (0x30) 18 items from 10000, the status block the old docs already list. The one reply that did arrive was rejected with ErrCode 502 (`Buffer: 02 be d0 45 00 03 00 b0 02`), so the card did answer at least once.
- `30 3e8 02`: read 2 items from 1000.
Framing is unchanged from the old analysis (`[seq u16 BE][CRC16][len u16 BE][unit 0][func][payload LE]`, funcs 0x30 read, 0x40 write, 0x26 block write, 0x20/0x21 stream, exception = func | 0x80).

## Deltas 0.0.0.52 -> 0.0.1.51 SP1 (what to diff next)

1. Register 0x64 write at connect (new, unknown).
2. Firmware V201.77 and `MinHardwareVer=20177` (old docs: check their value).
3. .NET 8 desktop runtime installer bundled (new, purpose unknown).
4. `Lang\` strings and the register windows in NCModule: diff rather than redo.

## Safety and legal notes carried over

- Never write MCC100 licence registers 150/151, and never issue `0x65 <- [0x65]` / `0x65 <- [0x66]`.
- Never bypass follower, pierce or gas interlocks in any backend.
- The remote monitor endpoint must be off by default in anything we build.
- Clean-room: behaviour and protocol only, no code transcribed from the vendor binaries. Do not copy `Pc_Software/bcx/user/*.dll` hashes into any public repo.
- Still open per `99-gaps.md`: stop/E-stop command vectors, status and alarm bit decoding, tick units. Close those with a read-only capture against the real card before any motion.
