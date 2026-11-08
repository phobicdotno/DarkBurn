# Bug hunt notes

Known problems to fix in the final bug hunt. Remove a line when its fix is committed.

- [ ] Flaky test: `darkburn-gcodebots` `marlin_emergency_stop_is_final_until_reconnect` (the alarm state can be read before the snapshot updates)
- [ ] Manual tab positions on shapes are not passed to the job engine
- [ ] LightBurn `.lbrn2` import: confirm bitmap and Y-axis orientation with a real file
- [ ] Per-shape start point is not in the document model (PLAN-13)
