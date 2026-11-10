# Bug hunt notes

Known problems to fix in the final bug hunt. Remove a line when its fix is committed.

- [ ] LightBurn `.lbrn2` import: confirm bitmap and Y-axis orientation with a real file
- [ ] `ui_smoke_tests::menu_edit` failed once in a parallel run of the UI suite (panic in `open_menu`, ui_smoke_tests.rs:162) and passed in the next four runs; cause not found
- [ ] Rare: the UI suite crashed once with an access violation when several wgpu devices were created and dropped in parallel; the harness now serialises device creation and teardown (`ui_harness::GPU`). If it returns, run the UI tests with `--test-threads=1` and report the adapter (`ui_smoke_tests::the_app_starts_and_draws`)
- [ ] A shape inside a group cannot get tabs or a start point without ungrouping first (`tabs.add` and `shape.set_start_point` take top-level shapes only); tabs and start points set before grouping do work in the job
- [ ] Library: entries made by a command before the Library panel was first shown are dropped when the panel first draws (`library::show` loads the device library on its first frame and replaces what is there)
- [ ] Job: "Start here" keeps an operation number (`job.start_op`) from the preview; after the design is edited the same number is used on the new job (Start, Send and Save machine file), so the job starts at an unrelated move. It should be dropped, or checked against the document revision, when the project changes (`job_cmds.rs`, `machine_ui.rs`)
- [ ] Job: the kerf offset of a Line sub-layer moves the outline for every sub-layer of the layer, so a Fill sub-layer on the same layer fills the grown shape (`job_build::collect` hands the engine one path per shape)
- [ ] Job: after the last layer with a Z offset or Z step the head stays at that height when the job ends; the next job then starts from there. Not changed because what Z the job should end at needs a decision (absolute `G0 Z..` versus "relative Z moves only")
- [ ] Job: the time estimate does not know the device's "fast whitespace scan" (the speed is changed only when the G-code is written), so the estimate is too long with it on
- [ ] Job: power scale means two things: on line and fill shapes it is a share of max power, on images a share of the way from min to max power (`job_build::image_inputs`). Pick one
- [ ] Job: lead-in and lead-out are also added to open shapes (on the left of travel), though the settings describe them for closed shapes only (`darkburn-job/src/line.rs`)
- [ ] Job (Ruida): start and end pauses and cut-through (`Op::Dwell` outside dot mode) are dropped without a warning when a job is turned into a Ruida job (`machine_job::toolpath_to_rd_with`)
- [ ] Images: the Adjust Image preview (`images.rs`) and the job (`job_build.rs`) map the same settings differently: Sketch gain 6.0 against 1.5, noise reduction divided by 100 against 10 (the document says 0 to 10), brightness and contrast clamped in one only. What is burned is not what was previewed for those settings
- [ ] Images: scanning offsets are applied to image scans only; fill scans (Fill, Fill+Line) at the same speed are not shifted
- [ ] Galvo: a galvo job ignores per-shape power scale, cut order priority, kerf offset and start points (`galvo_job::build_job` hands the board whole layers)
