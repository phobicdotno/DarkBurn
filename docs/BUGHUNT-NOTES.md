# Bug hunt notes

Known problems to fix in the final bug hunt. Remove a line when its fix is committed.

- [ ] LightBurn `.lbrn2` import: confirm bitmap and Y-axis orientation with a real file
- [ ] `ui_smoke_tests::menu_edit` failed once in a parallel run of the UI suite (panic in `open_menu`, ui_smoke_tests.rs:162) and passed in the next four runs; cause not found
- [ ] Rare: the UI suite crashed once with an access violation when several wgpu devices were created and dropped in parallel; the harness now serialises device creation and teardown (`ui_harness::GPU`). If it returns, run the UI tests with `--test-threads=1` and report the adapter (`ui_smoke_tests::the_app_starts_and_draws`)
- [ ] A shape inside a group cannot get tabs or a start point without ungrouping first (`tabs.add` and `shape.set_start_point` take top-level shapes only); tabs and start points set before grouping do work in the job
- [ ] Art Library: a library with unsaved changes (the `*` after its name) is lost when DarkBurn closes, without a question, and the open art libraries are not opened again at the next start (`art_library.rs`: `Open::dirty` is only shown, never asked about or auto-saved)
- [ ] File > Open of artwork (DXF, SVG, PDF...) reads on a worker thread while the window stays usable; edits made while it reads are thrown away without a question when the file arrives (`file_io::finish_open` calls `open_loaded`, which starts a new project)
- [ ] A NaN coordinate in the document would be saved as `null` and make the project unopenable; no way to get one in was found, but save should refuse or clean it
- [ ] Deleting a layer with its shapes removes a whole group if any child of the group is on that layer
- [ ] "Same width/height" against a zero-size reference shape collapses the selected shape to nothing
- [ ] Camera slider changes are written to the document without marking it changed
- [ ] Job: the kerf offset of a Line sub-layer moves the outline for every sub-layer of the layer, so a Fill sub-layer on the same layer fills the grown shape (`job_build::collect` hands the engine one path per shape)
- [ ] Job: after the last layer with a Z offset or Z step the head stays at that height when the job ends; the next job then starts from there. Not changed because what Z the job should end at needs a decision (absolute `G0 Z..` versus "relative Z moves only")
- [ ] Job: the time estimate does not know the device's "fast whitespace scan" (the speed is changed only when the G-code is written), so the estimate is too long with it on
- [ ] Job: power scale means two things: on line and fill shapes it is a share of max power, on images a share of the way from min to max power (`job_build::image_inputs`). Pick one
- [ ] Job: lead-in and lead-out are also added to open shapes (on the left of travel), though the settings describe them for closed shapes only (`darkburn-job/src/line.rs`)
- [ ] Job (Ruida): start and end pauses and cut-through (`Op::Dwell` outside dot mode) are dropped without a warning when a job is turned into a Ruida job (`machine_job::toolpath_to_rd_with`)
- [ ] Images: the Adjust Image preview (`images.rs`) and the job (`job_build.rs`) map the same settings differently: Sketch gain 6.0 against 1.5, noise reduction divided by 100 against 10 (the document says 0 to 10), brightness and contrast clamped in one only. What is burned is not what was previewed for those settings
- [ ] Images: scanning offsets are applied to image scans only; fill scans (Fill, Fill+Line) at the same speed are not shifted
- [ ] Galvo: a galvo job ignores per-shape power scale, cut order priority, kerf offset and start points (`galvo_job::build_job` hands the board whole layers)
- [ ] Job: a saved G-code file is written in machine coordinates; run later (Run G-code) on a GRBL machine that has an origin set, it lands shifted by the work offset. A job started from the app is now shifted to work coordinates (`machine_job::request_for`), a file cannot know the offset
