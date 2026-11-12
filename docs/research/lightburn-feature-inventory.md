# LightBurn 1.4.04 — Functional Feature Inventory (DarkBurn parity checklist)

Target: `C:\Program Files\LightBurn\LightBurn.exe` (PE32+, x86_64, build 2023-12-04, Qt 5 Widgets, LightBurn.pdb beside it). Window title observed: `<untitled> - LightBurn 1.4.04`.

Scope rule: this is a behavioral/UI inventory only. Nothing here describes how LightBurn's code implements a feature; symbol names and UI object names are quoted purely as evidence that a feature exists.

## Evidence sources used

| # | Source | What it gave |
|---|--------|--------------|
| 1 | `ChangeLog.txt` (2,580 lines, v0.2.22 → 1.4.04 + "1.future") | Every feature ever added, grouped by version/area. Read in full. |
| 2 | PDB/EXE string extraction (REA deep providers Ghidra/IDA/Kuna are not configured on this host, so `open_binary` succeeded but `binary_overview`/`search_procedures` returned `provider_unavailable`; fell back to raw string scan of `LightBurn.pdb` (100,734 strings) and `LightBurn.exe` (55,226 strings)) | 88 Qt `Ui_*` form classes, ~300 `action*` names, ~1,500 widget object names, protocol/connection class names, 9,688 UI label strings. |
| 3 | `inspect_pe_imports` / `inspect_pe_resources` (REA, worked) | 37 import DLLs (Qt5Core/Gui/Widgets/Network/SerialPort/Multimedia/MultimediaWidgets/PrintSupport/Xml/Svg, libusb-1.0, potrace, WS2_32, WINHTTP, CRYPT32, dbghelp, SETUPAPI); resources = icon group only (no embedded Qt .rcc visible as PE resources). |
| 4 | Running process | PrintWindow screenshot of main window; UI Automation tree (401 nodes: all docks, toolbars, laser panel, cuts list, text options); keyboard-driven screenshots of all 8 top-level menus. |
| 5 | Install folder | `languages\` (24 `.qm`), `palette.json` (30 colours), `EzCad2Driver\`, `CaptureLib.dll`, `LBFileDialog.exe`, `LBShellExtThumbnailHandler.dll`, `LexActivator.dll`, `SendUDP.exe`, `potrace.dll`, `libusb-1.0.dll`, `CorelExportScript.txt` + `ExportToLightBurn.gms` + Corel macro PDF, OpenSSL (`libeay32/ssleay32`). |
| 6 | docs.lightburnsoftware.com (1.7 and legacy trees; current root paths 404) | Cut Settings Editor (shared, Line, Fill, Image, Galvo), Laser window, Device Settings (index + Basic + GCode), Settings/Preferences, Optimization Settings, Move window, root sidebar (full page list). Note: 1.7 docs describe some features newer than 1.4.04 (flagged "post-1.4" below). |

---

## A. Main window layout

### A.1 Menu bar (verified by screenshots of the running 1.4.04)

**File**
- New (Ctrl+N), New Window (launches a separate instance), Recent Projects ▸ (24 entries), Open (Ctrl+O), Import (Ctrl+I), Show Notes (Ctrl+Alt+N)
- Save (Ctrl+S), Save As (Ctrl+Shift+S), Save GCode / Save RD / Save OUT / Save UD5 (label follows device; Alt+Shift+L), Export (Alt+X)
- Print (black only) (Ctrl+P), Print (keep colors) (Ctrl+Shift+P)
- Save Processed Bitmap, Save Background Capture (camera)
- Open prefs folder, Export Prefs, Import Prefs, Load Prefs Backup (backup browser)
- Edit Hotkeys (hotkey editor, `.lbhk` import/export)
- Exit (Ctrl+Q)

**Edit**
- Undo (Ctrl+Z), Redo (Ctrl+Shift+Z)
- Select All (Ctrl+A), Invert Selection (Ctrl+Shift+I)
- Cut (Ctrl+X), Copy (Ctrl+C), Duplicate (Ctrl+D), Paste (Ctrl+V), Paste in place (Alt+V), Delete
- Convert to Path (Ctrl+Shift+C), Convert to Bitmap (Ctrl+Shift+B), Close Path, Close selected paths with tolerance, Auto-Join selected shapes (Alt+J), Optimize selected shapes (Alt+Shift+O), Delete Duplicates (Alt+D)
- Select open shapes, Select open shapes set to fill, Select all shapes in current layer, Select contained shapes
- Image options ▸ (Replace Image, Replace Image To Fit, Refresh Image, Apply Crop Path to Image, Flatten Image Crop, Remove Crop from Image, Remove Backup Path — from action names)
- Settings, Device Settings, Machine Settings
- Debug Drawing, Convert to cut (debug) (developer items visible in release build)

**Tools**
- Select, Draw Lines (Ctrl+L), Rectangle (Ctrl+R), Ellipse (Ctrl+E), Polygon, Edit Nodes (Ctrl+`), Add Tabs (Ctrl+Tab), Edit Text (Ctrl+T), Position Laser (Alt+L), Measure (Alt+M), Create QR Code
- Offset Shapes (Alt+O), Weld Shapes (Ctrl+W), Boolean Union (Alt++), Boolean Difference (Alt+-), Boolean Intersection (Alt+*), Boolean Assistant (Ctrl+B)
- Cut Shapes (Alt+Shift+C), Adjust Image (Alt+I), Trace Image (Alt+T)
- Apply Path to Text, Apply Mask to Image, Resize Slots in Selection, Select shapes smaller than selected, Warp Selection (4 points), Deform Selection (16 points)

**Arrange**
- Group (Ctrl+G), Ungroup (Ctrl+U), Auto-Group
- Flip Horizontal (Ctrl+Shift+H), Flip Vertical (Ctrl+Shift+V), Mirror Across Line (Ctrl+Shift+M), Rotate 90° Clockwise (.), Rotate 90° Counter-Clockwise (,), Two-Point Rotate / Scale (Ctrl+2)
- Align ▸ (Left, Right, Top, Bottom, Horizontal Centers, Vertical Centers, Align Centers, Align Horizontally, Align Vertically; keys L/R/T/B/E/C in edit window), Distribute ▸ (Horizontally, Vertically, H-Centered, V-Centered, H-Spaced/padding, V-Spaced/padding, Move H-Together, Move V-Together)
- Nest Selected (SVGNest), Dock ▸ (Dock Left/Right/Up/Down), Move Selected Objects ▸ (to Page Center, Upper Left, Top, Upper Right, Left, Right, Lower Left, Bottom, Lower Right, Move to Laser Position), Move Laser to Selection ▸ (Center, Upper Left/Right, Lower Left/Right, Top, Bottom, Left, Right)
- Grid / Array, Circular Array, Copy Along Path
- Create rubber-band outline from selection, Break Apart (Alt+B)
- Push forward in draw order (PgUp), Push backward (PgDn), Push to front (Ctrl+PgUp), Push to back (Ctrl+PgDn)
- Lock Selected Shapes, Unlock Selected Shapes
- (action names also show: Edit Array, Add To Array, Remove From Array, Exit Array, Flatten Array, Duplicate Array — virtual-array context items)

**Laser Tools**
- Print and Cut ▸ (Set First Target Position, Set Second Target Position, Align Output to Targets, Align Output to Targets (no scaling), Reset Print & Cut, Print and Cut Wizard)
- Calibrate Camera Lens, Calibrate Camera Alignment
- Rotary Setup (Ctrl+Shift+R)
- Focus Test, Interval Test, Material Test, Center Finder
- (action names: Feeder Setup (Alt+Shift+F), Repeat Marking (galvo), Cylinder Correction, Material Test Galvo, Lens Correction Test (galvo), Get Controller Info — shown per device type)

**Window**
- Reset to Default Layout
- Preview (Alt+P), Zoom to Page (Ctrl+0), Zoom In (Ctrl+=), Zoom Out (Ctrl+-), Frame Selection (Ctrl+Shift+A)
- View Style: Wireframe/Coarse, Wireframe/Smooth, Filled/Coarse, Filled/Smooth, Toggle Wireframe/Filled (Alt+Shift+W)
- Dock/toolbar visibility toggles: Art Library, Arrange, Arrange (long), Modifiers, Camera Control, Console, Cuts / Layers, Cut Palette, Docking, File List, Laser, Library, Main, Modes, Move, Numeric Edits, Shape Properties, Text Options (also Variable Text, Tools — list continues below screen edge)

**Language** — 24 languages, each displayed in its native name (see J.3).

**Help**
- About LightBurn, LightBurn Support Forum, Quick Help and Notes (F1; also context-aware F1 over any control), Online Documentation, PDF Documentation Link, Online Video Tutorials, CorelDraw macro setup help
- Generate Support Data, Camera Selection Help
- Check for Updates
- License Management
- Enable Debug Log
- (vendor-branded builds add: Emblaser2 maintenance menu (Calibrate, Lens Clean Gen1/Gen2, Test Fire, Scan for WiFi Emblaser, Upload new Firmware), Darkly Labs help/community, Boss Laser support, Lotus Laser support, iLaser device info/help)

### A.2 Toolbars (all dockable/floatable, each togglable in Window menu)

| Toolbar (object) | Contents |
|---|---|
| Main (`tbMain`) | New, Open, Save, Import \| Undo, Redo \| Cut, Copy, Paste, Delete \| Pan/Drag View (toggle), Zoom to Page, Zoom In, Zoom Out, Frame Selection, Capture Camera Image \| Preview \| Settings, Device Settings. Can be split in two for narrow displays. |
| Arrange (`tbArrangement`) + "Arrange (long)" variant | Group, Ungroup \| Flip Vertical, Flip Horizontal, Mirror Across Line \| Align Centers, Align Vertically, Align Horizontally \| Distribute Vertically, Distribute Horizontally \| Make same width, Make same height, Move Selection (popup) |
| Tools (`tbTools`, left edge, vertical) | Select, Draw Lines, Rectangle, Ellipse, Polygon, Edit Nodes, Add Tabs, Edit Text, Position Laser, Measure |
| Modifiers (`tbBoolTools`) | Offset Shapes \| Weld, Boolean Union, Difference, Intersection \| Grid/Array, Circular Array \| Start Points (toggle tool), Round Corners (toggle tool) + Radius spin |
| Numeric Edits (`tbNumericEdits`) | XPos/YPos (with unit label), Width/Height, Scale %/% , Lock Aspect, 9-dot origin/anchor gadget (`OriginWidget` rb_UL..rb_BR), Rotate, mm/in toggle button. All fields accept equations and unit suffixes (e.g. `5' + 2"`). |
| Text Options (`tbText`) | Font combo (Recent / All Fonts sections, favourites, live preview, SHX fonts), Height, Bold, Italic, Upper Case, Distort (bent text), Welded, HSpace (letter spacing), VSpace (line spacing), Align X (Left/Middle/Right), Align Y (Top/Middle/Bottom), Variable-text type (Normal, Date/Time, Merge/CSV, Serial Number, Cut Setting), Offset |
| Cut Palette (`tbCutPalette`) | 30 colour layer buttons C00–C29 + tool layers T1, T2; optional numeric labels for colour-blind users; right-click flashes shapes on that layer; shift-click selects layer shapes |
| Modes (`tbModes`) | Rotary Setup, Print and Cut (+ Cylinder Correction when enabled) |
| Docking (`tbDocking`) | Dock Left/Right/Up/Down, "Move as group", "Lock inner objects", Padding |
| Status bar | Transform locks: Move / Size / Rotate / Shear toggles; cursor x,y in units; selection bounds (min/max) & object count; status tips/help text; layer+pass progress for GCode jobs; auto-save status |

### A.3 Dock panels (QDockWidgets, tabbed; per-device dock state saved)

Right-dock tab group 1: **Cuts / Layers**, **Console**, **Camera Control**, **Variable Text**, **Shape Properties**. Right-dock tab group 2: **Laser**, **Library**. Hidden by default: **Move**, **Art Library**, **File List**. Auto-compacts into "minimal right dock" on small screens.

- **Cuts / Layers** (`dwCuts`): tree with columns `#`, Layer (name, editable), Mode (Line/Fill/Fill+Line/Offset Fill/Image/Multi), Spd/Pwr, Output, Show, Air. Buttons: priority up/down, delete layer (shift-right-click deletes with content), copy/paste cut settings. Drag-and-drop reorder. Right-click header: show/hide all, enable/disable all, invert, enable/disable/invert all air, "Sort Cuts Last". Below: quick "cut info" box (Speed, Power Max, Power Min for DSP, Interval, Pass Count, layer colour; Laser 2 controls when enabled). Double-click opens Cut Settings Editor. Mouse-wheel on Mode deliberately disabled.
- **Laser** (`dwLaser`): see section G.
- **Library** (`CutLibraryWidget`): see H.5.
- **Console** (`ConsoleWidget`): scrolling log, command entry line, "Show all" (echo every GCode line), 6 programmable macro buttons (Macro0–5, right-click to edit label/content; `$`-commands streamed line by line). GCode devices only.
- **Move** (`MoveLaserWidget`): see G.3.
- **Camera Control**: camera selector, Update Overlay, Trace, Save Settings, Fade, Show, Width/Height, X Shift/Y Shift, status, live viewfinder.
- **Variable Text**: Start, End, Current, Advance by, Previous/Next, Auto-Advance, Test, Reset, Bake, CSV file Browse/Clear.
- **Shape Properties** (`ShapePropertiesWidget`, Qt property browser): per-shape editable properties — rectangle corner radius (negative = interior), polygon sides, ellipse size, text properties, image brightness/contrast/gamma/enhance amount/radius/denoise, cut order priority, power scale, locked, name; multi-edit of several shapes at once.
- **Art Library** (`ArtBrowser`): multiple `.lbart` libraries, folders, thumbnails with hover full-size pop-up, search, drag-in from desktop, add selection, rename, replace-on-duplicate, shared between instances.
- **File List** (`LaserFilesWidget`, DSP only): list files on controller, upload, download/start, delete (multi), delete all, Calc Time (Ruida).

### A.4 Edit window / canvas behaviours (`EditWindow`)
- Rulers (X/Y), grid with fine grid when zoomed, grid contrast setting, dark background option, work-area centre cross, full-screen crosshair cursor option, page outline sized from device (device origin corner determines flip), out-of-bounds shapes drawn faintly, non-output shapes faint.
- Zoom (wheel, +/- keys, Ctrl+0 zoom-to-page, invertible wheel), pan (space-drag, middle mouse, Pan/Drag View tool, auto-pan while dragging), double-middle-click frames selection/content.
- Selection: click, click-and-hold drag, marquee left→right = enclosing, right→left = crossing (colour coded), click anywhere inside filled shape, Tab/Shift-Tab cycles objects, Esc clears/returns to Select, Ctrl+Shift deselect, click-selection tolerance setting, hidden-layer shapes not selectable, locked shapes.
- Transform handles: move, size (corner = locked aspect, Shift unlocks), rotate (snaps 5°/15°/45° with Ctrl/Shift), shear/skew handles, nudge with arrows (3 increments: normal/Ctrl/Shift, Ctrl+Shift tiny 0.1 mm), `<`/`>` rotate 90°, `,`/`.` rotate with 5/10° modifiers, H/V flip keys.
- Snapping: grid snap (zoom-adaptive), object snap to nodes/centres/midpoints of lines & curves/intersections/along line & bezier, Alt widens snap distance, Ctrl shows un-snapped coords, Shift axis-aligns (lines, node drags, measure).
- Right-click context menu: Cut/Copy/Paste/Duplicate/Select All/Group/Ungroup/Preview/Show Properties, paste at cursor.
- Rendering: wireframe vs filled (coarse/smooth), antialiasing toggle, curve quality setting, reduce-motion, layer "show" dims images, flashing highlight of layer shapes.
- Drag & drop: project files, images, `.lbdev` device profiles, art into Art Library; Windows Explorer thumbnails (shell extension) and file association, single-instance re-use on double-click.
- Clipboard: system clipboard with auto-detect of SVG text (Illustrator/Inkscape/EngraveLab), bitmaps, text; inter-instance copy/paste.
- Auto-save with interval, restore file after crash, "modified" tracking, warning when opening newer-version files, version string embedded.

---

## B. Devices / controllers

### B.1 Device families (protocol classes seen in PDB; descriptor classes = entries in the New Device wizard list)

| Family (license tier) | Devices / profiles | Connection types | Notable per-device features |
|---|---|---|---|
| **GRBL** (GCode) | GRBL (1.1f+), GRBL-M3 (1.1e or earlier), GRBL-LPC, grblHAL, Gerbil / Gerbil-STM, Due (GRBL), Grbl-iLaser, X-Carve/Shapeoko auto-setup, Ortur, xTool, Atomstack, G-Weike Cloud detection | USB serial (baud selectable incl. 9600/19200), TCP/IP (GRBL & GRBL-LPC), Bluetooth serial | `$J` jogging, continuous jog, Machine Settings ($$ editor with homing speeds/invert masks), constant power (M3) option, S-value max, GRBL error/alarm translation, buffer-size query, speed/power overrides while running, Fire button, DTR enable, serial-port exclusion list |
| **Smoothieware** (GCode) | Generic Smoothie, FabKit, C3D | Serial, TCP | GCode clustering (fast rasters), HW flow control, faster-pause option, M108/M109 air |
| **Marlin** (GCode) | Marlin | Serial (baud selectable) | M3/M106/inline power modes, tool index (fan 0/1/2), M7/M8 air, Fire button, settings read/write (M503) |
| **Other GCode** | LinuxCNC (M67 power), SnapMaker (header, focus/material height, M106/M107), Emblaser 1 A3/A4, Emblaser 2 / Core (Wi-Fi scan, calibration wizard, lens-clean wizards, test fire, firmware upload, support height, air power), Emblaser 3, Cubiio 2 / 2 Plus (HTTP sync), CanCam, "Custom GCode" (post-1.4 docs) | Serial, TCP, HTTP, Wi-Fi | Vendor wizards |
| **Ruida** (DSP) | RDC6442/6442S/6445G/6445G-AEON/6445H/6445S/644XG/644XG1G/644XS/658XG, RDLC320A, RDLC-420B, RDV6332G, RDC5121, KT332N/KT322N (U-axis Z), Thunder Laser (incl. 1 Gb controller), Boss Laser / Boss HP, Vank, Kaitian, Baisheng, EagleLaser, LaserUp, LightObject R5-DSP, OMTech | Ethernet UDP, USB packet (FTDI/CDM) with port selection, **LightBurn Bridge** (TCP relay via Raspberry Pi, scanner wizard), MTU check | File list/upload/delete, Send (named file, auto-increment filename, overwrite prompt, "99 files" warning), Machine Settings (vendor settings incl. backlash, tube min/max, engraving mode, dual-tube enable/offset, door/water/air protect, focus distance, return position, wireless panel), Get Controller Info (job time, laser-on time), hardware overscan, Min Power/corner power, dot mode, PWM frequency override, cut-through delays, job start/end delays, real relative Z, Z-step per pass, stored scanning offsets, Laser 2 controls, Calc Time, rotary read/write, frame bounds check, Go to Origin |
| **Trocen / AWC** (DSP) | AWC 608 (FTDI serial), AWC 708c, LO-X7/RayScam, TL-A1 detection | USB (LibUSB, multi-device selection), Ethernet (network timeout) | Dual tube, PPI mode, auto-feed, overcut, continuous jog incl. Z, "Delete all files", origin-mode detection, UD5 files |
| **TopWisdom** (DSP) | TL-A1 and family | USB, Ethernet | Machine settings read/write, frame functions, Go to Origin, overcut, continuous jog, auto air-assist flag, OUT files |
| **Galvo / JCZ** (Galvo tier, `EzCad2Driver` folder) | EZCad2 boards (LMC/JCZ), ComMarker B4 & "Dynamic" FPGA boards, DIGIT boards; sources: Fiber (Raycus, IPG YLP/YLPM/YLP-E, JPT, MOPA, SPI), CO2 (tickle), UV | USB (LibUSB) | `.cor` lens correction + markcfg import, galvo 1/2 scale/skew/bulge/trapezoid/negate/is-X calibration & Lens Correction Test, field size/offset, frequency/Q-pulse, timings (jump speed/delays, laser on/off TC, end TC, polygon TC), wobble, grayscale & power scale, auto-rotate/angle-per-pass, kerf, advanced framing dialog (nudge, scale PgUp/PgDn, job counter, F2 start, Require framing before start), red-light enable, door switch, foot switch / Start-Marking trigger, Done Marking signal + duration, Repeat Marking (external axis + Test), rotary runner (by slice / by shape, max shape size, sanity check, progress), external axis setup, 3D Sliced Image, cylinder correction, split marking, Material Test (galvo variant), drill mode, no tabs/overscan/origin controls |
| **Design Only** | (1.future, i.e. first 1.5 build) | none | Editor without a laser |

Connection classes observed: `Connection_Serial` (sync/async), `Connection_TCP`, `Connection_TCP_Relay` (Bridge), `Connection_UDP`, `Connection_LibUSB`, `Connection_CDM` (FTDI packet USB), `Connection_HTTP_Sync`, `Connection_None`. UDP command socket for automation (`SendUDP.exe`; START/STATUS commands; "Automation with UDP" guide).

### B.2 Device management UI
- **Devices dialog** (`DevicesDialog`): list of profiles (drag reorder, sort by name/type), Find My Laser (auto-detect; returns serial, packet-USB Ruida, Bridge, xTool/Ortur detection; asks for size if unknown; auto DTR test), Create Manually (**New Device wizard**: type → connection → name/size → origin corner/auto-home → summary; galvo variant hides origin page and auto-imports `.cor`), Edit, Duplicate, Remove, Import/Export profile (`.lbdev`), LightBurn Bridge setup wizard, Emblaser/Cubiio Wi-Fi wizards.
- Per-device persisted state: cut library, material library, units, grid, cut origin, dock layout, default layer settings, COM port, machine-settings backups.

---

## C. File formats

**Native project:** `.lbrn` (XML) and `.lbrn2` (compact, default since 0.9.21) — embeds images, cut settings, optimization settings, notes, thumbnails (Explorer shell extension), variable-text state, virtual arrays, text backup paths for missing fonts, version string. Open / Import (merge) / Save / Save As; "Open" any supported file type as a project; templates via empty project with cut settings.

**Vector import:** SVG (CSS/style blocks, @font-face, symbols/use/switch, text with fonts & spans, clip paths, embedded images, 72/96 DPI setting, multiple `<svg>`, Vectornator/Affinity/Silhouette quirks), AI (incl. ZStd AI 2020, large canvas, embedded images, groups, hidden layers option, guides filtered), PDF (specific page selection, multi-page, inline/CMYK/palettized/1-bit/CCITT/LZW/JPG images, ICC), DXF (units auto-detect/fallback incl. km, splines/NURBS, blocks/inserts, MTEXT/TEXT/ATTRIB with styles, layers & ByLayer/true RGB colours, dash patterns, auto-join tolerance, hidden layers ignored), PLT/HPGL (precision setting, absolute arcs, PU/PD/PA), RD (Ruida machine files, colour mapped), SignCut `.sc` / `.scpro2`, EPS (disabled). Colour-to-layer mapping (closest colour / unique layer), group-on-import and select-on-import toggles, import to tool layers option, multi-file import, drag & drop.

**Raster import:** PNG, JPG, BMP (incl. broken 1-bit GIMP BMPs), GIF, TIFF, WebP; EXIF DPI; transparency; images embedded in vector files.

**Export:** SVG (bitmaps, offset-fill as fills, compound paths, layer order, SVGNest-compatible), AI (colours), DXF (polylines, circles as CIRCLE, arcs or segments option, layer colours), PNG/JPG/BMP (when selection is bitmap), export selection vs all, variable text evaluated first, remembers last extension/path; **Save processed bitmap**; **Save background capture**; **Print**.

**Machine files:** Save/Run GCode (`.gc/.gcode/.nc/.txt`), RD (Ruida), OUT (TopWisdom), UD5 (Trocen); "Save laser file" / "Send" to DSP; Run file from controller.

**Auxiliary:** `.clb` cut library, `.lbart` art library, `.lbdev` device profile, `.lbprefs` preferences, `.lbset` machine settings backup, `.lbcm` camera calibration, `.lbso` scanning offsets, `.lbhk` hotkeys, image-adjust presets & material-test presets (import/export), `.cor` galvo lens, `markcfg` EZCad config, `.csv/.txt` variable text, `.shx` fonts, `.bin` Emblaser firmware, `.ofr/.ofk/.dat` offline licence files, `DefaultPrefs.ini` shared-space override, job log CSV.

---

## D. Drawing & editing tools

**Creation tools:** Select; Draw Lines (pen: click lines, click-drag curves, Shift 45° snap, ruler readout, auto-connect to existing shapes on same layer, extend existing line); Rectangle (corner radius incl. negative/interior, remove rounded corners); Ellipse; Polygon (N sides); Text (multi-line, RTL, CJK input, I-beam, double-click edit, multi-object edit, "Edit Text Shapes" dialog with pan/zoom, bend/text-on-path with Distort option, auto-weld, upper-case, bold/italic, SHX single-line fonts, font favourites/MRU, default font & size, backup path for missing fonts); QR code (text/URL/Wi-Fi/MECARD contact editors, error correction, 2000-byte limit, editable); Barcode (post-1.4 docs); Measure (lengths, arc angles/lengths, areas, width/height, mm/in switch, remembers last position, Shift axis snap); Position Laser (click to jog); Tangent circle generator (post-1.4).

**Node editing** (`NodeEditTool`): drag nodes/handles, drag line converts to curve, Ctrl+click inserts node, multi-select nodes, keys I (insert), D (delete point / delete line & break), C (corner), S (smooth), L (line), B (break), T (trim), E (extend), snap/join on node drag, undo/redo, Shift constrained drag, start-vertex highlight, align selected nodes (1.3). Start Points tool (set start node/direction, shows start/end markers, works on compound shapes). Node Highlight tool. Path tree debug tool.

**Shape operations:** Convert to Path (also bakes variable text), Convert to Bitmap (DPI dialog), Close Path, Close with tolerance dialog, Auto-Join (tolerance, cancel-able, T-junction safe), Optimize Shapes (line/arc fit, smoothing, in/out counters, Esc reverts), Delete Duplicates, Delete Overlapping Lines, Break Apart, Weld, Boolean Union/Difference/Intersection (1/4096 mm precision), Boolean Assistant (beginner helper with preview, last-op display, Reset), Offset (distance with units/equations, inward/outward/both, Round/Bevel/Corner joins, delete original, select results, outer shapes only, open shapes, live update, zoom/pan while open), Cut Shapes (vector cutter incl. images), Trim (node mode), Round Corners / Fillet (radius incl. negative, failure cursors, auto path conversion), Resize Slots & Tabs (slot width/depth, inch units, rect & round slots, colour-coded line/curve/break highlighting), Warp (4-point) and Deform (16-point) tools, Two-Point Rotate/Scale, Mirror Across Line, Flip H/V, Rotate 90 CW/CCW, Make Same Width/Height (about centre), Create Rubber-band Outline, Select Contained Shapes, Select shapes smaller than selection, Apply Path to Text, Apply Mask (crop) to Image + Flatten/Remove crop, Lock/Unlock shapes.

**Arrays:** Grid Array (rows/cols or auto from dimensions, spacing or centre-to-centre, alternate row/column shift / hex, shift by half, mirror alternates, reverse direction, virtual arrays editable afterwards with Edit/Add/Remove/Exit/Flatten/Duplicate array, variable increment), Circular/Radial Array (count, start/sweep angles, rotate shapes, clockwise), Copy Along Path (count or spacing, rotate shapes, start at nearest point, respects start point), Nest Selected (SVGNest external; True Nest / Quick Nest are post-1.4).

**Arrangement:** Group/Ungroup/Auto-Group (groups contained shapes), Align (9 variants + Alignment Helper for beginners), Distribute (6 modes incl. padding, move-together), Docking (push selection against neighbours in 4 directions, move-as-group, lock inner objects, padding), Move Selected Objects (9 page positions + laser position), draw order (front/back/forward/backward), Lock aspect, 9-point anchor for numeric position/size/scale/rotate entry, Pages window (post-1.4).

---

## E. Cut settings / layers

**Layer model:** 30 colour layers (C00–C29) + 2 tool layers (T1/T2: never output, frame-able toggle, used for masks/text paths/guides). Each layer: name, mode, Output on/off, Show on/off, Air on/off, priority order, speed, max power, min power (DSP), pass count, and since 1.2 **sub-layers** (multi-pass sub-layers with independent settings, up to 11, tabbed in editor with add/duplicate/remove/reorder/enable-disable; Cuts list shows "Multi"). Layer defaults per device (Make Default / Make Default for All / Reset to Default; "Load default layer settings on new"). Per-shape overrides: cut order priority, power scale %, manual start point/direction, locked. Linked library entries (lock mode dropdown; assign without link).

**Modes:** Line (was "Cut"), Fill (was "Scan"), Fill+Line (Scan+Cut), Offset Fill (spiral), Image (bitmaps only; Line-mode images not allowed), Tool layer, Multi (sub-layers).

**Shared / main settings:** Speed (mm/s or mm/min, clamp 2000 mm/s gantry), Max Power %, Min Power % (DSP & grayscale), Number of Passes, Output, Air Assist (plus DSP air mode: by layer / with laser; Ruida 6445G high/low; Emblaser air power/speed), Laser 1 / Laser 2 selection and per-tube power (dual-tube DSPs), Constant Power Mode (GRBL M3) per sub-layer, Pause before / Pause after (galvo), Frequency kHz & Q-Pulse ns (galvo; Frequency also in Ruida material test), Global passes (galvo, post-1.4), cut-setting name, "Ludicrous DPI" warning, equation entry everywhere.

**Line mode — Common:** passes, Z Offset (±100 mm), Z Step per pass, Kerf Offset (closed shapes; keeps start point/direction; galvo supported), Perforation Mode (cut/skip lengths, ensures ≥1 perforation), Tabs/Bridges (enable without destroying tab data; auto even-spacing with min/max count, tabs per shape, tab size with units, tab height / partially cut tabs with Tab Cut Power, skip inner shapes, manual placement via Add Tabs tool, zoom-aware insertion, usable together with perforation/dot mode, lead in/out allowed, not emitted on galvo).
**Line mode — Advanced:** Start Pause / End Pause time (ms), Cut Through (independent start/end, pause power %), Overcut (distance), Override PWM Frequency (Ruida RF tubes, up to 100 kHz), Enable PPI (Trocen/AWC), Lead In / Lead Out (length, angle, Line or Arc style; zero-length troubleshooter), Dot Mode (Ruida: time & spacing; also "use dot mode" on GCode where supported), Ramp (via Fill), U Offset (Ruida U axis), Laser 2 output signal, direction polarity.

**Fill mode — Common:** Bi-directional fill, Cross-Hatch, Overscanning (% or distance; GCode only, shows overshoot distance; auto-overscan added to DSP preview), Line Interval (mm) ↔ Lines Per Inch, Scan Angle (0–360; DSP multiples of 90; angle increment per pass on galvo), passes, Z offset / Z step, Fill grouping: Fill all shapes at once / Fill groups together / Fill shapes individually (inner/outer auto-grouping), Flood Fill (galvo on Common tab).
**Fill mode — Advanced:** Ramp Length & Ramp Outer Edge (stamps), Flood Fill (gantry; respects scanning offsets & ramp), Override PWM Frequency, PPI, U Offset, Dot width correction (galvo), Auto-Rotate & angle increment (galvo), Wobble enable/size/step (galvo), galvo timing overrides (Jump Speed, Min/Max Jump Delay, Jump Distance Limit, Laser On TC, Laser Off TC, End TC, Polygon TC).

**Offset Fill:** same as Fill plus offset-spiral specific behaviour (power scale respected, fewer direction changes).

**Image mode:** Image Mode selector (see F), Line Interval / DPI (interlocked), Scan Angle, Bi-directional scanning, Overscanning (GCode), Negative Image, Pass-Through (external dither; disables mode & grayscale), Dot Width Correction, Cells per Inch + Halftone Angle (Halftone), Ramp Length (Threshold), Z offset, passes, Fill grouping (images merged into one scan when horizontal), Min Power for grayscale (white = empty when 0), Ruida PWM override / U offset, 3D Sliced image (galvo: cleanup pass options).

**Cut Settings Editor dialog** (`CutSettingEdit` / `CutSettingEditGalvo`): layer selector on left (edit multiple layers in one session), Common/Advanced tabs, scan-pattern helper images, dither sample preview, Beginner-mode hides advanced items, "Reset to default", library link indicator, Z controls disabled unless Z enabled, Laser 2 auto-select.

**Material Test generator** (`MaterialTest`, `MaterialTest_Galvo`): grid of speed × power (or interval/frequency/Q-pulse) cells, min/max, columns/rows, labels, unit labels, label layer settings, presets import/export, framing, runs on GCode/DSP/galvo (bypasses Ruida layer limit).

---

## F. Image engraving

- **Image modes:** Threshold, Ordered, Atkinson, Dither (Floyd-Steinberg / JJN high quality), Stucki, Jarvis, Newsprint, Halftone (cells/inch, angle), Sketch (high-pass, handwriting), Grayscale (power modulation; 3D engraving), Pass-Through. True black/white endpoints, sRGB gamma handling, dither preview in editor & in Adjust Image.
- **Shape-property image adjustments:** Brightness, Contrast, Gamma (live), Enhance Amount / Radius / Denoise (unsharp mask), Negative, DPI correction on import, transparency support (trace, dither, mask).
- **Adjust Image dialog** (`AdjustImageDialog`): side-by-side preview, all adjustments + dither mode + DPI, presets (save/load/import/export), negate display, memory-limit warnings, keeps rotated images in sync.
- **Trace Image** (`TraceImageDialog`, potrace): threshold slider, lower cutoff (band tracing), ignore less than, smoothness, optimize, fade, show points, clip rectangle, Sketch trace mode, delete image after trace, trace from camera capture; Multi-file trace (post-1.4).
- **Image tools:** Convert to Bitmap (vectors → image, size-limited), Save Processed Bitmap, Replace/Refresh/Replace-to-fit image, Apply Mask to Image (live crop by vector path, flatten), image masking of grouped shapes, Import 3D model → depth map (post-1.4), Print to printer (processed images optional).
- **Output behaviours:** images scanned together per layer, image merging by group, angled scans with arbitrary skew/scale, fast whitespace skip (GCode, G0 at speed 0), scanning offset (backlash) compensation table with import/export, Smoothie clustering only for images, reduced decimals for GCode grayscale.

---

## G. Laser control panel & job execution

### G.1 Laser dock (`dwLaser`) — verified
Status display (`LaserStatus`: Disconnected / Ready / Busy with progress bar & elapsed/remaining time / Framing / "No laser connected"), Pause, Stop, Start (Alt+S), Frame (rectangle, Alt+R), Frame (rubber-band/O-frame, Alt+Shift+R; Shift = laser on at Fire power; O-frame at full resolution), Save GCode/RD/OUT/UD5, Run GCode (run saved file), Send (DSP; Shift+Send starts immediately), Home, Go to Origin, Start From: Current Position / User Origin / Absolute Coords, Job Origin 9-dot gadget, Cut Selected Graphics, Use Selection Origin, Show Last Position, Optimize Cut Path toggle, Optimization Settings button, Devices button (right-click resets connection), serial port combo ("(Choose)"/auto, remembers port, exclusion list), device profile combo, Re-scan (Bridge), Enable Red Light (galvo), Enable Rotary / Cylinder Correction toggles (optional), mode status text (rotary / Print & Cut / cylinder), Fire button halo when enabled, Start disabled option for upload-capable machines, "Ignore Start when monitor off", job checklist dialog before start, job completion / failure sounds, job log CSV, confirm-start, out-of-bounds checks ("might be out of bounds" / discard out-of-bounds shapes), warn on close while streaming.

### G.2 Preview / simulation (`PreviewDialog`, `SimulationWidget`)
Full motion-planner simulation: play/pause, slider & playback speed, Shade according to power, Show traversal moves, Invert (negative), cut speed scale, cut distance / rapid distance / estimated time (incl. passes, overscan, tabs, lead-in, DSP auto-overscan), zoom/pan/maximize, Save Image (transparent with Shift), "Start Here" / Run from Here / Send from Here / Save from Here (start job part-way), layer/pass readout, uses Start From position, Read Controller Settings (GRBL/Ruida/Trocen) to calibrate simulation in Device Settings → Additional Settings (acceleration, speeds, min/max speed, engraving acceleration, rapid speed).

### G.3 Move window (`MoveLaserWidget`)
Jog arrows (X/Y, diagonals via numpad, Z dashed arrows, A-axis rotary arrows), Home centre button, Distance, Speed, separate Z speed, Continuous Jog toggle, Ctrl/Shift/Ctrl+Shift jog multipliers, Get Position (X/Y/Z/U readout; right-click fills Move-to fields), Move to Position X/Y (+Z) & Go, Saved Positions list + Manage (`StoredCoordinatesEditor`: add, Z enable, drag reorder, sort), Move From Machine Zero, Focus Z, Set Origin, Clear Origin, Set Finish Position (not Ruida), Fire button + Power %, Invert keypad direction, speed/power override sliders while running (GRBL), Rotary jog/positioning (galvo), Laser 2 numeric controls.

### G.4 Console & macros — see A.3. Also Machine Settings dialog (`MachineSettings`): read/write controller params for GRBL/Smoothie/Marlin/Ruida/Trocen-AWC/TopWisdom, vendor vs user settings tabs, axis calibration dialog (`CalibrateAxis`, `CalibrateSingleAxisDialog`), load/save `.lbset`, backup browser, class-reducing safety warning, homing speeds.

### G.5 Job origin / coordinates
Absolute Coords, Current Position, User Origin (Set/Clear), Job Origin 9 positions, Use Selection Origin, Laser (pointer) offset, machine origin corner per device, CNC machine (negative coords) mode, "Swap X/Y", auto-flip of files on origin mismatch, Return to Finish Position, Set Finish Position.

---

## H. Advanced features

### H.1 Camera (`CameraControlWidget`, wizards)
Camera selection (custom Windows capture library with exposure/brightness; default cross-platform; E2 camera), camera type chooser (overhead vs head-mounted), Lens Calibration Wizard (standard / fisheye, presets for official cameras, circles pattern), Camera Alignment Wizard (4-target cut, scale, air assist option, marker index prompts, rotary check), Head-Mounted Camera Alignment Wizard + stitching, Update Overlay / Fade / Show / Width-Height / X-Y shift, Trace from capture, Save background capture, Save Settings, camera settings import/export (`.lbcm`), colour or B/W view, rotate 90° (obsolete), Camera Print & Cut (post-1.4 docs), remember last camera, crop to page.

### H.2 Rotary (`RotarySetupDialog`, `RotarySetupDialog_Galvo`, `RotaryRunner_Galvo`)
Enable rotary, type Chuck / Roller, axis (Y/A/B, rotary axis per device), steps per rotation (or mm per rotation), roller diameter, object diameter or circumference, output centre, mirror output to rotary, split by slice / by shape with max size (galvo), run whole shapes if possible, Test (rotate & return), Read (Ruida), rotary jog/position, split marking, main-window Enable Rotary toggle, per-device Thunder variant, "Rotary mode (DSP/GCode/Galvo)" docs.

### H.3 Print & Cut (`PnCDialog`, `PnCController`)
Set first/second target position (via Position Laser / pointer offset), Align output to targets (scale) / no scaling (position + orientation only), Reset, wizard with scaled/unscaled modes, frame correction, status in main window.

### H.4 Variable text (`VariableTextWidget`, `VariableTextManager`)
Types: Date/Time (format codes), Serial Number, Merge/CSV (quoted fields, multi-line, file relocation), Cut Setting (speed/power/interval/passes `n`, layer `C`, laser name `L6,2`), current index/start/end/advance, auto-advance after send (only if enabled layers contain variable text), Test, Reset, Bake, QR codes honour index, works in virtual arrays and from galvo framing window.

### H.5 Material / Cut Library (`CutLibrary`, `CutLibraryWidget`)
Named libraries (`.clb`, New/Load/Save/Save As, auto-save, backup on save), entries by Material → Thickness (fractions in inch mode, "no thickness" title) → description, type icon (Cut/Fill/Image), Assign, Link (parameter linking; auto-unlink on delete), Create new from layer, Edit Cut, Edit Desc, Duplicate, Delete, per-device library selection, connected-laser checks.

### H.6 Cut planner / Optimization Settings (`OptimizationSettingsDialog2`)
Optimize Cut Path toggle; Order By list (Layer, Group, Priority — drag/sort, remove); Cut inner shapes first; Cut in direction order (4 directions); Reduce travel moves; Hide backlash; Reduce direction changes; Choose best starting point; Choose corners if possible; Choose best direction; Remove overlapping lines (+ distance); Allow reverse direction (older); Set as defaults / Reset; saved in project; spatial inner/outer sorting with open shapes, Sort Cuts Last, manual cut order priorities, forced start points honoured; when off, draw/file order used; progress & cancel.

### H.7 Galvo-only (see B.1 row) plus Lens Correction Test, Calibrate Galvo Scale, External Axis Setup, Repeat Marking, Door Protect stages, Red light framing with blanking (optional delays), field offset, EZCad config import.

### H.8 Other
Focus Test (`FocusTest`; Z range steps, text, save to file, inch/mm, SHX font), Interval Test, Center Finder (circle stock, T1 circle), Feeder Setup (Ruida/Trocen feed axis & delay), Cylinder Correction (incl. concave), Taper Warp (post-1.4), Checklist editor (pre-job checklist), Notes dialog (project notes shown on open), Hotkey editor, Boolean/Alignment helpers, Beginner Mode, Emblaser-specific wizards, SVGNest nesting, UDP automation, REST API (post-1.4), Corel export macro.

---

## I. Settings / preferences and Device Settings

### I.1 Settings dialog (`SettingsDialog`, rebuilt 0.9.08)
- **Display & Units:** units mm/in, mm/s vs mm/min, Curve Quality, Antialiasing, Filled Rendering, Reduce Motion, Font Size, Toolbar icon size (slider, affects palette & jog buttons), Cursor/node size, Dark Background, Grid Contrast, Visual Grid Spacing, Grid Snap Distance, Object Snap Distance, Snapping toggles, Click Selection Tolerance, Show full-screen cursor, Show work-area centre cross, Show palette button labels, Show rotary enable on main window, Show cylinder correction enable, Invert mouse wheel zoom.
- **Editor Settings:** Beginner Mode, Auto-save interval, Automatically check for updates, Auto-launch files in running copy, Default Cut Selected / Use Selection Origin states, Save Cut Selected / Job Origin to project, Save state of Move/Rotate/Scale/Shear switches, Ignore out-of-bounds shapes, Ignore Start when monitor off, Load default layer settings on new, Load/Save dialog choice (internal LBFileDialog / system / bypass), Play error sound, Play job-finished sound, Save Job Log, Use System Clipboard, Shape Move Increments (normal/Ctrl/Shift), Text settings (default font, height, auto-weld), Default prefs override for shared spaces, serial-port exclusion.
- **File Settings / Import-Export:** SVG DPI 96/72, DXF import (auto-detect units, fallback units, auto-close tolerance), DXF export arcs vs lines, HPGL/PLT precision, Import hidden AI layers, Group imported shapes, Select imported shapes, Allow importing to tool layers, Output curve tolerance.
- **Camera:** capture system, resolution, colour/B&W view, rotate 90.
- **Language** menu; **Reset to Default Layout**; prefs import/export/backup browser.

### I.2 Device Settings dialog (`DeviceSettingsDialog`, `DeviceSettingsDialog_Galvo`)
- **Basic:** working size W/H (up to 5000 mm, fractional), origin corner, CNC machine mode, Laser Offset (pointer), Z control (Enable Z, Reverse Z, Relative Z moves only, Optimize Z moves, OMTech Polar), Tab Pulse Width, Enable Job Checklist (+ editor), Frame Continuously (post-1.4), Overwrite files by default, Scanning Offset Adjust table (add/delete/import/export), Enable Scanning Offset Adjustment.
- **GCode devices:** Auto-home on startup, Swap X/Y, Fast Whitespace Scan + speed, Enable $J jogging, Enable DTR, Enable laser fire button, Laser on when framing, Out-of-bounds warning toggle, GCode Clustering (Smoothie), Return to Finish Position, Air assist M7/M8 (Marlin M106/M107, tool index), S-value max, Baud rate, Transfer mode (buffered/synchronous), Start/End GCode scripts, Use G0 for overscan, Emit S with every G1, Marlin M3/inline/tool index, Reduce Smoothieware pause delay, Emblaser support height / return.
- **DSP devices:** Enable U axis, Job Start/End delays (Ruida), Enable Laser 2 controls / Laser 2 offset, Increment filename on send, Disable Start button, Check bounds when framing, Network timeout, Ruida default power/speed buttons, Trocen auto-feed, UDP vs Packet/USB vs Bridge connection & IP.
- **Galvo:** field size & offset, galvo 1/2 calibration (scale/skew/bulge/trapezoid/negate/is-X), COR file load/clear, laser source type (fiber/CO2/UV; Raycus/IPG/JPT/MOPA/SPI), frequency & Q-pulse limits/enable, PWM (CO2) enable/rate/tickle, input/output ports (door switch, foot switch, Done Marking port & duration, red light), default timings (jump speed, delays, laser on/off TC, end TC, polygon TC), external axes, require framing before start, frame/fire options.
- **Additional Settings (simulation):** read controller settings; acceleration/speed parameters used by the preview time estimate.

---

## J. Anything else

### J.1 Keyboard shortcuts (defaults; all remappable via Hotkey Editor since 1.3)
See menus in A.1. Additional: F1 context help, F2 start (galvo frame/rotary windows), Pause key = pause, Ctrl+Break = stop, Alt+S start, Alt+R frame, Alt+Shift+R O-frame, Alt+Shift+S send, Alt+Shift+F focus Z / feeder, Alt+Shift+L save cut file, Alt+Shift+W wireframe toggle, Alt+Shift+O optimize, Alt+Shift+C cut shapes, Alt+L position laser, Alt+V paste in place, Alt+B break apart, Alt+D delete duplicates, Alt+O offset, Alt+P preview, Alt+I adjust image, Alt+T trace, Alt+J auto-join, Alt+M measure, Alt+X export, Ctrl+2 two-point rotate, Ctrl+Alt+N notes, Ctrl+Shift+A frame selection, Ctrl+0 zoom page, +/- zoom, PgUp/PgDn draw order, Tab cycles objects, Esc, Y/N in message boxes, numpad jogging (with Numlock), Alt+Ctrl+[ ] and Shift+Ctrl+[ ] jog, node-edit single-key commands (I/D/C/S/L/B/T/E), align keys L/R/T/B/E/C, H/V flip, `<`/`>` rotate, Space = pan.

### J.2 UI themes / appearance
Light Qt Fusion-style UI only (no dark UI theme in 1.4.04; only dark canvas background), toggle-switch widgets, colourised active tools, scalable icons & font size, high-DPI handling, "status tips" in status bar, tooltips with shortcuts, high-contrast dock title bars, vendor-branded builds (Darkly/Emblaser, Boss, Lotus, iLaser) with logos & extra menus.

### J.3 Languages (24 `.qm`)
ar, cs, da, de, el, en, es-ES, es-LA, fi, fr, hu, it, ja, ko, nl, no, pl, pt, ru, sl, sv, tr, zh-CN, zh-TW (Catalan mentioned in changelog but no file shipped). Language names shown natively; shortcuts reload on change; translated GRBL error messages.

### J.4 Licensing / trial behaviours — DO NOT replicate in DarkBurn
LexActivator-based licensing (`LexActivator.dll`): 30-day trial with extension, license tiers (GCode / DSP / Galvo) gating device types in the New Device list, online activation with proxy support, offline activation request/key files (`.ofr/.ofk`), deactivation, "all users" command-line license store, expiry date display & renewal flow, grace period when server unreachable, "time tampered" checks, floating/system-locked licenses, license entry character validation, trial identifier, "Buy LightBurn" button, usage data, daily update check. Also skip: vendor-branded help menus, Corel macro, Windows shell thumbnail handler (optional).

### J.5 Platform integration (nice-to-have parity)
Windows file association & single-instance launch, Explorer thumbnails, recent-files OS list, system vs internal file dialogs, silent installer flags, Debug Log, Generate Support Data, crash restore file, prefs auto-backup & corruption prevention, multiple instances sharing Art Library, `DefaultPrefs.ini` override, UDP command socket, Corel export macro, print support.

---

## K. Rough sizing for a clean-room Rust clone (1 = trivial, 5 = very large)

| Section | Effort | Why |
|---|---|---|
| A. Main window, menus, toolbars, docks, canvas | 4 | ~300 actions, 10 toolbars, 10 docks, rich canvas interaction (snaps, transforms, selection modes, rendering modes) |
| B. Devices — GCode family (GRBL/Smoothie/Marlin/LinuxCNC/SnapMaker) | 4 | streaming, buffer management, jog, overrides, machine settings parsers, many firmware quirks |
| B. Devices — Ruida/Trocen/TopWisdom DSP | 5 | three proprietary binary protocols, file management, machine settings, Bridge relay, UDP/USB packet transports (must be reverse-engineered independently or via published community specs) |
| B. Devices — Galvo/EZCad | 5 | USB board protocol, lens correction, timings, sources, rotary runner, repeat marking, 3D slice |
| C. File formats (lbrn2, SVG, AI, PDF, DXF, PLT, RD, images) | 5 | PDF/AI parsers alone are huge; DXF splines/blocks/MTEXT; SVG CSS/text; export paths |
| D. Drawing & editing tools | 5 | boolean/offset/weld geometry kernel with 1/4096 mm precision, node editor, arrays, warp/deform, slots, text engine incl. SHX & bend |
| E. Cut settings / layers / editor / material test | 3 | large but mostly data + forms; sub-layers and per-device visibility rules add complexity |
| F. Image engraving pipeline | 3 | 10 dither modes, adjustments, trace (potrace-equivalent), masking, pass-through, merged scans |
| G. Laser panel, preview/simulation, move, console, machine settings | 4 | motion-planner-based time estimate and simulation, Start-Here, overrides, framing variants |
| H. Camera | 4 | lens calibration (fisheye), alignment, head-camera stitching, overlay |
| H. Rotary / Print & Cut / Variable text / Library / Planner / misc | 3 each | planner (inner/outer, backlash, direction) is the hardest (4) |
| I. Settings & Device Settings | 2 | many fields, low algorithmic risk |
| J. Hotkeys, i18n (24 languages), platform integration | 2 | mechanical |

Approximate inventory counts: **88** distinct dialog/widget/wizard forms (`Ui_*`), **~300** menu/toolbar actions, **8** top-level menus, **10** toolbars, **10** dock panels, **~22** protocol/device families (plus ~40 named board/vendor profiles), **8** connection transports, **6** vector import formats + 6 raster + RD/SignCut, **4** vector export + 3 raster + 4 machine-file formats, **~15** auxiliary file types, **5** layer modes + sub-layers, **~60** per-layer settings (≈25 shared/line, ≈15 fill, ≈15 image, ≈20 galvo-only), **10** image modes, **13** optimization options, **24** languages, **30+2** palette layers.
