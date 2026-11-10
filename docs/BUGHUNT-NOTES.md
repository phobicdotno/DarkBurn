# Bug hunt notes

Known problems to fix in the final bug hunt. Remove a line when its fix is committed.

- [ ] Manual tab positions on shapes are not passed to the job engine
- [ ] LightBurn `.lbrn2` import: confirm bitmap and Y-axis orientation with a real file
- [ ] Per-shape start point is not in the document model (PLAN-13)
- [ ] Window-size problem: dock panels were missing at some forced window sizes in the real app (reported by the devices builder); confirm with the UI harness
- [ ] Widget ID clash warning "First/Second use of widget ID 6985" in the default layout
- [ ] About 19 text inputs and one slider have no accessible name (screen readers): failing ignored test `ui_smoke_tests::every_interactive_control_has_an_accessible_name`
- [ ] Rare: the UI suite crashed once with an access violation when several wgpu devices were created and dropped in parallel; the harness now serialises device creation and teardown (`ui_harness::GPU`). If it returns, run the UI tests with `--test-threads=1` and report the adapter (`ui_smoke_tests::the_app_starts_and_draws`)
- [ ] Escape does nothing in the Preview window while it is open (the canvas now leaves Escape to any open window; the Preview has no Escape handler of its own)
