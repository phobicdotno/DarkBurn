# Bug hunt notes

Known problems to fix in the final bug hunt. Remove a line when its fix is committed.

- [ ] LightBurn `.lbrn2` import: confirm bitmap and Y-axis orientation with a real file
- [ ] `ui_smoke_tests::menu_edit` failed once in a parallel run of the UI suite (panic in `open_menu`, ui_smoke_tests.rs:162) and passed in the next four runs; cause not found
- [ ] Rare: the UI suite crashed with an access violation inside `d3d10warp.dll` (the Windows software rasteriser, on one of its own worker threads) while some tests drew a screenshot and others made or destroyed their GPU device. `ui_harness::GPU` now serialises creation, teardown and drawing: 0 crashes in 87 loaded runs, against 3 in 120 with only creation and teardown serialised. It is a driver fault, so if it returns run the UI tests with `--test-threads=1`; the crash dumps are in `%LOCALAPPDATA%\CrashDumps`
- [ ] `darkburn-gcodebots` device profile calls the tab pulse width `tab_pulse_width_ms`, but the value is a length in millimetres (the app and the job engine treat it as mm); rename the field and its file key with a fallback for old profiles
- [ ] A shape inside a group cannot get tabs or a start point without ungrouping first (`tabs.add` and `shape.set_start_point` take top-level shapes only); tabs and start points set before grouping do work in the job
- [ ] Library: entries made by a command before the Library panel was first shown are dropped when the panel first draws (`library::show` loads the device library on its first frame and replaces what is there)
