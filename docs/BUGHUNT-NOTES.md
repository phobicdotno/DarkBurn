# Bug hunt notes

Known problems to fix in the final bug hunt. Remove a line when its fix is committed.

- [ ] LightBurn `.lbrn2` import: confirm bitmap and Y-axis orientation with a real file
- [ ] `ui_smoke_tests::menu_edit` failed once in a parallel run of the UI suite (panic in `open_menu`, ui_smoke_tests.rs:162) and passed in the next four runs; cause not found
- [ ] Rare: the UI suite crashed once with an access violation when several wgpu devices were created and dropped in parallel; the harness now serialises device creation and teardown (`ui_harness::GPU`). If it returns, run the UI tests with `--test-threads=1` and report the adapter (`ui_smoke_tests::the_app_starts_and_draws`)
- [ ] `darkburn-gcodebots` device profile calls the tab pulse width `tab_pulse_width_ms`, but the value is a length in millimetres (the app and the job engine treat it as mm); rename the field and its file key with a fallback for old profiles
- [ ] A shape inside a group cannot get tabs or a start point without ungrouping first (`tabs.add` and `shape.set_start_point` take top-level shapes only); tabs and start points set before grouping do work in the job
- [ ] Library: entries made by a command before the Library panel was first shown are dropped when the panel first draws (`library::show` loads the device library on its first frame and replaces what is there)
- [ ] LightBurn import: `crates/darkburn-import/src/lbrn.rs:353` slices `&s[i..i + 3]` after a `c`; when the next bytes are not ASCII the index is not a character boundary and the reader panics (seen in the output of `file_io::tests::damaged_files_of_every_kind_never_crash_the_reader`, which still passes)
- [ ] "Weld text automatically" (Settings, Text) only reaches text made by the `text.add` command; text typed with the text tool or made in the text dialog starts unwelded (`text_ui.rs` builds its `TextShape` without `opts.auto_weld`)
- [ ] Importing a preferences file brings that file's work area size and origin; the canvas then shows them while the job is still built for the selected device, until the next start (`file_io::import_prefs` does not go through `machine_ui::work_area_changed` or re-adopt the device). Not proven with a test
- [ ] `options::get()` and `keymap::key_for` read one snapshot per process, refreshed every frame by whichever window drew last: UI tests that change an option or a hotkey and then read it through the snapshot can see another test's values when the suite runs in parallel
- [ ] The node tool and the text tool read keys themselves (`node_tool::interact`, `text_ui`); whether they also act while a dialog is open over the canvas was not checked (the select tool's keys and the plain command shortcuts no longer do)
- [ ] The Move, Size and Rotate locks of the status bar stop mouse drags and arrow nudges, but not the X, Y, W, H and Rotate fields of the ribbon or the rotate and flip commands; unclear which is meant
- [ ] `cargo fmt --all` also walks into the SolidCraft path dependencies (`..\solidcraft\crates\...`): one run failed there with out of memory. `cargo fmt -p <crate>` stays inside DarkBurn
- [ ] `darkburn-grbl` `tests/driver.rs` `frame_rectangle_with_and_without_laser` failed once in a full workspace run under load (`e.stats().laser_switch_ons >= 1`, driver.rs:514) and passed three times in a row when run alone: it depends on timing
