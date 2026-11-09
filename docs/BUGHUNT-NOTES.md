# Bug hunt notes

Known problems to fix in the final bug hunt. Remove a line when its fix is committed.

- [ ] Flaky test: `darkburn-gcodebots` `marlin_emergency_stop_is_final_until_reconnect` (the alarm state can be read before the snapshot updates)
- [ ] Manual tab positions on shapes are not passed to the job engine
- [ ] LightBurn `.lbrn2` import: confirm bitmap and Y-axis orientation with a real file
- [ ] Per-shape start point is not in the document model (PLAN-13)
- [ ] Flaky test: `darkburn-mcp` control `an_oversized_request_after_login_closes_only_that_connection` (the server closes the socket with unread data, so on Windows the reset can arrive before the client reads the error reply; fix with a lingering close: finish writing, shut down the write side, drain the input briefly, then close)
- [ ] Flaky test: `darkburn` `machine_api_tests::ruida_simulator_runs_a_job_with_progress` (half-read bed size under load; the job-settings builder says it is fixed, confirm over many runs)
- [ ] Window-size problem: dock panels were missing at some forced window sizes in the real app (reported by the devices builder); confirm with the UI harness
- [ ] Widget ID clash warning "First/Second use of widget ID 6985" in the default layout
- [ ] About 19 text inputs and one slider have no accessible name (screen readers): failing ignored test `ui_smoke_tests::every_interactive_control_has_an_accessible_name`
- [ ] Flaky under load: `machine_api_tests::ruida_simulator_runs_a_job_with_progress` timed out once while the UI suite ran beside it (a timing wait, not a logic fault)
- [ ] Rare: the UI suite crashed once with an access violation when several wgpu devices were created and dropped in parallel; the harness now serialises device creation and teardown (`ui_harness::GPU`). If it returns, run the UI tests with `--test-threads=1` and report the adapter (`ui_smoke_tests::the_app_starts_and_draws`)
- [ ] Escape does nothing in the Preview window while it is open (the canvas now leaves Escape to any open window; the Preview has no Escape handler of its own)
