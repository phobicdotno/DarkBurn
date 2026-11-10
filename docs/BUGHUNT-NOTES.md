# Bug hunt notes

Known problems to fix in the final bug hunt. Remove a line when its fix is committed.

- [ ] Flaky test: `darkburn-gcodebots` `marlin_emergency_stop_is_final_until_reconnect` (the alarm state can be read before the snapshot updates)
- [ ] LightBurn `.lbrn2` import: confirm bitmap and Y-axis orientation with a real file
- [ ] Flaky test: `darkburn-mcp` control `an_oversized_request_after_login_closes_only_that_connection` (the server closes the socket with unread data, so on Windows the reset can arrive before the client reads the error reply; fix with a lingering close: finish writing, shut down the write side, drain the input briefly, then close)
- [ ] Flaky test: `darkburn` `machine_api_tests::ruida_simulator_runs_a_job_with_progress` (half-read bed size under load; the job-settings builder says it is fixed, confirm over many runs)
- [ ] `ui_smoke_tests::menu_edit` failed once in a parallel run of the UI suite (panic in `open_menu`, ui_smoke_tests.rs:162) and passed in the next four runs; cause not found
- [ ] Flaky under load: `machine_api_tests::ruida_simulator_runs_a_job_with_progress` timed out once while the UI suite ran beside it (a timing wait, not a logic fault)
- [ ] Rare: the UI suite crashed once with an access violation when several wgpu devices were created and dropped in parallel; the harness now serialises device creation and teardown (`ui_harness::GPU`). If it returns, run the UI tests with `--test-threads=1` and report the adapter (`ui_smoke_tests::the_app_starts_and_draws`)
- [ ] `darkburn-gcodebots` device profile calls the tab pulse width `tab_pulse_width_ms`, but the value is a length in millimetres (the app and the job engine treat it as mm); rename the field and its file key with a fallback for old profiles
- [ ] A shape inside a group cannot get tabs or a start point without ungrouping first (`tabs.add` and `shape.set_start_point` take top-level shapes only); tabs and start points set before grouping do work in the job
