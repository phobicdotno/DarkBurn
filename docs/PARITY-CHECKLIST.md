# DarkBurn parity checklist

Every feature, screen and menu item DarkBurn needs to match LightBurn 1.4.04.
Tick a box only when the item works in a build and has been tried by hand or by a test.

- `[ ]` not started, `[~]` partly working, `[x]` done
- IDs are stable. Commits that finish an item name its ID, for example `Done: FILE-03`.
- Source of the list: `docs/research/lightburn-feature-inventory.md`. It records *what* LightBurn does,
  never *how* it is built. Everything here is implemented fresh.
- Items marked *(later)* exist in newer LightBurn releases than 1.4.04. They are optional for parity.

**Progress:** 307 of 785 done (169 partly).

---

## 0. DarkBurn shell

- [x] SHELL-01 Cargo workspace, Rust + egui/eframe pinned to SolidCraft's versions
- [x] SHELL-02 SolidCraft theme crate applied (dark and light tokens)
- [x] SHELL-03 Widget kit copied from SolidCraft into a `darkburn-ui` crate, with attribution
- [x] SHELL-04 Frameless window with SolidCraft-style app bar doubling as title bar
- [x] SHELL-05 Windows resize border and taskbar icon
- [x] SHELL-06 Ribbon or toolbar strip in SolidCraft style
- [x] SHELL-07 Fixed left, right and bottom panel areas with tabbed panels
- [x] SHELL-08 Status bar
- [x] SHELL-09 Preferences saved to disk, with safe start after a crash
- [x] SHELL-10 IBM Plex fonts and SolidCraft icon style for all DarkBurn icons
- [x] SHELL-11 Theme picker (SolidCraft's bundled themes)
- [ ] SHELL-12 Windows installer (MSI) and app icon
- [ ] SHELL-13 Update check

---

## 1. Menus

### File
- [x] FILE-01 New
- [~] FILE-02 New window (second instance)
- [x] FILE-03 Recent projects list (24 entries)
- [x] FILE-04 Open
- [x] FILE-05 Import
- [x] FILE-06 Show project notes
- [x] FILE-07 Save
- [x] FILE-08 Save As
- [~] FILE-09 Save machine file (G-code, RD, OUT or UD5, label follows device)
- [x] FILE-10 Export
- [~] FILE-11 Print, black only
- [~] FILE-12 Print, keep colours
- [ ] FILE-13 Save processed bitmap
- [x] FILE-14 Save background camera capture
- [x] FILE-15 Open preferences folder
- [x] FILE-16 Export preferences
- [x] FILE-17 Import preferences
- [x] FILE-18 Load preferences backup (backup browser)
- [x] FILE-19 Edit hotkeys
- [x] FILE-20 Exit

### Edit
- [x] EDIT-01 Undo
- [x] EDIT-02 Redo
- [x] EDIT-03 Select all
- [x] EDIT-04 Invert selection
- [x] EDIT-05 Cut
- [x] EDIT-06 Copy
- [x] EDIT-07 Duplicate
- [x] EDIT-08 Paste
- [x] EDIT-09 Paste in place
- [x] EDIT-10 Delete
- [x] EDIT-11 Convert to path
- [ ] EDIT-12 Convert to bitmap
- [x] EDIT-13 Close path
- [~] EDIT-14 Close selected paths with tolerance
- [~] EDIT-15 Auto-join selected shapes
- [x] EDIT-16 Optimize selected shapes
- [x] EDIT-17 Delete duplicates
- [x] EDIT-18 Select open shapes
- [x] EDIT-19 Select open shapes set to fill
- [x] EDIT-20 Select all shapes in current layer
- [x] EDIT-21 Select contained shapes
- [ ] EDIT-22 Image options: replace image
- [ ] EDIT-23 Image options: replace image to fit
- [ ] EDIT-24 Image options: refresh image
- [ ] EDIT-25 Image options: apply crop path to image
- [ ] EDIT-26 Image options: flatten image crop
- [ ] EDIT-27 Image options: remove crop
- [ ] EDIT-28 Image options: remove backup path
- [~] EDIT-29 Settings
- [x] EDIT-30 Device settings
- [x] EDIT-31 Machine settings

### Tools
- [x] TOOLS-01 Select
- [x] TOOLS-02 Draw lines
- [x] TOOLS-03 Rectangle
- [x] TOOLS-04 Ellipse
- [x] TOOLS-05 Polygon
- [~] TOOLS-06 Edit nodes
- [~] TOOLS-07 Add tabs
- [~] TOOLS-08 Edit text
- [ ] TOOLS-09 Position laser
- [~] TOOLS-10 Measure
- [~] TOOLS-11 Create QR code
- [~] TOOLS-12 Offset shapes
- [x] TOOLS-13 Weld shapes
- [x] TOOLS-14 Boolean union
- [x] TOOLS-15 Boolean difference
- [x] TOOLS-16 Boolean intersection
- [~] TOOLS-17 Boolean assistant
- [x] TOOLS-18 Cut shapes
- [ ] TOOLS-19 Adjust image
- [ ] TOOLS-20 Trace image
- [x] TOOLS-21 Apply path to text
- [ ] TOOLS-22 Apply mask to image
- [~] TOOLS-23 Resize slots in selection
- [x] TOOLS-24 Select shapes smaller than selected
- [~] TOOLS-25 Warp selection (4 points)
- [~] TOOLS-26 Deform selection (16 points)

### Arrange
- [x] ARR-01 Group
- [x] ARR-02 Ungroup
- [x] ARR-03 Auto-group
- [x] ARR-04 Flip horizontal
- [x] ARR-05 Flip vertical
- [~] ARR-06 Mirror across line
- [x] ARR-07 Rotate 90° clockwise
- [x] ARR-08 Rotate 90° counter-clockwise
- [~] ARR-09 Two-point rotate and scale
- [x] ARR-10 Align left
- [x] ARR-11 Align right
- [x] ARR-12 Align top
- [x] ARR-13 Align bottom
- [x] ARR-14 Align horizontal centres
- [x] ARR-15 Align vertical centres
- [x] ARR-16 Align centres
- [x] ARR-17 Distribute horizontally
- [x] ARR-18 Distribute vertically
- [x] ARR-19 Distribute horizontal centres
- [x] ARR-20 Distribute vertical centres
- [x] ARR-21 Distribute horizontal spacing with padding
- [x] ARR-22 Distribute vertical spacing with padding
- [x] ARR-23 Move horizontally together
- [x] ARR-24 Move vertically together
- [~] ARR-25 Nest selected
- [~] ARR-26 Dock left, right, up, down
- [x] ARR-27 Move selection to one of 9 page positions
- [ ] ARR-28 Move selection to laser position
- [ ] ARR-29 Move laser to one of 9 selection positions
- [~] ARR-30 Grid array
- [~] ARR-31 Circular array
- [~] ARR-32 Copy along path
- [x] ARR-33 Create rubber-band outline
- [x] ARR-34 Break apart
- [x] ARR-35 Push forward in draw order
- [x] ARR-36 Push backward in draw order
- [x] ARR-37 Push to front
- [x] ARR-38 Push to back
- [x] ARR-39 Lock selected shapes
- [x] ARR-40 Unlock selected shapes
- [~] ARR-41 Virtual array: edit, add to, remove from, exit, flatten, duplicate

### Laser Tools
- [~] LT-01 Print and cut: set first target
- [~] LT-02 Print and cut: set second target
- [x] LT-03 Print and cut: align output to targets
- [x] LT-04 Print and cut: align without scaling
- [x] LT-05 Print and cut: reset
- [x] LT-06 Print and cut wizard
- [x] LT-07 Calibrate camera lens
- [x] LT-08 Calibrate camera alignment
- [x] LT-09 Rotary setup
- [x] LT-10 Focus test
- [x] LT-11 Interval test
- [x] LT-12 Material test
- [x] LT-13 Centre finder
- [ ] LT-14 Feeder setup
- [x] LT-15 Cylinder correction
- [x] LT-16 Get controller info
- [ ] LT-17 Repeat marking (galvo)
- [ ] LT-18 Lens correction test (galvo)
- [ ] LT-19 Material test, galvo variant

### Window
- [x] WIN-01 Reset to default layout
- [x] WIN-02 Preview
- [x] WIN-03 Zoom to page
- [x] WIN-04 Zoom in
- [x] WIN-05 Zoom out
- [x] WIN-06 Frame selection
- [x] WIN-07 View style: wireframe coarse, wireframe smooth, filled coarse, filled smooth
- [x] WIN-08 Toggle wireframe and filled
- [x] WIN-09 Show or hide each panel and toolbar

### Language
- [x] LANG-01 Language menu with names shown in their own language

### Help
- [x] HELP-01 About DarkBurn
- [x] HELP-02 Quick help and context help on F1
- [x] HELP-03 Online documentation link
- [x] HELP-04 Generate support data
- [~] HELP-05 Camera selection help
- [~] HELP-06 Check for updates
- [x] HELP-07 Enable debug log

---

## 2. Toolbars

- [x] TB-01 Main: new, open, save, import, undo, redo, cut, copy, paste, delete
- [x] TB-02 Main: pan view toggle, zoom to page, zoom in, zoom out, frame selection
- [ ] TB-03 Main: capture camera image, preview, settings, device settings
- [~] TB-04 Main can split in two on narrow screens
- [~] TB-05 Arrange: group, ungroup, flips, mirror, align, distribute
- [~] TB-06 Arrange: make same width, make same height, move selection pop-up
- [ ] TB-07 Arrange long variant
- [~] TB-08 Tools (vertical, left edge): all creation tools
- [~] TB-09 Modifiers: offset, weld, union, difference, intersection
- [~] TB-10 Modifiers: grid array, circular array
- [~] TB-11 Modifiers: start points tool, round corners tool with radius field
- [x] TB-12 Numeric edits: X, Y, width, height, scale %, lock aspect
- [x] TB-13 Numeric edits: 9-point anchor picker
- [x] TB-14 Numeric edits: rotate field, mm or inch toggle
- [x] TB-15 Numeric fields accept equations and unit suffixes
- [~] TB-16 Text options: font picker with recent and favourite fonts and live preview
- [~] TB-17 Text options: height, bold, italic, upper case, distort, welded
- [~] TB-18 Text options: letter spacing, line spacing, horizontal and vertical alignment
- [~] TB-19 Text options: variable text type, offset
- [x] TB-20 Cut palette: 30 colour layers and 2 tool layers
- [x] TB-21 Cut palette: optional number labels on buttons
- [x] TB-22 Cut palette: right-click flashes shapes, shift-click selects them
- [x] TB-23 Modes: rotary, print and cut, cylinder correction
- [~] TB-24 Docking: four directions, move as group, lock inner objects, padding
- [~] TB-25 All toolbars can be shown, hidden, docked and floated

## 3. Status bar

- [x] SB-01 Move, size, rotate and shear lock toggles
- [x] SB-02 Cursor position in current units
- [x] SB-03 Selection bounds and object count
- [x] SB-04 Help text for the control under the mouse
- [ ] SB-05 Layer and pass progress while a job runs
- [x] SB-06 Auto-save status

---

## 4. Panels

### Cuts / Layers
- [x] CUTS-01 Layer list with number, name, mode, speed/power, output, show, air columns
- [x] CUTS-02 Edit layer name in place
- [x] CUTS-03 Move layer up and down in priority, drag to reorder
- [x] CUTS-04 Delete layer; delete layer with its shapes
- [x] CUTS-05 Copy and paste cut settings between layers
- [x] CUTS-06 Header menu: show all, hide all, enable all, disable all, invert, air on/off/invert
- [x] CUTS-07 Sort cuts last
- [x] CUTS-08 Quick settings box under the list (speed, power, min power, interval, passes)
- [x] CUTS-09 Laser 2 quick settings when dual tube is on
- [x] CUTS-10 Double-click opens the cut settings editor
- [x] CUTS-11 Mouse wheel does not change mode

### Laser
- [x] LASER-01 Connection status: disconnected, ready, busy, framing, no laser
- [x] LASER-02 Job progress bar with elapsed and remaining time
- [x] LASER-03 Start, pause, stop
- [x] LASER-04 Frame as rectangle
- [x] LASER-05 Frame as rubber-band outline, optionally with laser on
- [~] LASER-06 Save machine file
- [x] LASER-07 Run a saved G-code file
- [x] LASER-08 Send file to controller, optionally start at once
- [x] LASER-09 Home
- [x] LASER-10 Go to origin
- [x] LASER-11 Start from: current position, user origin, absolute coordinates
- [x] LASER-12 Job origin 9-point picker
- [x] LASER-13 Cut selected graphics only
- [x] LASER-14 Use selection origin
- [x] LASER-15 Show last position
- [x] LASER-16 Optimize cut path toggle and optimization settings button
- [x] LASER-17 Devices button; right-click resets connection
- [x] LASER-18 Port picker with auto choice, remembered port, excluded ports
- [x] LASER-19 Device profile picker
- [~] LASER-20 Rescan network bridge
- [ ] LASER-21 Red light toggle (galvo)
- [x] LASER-22 Rotary and cylinder correction toggles, mode status text
- [x] LASER-23 Option to disable Start on machines that take uploads
- [x] LASER-24 Pre-job checklist before start
- [ ] LASER-25 Sounds on job finished and on error
- [ ] LASER-26 Job log saved as CSV
- [x] LASER-27 Out-of-bounds warning and option to drop out-of-bounds shapes
- [x] LASER-28 Warning when closing during a running job

### Library (materials)
- [x] LIB-01 Named library files: new, load, save, save as, auto-save, backup on save
- [x] LIB-02 Entries grouped by material, then thickness, then description
- [x] LIB-03 Thickness in fractions when using inches; entries with no thickness
- [~] LIB-04 Type icon for line, fill and image entries
- [x] LIB-05 Assign entry to layer
- [x] LIB-06 Link layer to entry; unlink when entry deleted
- [x] LIB-07 Create entry from layer
- [x] LIB-08 Edit cut, edit description, duplicate, delete
- [~] LIB-09 Library chosen per device

### Console
- [x] CON-01 Scrolling log and command line
- [x] CON-02 Show every sent G-code line option
- [x] CON-03 Six macro buttons, editable by right-click
- [x] CON-04 Multi-line macros sent line by line

### Move
- [x] MOVE-01 Jog arrows for X and Y, diagonals from numpad
- [x] MOVE-02 Z jog arrows and separate Z speed
- [~] MOVE-03 Rotary axis jog arrows
- [x] MOVE-04 Home button in the centre
- [x] MOVE-05 Jog distance and speed fields
- [x] MOVE-06 Continuous jog toggle
- [x] MOVE-07 Ctrl, Shift and Ctrl+Shift jog multipliers
- [x] MOVE-08 Get position readout for X, Y, Z, U
- [x] MOVE-09 Move to X, Y, Z position
- [x] MOVE-10 Saved positions list with editor (add, reorder, sort, Z on/off)
- [x] MOVE-11 Move from machine zero
- [~] MOVE-12 Focus Z
- [x] MOVE-13 Set and clear user origin
- [~] MOVE-14 Set finish position
- [x] MOVE-15 Fire button with power %
- [x] MOVE-16 Invert keypad direction
- [x] MOVE-17 Speed and power override sliders during a job (GRBL)
- [x] MOVE-18 Laser 2 controls

### Camera Control
- [~] CAMP-01 Camera picker
- [~] CAMP-02 Live viewfinder
- [x] CAMP-03 Update overlay
- [x] CAMP-04 Trace from capture
- [x] CAMP-05 Save settings
- [x] CAMP-06 Fade and show toggles
- [x] CAMP-07 Overlay width, height, X shift, Y shift

### Variable Text
- [~] VTP-01 Start, end, current, advance by
- [~] VTP-02 Previous and next
- [~] VTP-03 Auto-advance
- [~] VTP-04 Test, reset, bake
- [~] VTP-05 CSV file browse and clear

### Shape Properties
- [x] SP-01 Property grid for the selected shape
- [x] SP-02 Rectangle corner radius, including inward corners
- [x] SP-03 Polygon side count, ellipse size
- [x] SP-04 Text properties
- [x] SP-05 Image brightness, contrast, gamma, enhance amount, radius, denoise
- [x] SP-06 Cut order priority, power scale, locked, name
- [x] SP-07 Edit several shapes at once

### Art Library
- [x] ART-01 Several art library files open at once
- [x] ART-02 Folders and thumbnails, full-size preview on hover
- [x] ART-03 Search
- [x] ART-04 Drag in from the desktop; add selection
- [x] ART-05 Rename; replace on duplicate
- [~] ART-06 Shared between running instances

### File List (DSP controllers)
- [x] FL-01 List files on controller
- [x] FL-02 Upload, start, delete one or many, delete all
- [x] FL-03 Calculate job time (Ruida)

### Panel layout
- [x] PANEL-01 Default right-side tab groups as in LightBurn
- [x] PANEL-02 Move, Art Library and File List hidden by default
- [x] PANEL-03 Compact layout on small screens
- [~] PANEL-04 Panel layout saved per device

---

## 5. Canvas

- [x] CANVAS-01 X and Y rulers
- [x] CANVAS-02 Grid with finer grid when zoomed in; grid contrast setting
- [x] CANVAS-03 Dark background option
- [x] CANVAS-04 Work area centre cross
- [ ] CANVAS-05 Full-screen crosshair cursor option
- [x] CANVAS-06 Page outline sized from device; origin corner flips axes
- [x] CANVAS-07 Out-of-bounds and non-output shapes drawn faintly
- [x] CANVAS-08 Zoom by wheel, keys, zoom to page; option to invert wheel
- [~] CANVAS-09 Pan with space-drag, middle mouse, pan tool, auto-pan while dragging
- [x] CANVAS-10 Double middle click frames content
- [x] CANVAS-11 Click select, click inside filled shapes
- [x] CANVAS-12 Box select: left-to-right encloses, right-to-left crosses, colour coded
- [x] CANVAS-13 Tab and Shift+Tab cycle through objects
- [x] CANVAS-14 Esc clears selection and returns to select tool
- [~] CANVAS-15 Click tolerance setting; hidden and locked shapes not selectable
- [x] CANVAS-16 Move, size, rotate and shear handles
- [x] CANVAS-17 Rotation snapping with modifier keys
- [x] CANVAS-18 Arrow-key nudge with three step sizes plus a fine step
- [~] CANVAS-19 Keyboard rotate and flip shortcuts
- [x] CANVAS-20 Grid snap that adapts to zoom
- [x] CANVAS-21 Object snap to nodes, centres, midpoints, intersections, along lines and curves
- [x] CANVAS-22 Snap modifiers: wider snap, show unsnapped, axis lock
- [x] CANVAS-23 Right-click menu: cut, copy, paste, duplicate, select all, group, ungroup, preview, properties
- [x] CANVAS-24 Paste at cursor
- [x] CANVAS-25 Wireframe and filled rendering, coarse and smooth
- [x] CANVAS-26 Antialiasing and curve quality settings
- [x] CANVAS-27 Reduce motion setting
- [x] CANVAS-28 Hidden layer dims its images; flashing highlight of layer shapes
- [~] CANVAS-29 Drag and drop project files, images and device profiles
- [x] CANVAS-30 System clipboard with SVG text, bitmaps and plain text detection
- [x] CANVAS-31 Copy and paste between running instances
- [x] CANVAS-32 Auto-save on an interval and restore after a crash
- [x] CANVAS-33 Modified tracking and warning on newer-version files

---

## 6. Drawing tools

- [x] DRAW-01 Line tool: straight segments by click, curves by click-drag
- [x] DRAW-02 Line tool: 45° snap, length readout
- [~] DRAW-03 Line tool: connect to and extend existing shapes on the same layer
- [x] DRAW-04 Rectangle with corner radius, including inward corners
- [~] DRAW-05 Remove rounded corners
- [x] DRAW-06 Ellipse
- [x] DRAW-07 Polygon with N sides
- [~] DRAW-08 Text: multi-line, right-to-left, CJK input
- [~] DRAW-09 Text: double-click to edit, edit several text objects at once
- [~] DRAW-10 Text: edit text shapes dialog with zoom and pan
- [x] DRAW-11 Text on a path, with distort option
- [x] DRAW-12 Text: auto-weld, upper case, bold, italic
- [x] DRAW-13 Text: single-line SHX fonts
- [~] DRAW-14 Text: font favourites and recent fonts, default font and size
- [x] DRAW-15 Text: backup outline when a font is missing
- [x] DRAW-16 QR code with text, URL, Wi-Fi and contact editors
- [x] DRAW-17 QR code error correction level, size limit, editable later
- [~] DRAW-18 Measure: length, arc angle and length, area, width and height
- [~] DRAW-19 Measure: unit switch, remembers last position, axis snap
- [ ] DRAW-20 Position laser: click to move the head
- [x] DRAW-21 Barcode *(later)*
- [ ] DRAW-22 Tangent circle generator *(later)*

## 7. Node editing

- [~] NODE-01 Drag nodes and control handles
- [~] NODE-02 Drag a line to turn it into a curve
- [~] NODE-03 Ctrl+click inserts a node
- [~] NODE-04 Select several nodes
- [~] NODE-05 Keys: insert, delete point, delete segment, corner, smooth, line, break, trim, extend
- [~] NODE-06 Snap and join while dragging nodes
- [~] NODE-07 Constrained drag with Shift
- [~] NODE-08 Start node highlight
- [~] NODE-09 Align selected nodes
- [~] NODE-10 Start points tool: set start node and direction, show start and end markers
- [~] NODE-11 Start points work on compound shapes
- [~] NODE-12 Node highlight tool

## 8. Shape operations

- [x] SHAPE-01 Convert to path, including variable text
- [ ] SHAPE-02 Convert to bitmap with DPI dialog
- [x] SHAPE-03 Close path
- [~] SHAPE-04 Close path with tolerance dialog
- [~] SHAPE-05 Auto-join with tolerance, cancellable, safe at T-junctions
- [x] SHAPE-06 Optimize shapes: fit lines and arcs, smoothing, before and after counts
- [x] SHAPE-07 Delete duplicates
- [x] SHAPE-08 Delete overlapping lines
- [x] SHAPE-09 Break apart
- [x] SHAPE-10 Weld
- [x] SHAPE-11 Boolean union, difference and intersection with very fine precision
- [~] SHAPE-12 Boolean assistant with preview and reset
- [~] SHAPE-13 Offset: distance with units, inward, outward or both
- [~] SHAPE-14 Offset: round, bevel or sharp corners
- [~] SHAPE-15 Offset: delete original, select results, outer shapes only, open shapes
- [~] SHAPE-16 Offset: live update; zoom and pan while the dialog is open
- [x] SHAPE-17 Cut shapes, including through images
- [~] SHAPE-18 Trim in node mode
- [~] SHAPE-19 Round corners and fillet, including negative radius
- [~] SHAPE-20 Resize slots and tabs: width, depth, rectangle and round slots
- [~] SHAPE-21 Resize slots: colour-coded highlighting
- [~] SHAPE-22 Warp with 4 points
- [~] SHAPE-23 Deform with 16 points
- [~] SHAPE-24 Two-point rotate and scale
- [~] SHAPE-25 Mirror across line
- [x] SHAPE-26 Make same width and same height about the centre
- [x] SHAPE-27 Rubber-band outline
- [x] SHAPE-28 Apply path to text
- [ ] SHAPE-29 Apply mask to image, flatten and remove crop
- [ ] SHAPE-30 Lock and unlock shapes

## 9. Arrays and arrangement

- [~] ARRAY-01 Grid array by rows and columns or from target size
- [~] ARRAY-02 Grid array spacing by gap or centre to centre
- [~] ARRAY-03 Grid array alternate row or column shift, hex layout, shift by half
- [~] ARRAY-04 Grid array mirror alternates and reverse direction
- [~] ARRAY-05 Virtual arrays that stay editable
- [~] ARRAY-06 Variable text increments across arrays
- [~] ARRAY-07 Circular array: count, start angle, sweep, rotate shapes, direction
- [~] ARRAY-08 Copy along path: count or spacing, rotate shapes, start at nearest point
- [~] ARRAY-09 Nesting of selected shapes
- [ ] ARRAY-10 True nest and quick nest *(later)*
- [x] ARRAY-11 Auto-group groups shapes that sit inside each other
- [~] ARRAY-12 Alignment helper for beginners
- [~] ARRAY-13 Docking pushes selection against neighbours
- [x] ARRAY-14 9-point anchor used by position, size, scale and rotate fields
- [ ] ARRAY-15 Pages window *(later)*

---

## 10. Layers and cut settings

### Layer model
- [x] LAYER-01 30 colour layers and 2 tool layers that never output
- [~] LAYER-02 Tool layers can optionally be framed
- [x] LAYER-03 Per layer: name, mode, output, show, air, priority, speed, max power, min power, passes
- [x] LAYER-04 Sub-layers: up to 11 per layer with their own settings
- [x] LAYER-05 Sub-layers: add, duplicate, remove, reorder, enable, disable
- [x] LAYER-06 Layer list shows "Multi" when sub-layers exist
- [ ] LAYER-07 Default layer settings per device: make default, make default for all, reset
- [ ] LAYER-08 Load default layer settings on new project option
- [~] LAYER-09 Per shape overrides: cut order priority, power scale, start point and direction
- [~] LAYER-10 Layers linked to material library entries

### Modes
- [x] MODE-01 Line
- [x] MODE-02 Fill
- [x] MODE-03 Fill + Line
- [x] MODE-04 Offset fill
- [x] MODE-05 Image (bitmaps only)

### Shared settings
- [~] CUT-01 Speed in mm/s or mm/min
- [~] CUT-02 Max power and min power
- [~] CUT-03 Number of passes
- [x] CUT-04 Output on or off
- [~] CUT-05 Air assist, with DSP modes for by layer or with laser
- [~] CUT-06 Laser 1 and laser 2 choice and per-tube power
- [~] CUT-07 Constant power mode (GRBL M3)
- [x] CUT-08 Layer name
- [x] CUT-09 Warning for very high DPI
- [x] CUT-10 Equations in every number field

### Line mode
- [~] LINE-01 Z offset and Z step per pass
- [~] LINE-02 Kerf offset for closed shapes
- [~] LINE-03 Perforation mode with cut and skip lengths
- [~] LINE-04 Tabs: enable, even spacing with min and max count, tab size
- [~] LINE-05 Tabs: partly cut tabs with tab power, skip inner shapes
- [~] LINE-06 Tabs: manual placement with the add tabs tool
- [~] LINE-07 Start and end pause time
- [~] LINE-08 Cut-through start and end with pause power
- [~] LINE-09 Overcut distance
- [~] LINE-10 PWM frequency override (Ruida)
- [~] LINE-11 PPI mode (Trocen)
- [~] LINE-12 Lead in and lead out: length, angle, line or arc
- [~] LINE-13 Dot mode: time and spacing
- [~] LINE-14 U axis offset (Ruida)

### Fill mode
- [~] FILL-01 Bi-directional fill
- [~] FILL-02 Cross-hatch
- [~] FILL-03 Overscan as % or distance, with distance readout
- [~] FILL-04 Line interval and lines per inch, linked
- [~] FILL-05 Scan angle, with angle increase per pass
- [~] FILL-06 Fill all shapes at once, fill groups together, fill shapes one by one
- [~] FILL-07 Ramp length and ramp outer edge
- [~] FILL-08 Flood fill
- [~] FILL-09 Offset fill specifics: spiral path, power scale respected

### Image mode
- [~] IMGL-01 Image mode picker
- [~] IMGL-02 Line interval and DPI, linked
- [~] IMGL-03 Scan angle, bi-directional, overscan
- [~] IMGL-04 Negative image
- [~] IMGL-05 Pass-through (image is already dithered)
- [~] IMGL-06 Dot width correction
- [~] IMGL-07 Halftone cells per inch and angle
- [~] IMGL-08 Ramp length for threshold mode
- [~] IMGL-09 Images on one layer merged into one scan
- [~] IMGL-10 Min power for grayscale

### Cut settings editor dialog
- [x] CSE-01 Layer list on the left to edit several layers in one go
- [x] CSE-02 Common and Advanced tabs
- [x] CSE-03 Pictures that explain scan patterns
- [x] CSE-04 Dither sample preview
- [x] CSE-05 Beginner mode hides advanced settings
- [x] CSE-06 Reset to default
- [x] CSE-07 Shows when a layer is linked to the library
- [x] CSE-08 Z settings greyed out unless the device has Z

### Material test
- [x] MT-01 Grid of speed × power cells
- [x] MT-02 Alternative axes: interval, frequency, Q-pulse
- [~] MT-03 Min and max, rows and columns, labels and units
- [x] MT-04 Label layer settings
- [x] MT-05 Presets: save, load, import, export
- [~] MT-06 Frame the test before running
- [x] MT-07 Works on G-code, DSP and galvo devices

---

## 11. Image engraving

- [ ] IMG-01 Threshold
- [ ] IMG-02 Ordered dither
- [ ] IMG-03 Atkinson
- [ ] IMG-04 Floyd-Steinberg / Jarvis-Judice-Ninke dither
- [ ] IMG-05 Stucki
- [ ] IMG-06 Jarvis
- [ ] IMG-07 Newsprint
- [ ] IMG-08 Halftone
- [ ] IMG-09 Sketch
- [ ] IMG-10 Grayscale with power modulation
- [ ] IMG-11 Correct black and white end points and sRGB gamma
- [ ] IMG-12 Brightness, contrast, gamma
- [ ] IMG-13 Sharpen with amount, radius and denoise
- [ ] IMG-14 Transparent images handled in trace, dither and mask
- [ ] IMG-15 DPI read from file on import
- [ ] IMG-16 Adjust image dialog with side-by-side preview
- [ ] IMG-17 Adjust image presets: save, load, import, export
- [ ] IMG-18 Trace image: threshold, lower cutoff, ignore small, smoothness, optimize
- [ ] IMG-19 Trace image: fade, show points, clip rectangle, sketch trace, delete image after
- [ ] IMG-20 Trace from camera capture
- [ ] IMG-21 Multi-file trace *(later)*
- [ ] IMG-22 Fast skip of blank areas in G-code
- [ ] IMG-23 Scanning offset (backlash) table, import and export
- [ ] IMG-24 Angled image scans
- [ ] IMG-25 3D model to depth map *(later)*

---

## 12. Preview and simulation

- [x] PREV-01 Play, pause, slider and playback speed
- [x] PREV-02 Shade by power
- [x] PREV-03 Show travel moves
- [x] PREV-04 Invert view
- [x] PREV-05 Cut distance, travel distance and estimated time
- [x] PREV-06 Time estimate includes passes, overscan, tabs, lead-in and acceleration
- [x] PREV-07 Zoom, pan, maximise
- [x] PREV-08 Save preview image, optionally transparent
- [~] PREV-09 Start here: run, send or save from any point in the job
- [x] PREV-10 Layer and pass readout
- [ ] PREV-11 Read controller acceleration and speeds to make the estimate accurate

## 13. Job origin and coordinates

- [x] ORIG-01 Absolute coordinates
- [x] ORIG-02 Current position
- [~] ORIG-03 User origin: set and clear
- [x] ORIG-04 Job origin at 9 positions
- [x] ORIG-05 Use selection origin
- [ ] ORIG-06 Red-dot pointer offset
- [~] ORIG-07 Machine origin corner per device
- [ ] ORIG-08 Negative-coordinate (CNC) machine mode
- [ ] ORIG-09 Swap X and Y
- [~] ORIG-10 Return to finish position

## 14. Cut planner

- [x] PLAN-01 Optimize cut path on or off
- [x] PLAN-02 Order by layer, group and priority, sortable
- [x] PLAN-03 Cut inner shapes first
- [x] PLAN-04 Cut in direction order
- [x] PLAN-05 Reduce travel moves
- [x] PLAN-06 Hide backlash
- [x] PLAN-07 Reduce direction changes
- [x] PLAN-08 Choose best starting point, prefer corners
- [x] PLAN-09 Choose best direction
- [x] PLAN-10 Remove overlapping lines with distance
- [x] PLAN-11 Set as default and reset
- [x] PLAN-12 Saved in the project
- [~] PLAN-13 Manual cut order and forced start points respected
- [x] PLAN-14 Progress and cancel

---

## 15. Devices

### Device management
- [x] DEV-01 Devices dialog: list, drag reorder, sort by name or type
- [~] DEV-02 Find my laser: auto-detect serial, USB and network machines
- [x] DEV-03 New device wizard: type, connection, name and size, origin and homing, summary
- [x] DEV-04 Edit, duplicate, remove device
- [x] DEV-05 Import and export device profiles
- [ ] DEV-06 Network bridge setup wizard
- [~] DEV-07 Per device: libraries, units, grid, origin, panel layout, default layers, port, settings backups
- [x] DEV-08 Design-only device with no laser

### Connections
- [ ] CONN-01 USB serial with baud choice
- [ ] CONN-02 TCP/IP
- [ ] CONN-03 TCP relay through a network bridge
- [ ] CONN-04 UDP (Ruida Ethernet)
- [ ] CONN-05 Raw USB
- [ ] CONN-06 FTDI packet USB (Ruida over USB)
- [ ] CONN-07 HTTP
- [ ] CONN-08 Bluetooth serial
- [ ] CONN-09 UDP automation socket for start and status commands

### G-code family
- [ ] GRBL-01 GRBL 1.1 streaming with buffer tracking
- [ ] GRBL-02 Older GRBL (M3 only)
- [ ] GRBL-03 GRBL-LPC
- [ ] GRBL-04 grblHAL
- [ ] GRBL-05 FluidNC
- [x] GRBL-06 $J jogging and continuous jog
- [x] GRBL-07 Settings editor for $$ values, with backup
- [x] GRBL-08 Alarm and error codes shown in plain words
- [x] GRBL-09 Feed and power overrides while running
- [x] GRBL-10 Fire button
- [~] GRBL-11 DTR control
- [ ] GRBL-12 Auto-detect common diode lasers (xTool, Ortur, Atomstack, Sculpfun)
- [ ] GCODE-01 Smoothieware, including fast raster clustering
- [ ] GCODE-02 Marlin with M3, M106 or inline power
- [ ] GCODE-03 LinuxCNC
- [ ] GCODE-04 Snapmaker
- [ ] GCODE-05 Custom G-code device *(later)*

### DSP family
- [ ] RUIDA-01 Ruida over Ethernet (UDP)
- [ ] RUIDA-02 Ruida over USB
- [x] RUIDA-03 Start, pause, stop, frame, home, go to origin
- [x] RUIDA-04 Jog, including continuous jog
- [x] RUIDA-05 Send file with name, auto-increment name, overwrite prompt
- [x] RUIDA-06 File list on controller: upload, delete, run
- [x] RUIDA-07 Read and write machine settings, with backup
- [x] RUIDA-08 Controller info: job time, laser-on time
- [x] RUIDA-09 Dual tube and laser 2 offset
- [x] RUIDA-10 U axis, rotary read and write
- [ ] RUIDA-11 Job start and end delays
- [ ] RUIDA-12 Hardware overscan, corner power, dot mode, PWM override
- [ ] RUIDA-13 RD file save and import
- [ ] RUIDA-14 Known board list (644x, 654x, 658x, 5121, 320A, 420B, 6332G, KT332N)
- [ ] TROCEN-01 Trocen / AWC over USB and Ethernet, UD5 files
- [ ] TROCEN-02 Trocen dual tube, PPI, auto-feed, overcut
- [ ] TOPW-01 TopWisdom over USB and Ethernet, OUT files
- [ ] TOPW-02 TopWisdom machine settings, framing, overcut

### Galvo (fiber, UV, CO2 galvo)
- [ ] GALVO-01 EZCad2-style boards over USB
- [ ] GALVO-02 Laser source types: fiber, MOPA, UV, CO2
- [ ] GALVO-03 Lens correction file load and clear
- [ ] GALVO-04 Galvo scale, skew, bulge and trapezoid calibration
- [ ] GALVO-05 Field size and offset
- [ ] GALVO-06 Frequency and Q-pulse
- [ ] GALVO-07 Timing settings: jump speed and delays, laser on and off, end and polygon delays
- [ ] GALVO-08 Wobble
- [ ] GALVO-09 Red light framing with nudge and scale
- [ ] GALVO-10 Require framing before start
- [ ] GALVO-11 Door switch and foot switch inputs
- [ ] GALVO-12 Done-marking output signal
- [ ] GALVO-13 Repeat marking
- [ ] GALVO-14 Rotary runner by slice or by shape
- [ ] GALVO-15 External axis setup
- [ ] GALVO-16 3D sliced image
- [ ] GALVO-17 Lens correction test and scale calibration
- [ ] GALVO-18 Import EZCad config
- [ ] GALVO-19 Drill mode

### User's machines (acceptance targets)
- [ ] TARGET-01 Atomstack 400×400 mm, GRBL over USB serial at 115200 baud, end to end
- [ ] TARGET-02 A Ruida laser over Ethernet, end to end (no Ruida machine on hand yet; test against an emulator)
- [ ] TARGET-03 Gweike M3 Ultra, end to end through the steps in section 27

---

## 16. File formats

### Project
- [x] PROJ-01 DarkBurn project format
- [x] PROJ-02 Open LightBurn `.lbrn2` projects
- [x] PROJ-03 Open LightBurn `.lbrn` (older XML) projects
- [x] PROJ-04 Embedded images, cut settings, planner settings, notes, thumbnail
- [~] PROJ-05 Variable text state, virtual arrays, backup outlines for missing fonts
- [x] PROJ-06 Save in `.lbrn2` format for round trips *(decide)*

### Import
- [~] IMP-01 SVG: styles, fonts, symbols, text, clip paths, embedded images
- [x] IMP-02 SVG: 72 or 96 DPI setting
- [~] IMP-03 Adobe Illustrator files
- [x] IMP-04 PDF with page choice and embedded images
- [x] IMP-05 DXF: units, splines, blocks, text, layers, colours
- [x] IMP-06 PLT / HPGL
- [ ] IMP-07 Ruida RD files
- [ ] IMP-08 SignCut files
- [x] IMP-09 PNG, JPG, BMP, GIF, TIFF, WebP
- [x] IMP-10 Colour to layer mapping
- [x] IMP-11 Group and select imported shapes options
- [x] IMP-12 Import to tool layers option
- [x] IMP-13 Several files at once; drag and drop

### Export
- [x] EXP-01 SVG
- [x] EXP-02 Adobe Illustrator
- [x] EXP-03 DXF with arcs or segments
- [x] EXP-04 PNG, JPG, BMP when the selection is a bitmap
- [x] EXP-05 Export selection or everything
- [x] EXP-06 G-code file (.gc, .gcode, .nc, .txt)
- [ ] EXP-07 RD, OUT and UD5 machine files

### Other files
- [ ] AUX-01 Material library files
- [ ] AUX-02 Art library files
- [ ] AUX-03 Device profile files
- [x] AUX-04 Preferences files
- [ ] AUX-05 Machine settings backup files
- [ ] AUX-06 Camera calibration files
- [ ] AUX-07 Scanning offset files
- [x] AUX-08 Hotkey files
- [ ] AUX-09 Galvo lens correction files
- [x] AUX-10 CSV and text files for variable text
- [ ] AUX-11 SHX fonts
- [ ] AUX-12 Import LightBurn library, art, device and hotkey files *(decide)*

---

## 17. Camera

- [~] CAM-01 Camera selection with exposure and brightness
- [x] CAM-02 Overhead or head-mounted camera type
- [x] CAM-03 Lens calibration wizard, standard and fisheye
- [x] CAM-04 Camera alignment wizard
- [~] CAM-05 Head-mounted camera alignment and image stitching
- [x] CAM-06 Overlay on the canvas
- [x] CAM-07 Camera settings import and export
- [x] CAM-08 Colour or black-and-white view
- [x] CAM-09 Remember last camera, crop to page

## 18. Rotary

- [x] ROT-01 Enable rotary
- [x] ROT-02 Chuck or roller type
- [x] ROT-03 Rotary axis choice
- [x] ROT-04 Steps per rotation, roller diameter, object diameter or circumference
- [x] ROT-05 Mirror output
- [~] ROT-06 Test rotation
- [ ] ROT-07 Read settings from controller (Ruida)
- [x] ROT-08 Rotary toggle on the main window

## 19. Print and cut

- [~] PNC-01 Two target positions
- [x] PNC-02 Align with scaling
- [x] PNC-03 Align without scaling
- [x] PNC-04 Reset
- [x] PNC-05 Wizard
- [x] PNC-06 Status shown on main window

## 20. Variable text

- [x] VT-01 Date and time with format codes
- [x] VT-02 Serial numbers
- [x] VT-03 CSV merge with quoted and multi-line fields
- [x] VT-04 Cut setting values in text
- [x] VT-05 Auto-advance after a job
- [x] VT-06 QR codes follow the current index
- [x] VT-07 Works inside virtual arrays

## 21. Other tools

- [x] OTHER-01 Focus test
- [x] OTHER-02 Interval test
- [x] OTHER-03 Centre finder
- [x] OTHER-04 Feeder setup
- [x] OTHER-05 Cylinder correction, including concave
- [x] OTHER-06 Pre-job checklist editor
- [ ] OTHER-07 Project notes dialog shown on open
- [ ] OTHER-08 Hotkey editor
- [x] OTHER-09 Beginner mode
- [ ] OTHER-10 Taper warp *(later)*

---

## 22. Settings

### Display and units
- [x] SET-01 Units mm or inch; speed in mm/s or mm/min
- [x] SET-02 Curve quality, antialiasing, filled rendering, reduce motion
- [x] SET-03 Font size and toolbar icon size
- [x] SET-04 Cursor and node size
- [~] SET-05 Dark background, grid contrast, grid spacing
- [x] SET-06 Grid snap distance, object snap distance, snapping on or off
- [x] SET-07 Click tolerance
- [x] SET-08 Full-screen cursor, centre cross, palette labels
- [x] SET-09 Show rotary and cylinder toggles on main window
- [x] SET-10 Invert mouse wheel zoom

### Editor
- [x] SET-11 Beginner mode
- [x] SET-12 Auto-save interval
- [~] SET-13 Check for updates
- [~] SET-14 Open files in the running copy
- [~] SET-15 Default states for cut selected and use selection origin
- [~] SET-16 Save job origin and cut selected in the project
- [x] SET-17 Remember move, size, rotate and shear lock states
- [~] SET-18 Ignore out-of-bounds shapes
- [~] SET-19 Ignore Start when the monitor is off
- [~] SET-20 Load default layers on new project
- [~] SET-21 Built-in or system file dialogs
- [~] SET-22 Sounds on error and on job finished
- [~] SET-23 Save job log
- [x] SET-24 Use system clipboard
- [x] SET-25 Nudge distances for normal, Ctrl and Shift
- [~] SET-26 Default font, height, auto-weld
- [x] SET-27 Shared default preferences file for makerspaces
- [~] SET-28 Excluded serial ports

### Files
- [x] SET-29 SVG DPI
- [x] SET-30 DXF import units, fallback units, auto-close tolerance
- [x] SET-31 DXF export arcs or lines
- [x] SET-32 HPGL precision
- [x] SET-33 Import hidden Illustrator layers
- [x] SET-34 Group and select imported shapes
- [x] SET-35 Allow import to tool layers
- [~] SET-36 Output curve tolerance

### Camera
- [~] SET-37 Capture system, resolution, colour view

## 23. Device settings

- [x] DS-01 Work area size and origin corner
- [~] DS-02 Negative-coordinate machine mode
- [~] DS-03 Red-dot pointer offset
- [~] DS-04 Z: enable, reverse, relative moves only, optimize moves
- [~] DS-05 Tab pulse width
- [x] DS-06 Job checklist on and editor
- [x] DS-07 Overwrite files by default
- [~] DS-08 Scanning offset table
- [~] DS-09 G-code: auto-home on start, swap X/Y
- [~] DS-10 G-code: fast whitespace scan and its speed
- [x] DS-11 G-code: $J jogging, DTR, fire button, laser on when framing
- [~] DS-12 G-code: air assist commands, S-value max, baud rate
- [x] DS-13 G-code: buffered or synchronous transfer
- [~] DS-14 G-code: start and end scripts
- [~] DS-15 G-code: G0 for overscan, S with every G1
- [x] DS-16 DSP: U axis, laser 2, filename increment, disable start
- [~] DS-17 DSP: check bounds when framing, network timeout, connection type and IP
- [ ] DS-18 Galvo: all galvo settings from section 15
- [~] DS-19 Simulation: acceleration and speed values for the time estimate
- [ ] DS-20 Frame continuously *(later)*

## 24. Keyboard

- [~] KEY-01 Every default shortcut from the menus above
- [ ] KEY-02 Laser shortcuts: start, frame, outline frame, send, pause, stop
- [ ] KEY-03 Numpad jogging
- [ ] KEY-04 Node edit single-key commands
- [ ] KEY-05 Align single-key commands
- [x] KEY-06 Space to pan, Tab to cycle, Esc to cancel
- [x] KEY-07 F1 context help
- [x] KEY-08 All shortcuts remappable

## 25. Languages

- [x] I18N-01 Translation system
- [x] I18N-02 English
- [~] I18N-03 Norwegian
- [ ] I18N-04 The other 22 LightBurn languages
- [ ] I18N-05 Machine error messages translated

## 26. Platform

- [~] PLAT-01 Windows file association and single running copy
- [ ] PLAT-02 Explorer thumbnails for project files
- [ ] PLAT-03 Recent files in the Windows jump list
- [x] PLAT-04 Debug log
- [x] PLAT-05 Support data bundle
- [~] PLAT-06 Crash restore file
- [x] PLAT-07 Automatic preference backups
- [~] PLAT-08 Print support
- [ ] PLAT-09 macOS build
- [ ] PLAT-10 Linux build
- [ ] PLAT-11 REST API *(later)*

---

## 27. Beyond LightBurn: Gweike M3 Ultra

The M3 Ultra is a fibre plus CO2 flatbed run by an Au3tech MCC100 controller card over Ethernet.
LightBurn cannot drive it. DarkBurn reaches it in safe steps, file export first.

- [x] MCC-01 Export DXF with one layer per material, ready for Gweike's own software
- [ ] MCC-02 Export the vendor's craft file format
- [~] MCC-03 Export the G-code flavour the vendor software imports
- [ ] MCC-04 Fibre cut settings that LightBurn lacks: assist gas, pierce stages, follower height
- [ ] MCC-05 Controller card simulator so the backend can be tested without the machine
- [ ] MCC-06 Native Ethernet backend, built on the protocol work in phobicMlaserLinux, after it is confirmed on the real card
- [ ] MCC-07 Safety: the machine's height follower, piercing and gas logic stay in charge and are never bypassed

---

## Not copied on purpose

These exist in LightBurn and DarkBurn leaves them out.

- Trial periods, licence keys, activation, licence tiers that lock device types, and tamper checks
- Vendor-branded menus and help links for specific laser brands
- Usage data reporting
- Developer debug items left in LightBurn's Edit menu
- The CorelDRAW export macro
