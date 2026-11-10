# Bug hunt notes

Known problems to fix in the final bug hunt. Remove a line when its fix is committed.

- [ ] LightBurn `.lbrn2` import: confirm bitmap and Y-axis orientation with a real file
- [ ] `ui_smoke_tests::menu_edit` failed once in a parallel run of the UI suite (panic in `open_menu`, ui_smoke_tests.rs:162) and passed in the next four runs; cause not found
- [ ] Rare: the UI suite crashed once with an access violation when several wgpu devices were created and dropped in parallel; the harness now serialises device creation and teardown (`ui_harness::GPU`). If it returns, run the UI tests with `--test-threads=1` and report the adapter (`ui_smoke_tests::the_app_starts_and_draws`)
- [ ] `darkburn-gcodebots` device profile calls the tab pulse width `tab_pulse_width_ms`, but the value is a length in millimetres (the app and the job engine treat it as mm); rename the field and its file key with a fallback for old profiles
- [ ] A shape inside a group cannot get tabs or a start point without ungrouping first (`tabs.add` and `shape.set_start_point` take top-level shapes only); tabs and start points set before grouping do work in the job
- [ ] Art Library: a library with unsaved changes (the `*` after its name) is lost when DarkBurn closes, without a question, and the open art libraries are not opened again at the next start (`art_library.rs`: `Open::dirty` is only shown, never asked about or auto-saved)
- [ ] File > Open of artwork (DXF, SVG, PDF...) reads on a worker thread while the window stays usable; edits made while it reads are thrown away without a question when the file arrives (`file_io::finish_open` calls `open_loaded`, which starts a new project)
