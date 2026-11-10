# DarkBurn agent guide

DarkBurn is a laser cutter / engraver program (egui, Rust). These tools drive the running app: the
user watches the design change in the window. Everything the GUI can do is a **command**. With
`darkburn --mcp-headless` the same tools run without a window (see "No window").

## Units and axes

- Millimetres everywhere. Angles in degrees, counter-clockwise positive.
- **Y is up**, origin at the work area's lower-left corner for the default front-left origin
  (`inspect_ui` and `get_setting origin` tell). Screen coordinates are not used, except by
  `screenshot` regions (logical pixels of the window).
- Layer speeds are mm/s, power is percent 0..100, `interval` is the line spacing in mm.
- There are 32 layers: 0 to 29 are colour layers, 30 and 31 are the tool layers T1 and T2 (they
  never output). Shapes live on exactly one layer; the layer holds the cut settings.

## First steps

1. `skill_reference` (this guide), then `inspect_design {}`: the page size, shapes, the layers in
   use with every cut setting, the selection, notes, variable text and the undo stack.
2. Find commands with `list_commands {"filter": "align"}`. Never list the whole catalog (about 450
   commands): use `filter`, `prefix` (start of the id, for example `edit.`), `panel`, `compact`, `limit`.
3. `describe_command {"command": "<id>"}`: parameter JSON Schema, whether the command is enabled,
   whether it is hardware (`hardware_class`: connect, motion, config, fire_or_job, stop), whether it
   writes files, the checklist items it implements.
4. `execute {"command": "<id>", "params": {...}}`. Several commands: `batch`.
5. Check the result: `inspect_design`, `measure`, `screenshot`. `ok` alone proves little.

## Command groups

An id is `group.action`. Menu items, ribbon and toolbar buttons, tools, panel buttons, dialog actions
and settings are all commands; if you can click it, there is a command (find it by its label with
`list_commands {"filter": "<label>"}`).

| Group | What it holds |
|---|---|
| `file.*` | new, open, save, save as, import, export, print to PDF, G-code and LightBurn export, notes, preferences export and import, recent projects (`file.recent.N`), exit |
| `edit.*` | undo, redo, select all / invert / open shapes / by layer, cut, copy, paste, duplicate, delete, convert to path or bitmap, close and join paths, optimize, image crop and replace, settings, device settings |
| `tools.*` | the canvas tools (select, line, rectangle, ellipse, polygon, nodes, tabs, text, measure) and the shape tools: offset, weld, boolean union / difference / intersection, cut shapes, trace image, adjust image, mask image, warp, deform, start points, round corners, QR code |
| `arrange.*` | group, flip, mirror, rotate, align, distribute, dock, move together, same size, page and laser position, grid and circular arrays, copy along path, nest, break apart, order, lock |
| `shape.*` | add rectangle, ellipse, polygon, path, line; place, translate, resize, rotate, set layer, set properties, corner radius, start point, direction |
| `selection.*` | select shapes by id, clear |
| `layer.*` | make current, assign the selection, select a layer's shapes, mode, any cut setting (`layer.set_cut_settings`), order, delete |
| `tabs.*`, `node.*`, `measure.*` | tabs on outlines, node editing, measuring on the canvas |
| `text.*`, `variable.*` | text shapes, fonts, variable text (serial numbers, dates, merge); see `list_commands {"prefix": "text."}` |
| `qr.*`, `barcode.*` | QR codes and barcodes as shapes |
| `image.*` | adjust, trace, convert to bitmap, replace, save the processed bitmap, adjust presets, scanning offsets |
| `library.*`, `art.*` | the cut library (materials) and the art libraries |
| `project.*` | open, save, import, export and print by path; `project.info` summary |
| `settings.set`, `setting.set` | change one preference by key (`settings.set` takes any key from `list_settings`) |
| `hotkeys.*`, `help.*`, `support.*` | shortcuts, help topics, the support data bundle |
| `view.*`, `window.*`, `panel.*`, `toolbar.*`, `lang.*` | zoom and view, layout, which panels and toolbars show, the language, preview window |
| `camera.*`, `rotary.*`, `cylinder.*`, `pnc.*`, `test.*` | camera, rotary and cylinder setup, print and cut, focus / interval / centre tests |
| `job.*` | generate the job, G-code and simulation settings, optimization, job origin, user origin, material test, preview actions; `job.start`, `job.send`, `job.frame` and friends are hardware |
| `laser.*` | the Laser Tools menu: the document tools (`laser.rotary`, `laser.material_test`, print and cut) and the hardware ones (`laser.controller_info`, `laser.devices`) |
| `machine.*`, `device.*` | the connected machine (all hardware) and the device list (`device.list`, `device.select`, ...) |
| `gcode.*` | G-code dialects for Smoothieware, Marlin, Snapmaker, LinuxCNC and custom devices: settings and `gcode.export` (writes a file) |
| `galvo.*` | galvo (fibre, UV) devices: settings, lens correction files, red-light framing and repeat marking; actions that mark or arm are hardware |
| `gweike.*` | the Gweike M3 Ultra file workflow: layer to material and craft mapping, `.chf`, DXF and `.nc` export, the planner dry run and the card simulator. There is no command that connects to the real card |

The dedicated tools (`new_design`, `open`, `save`, `import`, `export`, `undo`, `redo`,
`set_setting`) are shortcuts for `file.new`, `file.open`, `file.save` / `file.save_as`,
`file.import`, `file.export`, `edit.undo`, `edit.redo`, `settings.set`.

## Addressing

- **Shapes** by stable id (`s12`), never by position: ids survive reordering and undo. The reply of
  `shape.add_*` carries the new id as a number (`{"id": 12}`); the shape is `s12`. `selection.set`
  takes numbers. `inspect_design {"path": "shapes"}` lists them, `{"path": "shapes.s12"}` shows one.
- **Layers** by index (`0` ... `31`) or name: `inspect_design {"path": "layers"}` (summary of all 32),
  `{"path": "layers.1"}` (every cut setting of layer 1, main and sub-layers). Commands take the index.
- Other design paths: `selection`, `notes`, `variables`, `bbox`, `page`, `undo`, `job`. Other tools:
  `inspect_ui` (dialogs, tool, panels, shortcuts; `path` digs in, for example `dialogs`),
  `list_settings` / `get_setting`, `machine_status`, `list_devices`.
- `depth` limits nesting; start with 1 or 2 on a big design.

## From an empty design to a G-code preview

Every call below is real; shapes are in mm, Y up.

```
new_design {}                                   (fails if the design has unsaved changes; use
                                                 execute file.new {"discard_changes": true})
batch {"steps": [
  {"command": "shape.add_rect",    "params": {"x": 10, "y": 10, "width": 60, "height": 40, "layer": 1}},
  {"command": "shape.add_ellipse", "params": {"x": 25, "y": 20, "width": 30, "height": 20, "layer": 2}},
  {"command": "layer.set_mode",    "params": {"layer": 1, "mode": "fill"}},
  {"command": "layer.set_param",   "params": {"layer": 1, "field": "speed", "value": 200}},
  {"command": "layer.set_param",   "params": {"layer": 1, "field": "max_power", "value": 35}},
  {"command": "layer.set_param",   "params": {"layer": 1, "field": "interval", "value": 0.1}},
  {"command": "layer.set_mode",    "params": {"layer": 2, "mode": "line"}},
  {"command": "layer.set_param",   "params": {"layer": 2, "field": "speed", "value": 20}},
  {"command": "layer.set_param",   "params": {"layer": 2, "field": "max_power", "value": 80}},
  {"command": "layer.set_param",   "params": {"layer": 2, "field": "passes", "value": 2}}
]}
inspect_design {"path": "layers.2"}              (mode "line", speed 20, max_power 80, passes 2)
measure {"target": "design"}                      (bbox, per-shape length and area)
preview_job {"format": "summary"}                 (moves, time, distances, warnings; nothing is sent)
preview_job {"format": "gcode", "max_lines": 40}  (the G-code text, capped)
screenshot {}
undo {"count": 3}
```

- `layer.set_param` fields: `name`, `speed`, `max_power`, `min_power`, `interval`, `passes`,
  `output`, `show`, `air`. Modes: `line`, `fill`, `fill_line`, `offset_fill`, `image`. Every other
  cut setting (kerf, tabs, overscan, ramps, lead-in, Z offset ...) is in
  `inspect_design {"path": "layers.N"}` and is changed with
  `layer.set_cut_settings {"layer": 1, "settings": {"kerf": 0.1, "overscan": 2}}` (optionally
  `sublayer`: 0 is the main settings, 1 and up the sub-layers). Unknown names and wrong types fail.
- `preview_job` builds the job on a worker thread (the call waits for it, up to `wait_ms`). It also
  takes `layers` to preview only some layers. `job.generate` builds without returning the toolpath.
  `job.save_gcode {"path": ...}` writes the G-code file (a file write, see below).

## Batches and undo

`batch {"steps": [...], "all_or_nothing": true}` runs in order. A failing step stops the batch; with
`all_or_nothing` (the default) the document is put back exactly as it was (`rolled_back: true`). Only
the design and selection are rolled back, not settings, the view or files already written. The safety
gate checks every step first: one denied step rejects the whole batch before anything runs.
`undo {"count": n}` / `redo`. `inspect_design {"path": "undo"}` lists the undo and redo step labels,
the next step first.

## Long operations

A job build or a window screenshot takes longer than a call should block. The reply `still running:`
carries an id; the work was **not cancelled**, do not send it again. `pending {"id": ...}` waits for
the result (`wait_ms`, default 10000). `running` lists what is in progress. `preview_job`, `measure`
with `what: ["time"]`, `job.generate` and `screenshot` wait for you; imports and traces run inside the
call.

## Machines and safety

- Anything that moves axes, fires the laser, sends a job, writes machine settings or controller
  files, or connects to a device is a **hardware command** (`hardware: true`): `machine.*`,
  `job.start`, `job.send`, `job.frame`, `job.home`, `job.run_gcode`, `tools.position_laser`,
  `arrange.laser_*`, `edit.device_settings`, `edit.machine_settings`, `laser.devices` and more.
  `describe_command` tells for any id. Changing the machine's setup counts too: selecting, adding,
  removing or importing a device, its connection, the start and end scripts and limits
  (`job.set_gcode_settings`) and the galvo settings.
- By default hardware commands are **denied**: only emergency stop, pause and stop work. The error
  says "ask the user". Do not try to work around it: ask the user to turn on "Allow AI agents to
  control machines" in DarkBurn (Settings > AI agents). `inspect_ui`/`machine_status` still show the
  machine's state, read only.
- With machine control on, starting a job or firing the laser (`job.start`, `job.send`,
  `machine.start`, `machine.fire`, `machine.file_run`, raw console lines) still needs the user's click
  on Confirm in a dialog: the call returns `confirmation_required` and a pending id. Tell the user,
  wait with `pending {"id": ...}`, which returns a `confirmation_token`, then repeat the call with
  `confirmation_token`. The token works once and only for the same command and arguments, and
  only while the design, the selection and the device are as they were when the user was asked:
  finish your edits first, then ask. `job.pause` only pauses; going on is `machine.resume`. A
  Cancel click answers `confirmation_denied`: do not retry unless the user asks.
- Gweike machines (1200 W fibre cutter) need a separate user opt-in ("Allow Gweike commands"). No
  command can bypass its height follower, piercing or gas logic.
- Writing files (`file.save`, `file.export`, `job.save_gcode`, ...) is allowed unless the user turned
  "Allow file writes" off. The file commands take a `path`; no dialog opens for an agent.
- You cannot change these switches: `settings.set` refuses `mcp.*`, and preferences imported from a
  file never carry them in.
- Every call is in the audit log (`events`, kind `audit`) and in the Settings page.

## No window

`darkburn --mcp-headless` serves the same tools over stdio without a window (for CI and scripts): the
document, settings, command registry, job preview and measuring all work. `screenshot` returns an
offscreen drawing of the design (page, shapes in layer colours; `region` is then in design mm),
not the GUI. Dialogs only set state (`inspect_ui` lists them). Hardware commands are denied; machines
are limited to the built-in simulators.

## Traps

- Y is up: a shape "above" another has a larger y.
- `shape.add_*` take the lower-left corner (`x`, `y`) and size; polygons take the centre. Commands that
  act on "the selection" (`shape.rotate`, `shape.place`, `arrange.*`) need a selection: new shapes are
  selected when added; `selection.set {"ids": [12]}` selects.
- Do not guess ids: `list_commands {"filter": ...}`. An unknown id fails without side effects.
- A disabled command (`enabled: false`) fails; `inspect_ui` / `inspect_design` show why (nothing
  selected, wrong tool, nothing to undo). A command not yet implemented says so.
- Commands that open a dialog leave it open for the user: `inspect_ui` lists `dialogs`, `events` says
  when one opened. Prefer the argument-taking forms (`project.*`, `file.*` with a `path`).
- `file.new` and `file.open` fail on unsaved changes unless `discard_changes` is true.
- Parameters are checked against the JSON Schema: unknown keys are errors, numbers are not strings.
- A setting that writes to the machine is a hardware command (`machine.write_setting`), not `settings.set`.
- `list_devices` only lists; it never opens a port. Connecting is `machine.connect`, a hardware command.
- `file.exit` closes the window (and the connection).
