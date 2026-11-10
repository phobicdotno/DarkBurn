# Bug hunt notes

Known problems to fix in the final bug hunt. Remove a line when its fix is committed.

- [ ] LightBurn `.lbrn2` import: confirm bitmap and Y-axis orientation with a real file
- [ ] `ui_smoke_tests::menu_edit` failed once in a parallel run of the UI suite (panic in `open_menu`, ui_smoke_tests.rs:162) and passed in the next four runs; cause not found
- [ ] Rare: the UI suite crashed once with an access violation when several wgpu devices were created and dropped in parallel; the harness now serialises device creation and teardown (`ui_harness::GPU`). If it returns, run the UI tests with `--test-threads=1` and report the adapter (`ui_smoke_tests::the_app_starts_and_draws`)
- [ ] `darkburn-gcodebots` device profile calls the tab pulse width `tab_pulse_width_ms`, but the value is a length in millimetres (the app and the job engine treat it as mm); rename the field and its file key with a fallback for old profiles
- [ ] A shape inside a group cannot get tabs or a start point without ungrouping first (`tabs.add` and `shape.set_start_point` take top-level shapes only); tabs and start points set before grouping do work in the job
- [ ] Library: entries made by a command before the Library panel was first shown are dropped when the panel first draws (`library::show` loads the device library on its first frame and replaces what is there)
- [ ] Ruida UDP: a datagram that is sent again because its acknowledge was late (not lost) reaches the controller twice; the protocol has no datagram number, so the driver cannot prevent it. Check on a real controller whether a repeated chunk of a file upload is dropped or stored twice
- [ ] Galvo: a `MoveZ` step sends `abs(mm)` (`machine.rs`, `Step::MoveZ`), so a move down and a move up by the same amount give the same Z word; whether the board takes an absolute position or needs a direction is not known from the sources
- [ ] GRBL: disconnecting from a controller that no longer answers blocks the caller for up to 6 s (the worker waits out two stop steps before it lets go); the caller is the UI thread
- [ ] GRBL: when a controller sends no start-up message after the stop reset ("No reset reply"), the lines counted as in flight are never cleared, so later commands can wait for buffer room that is already free
- [ ] GRBL through the library adapter (`darkburn-gcodebots` `GrblAdapter::set_air`): air assist is sent as a macro, so switching the air while the fire button is on now also switches the fire button off (the line machines keep it on)
- [ ] Ruida `move_to` / frame: a coordinate that is not a number passes the bed check and is sent as 0 (`mm_to_um(NaN)`); GRBL and the line machines now refuse such values
- [ ] MCC model: `stop_sequence` returns no writes at all when one of its builders fails (`?`), and `SimLink::stop_sequence` then sends nothing; with the default parameters no builder fails
