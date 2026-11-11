# Attribution

DarkBurn is licensed under the MIT License or the Apache License, Version 2.0, at your option
(`LICENSE-MIT`, `LICENSE-APACHE`).

This file names what DarkBurn takes from other projects: code, artwork, fonts and libraries.
Material from another project keeps that project's licence and notice. The last section lists
the licence texts kept in `licenses/`.

DarkBurn reuses code and artwork from **SolidCraft** (https://github.com/tangency-labs/solidcraft,
fork https://github.com/phobicdotno/solidcraft), Copyright (c) 2026 SolidCraft contributors,
licensed MIT OR Apache-2.0. SolidCraft's licence and notice are kept in
`licenses/SolidCraft-LICENSE-MIT` and `licenses/SolidCraft-NOTICE`. Its Apache-2.0 text is the
same as `LICENSE-APACHE`. The file names inside the notice (`ATTRIBUTION.md`,
`THIRD-PARTY-LICENSES.txt`, `vendor/`) are SolidCraft's own files, not DarkBurn's.

The copied source and artwork were taken from SolidCraft 0.40.0-rc.3, commit
`be6e439550d0a7232061557e9d34faf522b18beb`. The crates used by path are built from the next
commit, `ae7da14907e00a11ed04656385cc0c2db386dc55` (the same version), which `scripts/install.sh`
pins (`SOLIDCRAFT_PIN`). Both commits are in the fork, on branch `params-resize`. They follow
upstream commit `419bbcda0`. The pinned commit is also the head of pull request #13422 in the
upstream repository, where the change was merged as commit `ca171fe9`. The second commit changes
one copied file, `crates/ui-egui/src/ui_kit/panel.rs`; DarkBurn's copy has the form of the first.
The sources of the files copied later (`crates/darkburn-mcp/src/server.rs`, the Windows
packaging) are the same at both commits.

DarkBurn contains no LightBurn code, icons, artwork, pictures, fonts or sample files.

## Used as crates (by path, unchanged)

| Crate | Path | Used by |
|---|---|---|
| solidcraft-theme | `../solidcraft/crates/theme` | `crates/darkburn-ui`, `apps/darkburn` |
| solidcraft-ui-text | `../solidcraft/crates/ui-text` | `crates/darkburn-ui`, `apps/darkburn` |
| solidcraft-ui-registry | `../solidcraft/crates/ui-registry` | `crates/darkburn-ui`, `apps/darkburn` |
| solidcraft-color | `../solidcraft/crates/color` | `crates/darkburn-ui`, and `solidcraft-theme` itself |
| solidcraft-mcp-server | `../solidcraft/crates/mcp-server` | `crates/darkburn-mcp` |

All five are version 0.40.0-rc.3, MIT OR Apache-2.0. Cargo builds them from a SolidCraft checkout
beside this repository (`../solidcraft`) at the pinned commit. They take their version, licence
and dependency versions from SolidCraft's own workspace file, so the whole checkout must be
there. Their source is not kept in this repository, with one exception:
`crates/darkburn-mcp/src/server.rs` is a changed copy of `crates/mcp-server/src/server.rs`, a
source file of `solidcraft-mcp-server` (see Copied source).

## Copied source

| DarkBurn file | SolidCraft source | Changes |
|---|---|---|
| `crates/darkburn-ui/src/ui_kit/{actions,attention,checks,choice,color,confirm,container,dash,details,disclosure,drag,fields,layout,menu,metrics,panel,recent_colors,search,settings_search,shortcuts,stages,status,table,text,text_menu,tree,value_focus,wheel,preferences_layout,preferences_small}.rs` | `crates/ui-egui/src/ui_kit/` | Test modules removed; `pub(crate)` made `pub`; two calls into SolidCraft's app replaced (key names in `text_menu.rs`, label translation in `tree.rs`). Added by DarkBurn since: sums and units in number fields (`fields.rs`: `NumberParser`, `install_number_parser`, `number_parser`, `typed_number`; used in `color.rs`) and the Enter key in dialogs (`actions.rs`: `note_enter_kept_from_ok`, called from `panel.rs`) |
| `crates/darkburn-ui/src/ui_kit/mod.rs` | `crates/ui-egui/src/ui_kit/mod.rs` | Only the modules above, and `compat` |
| `crates/darkburn-ui/src/ui_kit/compat.rs` | Pieces of `crates/ui-egui/src/ui_kit/{assistant,navigation,navigation_menu,preferences,preferences_cards,reports,history}.rs` and `crates/ui-egui/src/keymap.rs` | Lifted helper functions and constants only |
| `crates/darkburn-ui/src/fonts.rs` | `crates/ui-egui/src/fonts.rs` | Bundled faces only; system and CJK fallbacks removed |
| `crates/darkburn-ui/src/icons.rs`, `icon_svg.rs` | `crates/ui-egui/src/icons.rs`, `icon_svg.rs` | Tests removed; `pub(crate)` made `pub`; only the glyph source module kept; in `icon_svg.rs` the call to the theme's icon colour mapping is replaced by the plain colour |
| `crates/darkburn-ui/src/frame.rs`, `value_step.rs`, `theme_files.rs`, `theme_options.rs` | `crates/ui-egui/src/` (same names) | Tests removed from `frame.rs`, `value_step.rs` and `theme_files.rs`; `pub(crate)` made `pub`; in `value_step.rs` the engine unit lookup is replaced by a fixed list and the wheel claim is written inline. `theme_options.rs` had no tests; apart from `pub` its only change is line wrapping |
| `crates/darkburn-ui/src/titlebar.rs` | `crates/ui-egui/src/titlebar.rs` (itself adapted from VectorCraft, see below) | Tests removed; close goes straight to the viewport; the resize zones take drags only |
| `crates/darkburn-ui/src/blur.rs` | `crates/ui-egui/src/blur.rs` | Only the no-blur stub |
| `crates/darkburn-ui/build.rs` | `crates/ui-egui/build.rs` | Only the `icon_data` and `builtin_themes` steps |
| `apps/darkburn/src/native_frame.rs` | `apps/solidcraft/src/native_frame.rs` | Uses DarkBurn's title-bar geometry; test removed |
| `apps/darkburn/src/window_fit.rs` | `apps/solidcraft/src/window_fit.rs` | Tests removed |
| `apps/darkburn/src/startup.rs` | Parts of `apps/solidcraft/src/startup.rs` | Settings check and failed-start counter only; the safe-mode variable is `DARKBURN_SAFE_MODE`; `portable_dir` and its test are DarkBurn's |
| `apps/darkburn/src/main.rs`: `fit_window_to_work_area`, `PrefsWriter`, `changes_settings` and the window setup (the `egui::ViewportBuilder` chain) | `apps/solidcraft/src/main.rs` (window setup itself adapted from VectorCraft, see below) | Doc comments dropped; `PrefsWriter` has DarkBurn's `store_beside` and `beside`, which keep what a second window stored; DarkBurn's title, icon and app id |
| `apps/darkburn/build.rs` | `apps/solidcraft/build.rs` | DarkBurn's icon and names; the version resource text is written inline |
| `apps/darkburn/src/shell.rs`: the function `app_bar` | `crates/ui-egui/src/toolbar.rs`: the function `app_bar` | Adapted. DarkBurn's mark, name, menus and commands; the order of the steps and the sizes of the bar's parts are SolidCraft's. The rest of the file is DarkBurn's own |
| `apps/darkburn/src/ribbon.rs`: the constants `TAB_ROW_HEIGHT`, `GROUP_HEIGHT`, `RIBBON_HEIGHT`, `BUTTON_HEIGHT`, `BUTTON_MIN_WIDTH`, `BUTTON_MAX_WIDTH` and `BUTTON_GAP` | `crates/ui-egui/src/toolbar.rs` (the same names) | Values unchanged. The rest of the file is DarkBurn's own |
| `apps/darkburn/src/updates.rs`: the HTTPS client set-up in the function `check`. `apps/darkburn/Cargo.toml`: the `ureq` and `rustls` lines | `apps/solidcraft/src/main.rs`: the functions `http_call` and `agent_http`. `apps/solidcraft/Cargo.toml`: the same two lines | DarkBurn's user agent and time limits; redirects are followed one at a time and each address is checked. The rest of `updates.rs` is DarkBurn's own |
| `crates/darkburn-mcp/src/server.rs` | `crates/mcp-server/src/server.rs` (JSON-RPC framing and MCP lifecycle from CADCraft, see below) | `Tools::read_resource` returns a `Resource` with its own MIME type and text; `check_arguments` comes from the crate |

Followed, not copied. These parts are DarkBurn's own code:

- The MCP tool set and its names in `crates/darkburn-mcp/src/tools.rs` follow SolidCraft's
  `crates/mcp` and `docs/mcp.md`. The first tests in `crates/darkburn-mcp/tests/protocol.rs`
  follow the shape of the tests in SolidCraft's `crates/mcp/src/tests.rs`.
- The window layout follows SolidCraft's: the rest of `apps/darkburn/src/shell.rs` and
  `ribbon.rs`, and `prefs.rs` and `settings.rs` in the same folder.

### Windows packaging

| DarkBurn file | SolidCraft source | Changes |
|---|---|---|
| `packaging/windows/darkburn.wxs` | `assets/platforms/windows/solidcraft.wxs` | Install for one user or for all users, with the `WixUI_Advanced` wizard; DarkBurn's file types; a desktop shortcut; the settings folder is removed on request. Most of the file is DarkBurn's own |
| `scripts/package-common.ps1`, `scripts/build-installer.ps1` | `packaging/windows/package.ps1` | Split into shared steps and an installer script; two programs; the licence list is written by `cargo about`; x64 only |

Both SolidCraft files say in their own header that they are adapted from PhotoCraft's packaging,
under Apache-2.0. SolidCraft's `ATTRIBUTION.md` records the same for
`assets/platforms/windows/solidcraft.wxs` and for its `packaging/` folder, and its `NOTICE` for
`packaging/`. The licence is Apache-2.0, not MIT OR Apache-2.0. The parts DarkBurn took from the
two files keep that licence. The Apache-2.0 text is `LICENSE-APACHE`.

### Credits SolidCraft passes on

Some of the copied material came to SolidCraft from its sibling projects. SolidCraft's
`ATTRIBUTION.md` records this; its `NOTICE` names CADCraft, GridCraft and PhotoCraft. DarkBurn
passes the credit on.

| Project | Licence | What reached DarkBurn |
|---|---|---|
| VectorCraft, https://github.com/storytold/vectorcraft, Copyright (c) 2026 ArtCraft Team and the VectorCraft contributors | MIT OR Apache-2.0 (`licenses/VectorCraft-LICENSE-MIT`) | Title bar drag, caption buttons and resize zones in `crates/darkburn-ui/src/titlebar.rs`; the caption glyph geometry in `assets/icons/ui/source/glyphs.rs`; the window setup in `apps/darkburn/src/main.rs` |
| CADCraft, https://github.com/storytold/cadcraft, Copyright (c) 2026 ArtCraft Team and the CADCraft contributors | MIT OR Apache-2.0 (`licenses/CADCraft-LICENSE-MIT`) | JSON-RPC framing and MCP lifecycle in `crates/darkburn-mcp/src/server.rs`. SolidCraft gives this credit for its `crates/mcp/src/server.rs`; it moved that code to `crates/mcp-server/src/server.rs` in commit `8c197e4d` |
| CADCraft (as above) and GridCraft, https://github.com/storytold/gridcraft, Copyright (c) 2026 ArtCraft Team and the GridCraft contributors | MIT OR Apache-2.0 (`licenses/CADCraft-LICENSE-MIT`, `licenses/GridCraft-LICENSE-MIT`) | Tool schema helpers and argument checking inside the `solidcraft-mcp-server` crate (`crates/mcp-server/src/schema.rs`). Used as a crate, not copied. SolidCraft gives this credit for its `crates/mcp/src/tools.rs` |
| PhotoCraft, https://github.com/storytold/photocraft, Copyright (c) 2026 ArtCraft Team and the PhotoCraft contributors | Apache-2.0, as SolidCraft records it (`LICENSE-APACHE`). PhotoCraft's own notice says MIT OR Apache-2.0 (`licenses/PhotoCraft-NOTICE`). DarkBurn keeps the narrower one, Apache-2.0 | The Windows packaging files above |

## Ported from other projects

The Rust code is new. Constants, tables, names and the order of steps come from the sources. The
MeerK40t and ruida-pa commits that were read were not recorded.

- **MeerK40t**, https://github.com/meerk40t/meerk40t, Copyright (c) 2021 meerk40t, MIT
  (`licenses/MeerK40t-LICENSE-MIT`).
- **ruida-pa** (Ruida Protocol Analyzer), https://github.com/StevenIsaacs/ruida-pa,
  Copyright (c) 2025 Steve Isaacs, MIT (`licenses/ruida-pa-LICENSE-MIT`).
- **phobicMlaserLinux**, https://github.com/phobicdotno/phobicMlaserLinux, a separate project of
  DarkBurn's owner: notes and Python code from the owner's analysis of the control program of
  the Gweike M3 Ultra (Mlaser). At commit `6adc2893` its `pyproject.toml` gives the licence as
  MIT, and it has no licence file. No file of the control program is in this repository.

| DarkBurn files | Source | What was taken | Changes |
|---|---|---|---|
| `crates/darkburn-ruida/src/{command,encoding,swizzle,rd,settings,emulator,machine,packet,transport}.rs` | MeerK40t `meerk40t/ruida/` (`rdjob.py`, `emulator.py`, `ruidasession.py`, `README.md`). The research notes also name `controller.py`, `udp_transport.py`, `usb_transport.py`, `serial_connection.py`, `loader.py` and `device.py` of that folder as read | Opcode constants and their parameter layouts; the rule that a byte of 0x80 or more starts a command; swizzle with any magic value; magic detection by byte histogram and by card-ID probe; the `.rd` header, layer and trailer layout and the file sum; the choice between relative and absolute moves; the controller memory address table; the emulator's answers; file commands `E8 00` to `E8 04`; jog key codes | Written again in Rust |
| `crates/darkburn-ruida/src/command.rs` (`OverscanMode`, reference point bytes), `encoding.rs` (power scale), `settings.rs` (`KNOWN_BOARDS`, `StatusWord`) | ruida-pa `ruida_pa/protocols/ruida/ruida_protocol.py`, `ruida_pa/rpalib/ruida_transcoder.py` | The 14-bit power scale (0x4000 is 100 %); the names of the overscan modes `CA 01 00` to `CA 01 05`; the card ID of the RDC8445S; status bits 0, 1 and 24; the reference point bytes `D8 10`, `D8 11`, `D8 12`; times in microseconds | Constants and names only |
| `crates/darkburn-galvo/src/{codec,cor,settings,machine,transport,emulator}.rs` | MeerK40t `meerk40t/balormk/` (`controller.py`, `usb_connection.py`, `mock_connection.py`, `device.py`) | Opcode numbers and names; the 12-byte command and the 3072-byte list packet; status bits; the USB IDs 0x9588:0x9899 and the endpoints; the bring-up order and default words; the power and frequency formulas; delay encoding; port bits; the job sequence; the two `.cor` layouts and the lens table upload | Written again in Rust. MeerK40t's `balormk/README.md` says its driver began as a plugin for Balor by Bryce Schroeder and still uses many of the original names. See the note on Balor under this table |
| `crates/darkburn-import/src/lbrn.rs` | MeerK40t `meerk40t/extra/lbrn.py` | How `VertList` and `PrimList` are split into tokens; the `LineClosed` and empty `PrimList` forms; XForm composition; polygon vertex placement; bitmap placement | Written again in Rust. MeerK40t's file only reads; the `.lbrn2` writer is DarkBurn's own. The element names, attribute names and cut setting fields that the reader and the writer use come from `docs/research/laser-protocols-and-devices.md`, part 2e; its sources are in the next table |
| `crates/darkburn-mcc/src/{frame,crc,registers,command,fifo,gate,sim,chf,craft,planner}.rs` | phobicMlaserLinux: `docs/analysis/`, `src/nexcut/mcc/`, `src/nexcut/io/chf.py`, `src/nexcut/model/` | Frame layout, CRC and byte orders; register blocks, status and alarm bits; command vectors; the FIFO item grammar; the safety gate; the card simulator; the `.chf` grammar, reader and writer; the craft parameter model; a few message frames recorded in the control program's log files, used as test vectors in `frame.rs`, `crc.rs` and `command.rs` | Ported from Python to Rust. `apps/darkburn/src/{gweike_workflow,gweike_ui}.rs` show and write the craft parameters through this crate |

Balor (https://gitlab.com/bryce15/balor) is the project MeerK40t's galvo driver began from. Its
`LICENSE` file is the text of the GNU GPL, version 3; its GitLab page lists the Server Side
Public License, version 1. `crates/darkburn-galvo/ATTRIBUTION.md` records that Balor was not read
for code and not ported. The opcode names in `crates/darkburn-galvo` are the ones in MeerK40t's
MIT files.

Twelve crates keep their own file with more detail:
`crates/darkburn-{galvo,gcodebots,geom,grbl,import,job,mcc,mcp,raster,ruida,text,vision}/ATTRIBUTION.md`.
This file is the newer one. Where they differ, this file holds. The known differences:

- `crates/darkburn-{grbl,job}/ATTRIBUTION.md` say that no GRBL, grblHAL or FluidNC source was
  opened or read. The research note `docs/research/laser-protocols-and-devices.md` lists source
  files of the three among its sources. See the first row of "Read for facts only".
- `crates/darkburn-{grbl,gcodebots}/ATTRIBUTION.md` say that all message wording is DarkBurn's
  own. The emulators print the firmware's reply lines. See the first two rows of the same table.
- `crates/darkburn-ruida/ATTRIBUTION.md` says that only the documentation of jnweiger/ruida-laser
  and LibLaserCut was read. The research notes list a source file of each. See the same table.
- `crates/darkburn-mcc/ATTRIBUTION.md` gives a folder on the owner's computer in place of the
  address of phobicMlaserLinux, and says that no file of the control program was opened for the
  crate. The crate's facts come from the owner's analysis of that program, and the research
  notes record a static inspection of it. See the list above and the Mlaser row of "Read for
  facts only".
- `crates/darkburn-galvo/ATTRIBUTION.md` calls Balor's licence ambiguous. The paragraph above says
  what Balor's repository carries.
- `crates/darkburn-vision/ATTRIBUTION.md` says the camera library `nokhwa` is used only with a
  feature that is off by default. `apps/darkburn/Cargo.toml` turns that feature on (`live-camera`),
  so `nokhwa` is built into `darkburn`. See "Built on".
- `crates/darkburn-{import,ruida,text}/ATTRIBUTION.md` each have a sentence about LightBurn or
  about commercial laser software that is wider than the sentence at the top of this file.

## Read for facts only

The sources below were read for facts: command bytes, numbers, names and file layouts. No code
from them was copied or ported. Messages to the user are in DarkBurn's own words. Two kinds of
text are the firmware's own: eleven GRBL setting labels, and the reply lines that the two
emulators print. The first two rows name them.

Some rows say "listed in the research notes". For these the notes name the source as a
reference, and the row says all that the repository records about its use.

| Source | Licence | Facts used | Where |
|---|---|---|---|
| GRBL v1.1 (https://github.com/gnea/grbl), grblHAL (https://github.com/grblHAL/core) and FluidNC (https://github.com/bdring/FluidNC): their wikis. The research notes also list `grbl/settings.h`, `doc/script/stream.py`, grblHAL `errors.c` and `alarms.c`, and FluidNC `Report.cpp` as sources | GNU GPL, version 3 or later | Real-time command bytes; the `ok` and `error:N` replies; the status report grammar; `$` commands; setting, error and alarm numbers and what they mean; jogging and laser mode rules; start-up banners. Eleven setting labels are GRBL's own short names (for example `Step pulse time`, `Junction deviation`). The emulator in `crates/darkburn-grbl/src/emulator.rs` prints GRBL's start-up banner, its help line and its `[MSG:...]` lines as the firmware prints them | `crates/darkburn-grbl`, `crates/darkburn-job` |
| Smoothieware, Marlin and LinuxCNC documentation | The firmware is under the GNU GPL: version 3 (Smoothieware, Marlin), version 2 (LinuxCNC) | Command names, reply shapes and which commands exist. The emulator in `crates/darkburn-gcodebots/src/emu.rs` prints Smoothieware and Marlin reply lines as the firmware prints them, among them the halt and kill lines, the `version` reply, the `fire` reply and the firmware and settings reports | `crates/darkburn-gcodebots`, `crates/darkburn-job` |
| Snapmaker public wiki and forum posts. `docs/research/other-controllers.md` also names Snapmaker's G-code reference (https://snapmaker.github.io/Documentation/gcode/) | None stated for the documents. Snapmaker Luban is under the GNU AGPL, version 3 | The fields of the laser file header | `crates/darkburn-gcodebots/src/snapmaker.rs` |
| Console output that owners of Ortur, xTool, Atomstack and Sculpfun machines have posted in public (forum threads and vendor help pages) | None stated | The tags and greetings by which a machine is recognised | `crates/darkburn-grbl/src/detect.rs` |
| Vendor pages, owner notes and forum threads on Emblaser, Cubiio, Trocen and TopWisdom machines, named in `docs/research/other-controllers.md` | None stated | Which firmware family the Emblaser machines speak; that the others have no public description of their protocol | `apps/darkburn/src/devices.rs` |
| EduTechWiki "Ruida" (https://edutechwiki.unige.ch/en/Ruida) | CC BY-NC-SA (version not recorded) | Ports, checksum, magic values, command list, address map | `crates/darkburn-ruida` |
| Stefan Schuermans, "Reverse Engineering of RDLxxx and RDCAM" | None stated | Scrambling, message table, USB notes | `crates/darkburn-ruida` |
| Fablab Nürnberg wiki, "Diskussion:Nova 35" | CC BY-NC-SA 3.0 | UDP framing and ACK bytes | `crates/darkburn-ruida` |
| kkaempf/ruida documentation (https://github.com/kkaempf/ruida) | Ruby licence | USB serial notes | `crates/darkburn-ruida` |
| jnweiger/ruida-laser (https://github.com/jnweiger/ruida-laser); LibLaserCut `drivers/Ruida.java` (https://github.com/t-oster/LibLaserCut) | GNU GPL, version 2; GNU LGPL, version 3 | Listed in the research notes as read-only references for the Ruida protocol (`src/ruida.py` and `doc/protocol.md`; `Ruida.java`) | `crates/darkburn-ruida` |
| lbrn2-to-svg (https://github.com/jlucaso1/lbrn2-to-svg), Copyright (c) 2025 João Lucas de Oliveira Lopes | MIT | A cross-check of shape placement and group transforms | `crates/darkburn-import/src/lbrn.rs` |
| lbrnts (https://github.com/tscircuit/lbrnts), Copyright (c) 2025 tscircuit Inc. | MIT | Listed in the research notes as a typed model of the LightBurn project format, with the full list of cut setting fields | `crates/darkburn-import/src/lbrn.rs` |
| Boxes.py, its lbrn2 exporter (https://github.com/florianfesti/boxes) | GNU GPL, version 3 or later | Listed in the research notes as a read-only reference for the writer. `crates/darkburn-import/ATTRIBUTION.md` records that it was not used | `crates/darkburn-import/src/lbrn.rs` |
| Three LightBurn forum threads on the project file format, named in the research notes | Forum posts; no licence recorded | That the files are plain XML, and how format version 0 (`.lbrn`) and version 1 (`.lbrn2`) differ | `crates/darkburn-import/src/lbrn.rs` |
| The control program Gweike ships with the M3 Ultra (Mlaser, a rebrand of Au3tech NexCut) | Proprietary | Which file types the program opens. The research notes record a static inspection of the program's package: file names, text strings, import tables, configuration and log files. No file of the program is in this repository | `crates/darkburn-import/src/gweike.rs`. `apps/darkburn/src/{gweike_workflow,gweike_ui}.rs` point to `docs/research/mlaser-rea-findings.md` for what is still open |
| Au3tech MCC100-X1 user manual | Vendor document | Listed in the research notes for wiring, network set-up and parameter pages | `crates/darkburn-mcc` |
| OpenLaser (https://github.com/nkapron/OpenLaser) | GNU GPL, version 3 | Listed in the research notes as a read-only reference for the MCC100 card. `crates/darkburn-mcc/ATTRIBUTION.md` records that no GPL code was used | `crates/darkburn-mcc` |
| fiber_laser (https://github.com/rahilm/fiber_laser), Copyright (c) 2026 the mlaser contributors | MIT | Listed in the research notes as a rebuild of Mlaser | `crates/darkburn-mcc` |
| ZXing's barcode contents wiki; NTT DoCoMo's description of MECARD | Public documents; no licence recorded | The `WIFI:` and `MECARD:` text layouts for QR codes | `crates/darkburn-text/src/qr.rs` |

The research notes are in `docs/research/`. The rows above draw on
`laser-protocols-and-devices.md`, `other-controllers.md` and `mlaser-rea-findings.md`. The folder
holds two more notes: `solidcraft-ui-reuse.md`, the study behind the SolidCraft sections of this
file, and `lightburn-feature-inventory.md`, which `docs/PARITY-CHECKLIST.md` names as the source
of its list.

`docs/research/laser-protocols-and-devices.md` also records that two project files the owner
had saved with LightBurn were looked at for their root attributes (`AppVersion`,
`FormatVersion`, `MirrorY`). They are not in the repository. The test files in that format are
written by `crates/darkburn-import/tests/gen_fixtures.py`.

The research notes name more projects as surveyed only: Rayforge, raydriver, the `ruida` crate
by lyonlabs, galvoplotter and SnapmakerSACP-Python (MIT), gcodekit5 (MIT or Apache-2.0), glase
(BSD-3-Clause), and LaserGRBL and rCandle (GNU GPL, version 3). The licences are as the notes
give them. No crate file records anything taken from these projects, and no DarkBurn source
file names them.

No code from potrace, LaserGRBL, LibLaserCut, OpenLaser or any GRBL firmware is in DarkBurn. The
image tracer in `crates/darkburn-raster/src/trace/` is DarkBurn's own: pixel-edge loops, corner
detection and least-squares cubic fitting.

File formats (DXF, PDF, HPGL, SVG, G-code, the AutoCAD shape font format SHX), the barcodes
Code 128, Code 39, EAN-13, UPC-A and Interleaved 2 of 5, and the methods in
`crates/darkburn-raster` and `crates/darkburn-vision` (dithering, distance transform, thinning,
curve fitting, camera calibration) are written from public specifications and published papers.
The crate files name each one.

## Assets

| Files | Author | Source | Licence |
|---|---|---|---|
| `assets/fonts/ibm-plex-sans/IBMPlexSans-Variable.ttf`, `assets/fonts/ibm-plex-mono/IBMPlexMono-Regular.ttf` | IBM Corp. | IBM Plex (https://github.com/IBM/plex), unmodified, via SolidCraft `assets/fonts` | SIL Open Font License 1.1 (`OFL.txt` beside each font, and `licenses/IBM-Plex-OFL`) |
| `crates/darkburn-text/tests/fonts/*` | IBM Corp. | Copies of the two fonts above and of their licence text (`OFL.txt`, `OFL-sans.txt`). Used only by tests | SIL Open Font License 1.1 |
| `assets/icons/ui/design/svg/{flat,model,small,workspace}/*.svg` except `model/db_*.svg` (375 files) | SolidCraft contributors | SolidCraft `assets/icons/ui/design/svg`, unchanged | MIT OR Apache-2.0 |
| `assets/icons/ui/design/{palette,aliases,references}.toml` | SolidCraft contributors | SolidCraft `assets/icons/ui/design`, unchanged | MIT OR Apache-2.0 |
| `assets/icons/ui/source/{glyphs,markers,navigation,pointer}.rs` | SolidCraft contributors; the caption glyph geometry in `glyphs.rs` is adapted from VectorCraft | SolidCraft `assets/icons/ui/source`, unchanged. DarkBurn compiles only `glyphs.rs` | MIT OR Apache-2.0 |
| `assets/icons/ui/STYLE_GUIDE.md` | SolidCraft contributors | SolidCraft `assets/icons/ui`, unchanged | MIT OR Apache-2.0 |
| `assets/styles/ui/themes/*.json` except `warning-label.json` (25 files) | SolidCraft contributors; the palettes are in the next table | SolidCraft `assets/styles/ui/themes`, unchanged | MIT OR Apache-2.0 |
| `assets/styles/ui/themes/warning-label.json` | DarkBurn contributors | DarkBurn's own colour scheme, built from SolidCraft's Graphite theme (`graphite.json`) with new colours | MIT OR Apache-2.0 |
| `assets/icons/ui/design/svg/model/db_*.svg` (40 files) | DarkBurn contributors | Original artwork in SolidCraft's icon style, written by `tools/make_icons.py` | MIT OR Apache-2.0 |
| `assets/icons/app/darkburn-256.png`, `assets/icons/app/darkburn.ico` | DarkBurn contributors | DarkBurn's own mark, a rendering of `assets/icons/ui/design/svg/model/db_app.svg` | MIT OR Apache-2.0 |

Twelve of the copied theme files use the colours of a named palette. Each file names it in its
`provenance` field. Authors, addresses and licences are as SolidCraft's `ATTRIBUTION.md` gives
them.

| Theme file | Palette | Author and address | Palette licence |
|---|---|---|---|
| `nord.json` | Nord | Sven Greb and the Nord contributors, https://github.com/nordtheme/nord | MIT |
| `gruvbox-dark.json`, `gruvbox-light.json` | Gruvbox | Pavel Pertsev, https://github.com/morhetz/gruvbox | MIT |
| `solarized-dark.json`, `solarized-light.json` | Solarized | Ethan Schoonover, https://github.com/altercation/solarized | MIT |
| `nosferatu.json` | Dracula | Zeno Rocha and the Dracula Theme contributors, https://github.com/dracula/dracula-theme | MIT |
| `catppuccin-mocha.json` | Catppuccin | The Catppuccin organisation, https://github.com/catppuccin/catppuccin | MIT |
| `tokyo-night.json` | Tokyo Night | enkia, https://github.com/enkia/tokyo-night-vscode-theme | MIT |
| `one-dark.json` | Atom One Dark | GitHub Atom, https://github.com/atom/atom | MIT |
| `okibi.json`, `akari.json` | The Okibi design system (Okibi dark, Akari light) | Kai Evans, https://kaievns.github.io/okibi-akiri-design-system-spec/ | None given. SolidCraft names the author of the two files as "SolidCraft contributors (Kai Evans)" |
| `cga.json` | The 16 colours of the IBM Color Graphics Adapter | A hardware palette | None |

## Test data and pictures

All of these are made by code in this repository, except the pictures in the last row, which
were saved by hand from DarkBurn's own window. None shows another program or was saved from
one.

| Files | Made by |
|---|---|
| `crates/darkburn-import/tests/data/*` (16 files: DXF, PDF, AI, PLT, `.lbrn`, `.lbrn2`, `.sc`) | `crates/darkburn-import/tests/gen_fixtures.py` (needs Pillow). The program names and version numbers inside the files are text the script writes |
| `crates/darkburn-raster/tests/data/gradient.png`, `shapes.png` | Drawn by `make_gradient` and `make_shapes` in `crates/darkburn-raster/tests/golden.rs` |
| `crates/darkburn-raster/tests/data/golden_*.png`, `crates/darkburn-job/tests/golden/*.gcode`, `crates/darkburn-ruida/tests/golden/*.rd` | The crates' own output, written by their tests when `DARKBURN_BLESS` is set |
| `apps/darkburn/tests/golden-ui/*.png` | The UI test harness (`apps/darkburn/src/ui_harness.rs`), when `DARKBURN_BLESS` is set |
| `docs/img/*.png` | Pictures of DarkBurn, drawn in the UI test harness by the ignored tests in `apps/darkburn/src/readme_shots.rs` |
| `docs/research/img/*.png` (16 files) | Screen captures of DarkBurn's own window, saved by hand from the running program while it was built (`docs/WIRING-RULES.md`) |

There is no SHX font and no `.chf` file in the repository. The tests build them in memory.

## Built on

DarkBurn is built with open-source Rust libraries from crates.io. Cargo downloads them when
DarkBurn is built. None of their source is kept in this repository. `Cargo.lock` names every
version.

The table names the main libraries. It is not the full list. The full list, with the licence
texts of the libraries, is `THIRD-PARTY-LICENSES.txt`. That file is not kept in the repository.
`cargo about` writes it from `packaging/about.toml` and `packaging/about.hbs` when a package is
made:

```
cargo install cargo-about --locked --features cli
cargo about generate --locked --workspace --config packaging/about.toml packaging/about.hbs -o THIRD-PARTY-LICENSES.txt
```

- The Windows installer and the portable zip carry it (`scripts/package-common.ps1`).
- The Linux and macOS archives carry it when `cargo-about` is installed
  (`scripts/package-unix.sh`). Without `cargo-about` the script leaves the list out and says so;
  with `--require-licences`, which the release workflow uses, it stops instead.
- Every package also carries this file and the `licenses/` folder. The Windows installer names
  each file of that folder (`packaging/windows/darkburn.wxs`), and packaging stops when one is
  not named there.
- The list is made for one target. `packaging/about.toml` names the Windows target;
  `scripts/package-unix.sh` adds `--target` with the target of its own package.
- The list leaves out libraries that are used only to build or to test.
- `packaging/about.toml` also lists the licences a library may have; packaging stops at any
  other.

The licence texts for the fonts inside egui are kept in `licenses/` (see "Fonts and data inside
the libraries").

| Area | Libraries | Licence |
|---|---|---|
| Window, widgets, painting | egui, eframe, egui-wgpu, egui-winit, epaint | MIT OR Apache-2.0 |
| Window system | winit | Apache-2.0 |
| Screen reader support | accesskit | MIT OR Apache-2.0 |
| GPU drawing | wgpu, naga | MIT OR Apache-2.0 |
| Previews, thumbnails, raster output | tiny-skia | BSD-3-Clause |
| Curves and paths | kurbo | Apache-2.0 OR MIT |
| Boolean operations, outline offsets | i_overlay | MIT OR Apache-2.0 |
| Kerf offsets with arcs | cavalier_contours | MIT OR Apache-2.0 |
| Reading and writing pictures | image | MIT OR Apache-2.0 |
| SVG import | usvg | Apache-2.0 OR MIT |
| DXF | dxf | MIT |
| PDF and AI | lopdf, fax | MIT |
| XML | quick-xml | MIT |
| Project files | zip | MIT |
| Text shaping, system fonts | rustybuzz, fontdb | MIT |
| Font outlines, text direction | ttf-parser, unicode-bidi | MIT OR Apache-2.0 |
| QR and Data Matrix codes | qrcode, datamatrix | MIT OR Apache-2.0 |
| Variable text | chrono (MIT OR Apache-2.0), csv (Unlicense OR MIT) | As named |
| Serial ports | serialport | MPL-2.0 |
| USB | nusb | Apache-2.0 OR MIT |
| Camera | nokhwa | Apache-2.0 |
| File dialogs | rfd | MIT |
| Clipboard | arboard | MIT OR Apache-2.0 |
| Update check over HTTPS | ureq (MIT OR Apache-2.0), rustls (Apache-2.0 OR ISC OR MIT), ring (Apache-2.0 AND ISC) | As named |
| MCP server | jsonschema (MIT), beside SolidCraft's `solidcraft-mcp-server` | As named |
| Settings and files | serde, serde_json, ron | MIT OR Apache-2.0 |

`serialport` is under the Mozilla Public License 2.0. DarkBurn uses it unchanged. Its source
code, for the version that `Cargo.lock` names, is at https://crates.io/crates/serialport; the
repository is https://github.com/serialport/serialport-rs.

No library in `Cargo.lock` is under the GPL, LGPL or AGPL alone. Two of the libraries built into
DarkBurn offer the GPL as one of two choices: `self_cell` (Apache-2.0 OR GPL-2.0-only) and, on
Linux only, `unescaper` (MIT OR GPL-3.0-only). `packaging/about.toml` accepts no GPL licence, so
they are taken under Apache-2.0 and MIT. `Cargo.lock` also names `r-efi`
(MIT OR Apache-2.0 OR LGPL-2.1-or-later). It is for UEFI targets only and is not built into
DarkBurn for Windows, Linux or macOS.

### Fonts and data inside the libraries

DarkBurn builds egui with its `default_fonts` feature, so egui's own fonts are compiled into
`darkburn`. egui's Noto Emoji font is not: its feature `monochrome_emoji_fonts` is off.

| What | Comes with | Author | Licence |
|---|---|---|---|
| Hack Regular, the fallback font for symbols | egui (`epaint_default_fonts`) | Source Foundry Authors; Bitstream, Inc. | MIT and the Bitstream Vera licence (`licenses/Hack-LICENSE`) |
| Ubuntu Light | egui (`epaint_default_fonts`) | Canonical Ltd. | Ubuntu Font Licence 1.0 (`licenses/Ubuntu-Font-Licence-1.0`) |
| `egui-icons.ttf`, five glyphs from emoji-icon-font | egui (`epaint_default_fonts`) | John Slegers, https://github.com/jslegers/emoji-icon-font | MIT (`licenses/emoji-icon-font-LICENSE-MIT`) |
| Unicode character data | The `icu_*` and `unicode-*` crates | Unicode, Inc. | Unicode-3.0 |
| Text encoding tables | encoding_rs | WHATWG | BSD-3-Clause |

### Used only to test and to build

These are not part of any package.

| Tool | Licence | Use |
|---|---|---|
| egui_kittest, with kittest | MIT OR Apache-2.0 | The headless UI test harness (`apps/darkburn/src/ui_harness.rs`, `docs/UI-TESTING.md`): it runs the real app without a window and draws it offscreen |
| embed-resource | MIT | `apps/darkburn/build.rs`: puts the icon and the version resource into `darkburn.exe` on Windows |
| toml | MIT OR Apache-2.0 | `crates/darkburn-ui/build.rs`: reads `palette.toml` and `aliases.toml` for the icon tables |

## Licence texts

| File | For | Copyright line | Copied from |
|---|---|---|---|
| `licenses/SolidCraft-LICENSE-MIT` | SolidCraft | Copyright (c) 2026 SolidCraft contributors | SolidCraft `LICENSE-MIT` |
| `licenses/SolidCraft-NOTICE` | SolidCraft | Copyright (c) 2026 SolidCraft contributors | SolidCraft `NOTICE` |
| `licenses/VectorCraft-LICENSE-MIT` | VectorCraft | Copyright (c) 2026 ArtCraft Team and the VectorCraft contributors | `LICENSE-MIT` of https://github.com/storytold/vectorcraft at commit `566f7208` (branch `main`, 11 November 2026) |
| `licenses/CADCraft-LICENSE-MIT` | CADCraft | Copyright (c) 2026 ArtCraft Team and the CADCraft contributors | `LICENSE-MIT` of https://github.com/storytold/cadcraft at commit `5a6aa9b6` (branch `main`, 11 November 2026) |
| `licenses/GridCraft-LICENSE-MIT` | GridCraft | Copyright (c) 2026 ArtCraft Team and the GridCraft contributors | `LICENSE-MIT` of https://github.com/storytold/gridcraft at commit `7e0d96d7` (branch `main`, 11 November 2026) |
| `licenses/PhotoCraft-NOTICE` | PhotoCraft | Copyright (c) 2026 ArtCraft Team and the PhotoCraft contributors | `NOTICE` of https://github.com/storytold/photocraft at commit `f1b5e7d7` (branch `main`, 11 November 2026) |
| `licenses/MeerK40t-LICENSE-MIT` | MeerK40t | Copyright (c) 2021 meerk40t | `LICENSE` of https://github.com/meerk40t/meerk40t at commit `d511944e` |
| `licenses/ruida-pa-LICENSE-MIT` | ruida-pa | Copyright (c) 2025 Steve Isaacs | `LICENSE` of https://github.com/StevenIsaacs/ruida-pa at commit `32ae4a8c` |
| `licenses/IBM-Plex-OFL` | IBM Plex Sans and IBM Plex Mono | Copyright © 2017 IBM Corp. with Reserved Font Name "Plex" | `assets/fonts/ibm-plex-mono/OFL.txt`, the same text as `assets/fonts/ibm-plex-sans/OFL.txt` |
| `licenses/Hack-LICENSE` | Hack Regular | Copyright (c) 2018 Source Foundry Authors; Copyright (c) 2003 by Bitstream, Inc. | `fonts/Hack-Regular.txt` in the crate `epaint_default_fonts` 0.37.0 |
| `licenses/Ubuntu-Font-Licence-1.0` | Ubuntu Light | Copyright 2011 Canonical Ltd. (from the font file) | `fonts/UFL.txt` in the same crate |
| `licenses/emoji-icon-font-LICENSE-MIT` | `egui-icons.ttf` | Copyright (c) 2014 John Slegers | `fonts/emoji-icon-font-mit-license.txt` in the same crate |

The MIT licences of SolidCraft, VectorCraft, CADCraft, GridCraft, MeerK40t and ruida-pa have the
same text as `LICENSE-MIT` apart from the copyright line. PhotoCraft's Apache-2.0 text is the
same as `LICENSE-APACHE`.

`licenses/PhotoCraft-NOTICE` is PhotoCraft's whole notice, unchanged. Its first lines give
PhotoCraft's copyright and licence. The file names and the third-party material it then lists
are PhotoCraft's own, not DarkBurn's.
