# Bug hunt notes

Known problems to fix in the final bug hunt. Remove a line when its fix is committed.

- [ ] LightBurn `.lbrn2` import: confirm bitmap and Y-axis orientation with a real file
- [ ] Rare: the UI suite crashed with an access violation inside `d3d10warp.dll` (the Windows software rasteriser, on one of its own worker threads) while some tests drew a screenshot and others made or destroyed their GPU device. `ui_harness::GPU` now serialises creation, teardown and drawing: 0 crashes in 87 loaded runs, against 3 in 120 with only creation and teardown serialised. It is a driver fault, so if it returns run the UI tests with `--test-threads=1`; the crash dumps are in `%LOCALAPPDATA%\CrashDumps`
- [ ] A shape inside a group cannot get tabs or a start point without ungrouping first (`tabs.add` and `shape.set_start_point` take top-level shapes only); tabs and start points set before grouping do work in the job
- [ ] Art Library: a library with unsaved changes (the `*` after its name) is lost when DarkBurn closes, without a question, and the open art libraries are not opened again at the next start (`art_library.rs`: `Open::dirty` is only shown, never asked about or auto-saved)
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
- [ ] Offset with the sharp join: a corner past the miter limit is clipped at the limit, not bevelled as `offset.rs` says, and a corner sharper than about 5 degrees stops well short of a true miter (the overlay engine's own limit); the result is a valid outline, only the corner differs
- [ ] Kerf offset: a hole smaller than the kerf vanishes from the job without a warning (`kerf_offset` returns nothing for it)
- [ ] A manual tab longer than the whole outline of a small shape leaves nothing of that shape to cut (`tabs::place_tabs_at`); automatic tabs reduce their count instead
- [ ] `edit.convert_path` on a text with no visible characters leaves an empty path shape in the document
- [ ] Booleans and offsets read a text's stored outline; a text that was never rendered would give its placeholder rectangle (not seen in a test: the text commands render after every change)
- [ ] Adjust Image is slow at the limits: forty mode and DPI changes on a 2 x 1 mm image at 25400 DPI with 1000 halftone cells took over a minute in a test (results were right)
- [ ] "Weld text automatically" (Settings, Text) only reaches text made by the `text.add` command; text typed with the text tool or made in the text dialog starts unwelded (`text_ui.rs` builds its `TextShape` without `opts.auto_weld`)
- [ ] `options::get()` and `keymap::key_for` read one snapshot per process, refreshed every frame by whichever window drew last: UI tests that change an option or a hotkey and then read it through the snapshot can see another test's values when the suite runs in parallel
- [ ] The node tool and the text tool read keys themselves (`node_tool::interact`, `text_ui`); whether they also act while a dialog is open over the canvas was not checked (the select tool's keys and the plain command shortcuts no longer do)
- [ ] The Move, Size and Rotate locks of the status bar stop mouse drags and arrow nudges, but not the X, Y, W, H and Rotate fields of the ribbon or the rotate and flip commands; unclear which is meant
- [ ] `cargo fmt --all` also walks into the SolidCraft path dependencies (`..\solidcraft\crates\...`): one run failed there with out of memory. `cargo fmt -p <crate>` stays inside DarkBurn
- [ ] Flaky under load: `darkburn` `connections_tests::http_runs_a_job_against_a_loopback_web_server` (the job did not finish over HTTP, connections_tests.rs:649, after 148 s, once in a full run under load; 3 of 3 alone)
- [ ] Ruida UDP: a datagram that is sent again because its acknowledge was late (not lost) reaches the controller twice; the protocol has no datagram number, so the driver cannot prevent it. Check on a real controller whether a repeated chunk of a file upload is dropped or stored twice
- [ ] Galvo: a `MoveZ` step sends `abs(mm)` (`machine.rs`, `Step::MoveZ`), so a move down and a move up by the same amount give the same Z word; whether the board takes an absolute position or needs a direction is not known from the sources
- [ ] GRBL: disconnecting from a controller that no longer answers blocks the caller for up to 6 s (the worker waits out two stop steps before it lets go); the caller is the UI thread
- [ ] GRBL: when a controller sends no start-up message after the stop reset ("No reset reply"), the lines counted as in flight are never cleared, so later commands can wait for buffer room that is already free
- [ ] GRBL through the library adapter (`darkburn-gcodebots` `GrblAdapter::set_air`): air assist is sent as a macro, so switching the air while the fire button is on now also switches the fire button off (the line machines keep it on)
- [ ] Ruida `move_to` / frame: a coordinate that is not a number passes the bed check and is sent as 0 (`mm_to_um(NaN)`); GRBL and the line machines now refuse such values
- [ ] MCC model: `stop_sequence` returns no writes at all when one of its builders fails (`?`), and `SimLink::stop_sequence` then sends nothing; with the default parameters no builder fails
