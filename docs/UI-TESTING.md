# UI testing

DarkBurn's windows, menus, ribbon and dialogs are tested by driving the real app in a headless
harness: no desktop window, no OS-level clicks, no focus problems. Input is simulated as egui
events, and the screen is drawn offscreen into a picture you can open.

The code is in `apps/darkburn/src/ui_harness.rs` (the harness), `ui_smoke_tests.rs` (the sweeps) and
`ui_bug_tests.rs` (tests for bugs that were found and fixed).
Both are `#[cfg(test)]` modules of the `darkburn` binary, so there is nothing extra to build.

## Run

```
cargo test -p darkburn ui_smoke_tests::            # the whole UI suite, about 20 seconds
cargo test -p darkburn ui_smoke_tests::user_story  # one test
DARKBURN_BLESS=1 cargo test -p darkburn the_default_layout      # rewrite the golden picture
```

Pictures land in `target/ui-snapshots/<name>.png` (the last run of each test overwrites them).
Open them to look at what a test saw. A failing test saves what was on screen as `id-clash.png`,
`missing-label.png`, `missing-id.png` or `missing-menu-item.png` there.

## How it works

- `egui_kittest` 0.37 (egui's own test runner, MIT/Apache) runs `crate::App`, the same `eframe::App`
  as `main.rs`, so `logic()` and `ui()` both run every frame. Only the MCP server stays idle.
- The picture is drawn with wgpu into an offscreen texture (a software adapter when one exists).
  The pixels the window would leave to its clear colour are filled with it.
- A frame is 250 ms of egui time (the harness default), so animations finish in a few frames.
  Every queued input event gets a frame of its own, as in a real window.

## Safety

`Gui::new` makes the app safe to drive:

- A temp settings folder, deleted when the `Gui` drops; safe mode, so nothing is saved.
- The default device list: simulators and design-only. After every frame the harness checks that
  no real machine is connected and fails the test if one is.
- File dialogs never open. The app calls them as `rfd::FileDialog`, which is the
  `crates/darkburn-dialogs` crate (a thin layer over the real `rfd`). The harness switches it to
  scripted mode on the test's thread: each dialog takes the next answer you queue with
  `answer_dialog(path)` (none queued means "cancelled") and is logged (`dialog_log()`).
- Under `cfg!(test)` the browser (`help::open_url`), the OS print, the file manager, a second
  window (`file_io::spawn_window`), the update check (unless it goes to a loopback server) and the
  system clipboard are all inert. If you add a new way out of the process, add the same guard.
- egui's duplicate-widget-ID warning is on, and a frame that shows one fails the test at once
  (`allow_id_clashes()` turns that off).

## The harness

```rust
let mut g = Gui::new(1600.0, 1000.0, 1.0);   // width, height in points, pixels per point
let mut g = Gui::standard();                  // 1600 x 1000 at scale 1
```

| Need | Call |
| --- | --- |
| The app, for assertions and set-up | `g.app()` (a `RefMut<DarkBurn>`), `g.ctx()`, `g.screen()` |
| Frames | `g.frames(n)`, `g.settle()` |
| Click | `click_label("Offset Shapes")`, `click_id("ribbon:file.new")`, `click_pos(pos)`, `click_a11y(Some("Button"), "Close")`, `click_mm(x, y)`, `right_click_pos(pos)` |
| Double click | `double_click_pos(pos)`, `double_click_label("00")` |
| Drag | `drag(from, to)` in screen points; `drag_mm((x, y), (x, y))` in design millimetres |
| Wheel | `scroll(at, vec2(0.0, 120.0))` |
| Keys and text | `key(Key::Z, Modifiers::COMMAND)`, `escape()`, `enter()`, `type_text("250")` |
| Menus | `open_menu("Tools", "Offset Shapes")`, `open_menu("Edit", "Align > Left")` for submenus, `open_menu_only("Tools")` |
| Run a command | `run_command("file.new")` as the UI runs it; `exec("shape.add_rect", json!({..}))` with arguments |
| Sample content | `sample_design()` (a rectangle and an ellipse, selected) |
| Window | `set_window_size(w, h)`, `set_scale(s)` |
| Pictures | `screenshot("name")`, `check_golden("name", tolerance)`, `render_image()` |
| What is on screen | `painted()` (every text with its rectangle), `find_text`, `has_text`, `rect_of("name")`, `rects("prefix")`, `a11y()`, `find_a11y(role, label)`, `window_titles()`, `dialog_windows()` |
| Dialogs | `dialog_open()`, `close_dialogs()`, `answer_dialog(path)`, `dialog_log()` |
| Layout | `layout_problems()` lists clipped, overflowing, overlapping or broken text and controls |

### Finding a control

Most of DarkBurn's chrome (ribbon, tool column, menus, tabs) is painted by hand, so the
accessibility tree only has the standard egui widgets (dialog buttons, fields, check boxes,
windows). Three ways to find a control, from most to least stable:

1. **By name from the UI registry** (`click_id`, `rect_of`). Widgets publish a rectangle under a
   name with `solidcraft_ui_registry::publish_target(name, rect)`. Names in use: `ribbon:<command id>`,
   `ribbon:tab:<tab>`, `tool:<command id>`, `appbar:menu:<name>`, `appbar:help`, `appbar:settings`,
   `panel:tab:<panel id>`, `panel:overflow:<Area>`, `panel:area:<Area>` (a dock's tab group), `canvas`, `status:*`, and many
   `dialog:button:<label>`, `dialog:control:<label>`, `dialog:cancel`, `dialog:window`. The registry
   is cleared at the start of every frame, so it holds what the last frame drew. When you add a
   control that tests should reach, publish it.
2. **By painted text** (`click_label`, `find_text`): the topmost visible text equal to the label.
3. **By accessibility node** (`click_a11y`, `find_a11y`): role and label, e.g. `("Window", "About DarkBurn")`.

## Writing a test

```rust
#[test]
fn offset_shapes_dialog_opens_and_closes() {
    let mut g = Gui::standard();
    g.sample_design();
    g.open_menu("Tools", "Offset Shapes");
    assert!(g.window_titles().contains(&"Offset Shapes".to_string()));
    g.screenshot("offset-dialog");          // look at target/ui-snapshots/offset-dialog.png
    g.close_dialogs();
}
```

- Set up state with `g.app()` or `g.exec(...)`, then drive the UI, then assert on `g.app()`.
- Work started on a thread (job build, file read) finishes later: loop `g.frames(2)` and sleep
  a few milliseconds until the state you wait for appears (`wait_until` in the story test).
- A new dialog needs no new test to be covered for position and close: `every_command_opens_dialogs_that_fit_and_close`
  runs every registered command in the UI and checks each window fits at 960 x 600, 1280 x 720 and
  1920 x 1080. A dialog that opens only from state (`app.xyz_open = true`) goes in the list in
  `state_dialogs`. A new menu must be added to `menu_tests!` (a test fails until it is).
- Double clicks are sent in one frame, because the harness's 250 ms frames are slower than egui's
  double-click window.
- Escape closes one thing per press: a menu or drop-down list first, then the window on top. The
  canvas and its tools leave Escape alone while any window is open (`escape.rs`). A window that is
  not a `ui_kit::Window` must ask `escape::closes(ctx, id)`; `escape_closes_every_window_a_command_opens`
  fails until it does.

## What the suite covers

| Test | What it does |
| --- | --- |
| `menu_*` | Opens every menu and clicks every item once (submenus, panels, toolbars, languages); dialogs are closed again |
| `every_ribbon_button_on_every_tab_can_be_clicked`, `every_tool_in_the_tool_column_can_be_picked_and_the_canvas_used` | Every ribbon tab and button, every tool, each used on the canvas |
| `every_panel_can_be_shown_hidden_and_cycled`, `every_toolbar_and_the_status_bar_can_be_toggled` | All panels shown, every tab cycled with and without a selection, all hidden, layout reset; all toolbars |
| `every_command_opens_dialogs_that_fit_and_close` | Every registered command from the UI; every window fits the screen and closes |
| `every_state_driven_dialog_fits_and_closes` | The windows nothing in a menu opens, set by state, at three sizes |
| `nothing_clips_at_*` | At 1280x720, 1920x1080, 2560x1440 and 3840x2160 pixels, at scale 1 and 2 (the window is never smaller than 960 x 600 points): no control outside the window, no text cut off or broken mid-word, no overlapping ribbon buttons, on every ribbon tab, with the default panels and with all of them |
| `user_story_draw_set_up_preview_save_undo_and_reopen` | New project, draw a rectangle by dragging, set layer mode and speed, Preview, save G-code, undo and redo, save the project, reopen it |
| `escape_*` (`ui_bug_tests.rs`) | One Escape closes every window a command or state opens, whatever tool is picked; menus and lists close before the window; windows close one at a time |
| `every_interactive_control_has_an_accessible_name`, `every_control_in_every_window_has_an_accessible_name`, `fields_are_named_after_their_visible_labels` | Every field, slider, check box and button in every panel, ribbon tab and window has a name for screen readers, taken from its visible label. A bare `egui::TextEdit`, `DragValue` or `Slider` needs `a11y::text/number/slider` (or `ui_kit::named_control` round a kit field outside a row) |
| `no_widget_id_is_used_twice_in_any_panel_arrangement` | No egui duplicate-ID warning in the default layout, with every panel shown, with all panels in each dock and dealt round the docks, with each tab's right-click menu and overflow list open, at 1600 x 1000 and 960 x 600 |
| `docks_*`, `a_dragged_dock_*`, `every_dock_is_usable_at_every_window_size`, `a_tiny_window_*` | The docks open at their default sizes and keep them; a dragged dock keeps its size and it is saved; from 960 x 600 to 3840 x 2100 every dock and the drawing area are on screen and usable, also after the window shrinks with the docks at their widest; the sizes come back when the window grows; a window far below the minimum draws without a panic |
| `the_harness_*`, `the_widget_registry_*`, `no_file_dialog_*`, `the_default_layout_matches_its_golden_picture` | The harness itself |

Every test fails on a duplicate widget ID.

## Golden pictures

`check_golden("name", tolerance)` compares the frame with `apps/darkburn/tests/golden-ui/name.png`.
`tolerance` is the share of pixels (0 to 1) allowed to differ by more than 24 per channel. On a
mismatch the test saves `<name>.diff.png` (differences in magenta) beside the snapshot. Rewrite a
golden with `DARKBURN_BLESS=1` after a change you meant to make, and look at the picture before you
commit it. Goldens depend on the fonts and the GPU adapter (the harness prefers a software one), so
keep them few and the tolerance loose.

## Known limits

- Accessibility names are checked for the standard egui widgets only. The hand-painted chrome
  (ribbon buttons, tool column, panel tabs, status bar) is not in the accessibility tree at all.
- The harness does not test native window behaviour: the title bar buttons, the resize border,
  drag and drop from the OS, and the file dialogs themselves.
- Pointer position is exact; there is no hover delay or touch input.
