# DarkBurn agent guide

DarkBurn is a laser cutter / engraver program (egui, Rust). These tools drive the running app:
the user watches the design change in the window. Everything the GUI can do is a **command**.

## Units and axes

- Millimetres everywhere. Angles in degrees, counter-clockwise positive.
- **Y is up**, origin at the machine origin (the lower-left corner for most machines, see
  `inspect_ui` / `machine_status` for the origin setting). Screen coordinates are not used.
- Speeds are mm/s unless a command says mm/min; power is percent 0..100.

## First steps

1. `skill_reference` (this guide), then `inspect_design {}` to see what is open.
2. Find commands with `list_commands {"filter": "align"}`. Never list the whole catalog: use
   `filter`, `prefix` (start of the id, for example `edit.`), `panel`, `compact: true`, `limit`.
3. `describe_command {"command": "<id>"}` gives the parameter JSON Schema, the result schema, whether
   the command is enabled and whether it touches hardware.
4. `execute {"command": "<id>", "params": {...}}`. Several commands: `batch`.
5. Check the result: `inspect_design`, `measure`, `screenshot`. `ok` alone proves little.

## Command ids

Dotted, lower case: `file.new`, `file.open`, `file.save`, `file.save_as`, `file.import`,
`file.export`, `edit.undo`, `edit.redo`, `settings.set`, then the app's own groups (`shape.*`,
`edit.*`, `layer.*`, `arrange.*`, `laser.*`, `machine.*`, `job.*`, `view.*`). Menu items, ribbon and
toolbar buttons, tool actions, panel buttons, dialog buttons and every setting are commands. If you
can click it, there is a command; find it by its label with `list_commands {"filter": "<label>"}`.

The dedicated tools (`new_design`, `open`, `save`, `import`, `export`, `undo`, `redo`, `set_setting`)
are shortcuts for those commands.

## Addressing

- **Shapes** by stable id (`s12`), never by position in a list: ids survive reordering and undo.
  `inspect_design {"path": "shapes"}` lists them; `{"path": "shapes.s12"}` shows one.
- **Layers** by index (`0`, `1`, ...) or by name (`Cut`). `inspect_design {"path": "layers.Cut"}`.
- Other design paths: `selection`, `cut_settings`, `transforms`. Other roots: `inspect_ui`
  (panels, dialogs, focus), `list_settings` / `get_setting`, `machine_status`, `list_devices`.
- `depth` limits how deep a reply nests; start with 1 or 2 on a big design.

## Batches

`batch {"steps": [{"command": ..., "params": ...}, ...], "all_or_nothing": true}` runs in order. A
step that fails stops the batch; with `all_or_nothing` (the default) earlier steps are undone when
the app can (`rolled_back` says so). The safety gate checks every step first: one denied step
rejects the whole batch before anything runs. A step may use the result of an earlier one only by
you reading the reply and sending the next call (ids come back in the reply).

## Long operations

A command that takes long (tracing an image, generating a job) returns `still running:` with an id.
It was **not cancelled**; do not send it again. `pending {"id": ...}` waits for the result
(`wait_ms`, default 10000). `running` lists operations in progress.

## Machines and safety

- Anything that moves axes, fires the laser, sends a job, writes machine settings or controller
  files, or connects to a device is a **hardware command** (`hardware: true`).
- Hardware commands are **denied** until the user turns on "Allow AI agents to control machines" in
  DarkBurn's settings. The error tells you; ask the user, do not try to work around it.
- Emergency stop, pause and stop are always allowed.
- Starting a job or firing the laser needs the user to click Confirm in a dialog: the call returns
  `confirmation_required` and a pending id. Tell the user, wait with `pending {"id": ...}`, which
  returns a `confirmation_token`, then repeat the call with `confirmation_token`. The token works
  once and only for the same command and arguments.
- Gweike machines (1200 W fibre cutter) need a separate user opt-in. Never try to bypass its height
  follower, piercing or gas logic; no command exists for that.
- Writing files (`save`, `export`, machine files) needs "Allow AI agents to write files".
- Use `preview_job` to see the toolpath or G-code **without** sending anything.

## Worked examples

Draw two rectangles and check them:

```
batch {"steps": [
  {"command": "shape.add_rect", "params": {"x": 0, "y": 0, "width": 40, "height": 20, "layer": "Cut"}},
  {"command": "shape.add_rect", "params": {"x": 50, "y": 0, "width": 40, "height": 20, "layer": 1}}
]}
measure {}
```

Change a setting: `list_settings {"filter": "grid"}`, then `set_setting {"key": "grid.spacing", "value": 5}`.

Look at the result: `screenshot {}` (PNG of the window), `screenshot {"region": {"x":0,"y":0,"width":800,"height":600}}`.

Run a job (after the user enabled machine control):

```
preview_job {"format": "summary"}
execute {"command": "job.start"}                    -> confirmation_required, id c1a2b3c4
pending {"id": "c1a2b3c4"}                           -> confirmation_token (after the user clicked)
execute {"command": "job.start", "confirmation_token": "..."}
```

## Traps

- Y is up: a shape "above" another has a larger y.
- Do not guess ids: `list_commands {"filter": ...}`; an unknown id fails without side effects.
- A disabled command (`enabled: false`) fails; `inspect_ui` / `inspect_design` show why (no
  selection, wrong tool, no document).
- A command that opens a dialog may leave the dialog open: check `inspect_ui {"path": "dialogs"}` and
  the dialog's own commands.
- `events` shows what happened lately (command results, dialogs opened, errors, machine events and
  the audit log of your own calls).
- `new_design` and `open` replace the document; unsaved changes are lost unless the user saved.
- Parameters are checked against the JSON Schema: unknown keys are errors, numbers are not strings.
- A setting that writes to the machine is a hardware command (`machine.*`), not `settings.set`.
- `list_devices` only lists; it never opens a port. Connecting is `machine.connect`, a hardware command.
