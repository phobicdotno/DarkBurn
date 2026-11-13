# Other controllers: what public knowledge exists

Date: 2026-11-08. Scope: Trocen/AWC, TopWisdom, Emblaser, Cubiio, Snapmaker, plus Ruida details still open in `docs/HARDWARE-CHECKS.md`.

Method: public documentation and permissively licensed code only (see `docs/BUILD-RULES.md`). No GPL/LGPL/AGPL code was opened; where such a project is named, only its README/docs or its licence field was looked at. Nothing under `C:\Program Files\LightBurn` was touched. No device or network connection was made. Forum posts are used only for user-visible facts (file extensions, connection type, default addresses).

Tags used below:
- **[src]** observed in a cited source (code, manual or documentation).
- **[forum]** reported by a user or staff member in a public forum, unverified.
- **[inferred]** my reasoning, not stated by any source.

Note: when this was written, `docs/HARDWARE-CHECKS.md` was not yet in the main tree of DarkBurn. The copy read was the one in a work-in-progress tree, next to the `darkburn-ruida` crate and its `ATTRIBUTION.md`.

---

## 0. Summary table

| Controller | Rating | Short reason |
|---|---|---|
| Snapmaker 2.0 (A150/A250/A350), serial G-code | **Clean-room implementable now** | Vendor G-code reference is public; Marlin-derived dialect. |
| Snapmaker Artisan / J1 / Ray (SACP) | **Implementable now (framing); needs capture for job details** | Packet format documented by the vendor; MIT sample client exists. |
| Snapmaker 2.0 over network (HTTP) | **Needs real hardware capture** | Only a table saying "HTTP: yes" was found; endpoints sit in AGPL Luban (not readable). |
| Emblaser 1 (GRBL-based) | **Clean-room implementable now** | Standard GRBL profile at 115200. |
| Emblaser 2 / Core / Pro | **Serial implementable now; WiFi needs capture** | Firmware reported as Smoothie-family; WiFi TCP port not documented. |
| Emblaser 3 | **Not found** | No public evidence that this model exists. |
| Cubiio 2 | **G-code profile now; upload needs capture** | Wi-Fi only, `.txt` G-code, undocumented HTTP interface. |
| Ruida gaps | **Mostly answerable now from MIT sources** (section 4); a few need a capture | |
| TopWisdom (TL-A1 and family) | **Needs real hardware capture** | No protocol or file spec public anywhere found. |
| Trocen / AWC / LO-X7 (USB, Ethernet, `.ud5`) | **Needs real hardware capture** | No protocol or file spec public; one forum hint that `.ud5` is scrambled with a model-dependent key. |
| Lihuiyu M2 Nano family (stock K40), EGV | **Files and emulator done; USB needs approval and hardware** | MIT MeerK40t driver; see section 7. |

Neither Trocen nor TopWisdom is blocked by licence: there is simply nothing to read. The only code found that names them is a stub with no byte-level content (section 1.3).

---

## 1. Trocen / AWC608 / AWC708 / AWC7xxx / LO-X7 (TROCEN-01/02)

### 1.1 What is public

| Source | URL | Licence / status | What it gives |
|---|---|---|---|
| AWC708C Plus user manual (hardware) | https://cnc-tehnologi.ru/files/docs/%D0%9A%D0%BE%D0%BD%D1%82%D1%80%D0%BE%D0%BB%D0%BB%D0%B5%D1%80_AWC708CPlus/AWC_708CPlus_User_Manual.pdf | Vendor manual; facts only | USB cable and network cable connect PC to panel; 4.3 inch panel; "6 axes, 4 laser heads" [src]. No protocol. |
| AWC708C Plus panel manual | https://cnc-tehnologi.ru/files/docs/%D0%9A%D0%BE%D0%BD%D1%82%D1%80%D0%BE%D0%BB%D0%BB%D0%B5%D1%80_AWC708CPlus/AWC_708CPlus_Panel_User_Manual.pdf | Same | Offline work files on a FAT32 USB stick have suffix **`ud5`** (JPEG files cannot be read directly); config export is **`Params.cf5`**; firmware upgrade files are named AWC708L (Lite) and AWC708P (Plus); "Laser 1 must be wired to Laser 1 on the wiring board"; head-spacing settings for up to 4 heads [src]. |
| Trocen AWC708S manual, Network Settings page | https://www.manualslib.com/manual/2263964/Trocen-Awc708s.html?page=71 | Same | Default panel IP **192.168.8.8**; PC must be on the same subnet (example 192.168.8.28) [src, via search summary]. |
| Trocen support page | https://www.sztrocen.com/m/support.php | Vendor | About 25 firmware packages and one 64-bit USB driver for AWC608/AWC608C. **No SDK, no protocol document, no file converter** listed [src]. |
| BuildYourCNC LaserCAD install tutorial | https://buildyourcnc.com/blogs/tutorial/tutorial-trocen-anywells-lasercad-install | Retail blog | One user says the AWC608 board enumerates as a standard **FTDI** USB-serial bridge and that the bundled driver often fails [forum-grade]. |
| Driver aggregator "AWC USB DEVICE" | https://outbyte.com/drivers/network/trocen-anywells-product/awc-usb-device/ | Third party | Hardware ID `USB\VID_1FC9&PID_0085` (1FC9 is NXP's vendor ID, not FTDI's 0403) [unverified]. Conflicts with the FTDI claim; may be different board generations. |
| LightBurn docs, "Ethernet Connection (Trocen)" | https://docs.lightburnsoftware.com/Guides/Trocen-Ethernet.html | Public web docs | Trocen Ethernet is **UDP**, IPv4, static IP set from the controller's Network Settings menu; no port number [src]. |
| Forum: LO-X7 UD5 file output | https://forum.lightburnsoftware.com/t/lo-x7-ud5-file-output/7697 | Forum | A `.ud5` made by third-party software showed blank on an LO-X7. User believed Trocen controllers use different **scramble keys**; staff reply (2019): the key is auto-detected on connect, no manual choice [forum]. |
| Forum: Sending files to DSP controller | https://forum.lightburnsoftware.com/t/sending-files-to-dsp-controller-so-dont-have-to-have-pc-connected-while-running/63243 | Forum | Trocen's own CAD saves `.ud5` for USB stick or Ethernet transfer; jobs can be pushed to controller storage over Ethernet and run without the PC [forum]. |
| Forum: Connecting problem (AWC708C Lite) | https://forum.lightburnsoftware.com/t/connecting-problem/37188 | Forum | USB link can stop responding after a job until the cable is replugged; firmware string `0.1.1.5.31` seen [forum]. |
| Forum: Torcen 708s Ethernet through WiFi/LAN | https://forum.lightburnsoftware.com/t/torcen-708s-ethernet-connection-through-wifi-or-lan-and-lightburn-setup/63508 | Forum | Works through a plain Ethernet-to-WiFi bridge, which fits an ordinary UDP protocol [forum]. |
| Forum: Is Trocen-AWC not supported anymore | https://forum.lightburnsoftware.com/t/is-trocen-awc-not-supported-anymore/8227 and search summaries | Forum | Rotary settings cannot be sent from software (set on panel); older firmware cannot move Z inside a job, Z jog is continuous only [forum]. Relevant to TROCEN-02. |

### 1.2 Open-source implementations found

None. GitHub repo and code searches for `trocen`, `awc708`, `awc608`, `anywells`, `ud5`, `LO-X7` returned no protocol or file-format implementation. Specifically:
- MeerK40t (MIT, https://github.com/meerk40t/meerk40t): its `meerk40t/` tree has `balormk, ch341, grbl, lihuiyu, moshi, newly, ruida` device folders and **no Trocen or TopWisdom driver** [src, GitHub API directory listing, 2026-11-08].
- LibLaserCut (LGPL-3.0, https://github.com/t-oster/LibLaserCut): driver list has Epilog, Ruida, Smoothie, Grbl, Marlin, Lasersaur, Laos, K40 Nano and others, **no Trocen**. (TroTec is a different company, also absent.) Not usable for porting anyway.
- LaserWeb4 (AGPL-3.0): no Trocen driver found. Not usable for porting.

### 1.3 Things that look relevant but are not usable

- `wdunn001/lasercraft` (no licence field): a Rust project whose `crates/dsp/src/lib.rs` has Trocen and TopWisdom descriptors. Its comment says the framing is to be "recovered later from the decompiler" from a reference binary. It contains **no byte-level content** and its method is not clean-room. Do not use.
- `schmnr/beambench` (GPL-3.0), `docs/controller-detection-matrix.md`: lists "Trocen and TopWisdom DSP controllers" as **Deferred** with "no sufficiently documented, safe generic probe has been verified". Docs only, nothing ported. Independent confirmation that no public spec exists, and a warning not to send GRBL/Ruida probes at these devices.

### 1.4 Protocol facts

USB:
- Cable USB to the controller; Windows shows a vendor driver ("Trocen AWC USB DEVICE") [forum-grade].
- Chip is FTDI (VID 0403) on at least some AWC608 boards per one retail tutorial; another listing shows VID 1FC9 PID 0085 [conflicting, unverified]. **FTDI vs libusb cannot be settled from public sources.** [inferred] Different board generations probably differ; an NXP ID would point at a native USB device needing a libusb/WinUSB path, the FTDI one at a serial-style path.
- No framing, command or checksum information is public.

Ethernet:
- UDP [src: LightBurn docs]. Static IP set from the controller menu. Factory default 192.168.8.8 on AWC708S; an AWC708C Lite user reports the same [src/forum].
- UDP port number, framing, ack scheme: not public.
- [inferred] Because the controller works through a dumb WiFi bridge and the vendor tool can push `.ud5` into controller storage, the Ethernet path is probably "send file, then start", like Ruida.

`.ud5` file:
- Offline work file suffix `ud5`, readable by the panel from a FAT32 USB stick, with a preview on the panel [src: panel manual].
- Content scrambled with a controller/model-dependent key [forum]. Format, header, command set and checksum: not public.
- Other suffixes: `.cf5` (parameter export), firmware packages `AWC708L` / `AWC708P` [src].
- [inferred] Ruida's `.rd` is swizzled with a per-model magic byte; the "scramble key" remark suggests a similar scheme. Hypothesis only.

### 1.5 Feasibility

**Needs real hardware capture.** No clean-room path exists from public documents. A workable path:
1. Owner obtains a controller (an AWC608 or AWC708 Lite, for example).
2. Owner generates a handful of tiny `.ud5` files with the vendor's own tool (one line at known power/speed, one rectangle, two layers, one raster) and compares bytes. This is black-box analysis of files the owner created; DarkBurn code never touches the machine.
3. Owner takes passive captures of USB and Ethernet traffic with standard tools (Wireshark, USBPcap) while the vendor tool sends those same files.
4. Only then write the transport impls and an in-memory emulator.

Decision for the owner: whether to analyse vendor binaries is outside the public-knowledge scope of this note. The safe line is captures and sample files only.

TROCEN-02 (dual tube, PPI, auto-feed, overcut): forum evidence shows these are panel settings and that rotary and Z-in-job are limited. No wire-level information. PPI (LINE-11) can be modelled as a layer property but cannot be sent until `.ud5` is understood.

---

## 2. TopWisdom (TL-A1 and family) (TOPW-01/02)

### 2.1 What is public

| Source | URL | Licence | Facts |
|---|---|---|---|
| TL-A1 manual | https://www.manualslib.com/manual/3135251/Topwisdom-Tl-A1.html | Vendor manual | Companion software is **AutoLaser**; Wi-Fi in AP and Station modes (device chosen by IP in AutoLaser); USB communication cable and U-disk port; up to 500 stored files; firmware file `TZD_L020.TFL`. **No Ethernet mentioned**, no job file extension given [src]. |
| TopWisdom product page | http://m.topwisdom.com.cn/Product/detail/l/en/id/2.html | Vendor | USB 2.0 with U-disk, 2.4 inch colour screen, "custom development" available [src, via search]. |
| Vendor download page | http://m.topwisdom.com.cn/Download/index/l/en/id/19.html | Vendor | Could not be fetched (expired TLS certificate). SDK or protocol documents not verified either way. |
| Forum: TopWisdom controller beta testing | https://forum.lightburnsoftware.com/t/topwisdom-controller-beta-testing/6190 | Forum | Support first shipped in 2019 (version 0.9.05); framing and jogging worked; rotary not supported [forum]. |
| Forum: Will a TopWisdom controller work | https://forum.lightburnsoftware.com/t/will-a-topwisdom-controller-work-with-lightburn/5814 | Forum | Developer: connection is **serial over USB**; models TL-410C and TL-403CB mentioned; a user concluded it had no Ethernet [forum]. |
| Forum: message says "Other Type File" | https://forum.lightburnsoftware.com/t/topwisdom-message-says-other-type-file/177406 | Forum | Job file extension **`.out`**; controller is a DSP, not G-code; an "other type" message appears when the file is not what the controller expects; a possible workaround is an "alternate machine ID" setting [forum]. |
| Forum: Rotary setup menu missing | https://forum.lightburnsoftware.com/t/rotary-setup-menu-missing/27344 | Forum | Rotary is configured on the controller itself [forum]. |

### 2.2 Open-source work

None found. GitHub searches for `topwisdom`, `TL-A1`, `top-wisdom laser` return unrelated projects. MeerK40t, LibLaserCut, LaserWeb4 and Rayforge have no TopWisdom code. The `lasercraft` stub (section 1.3) is the only code naming it and has no content.

### 2.3 Protocol facts

- Transport: USB presents as a **serial port** [forum, LightBurn developer]. Baud, framing and checksum unknown. [inferred] Probably a virtual COM port; driver requirements not documented.
- Wi-Fi: the TL-A1 has it; protocol (TCP/UDP, port) not documented [src for existence only].
- `.out`: extension from the forum; content unknown, including whether it is scrambled.
- The "other type file" message suggests the controller validates a file-type or machine-ID field in the header [inferred].

### 2.4 Feasibility

**Needs real hardware capture**, same recipe as Trocen: owner-made sample `.out` files from the vendor tool plus a passive capture of the USB-serial traffic. Serial sniffing is the easiest of all the unknowns to capture. TOPW-02 items (framing, overcut, machine settings) are controller-side panel settings with no public wire information.

---

## 3. Emblaser, Cubiio, Snapmaker

### 3.1 Emblaser (Darkly Labs)

Sources:
- Updating Emblaser 1: https://darklylabs.com/article/updating-emblaser-1-for-lightburn/ (vendor). Baud **115200**; flashed as an Uno/ATmega328 target; afterwards the user types `$RST=*` then `$#` in the console [src]. GRBL-style console syntax. The vendor's Emblaser 1 manual describes a "GRBL 9g (custom)" controller [src via search summary of https://www.manualslib.com/manual/3043977/Darkly-Labs-Emblaser.html].
- Emblaser 2 manual: https://www.bhphotovideo.com/lit_files/626187.pdf (fetch returned 403; secondhand via search) says the firmware reports as "smoothie edge" and lists 115200 [src, secondhand]. Current firmware is identified by strings like `DL126` or `v126` from a `version` console command; firmware older than v123 shows up as a USB drive and updates differently [src: https://darklylabs.com/article/updating-firmware/].
- Emblaser Pro WiFi: https://darklylabs.com/article/setting-up-wifi/ . Console commands `wifi-help`, `wifi-scan`, `wifi-join:SSID,password`, `wifi-info`, `version`; minimum FWCON 2.2.1700 and FWWI 1.1.1; WiFi is set up over USB first. Used as an "Ethernet/TCP" device by IP; **port not given** [src]. The module appears on the network with an "espressif" or "Emblaser" MAC vendor [src: https://darklylabs.com/article/setting-up-wifi-using-angry-ip/].
- Emblaser 3: **no public evidence found.** A GitHub repo named `RocketRaymond/emblaser3_stm32f767` is empty and unlicensed. Treat as unknown; do not add a device type until a user supplies one.
- Smoothieware is GPL (docs only). The only Emblaser-specific code found, `Wol/emblaser-inkscape-gcode-exporter`, is GPL-2.0: not read.

Facts that matter:
- Emblaser 1: GRBL dialect at 115200 [src]. Laser-mode details not given by the vendor page.
- Emblaser 2/Core/Pro: Smoothie-family G-code [inferred from "smoothie edge"]; laser power via `S` scaled by the firmware's configured maximum.
- WiFi variants: TCP stream carrying the same G-code, port to be captured [inferred].

Feasibility: **Emblaser 1 and the serial path of Emblaser 2/Core/Pro are implementable now** as profiles on the existing GRBL/Smoothie code paths. WiFi port and handshake **need a capture** (the owner can read the open port on their own unit).

### 3.2 Cubiio 2

Sources:
- Vendor quick guide: https://cubiio.com/support/guide/cubiio2-quick-guide/ . **No USB**, Wi-Fi only (AP or station; WPS); accepts G-code only as **`.txt`**, not `.gcode`; "Go Home" must be run before a job and the lid interlock must read closed or the unit says "not ready" [src].
- Owner notes: https://github.com/kovyrin/cubiio2 (no licence; facts only, no text copied). The device serves an **unauthenticated web UI** at its IP or `cubiio2-<SN>.local`; firmware update page is `/fw_update.htm`; two lid sensors must both read closed [src].
- G-code dialect from a forum thread: https://forum.lightburnsoftware.com/t/cubiio2-gcode-profile/31608 . Header `G21`, `G90`; laser on `M03 P100 S255`; off `M05`; moves `G1 ... F300`; an `F0` on the first move caused a ~5 s pause; fan commands ignored (fan is software-controlled) [forum].
- A desktop app reached the unit once the IP was placed in the device's "Info" field [forum: https://forum.lightburnsoftware.com/t/connecting-the-cubiio-2-to-lightburn/103290]. The protocol used (HTTP or raw TCP) is not stated.
- `pfsin555/Cubiio2RE` (unlicensed, one ~213 KB `index.html`, pushed 2026-11-04): evidently a reverse-engineered web client. **Not read**; unlicensed means all rights reserved. Do not port.
- The device's HTTP API is not documented anywhere found.

Feasibility: a Cubiio profile (`.txt` output, `M03 P S`, avoid `F0`) is **implementable now**. Sending a job (upload, start, stop endpoints) **needs a capture**: the owner opens the unit's own web UI with browser developer tools and notes the requests, by hand.

### 3.3 Snapmaker

Sources:
- G-code reference: https://snapmaker.github.io/Documentation/gcode/ (repo `Snapmaker/Documentation`, no licence file; public docs, facts only). Laser: `M3 P<percent>` (plain `M3` is full power), `M5` off, both wait for moves to finish [src]. `M2000` system information: `S0` coordinate info, `S1`/`S2` PC/HMI log levels, `S3` read exception code, `S4` clear it [src]. `M2001` peripheral power: `S` HMI, `T` 8-pin port, `B` bed and 4-pin add-on; `0` on, `1` off [src]. `M114` position; `M115` returns Marlin-style firmware info, so Marlin-derived [src].
- SACP docs: https://github.com/Snapmaker/Snapmaker-SACP/tree/main/docs (no licence file; vendor docs). [src]:
  - Serial: Snapmaker 2.0 and J1 speak **plain G-code**, at **115200 or 460800**; Artisan speaks G-code and SACP; Ray speaks SACP (partial G-code).
  - Network: Snapmaker 2.0 uses **HTTP**; J1 and Artisan use **SACP over TCP port 8888**; Ray uses SACP over UDP port 8889.
  - Discovery: UDP broadcast of the text `discover`; reply `{Name}@{IP}|model:{model}|SACP:{version}`; models `Snapmaker A150`, `A250`, `A350`, `J1`, `Artisan`.
  - Switch a serial link to SACP: `M2000 S5 P1` (Artisan, J1); `M2000 U5` and `$PS` (Ray).
  - Packet: `AA 55`, u16 length, version `01`, receiver id, CRC-8 of first 6 bytes, sender id, attribute (0 request, 1 ack), u16 sequence, command set, command id, payload, u16 internet checksum of bytes 7 onward. Peer ids: 0 software, 1 controller, 2 screen.
  - Laser command set `0x12`: `0x01` module info (focal length, platform height, tube power), `0x02` set power (float percent), `0x04` set focal length, `0x08` set platform height.
  - File transfer set `0xB0`: `0x10` start compressed transfer (zlib deflate; file name, length, chunk count, MD5), `0x11` controller requests chunks by index, `0x02` result (0 ok, 1 validation failed, 2 timeout, 12 hardware error).
  - The docs mark 3D printing and CNC groups as TODO.
- MIT client: https://github.com/matze19999/SnapmakerSACP-Python (MIT, pushed 2025-03-04). Readable and portable with credit in `ATTRIBUTION.md`. Confirms the packet layout, CRC-8 (poly 7), 16-bit ones-complement checksum, a hello packet (set `01`, id `05`, string `sm2uploader`, receiver id 2) and port 8888 [src].
- The TypeScript SDK in `Snapmaker/Snapmaker-SACP` declares **no licence**: not portable; docs only.
- Luban (https://github.com/Snapmaker/Luban) is **AGPL-3.0** and the controller firmware repos are **GPL-3.0**: not read.
- The Luban serial-connection wiki page could not be retrieved in usable form, so any touchscreen-confirmation step on connect is unverified [inferred, check by hand].

Feasibility:
- Snapmaker 2.0 serial G-code (GCODE-04): **implementable now** (baud choice 115200/460800, laser profile `M3 P` / `M5`, transport trait).
- Artisan/J1/Ray via SACP: framing, CRC, laser and file-transfer commands **implementable now** against an emulator; semantics beyond the documented sets **need a capture**.
- Snapmaker 2.0 HTTP: **needs a capture**; no permissive source documents the endpoints or token handshake.

---

## 4. Ruida peculiarities (MIT sources)

Sources read for code (MIT, allowed):
- MeerK40t `meerk40t/ruida/` (MIT, https://github.com/meerk40t/meerk40t). Latest release **0.9.8100** (2025-08-15); `main` pushed 2026-09-25. Latest commits touching `ruida/`: 2025-12-29 "Adding resume commands", 2025-12-28 exit logic, 2026-02-28 serial helper, 2026-09-05 GUI fix. Files used: `rdjob.py`, `emulator.py`, `controller.py`, `usb_transport.py`, `README.md`.
- ruida-pa, Ruida Protocol Analyzer (MIT, https://github.com/StevenIsaacs/ruida-pa), pushed 2026-10-31. Its `ruida_pa/protocols/ruida/ruida_protocol.py` tags many entries "Verified RDC6442S" or "Verified RDC8445S" (author-asserted hardware checks). Also `docs/api/RdDriver-interface.md`. This is the most detailed public table found, newer and more hardware-checked than MeerK40t's.
- Rayforge (MIT, https://github.com/barebaric/rayforge, pushed 2026-11-05) drives Ruida over USB, UDP and TCP through ruida-pa; tested only on an RDC6442S (Monport MP570) [src: its `ruidarpa/README.md`].
- `ruida` crate 0.1.1 by lyonlabs (MIT, crates.io, 2025-07-15): thin library, no register tables found; not used.

Address note [inferred, consistent across both sources]: the wire carries a memory address as two 7-bit bytes. MeerK40t's emulator uses the combined value `(msb << 7) | lsb`; ruida-pa uses `msb`/`lsb` groups. So MeerK40t `0x0200` is wire `04 00`, `0x02FE` is `05 7E`, `0x0103` is `02 03`, `0x014C` is `02 4C`. Keep this in mind when comparing the two.

### 4.1 Dual-tube / second laser

- Settings registers for laser 2 (wire `00 xx`) [src: names in both projects; the emulator returns zeros so no values were observed]:
  `17` PWM frequency 2, `18` min power 2, `19` max power 2, `1C` standby frequency 2, `1D` standby pulse 2. Laser 1 equivalents: `11`, `12`, `13`, `1A`, `1B`. Laser 3: min `63`, max `64`, PWM `65`, standby `66`/`67`. Laser 4: `68`, `69`, `6A`, `6B`/`6C`. Also `16` laser attenuation. Only the laser-1 trio is tagged "Verified RDC6442S".
- In-job commands [src, both agree on the bytes]:
  - Immediate / end power: laser 1 `C7`/`C8`, laser 2 `C0`/`C1`, laser 3 `C2`/`C3`, laser 4 `C4`/`C5` (14-bit power).
  - Min/max power: laser 1 `C6 01`/`C6 02`, laser 2 `C6 21`/`C6 22`, laser 3 `C6 05`/`C6 06`, laser 4 `C6 07`/`C6 08`; per-layer forms (layer byte first) `C6 31/32` (1), `C6 41/42` (2), `C6 35/36` (3), `C6 37/38` (4).
  - Through power: `C6 50` (1), `C6 51` (2), `C6 55` (3), `C6 56` (4).
  - `C6 60` layer frequency carries a laser byte, a layer byte and the frequency.
  - Laser on/off delay for the second set: `C6 15` / `C6 16`.
  - `CA 01 10` "laser device 0" and `CA 01 11` "laser device 1".
  - `CA 01 30` / `CA 01 31` "enable laser 2 offset 0/1" and `CA 03 <switch>` "enable laser tube start".
- What MeerK40t's writer emits per layer [src, `rdjob.py`]: laser device 0, speed, laser on/off delay 0, min and max power for **both** laser 1 and laser 2 set to the layer power, then `CA 03 01`; at layer end `CA 01 30`. A single-tube RDC6442S accepts that stream.
- Not found anywhere: the register holding the tube 1 to tube 2 distance (the offset that `CA 01 30/31` switches), and what `CA 01 10/11` do on a real dual-tube machine. MeerK40t's `Offset 1..11 Start/End` names (wire `02 50` to `02 5A`, `01 40` to `01 46`, `02 69`) return zeros: no meaning or unit.
- **Answer for "Laser 2 offset and dual-tube enable addresses":** the job-stream enable bytes are known (above); the offset amount register is still unknown and needs a capture on a dual-tube machine.

### 4.2 Hardware overscan

- ruida-pa lists `CA 01 00..05` as **overscan modes**: 00 off, 01 horizontal bidirectional, 02 horizontal unidirectional, 03 vertical bidirectional, 04 vertical unidirectional (00 to 04 tagged "Verified RDC6442S"), 05 diagonal (not tagged verified), plus `CA 01 55` "overscan 5" [src]. Its gluescript layer API maps NONE, X, X_BI, Y, Y_BI to these and treats diagonal as unsupported.
- MeerK40t's `rdjob.py` names the same bytes `WORK_MODE_1..4` (`CA 01 01..04`), `WORK_MODE_5 = CA 01 55`, `WORK_MODE_6 = CA 01 05`, and emits `CA 01 03` for a raster with an X step and `CA 01 01` for a raster with a Y step [src]. The projects agree on the bytes but not the labels; ruida-pa's 03 = vertical bidirectional does not obviously match MeerK40t using 03 for an X-step raster. **Needs a capture to settle the axis of each mode.** [inferred] Both can be right if "X step" means lines spaced along X.
- The overscan **distance** is not in the job stream in either source. MeerK40t's README says raster layers must be set to overscan 0 in MeerK40t [src], meaning it handles overscan in software. [inferred] The controller takes the distance from a machine setting; candidates by name only: `02 4C` "Laser Reset Time", `02 4D` "Laser Start Distance", `05 44` / `05 45` "Read Scan Backlash flag/1". None has a verified meaning or unit. Open item.

### 4.3 Job start / end delay settings

- In-job [src]: `C6 12` laser-on delay, `C6 13` laser-off delay (second pair `C6 15`/`C6 16`), `C6 11` add-cut delay, `C6 10` cut pierce time (MeerK40t calls it `LASER_INTERVAL`, ruida-pa `CUT_PIERCE`). All carry a 5-byte value. MeerK40t encodes `time_ms * 1000` and ruida-pa displays `mS`, so the raw unit is **microseconds** (both consistent).
- Machine settings, by name only: `02 03` G0 delay, `02 07` feed delay after, `02 0B` feed delay prior, `02 0D` shut down delay, `02 39` repeat delay, and an on/off delay block "Delay 1..9" starting at wire `02 5C` (those look like auxiliary output delays, not job start or end) [name-based, inferred].
- **No source names a register or opcode as "job start delay" or "job end delay".** Nearest names: `02 38` "Sts Ahead Time", `03 21` "Work End Time" (names only). Needs a capture while the panel setting is changed.

### 4.4 Board ID list

Card ID is read with `DA 00 05 7E` and replied as a 5-byte value:

| Card ID | Model | Source |
|---|---|---|
| `0x65106510` | RDC6442S | MeerK40t `rdjob.py` and ruida-pa (tagged verified) |
| `0x90109010` | RDC8445S | ruida-pa `CARD_IDS`; several RDC8445S axis registers are tagged verified on that board |
| `0x65006500` | labelled "RDC6442G" | MeerK40t emulator's own answer; **emulator value, not a hardware reading** |

No public source gives card IDs for RDC6445G, 654x, 658x, 5121, 320A, 420B, 6332G or KT332N. Beam Bench (GPL, docs only) states its 6445G path accepts any card that answers identity and status queries because the real ID is not registered, which agrees [docs]. The firmware string is at wire `05 7F`; ruida-pa's changelog quotes `RDLC-V8.01.70` as a real reply on its test unit (MeerK40t's emulator returns its own name) [src]. This answers "Firmware version reply format" in part: a NUL-terminated ASCII string.

### 4.5 Machine status word

- Read with `DA 00 04 00` (MeerK40t numbering `0x0200`). Both projects define a bitmask [src]: **bit 24 `0x01000000` moving; bit 1 `0x00000002`; bit 0 `0x00000001` job running**. ruida-pa calls bit 1 "paused" (register tagged verified RDC6442S); MeerK40t calls it "part end". The two disagree on bit 1.
- MeerK40t's **emulator** answers that register with plain values 21 running, 22 idle, 23 paused. That cannot be the same scheme (22 has bit 1 set yet means idle). It is an emulator choice, [inferred] unreliable; do not copy it.
- ruida-pa's older design doc numbers the bits 0/1/2; its code and API doc say 24/1/0. Trust the code.
- Idle = none of the three bits set [inferred; MeerK40t's controller reports "Idle" when none are set].
- Answers "Machine status word" with a good candidate; the meaning of bit 1 wants a capture during a pause.

### 4.6 Units and other answers for `HARDWARE-CHECKS.md`

| Open item | Answer from public MIT sources | Confidence |
|---|---|---|
| Units of power | 14-bit value, percent = raw / 163.84 (16384 = 100 %), resolution about 0.006 %. **Not** 0.1 % steps | [src] both projects |
| Units of time | 5-byte value in microseconds (displayed as ms) | [src] |
| Frequency | 5-byte value, shown in kHz (MeerK40t sends `value * 1000`) | [src] |
| Speed | 5-byte value, shown as mm/s (micrometre scaling) | [src] ruida-pa |
| Coordinates | 5-byte absolute in micrometres; 14-bit relative ("near") moves, absolute needed beyond +/-8.192 mm | [src] MeerK40t |
| Position report | `DA 00 04 21` / `04 31` / `04 41` / `04 51` for X / Y / Z / U | [src] |
| Job time result | Wire `07 10` "document time" per ruida-pa; MeerK40t's emulator handles an `E8 04` "calculate document time" request. Unit unconfirmed | [src] |
| Z / U / A moves | In-job: `80 01` Z abs (MeerK40t comments "should be 0x80 0x08?"), `A0 00` A, `A0 08` U. Immediate: `D9 02` Z, `D9 03` U, `D9 30` XYU. MeerK40t itself flags the A-versus-Y ambiguity | [src] partial |
| Go to origin | `D8 10` machine zero, `D8 11` origin/anchor, `D8 12` current position (ruida-pa tags these verified RDC6442S) | [src] |
| Homing | `D8 2A` XY, `D8 2C` Z, `D8 2D` U, `D8 2E` focus Z | [src] |
| Pause / resume | `D8 02` / `D8 03` (ruida-pa verified; MeerK40t added resume commands 2025-12-29) | [src] |
| USB transport | FTDI `0403:6001`, serial; MeerK40t opens at 115200 and notes baud barely matters | [src] |
| Ports | UDP 50200 (data), 40200 (reply side), 50207 (keypad/jog) | [src] |
| Query replies | Replies are swizzled and lack the 2-byte checksum prefix that outbound UDP packets carry; `CC` ack, `CF` nak, `CD` error, `CE` enquiry | [src] |
| Dot mode, file-name length, X mirroring, jog direction per origin, job-bounds corner | Not answered by these sources. `CA 04` "X sign map" (one value byte) may carry X mirroring [name-based, inferred] | open |

---

## 5. Recommended implementation order

1. **Snapmaker 2.0 serial G-code (GCODE-04)**, with SACP framing and CRC behind the same transport trait. Everything needed is in vendor docs plus one MIT client. Highest certainty, lowest risk.
2. **Emblaser 1 and 2 serial profiles** on the existing GRBL/Smoothie paths. Pure profile work.
3. **Ruida updates from section 4**: power units (about 0.006 %, not 0.1 %), status bitmask, overscan mode bytes `CA 01 00..05`, laser-2 power, frequency and delay bytes, card IDs `65106510` and `90109010`. Keep the open items (laser-2 offset amount, overscan distance register, job start/end delay, bit 1 meaning, other board IDs) listed in `HARDWARE-CHECKS.md` for the owner's hands-on captures.
4. **Cubiio 2 G-code profile**, then the HTTP upload once the owner has recorded the browser requests from their own unit.
5. **Snapmaker 2.0 HTTP and SACP job paths**, after a capture (Artisan/J1 only if the owner has one).
6. **TopWisdom, then Trocen**, gated on hardware and sample files. TopWisdom first because its USB side is a serial port (easiest to capture). For Trocen start with Ethernet (UDP, plain IP, static address) and the `.ud5` sample-diff exercise; USB afterwards because the chip identity is unresolved (FTDI vs VID 1FC9). Do not write emulators or file writers before real samples exist; an invented format is worse than none.

## 6. Source and licence list

| Project / document | Licence | May read for porting? |
|---|---|---|
| MeerK40t | MIT | Yes (credit in `ATTRIBUTION.md`) |
| ruida-pa (StevenIsaacs) | MIT | Yes |
| Rayforge | MIT | Yes |
| SnapmakerSACP-Python (matze19999) | MIT | Yes |
| ruida crate (lyonlabs) | MIT | Yes (little content) |
| Snapmaker/Documentation, Snapmaker-SACP docs | none declared | Facts from public docs only; do not copy text or the TypeScript SDK |
| kovyrin/cubiio2, Cubiio2RE, lasercraft | none declared | No code; the notes are cited as facts only; Cubiio2RE and lasercraft internals not used |
| LibLaserCut, jnweiger/ruida-laser, smiilaserpants/RuiDa-Machine-Code | LGPL-3.0 / GPL-2.0 / GPL-3.0 | No (docs and README only) |
| Luban, Snapmaker controller firmware, Beam Bench, LaserWeb4, All4Laser | AGPL-3.0 / GPL-3.0 | No (Beam Bench docs quoted once for the "deferred" statement) |
| Trocen, TopWisdom, Darkly Labs, Cubiio vendor manuals | proprietary | Facts only |

## 7. Lihuiyu / K40 (M2 Nano family)

Date: 2026-11-13. Crate: `crates/darkburn-lihuiyu`. Source: MeerK40t `meerk40t/lihuiyu/`, `meerk40t/ch341/` and its tests, MIT, read at commit `d511944e08788e88077727f682957759884ff5ca` (25 September 2026). LibLaserCut (LGPL) and K40 Whisperer (GPL) were not opened. No device was connected.

### 7.1 What was learned

- **Boards** [src]: the stock K40 boards (M2 Nano, M1, M3, B2, B, B1, A, M) all take EGV ("LHYMICRO-GL"), a text language of relative moves in mils (1/1000 inch). The power is not in the language: the laser is only switched on (`D`) and off (`U`), and the strength is set with the knob or the panel on the machine. M3 boards with new firmware also take a PWM power value; DarkBurn reads it back but does not write it.
- **Moves** [src]: `B` right, `T` left, `L` up (towards the back), `R` down, `M` diagonal, each followed by a distance (`a`–`y` for 1–25, `|a`–`|z` for 26–51, three digits up to 254, `z` per 255). Native origin is the back-left corner with y growing towards the front.
- **Modes** [src]: rapid moves are `I…S1P`; a cut block is `I`, a speed code, `N`, the declared directions and `S1E`, then moves, closed with `FNSE-`. Raster mode is the same with a raster step in the speed code (`G` and three digits): reversing the row direction makes the board step one raster step on its own.
- **Speed codes** [src]: a 16-bit timer period per board and acceleration, written as `CV` or `V` plus two three-digit numbers, an acceleration digit, sometimes a `C` suffix, and for program mode a diagonal value. All 3064 codes from MeerK40t's test list (taken from the original board software) are checked against DarkBurn's output in `tests/speed_codes.rs`.
- **Transport** [src]: a CH341 USB chip (`1a86:5512`) in EPP mode. Packets are 30 bytes of text padded with `F`, framed as `0x00` + payload + CRC-8 (32 bytes). The CH341 takes at most 31 data bytes after its EPP write command byte `0xA6`, so on the wire a packet is 34 bytes: `A6 00 <30 bytes> A6 <crc>` (MeerK40t's `CH341EppWrite`). After each packet the host reads a status byte: 206 OK, 238 busy, 207 CRC error (send again), 236 finished, 239 power problem, 204 serial OK (or finished on M3). A `-` after a job means "wait until the board has finished".
- **Files** [src]: `.egv` files are the EGV text behind a short header (`Document type : LHYMICRO-GL file`, version, creator, then `%0%0%0%0%`).

### 7.2 What DarkBurn does

- Writes vector cuts and horizontal bidirectional fills from a DarkBurn toolpath as EGV, and saves `.egv` files. Fills whose rows the raster mode cannot follow (angled rows, one-way rows, rows that burn while changing lines) are written as cut lines instead.
- Reads EGV back with an in-memory board; every behaviour is tested by round trip, on every board model and many speeds, also through the packet layer.
- Changes speed by leaving the block (`FNSE-`) and starting a new one. MeerK40t can switch on the fly with `@NSE`, but its own driver notes say testing showed "some chance of a runaway" with that switch, so DarkBurn does not use it.
- In program mode it always writes acceleration 1 with the `C` suffix. MeerK40t writes the digit 1 there too but computes the value with the speed's own acceleration, which gives a different speed on B2, A, B and B1 boards above 25.4 mm/s. DarkBurn keeps the value and the digit consistent [inferred].
- Clamps speeds to 140 mm/s for cuts and 750 mm/s for rasters, and raises rasters below the slowest the board can do (about 5.1 mm/s on M2, 9.5 mm/s on B2) to that speed; each clamp gives a warning.
- Refuses jobs still in design coordinates and points outside the bed (310 x 210 mm by default).
- There is a `Transport` trait and a send loop, tested only against the in-memory board. There is **no** real USB link.

### 7.3 Open hardware questions

| Question | Why it matters | Status |
|---|---|---|
| Does the real speed match the code? MeerK40t says M2 boards move at about 92 % of the speed asked for with the original software's equation, and offers a "fix speeds" equation | Cut depth and engrave darkness | DarkBurn writes the original equation by default (`fix_speeds` off); measure on a machine |
| Is the laser off after a raster row reversal, or does it carry over? | DarkBurn always switches it off before reversing, which is safe but may add a `U`/`D` per row | open |
| Can the board model be read from the board? | A wrong model gives wrong speeds | The user must pick it; open |
| Are the A, B and B1 equations right? Only one equation per board is known, and none for the `C` suffix | Speeds on older boards | Untested; DarkBurn's "acceleration 1 everywhere" choice in program mode needs a check on a B2 board |
| Does the CH341 link work as described (34-byte EPP writes `A6 00 <30> A6 <crc>` on endpoint `0x02`, status by a vendor control request)? | Sending jobs straight from DarkBurn | Not written; needs the owner's approval and a test on a real K40 |
| How does M3 PWM (`W` codes) map to power, and from which firmware? | Power from the layer setting instead of the knob | Read only; not written |
| What is the slowest raster speed that still steps cleanly? | Rasters below it are raised with a warning | Calculated limits only |
