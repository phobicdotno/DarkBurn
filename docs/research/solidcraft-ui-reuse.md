# Reusing SolidCraft's UI crates in DarkBurn (Rust + egui)

Source: a local checkout of SolidCraft, version 0.40.0-rc.3, HEAD `be6e43955`, which sits on upstream `419bbcda0`. Rust edition 2024, rust-version 1.95, egui/eframe 0.37. I only read the repo; nothing in it was modified.
Section 7 describes the look from a screenshot of the running app (installed v0.40.0-rc.3). The screenshot is not kept in this repository.

## TL;DR

- **Take as crates, no changes:**
  - `solidcraft-theme`: the token contract, the light and dark palettes, and `apply(ctx)`.
  - `solidcraft-ui-text`: message catalogs and translation.
  - `solidcraft-ui-registry`: named widget handles, used by tests and automation.
  - `solidcraft-color`: the sRGB rule.

  All four are tagged "Shared" in the layer check: they depend only on egui and serde-level crates, never on the engine. `cargo xtask shared` builds and tests each one on its own, in a scratch workspace outside the repo. That is the design intent of commit 419bbcda0.
- **Do NOT depend on `solidcraft-ui-egui`.** It has non-optional dependencies on `solidcraft-engine` (and through it the kernel, doc, io and render crates), `solidcraft-gpu`, `solidcore-record`, the patched `egui-wgpu`, and others, and it has no `[features]` to switch them off.
- **Copy the `ui_kit/` folder out of ui-egui into DarkBurn** (a `darkburn-ui` crate). Outside test code, no `ui_kit` file uses the engine or `SolveApp`. It needs a few sibling files copied with it: `fonts.rs`, `icons.rs`, `icon_svg.rs`, `frame.rs` and the blur stub, plus three generator steps from `build.rs`. These pieces give the pixel-identical controls: containers and panels, windows, forms, tables, menus, trees, fields with units, sliders, segmented controls and search.
- **Rebuild against SolidCraft's layout:** the app bar and title bar, ribbon, status bar, start page, Preferences window, keymap and update UI. All of these live in `SolveApp` code.
- **Copy verbatim from `apps/solidcraft/src`:** `native_frame.rs` plus the geometry functions from `titlebar.rs`, `window_fit.rs`, `come_forward.rs`, the `PrefsWriter` pattern, and most of `startup.rs`.

---

## 1. `crates/theme` (solidcraft-theme)

Files: `crates/theme/src/lib.rs` (1891 lines), `crates/theme/src/palettes.rs` (537), `README.md`.
Dependencies: `egui`, `serde`, `serde_json`, `solidcraft-color`. No data files, and no knowledge of the theme-file format.

### What it exports

| Item | Purpose |
|---|---|
| `Tokens` (a `Copy` struct of about 250 fields) | Every colour and size the UI draws with |
| `Tokens::light()`, `Tokens::dark()` (`const fn`, in `palettes.rs`) | SolidCraft's built-in values. Dark is the default. |
| `Tokens::get()` / `Tokens::get_in(vision)` / `Tokens::view()` / `Tokens::base()` / `Tokens::themed(dark, vision)` | The tokens to draw with right now |
| `apply(ctx: &egui::Context)` | Sets egui `Visuals` and spacing from the tokens, then `request_repaint()` |
| `set_dark(bool)`, `is_dark()`, `set_vision(ColorVision)`, `vision()`, `set_viewport_dark`, `set_viewport_background`, `set_logo_accent` | Theme state, kept per thread. The host sets it every frame. |
| `set_custom(Option<Custom{dark,tokens}>)`, `custom()`, `scoped_tokens(Tokens)`, `scoped_panel_tokens(Tokens)` | Use your own palette in place of the built-in one, either for the whole thread or for one scope (an RAII guard) |
| `ColorVision {Off, Protan, Deutan, Tritan}` | Colour-vision variants of the colours that carry meaning |
| `TabAccent {color, ink, soft}`, `for_tab`, `current(ctx)`, `set_current`, `number_focus`, `TAB_NAMES` | Per-tab selection colours. The names are SolidCraft's tabs (MODEL, SKETCH, ...); any other name falls back to the Model accent. |
| `Borders`, `BorderRole`, `Bevel`, `InsetBorder` + `Tokens::{primary_border, secondary_line, container_rect, input_bevel, button_bevel, ...}` | Structural strokes, and the optional bevelled style used by retro themes |
| `contrast_ratio`, `composite`, `panel_surface_fill`, `panel_surface_readable`, `panel_composite_bounds`, `Tokens::apply_panel_opacity`, `apply_surface_brightness`, `protect_tab_panel` | WCAG contrast guards for translucent panels |
| `Surfaces`, `Fade`, `PublishedTab`, `Environment`, `set_published` | Per-tab surface fades. These are optional and can be ignored. |
| `panel_motion_duration(ctx, reduced)` (0.12–0.16 s), `panel_motion_ease` (cubic-out) | Animation timing |
| `Tokens::shell_shadow()` | The subtle shell elevation: offset 2, blur 8, `app_bar` at 18% |
| `icon_strokes` | Primary icon stroke colours, shared with the icon `palette.toml` |
| `PANEL_INLINE_MAX_HEIGHT = 88.0` | Panel layout constant |

### What `apply(ctx)` does

`visuals(dark)` starts from egui's dark or light visuals and then sets:
- `panel_fill` and `window_fill` to `panel`
- `extreme_bg_color` to `field`, and `faint_bg_color` to `panel_header`
- `override_text_color` to `text`
- `selection` to `accent_soft` with a 1 px `accent` stroke
- `hovered` to `hover`
- window and menu corner radius to `radius_md` (6); widget corners to `radius_sm/2` (2), or 3 when hovered

It also sets spacing: `item_spacing = (6,4)`, `button_padding = (6,3)`.

### Built-in values

The shared sizes are the same in both palettes:
- **Spacing:** `space_2/3/4/6/8/10/12/14/16/24`
- **Radii:** `radius_sm/md/lg/xl` = 4/6/8/10
- **Fonts (pt):**
  - badge 9.5, micro 10, panel 10.5, small 11, caption 11.5
  - body 12, label 12.5, button 13, heading 14, subtitle 15, intro 16
  - page heading 20, title 22, display 26
- **Dense forms:**
  - `control_height` 28, `control_radius` 6
  - `form_label_width` 70, `form_gap_y` 5
  - header 40, footer 48, card width 264, checkbox row 22

Key colours (dark is the default, a blue-grey in the spirit of desktop CAD):

| Token | Dark | Light |
|---|---|---|
| app_bar | #1C1F26 | #343840 (the bar stays dark in the light theme too) |
| toolbar / panel | #272B34 | #F6F7F9 / #FAFBFC |
| panel_header | #2F343E | #ECEEF2 |
| border | #3F4552 | #D0D4DC |
| text / text_dim | #DEE2E8 / #949BA8 | #262A32 / #6E7480 |
| accent / accent_soft | #3A88EE / #2A466E | #2678DA / #D6E6FA |
| field / control_fill | #1E2229 / #1C1F26 | #FFFFFF |
| hover | #363D4A | #E2EAF6 |
| viewport_top = bottom | #2E3440 (flat) | #F0F3F7 to #CBD2DD (gradient) |
| grid / grid_major | #3B414C / #494F5A | #C4CAD4 / #AAB2BE |
| error / warning | #F06460 / #ECAA46 | #D63636 / #DE961E |
| selected_ink / selected_bg | #9CC7F5 / #2A466E | #1A507F / #E8F1FA |

### Where the CAD naming shows

Many tokens are named for CAD concepts, for example `sketch_*`, `sel_face`, `cube_*`, `origin_*` and `comb_*`.

For DarkBurn:
- Use the generic ones as they are: app_bar, toolbar, panel, text, accent, field, form_*, dialog_*, control_*, file_*, grid, viewport_*, error and warning.
- Map laser concepts onto existing tokens:

| Laser concept | Existing token |
|---|---|
| job path | `sketch` |
| selection | `sel_edge` |
| traversal moves | `preview_sketch` |
| work area | `grid` |

- Or build a `Tokens { ..Tokens::dark() }` with overrides and hand it over with `set_custom`.

### What 419bbcda0 says about reuse

The commit is "Shared UI crates: the token contract as a shared crate, solidcraft-theme (#13384)". It introduces the "Shared" crate class:

> A Shared crate depends only on other Shared crates, solidcore and external crates (UI toolkits included); any layered or testkit crate may use one.

`cargo xtask shared` (`xtask/src/shared.rs`) "prove[s] each shared crate would build after extraction from this repository". It copies the crate into `target/xtask-shared/<crate>/` with only its Shared dependencies and runs `cargo test` there. It also checks that no `include_str!` or `include_bytes!` reaches outside the crate.

The theme README says:

> a component vendored into another app draws with that app's tokens and needs no theme files, built-in themes or icon artwork.

In the same PR:
- The icon palette moved out of `Tokens`.
- The message-catalog machinery moved into `solidcraft-ui-text`.
- The widget registry moved into `solidcraft-ui-registry`.
- `solidcraft-color` was split out.
- `ui_kit` stopped depending on the expression language: the host installs an `Expressions` trait object per context.

These changes are what make the kit extractable.

## 2. Reusable widgets in ui-egui, ui-text, ui-registry and color

### Small crates (use as dependencies)

- **solidcraft-ui-text** (dependencies: serde_json, log, unicode-bidi, sys-locale; no UI toolkit). See section 3 for how the catalogs work.
  - Catalogs: `Catalog::new(id, name, json)` (`const`, so it can be a `static`), `install(&'static [&Catalog])`.
  - Lookup: `tr(id)`, `tr_label(label)`, `format(id, &[("name", val)])`.
  - Language: `scope(lang)` guard, `language()`, `locale()`, `Locale`, `SYSTEM`, `system_language()`, `effective_language(pref)`, `is_supported`, `pin_system_language`.
  - Right-to-left text: `bidi::{visual, visual_template, has_rtl, screen_order}`.
  - English is the fallback, then the id itself. There are no plural rules and no macro.
- **solidcraft-ui-registry** (dependency: egui). A per-thread registry of named handles, rectangles and counts, cleared each frame:
  - `publish_handle`, `publish_rect`, `publish_target(_as)`, `publish_count`
  - `handle_at`, `handles(prefix)`, `rect_of`, `rects`, `count_of`, `clear_handles`

  Every kit widget publishes into it, so DarkBurn gets automation hooks by name for free. That is useful for an MCP or test harness.
- **solidcraft-color** (no dependencies): `srgb::{encode, decode, encode8, decode8}` and `AnalysisColors`, which is CAD-only but required by the theme crate.

### `crates/ui-egui/src/ui_kit/`: generic after extraction

**What the kit pulls in from the rest of `crate::`** (copy these alongside):

| Module | Used for / what to change |
|---|---|
| `fonts` | `semibold`, `medium`, `bold`, `RichTextExt`. In `ui_font()`, replace `solidcraft_engine::sketch::font_data`. |
| `icons` + `icon_svg` | Painting icons |
| `frame` | `Side`, the dialog width limits, `drag_dock` |
| `blur` | Use the non-native stub in `blur.rs` (lines ~188–240), where `enabled()` returns false |
| `scenario::*` | Just re-exports of ui-registry; point the calls there |
| `i18n::{tr_name, tr_dyn}` | Used in `tree.rs`; change to `tr_label` |
| `keymap::display` | Used in `text_menu.rs`; copy this pure function |
| `theme_files` | Optional; only used by `preferences.rs` theme rows |

Most kit items are `pub(crate)`. Either make them `pub` in a `darkburn-ui` crate, or keep the kit as a module inside the app.

**Widgets for DarkBurn's needs:**

| Need | Kit module and items | Verdict |
|---|---|---|
| Outer containers (the house rule is a single host) | `container.rs`: `Container::new(ctx, Surface)` with `.frame/.area/.floating/.joined/.central()`; `Surface::{Panel, Feature, Palette, Chrome, ApplicationBar, Window, Popup, Canvas, Toast, Navigation, ViewportStatus, Viewport, InputShield}`; `BackingPopup`, `BackingModal`, `ModalHost`, `FeatureWindow`; the `BackingTooltipExt` tooltips (`backing_on_hover_text/ui`) | generic (needs blur stub) |
| Docked side, bottom and floating cards | `panel.rs`: `Panel::new(title, id, anchor, viewport, side, frame)` with `.workspace()`, `.bottom_strip()`, `.actions()`, `.body_height()`; `header`, `collapsible_header`, `inspector_header` | generic |
| System windows and dialogs | `panel.rs`: `Window::new(id, icon, title).actions(ok, cancel, apply).show(ctx, valid, body)`; `show_pages<S>` with `WindowPage<S>{icon, label, group, handle, scroll, draw}`, which gives a sidebar of pages (the Preferences layout) | generic |
| Confirmations | `confirm.rs`: `Confirm::new(id, icon, title).tone(..).actions(..).show(..)` | generic |
| Buttons and icon buttons | `actions.rs`: `button`, `primary_button`, `text_button`, `icon_button_at`, `compact_button`, `icon_action`, `icon_label_button`, `footer(..) -> Action`, `HeaderActions`, `action_menu`, `command_menu_button` | generic |
| Numeric fields with units and expressions | `fields.rs`: the `Expressions` trait (`is_literal`, `is_unit`, `complete`, `unit_scale`, `length_number`, `parse_length_mm`), installed with `install_expressions(ctx, Arc<dyn Expressions>)`. With nothing installed, fields edit plain mm. Also `parse_number` (accepts a comma as the decimal mark, and a Unicode minus), `field`, `count_field`, `scalar_field`, `slider`, `mixed_slider`, `number_pair`, `fraction_field`, `cell_editor`, `dropdown*`, `searchable_dropdown`, `search_picker`, `text_field*`, `search_field` | generic. Implement `Expressions` for mm/in, mm/s, power %, passes. |
| Up/down and mouse-wheel stepping | `src/value_step.rs` (204 lines, one engine reference) | copy and remove that reference |
| Property and parameter forms | `layout.rs`: `row(ui, label, ctrl)`, `expr_row`, `count_row`, `text_row`, `choice_row`, `checked_row`, `hint_row`, `section`, `section_heading`, `editor_section`, `divider`, `caption`, `warning_caption`, `status_line(StatusTone)`, `settings_form` | generic |
| Tables (cut settings, layers) | `table.rs`: `table_custom(ui, id, &[Column], ..)`, `Column::new(..).flex().icon_only()`, `Table::headers_resizable`, `sort_header`, `bin_button`, `active_pill`; `name_list.rs` | generic |
| Tree (objects and layers) | `tree.rs`: `tree_row(ui, id, &Row) -> RowResp` (depth, fold, eye, icon, label, selected, badges, actions, swatch); `tree_header` | generic after switching to `tr_label` |
| Menus and context menus | `menu.rs`: `Item::action(id, label, icon).key(..)`, `Item::sep()`, `Menu::{context, button, text, submenu, show, separator, heading}`; `text_menu.rs` (Cut/Copy/Paste); `radial.rs` | generic. The caller handles dispatch. |
| Segmented controls, chips, tab strips, colour swatches | `choice.rs`: `segmented`, `segmented_icons`, `choice_chip`, `settings_tab_chip`, `checkbox_row`, `color_swatch`, `swatch_grid`, `popup_list`; `checks.rs` | generic |
| Colour pickers (layer colours) | `color.rs`: `color_field`, `color_popover`; `recent_colors.rs` | generic |
| Progress and job status | `status.rs`: `progress`, `progress_action(.., cancel)`, `steps`, `status_header`; `stages.rs`: `StageBar` (for example Prepare → Frame → Burn); `viewport_status.rs` pills | generic |
| Command search | `search.rs`: `CommandSearch`, `SearchBadge` | generic (you rank the results) |
| Preferences search | `settings_search.rs`: `begin(ui, filter)`, `begin_page`, `begin_section`, `set_control`, `hits`. It finds a control by its being drawn, with no registration step. | generic |
| Shortcut key caps | `shortcuts.rs`: `shortcut_binding` | generic |
| Start page pieces | `file_view.rs`: `FileView`, `RecentRow`, `SampleCard`, `RecoveryMessage` | generic |
| Theme chooser | `theme_picker.rs`, plus `src/theme_files.rs` and `theme_options.rs`. These parse `solidcraft-theme/1` JSON and give you the 25 built-in themes in `assets/styles/ui/themes/*.json` (Nord, Gruvbox, Solarized, Tokyo Night, SolidCraft 95, phosphor, and others). No engine dependency. | generic |
| Small helpers | `metrics`, `text` (middle ellipsis), `dash`, `drag`, `wheel`, `disclosure`, `details`, `attention`, `reports`, `review`, `compare`, `framing`, `value_focus`, `history*` | generic |
| CAD-only (drop) | `navigation*`, `sphere`, `look_chain`, `assistant*`, `preferences_figure*` (4.6k lines of CAD illustrations), `panel_host`, `gallery` (tests) | — |

### Coupled to the CAD engine or `SolveApp` (rebuild instead)

| Module | Why it is coupled | What to do |
|---|---|---|
| `panel_runtime/` (dockable tab groups) | Built on `solidcraft_engine::panels` | Use fixed `Panel` cards instead |
| `toolbar.rs` (app bar, ribbon, File menu, shortcuts) | 57 `SolveApp` references, built on `CommandSpec` | Rebuild. Copy the pure `fit()`/`Plan` ribbon fitting, the label-wrap painters and the constants: `APP_BAR_HEIGHT` 34, `TAB_ROW_HEIGHT` 30, `GROUP_HEIGHT` 86. |
| `titlebar.rs` | Only the caption buttons and close action touch `SolveApp` | Keep the geometry as is |
| `status_bar.rs` | Height 24, on `Surface::ApplicationBar` | Rebuild; small |
| `home.rs` | — | Rebuild with `file_view` |
| `prefs.rs` | — | Rebuild with `Window::show_pages` + `settings_search` + `preferences_cards` |
| `keymap.rs` | Bound to `CommandSpec` | Rebuild; `key_name` and `display` are pure and can be copied |
| `context_menu.rs`, `shortcut_box.rs`, `dialogs*.rs`, `browser.rs`, `timeline*.rs`, `viewport.rs`, `navigation.rs`, `issue_report.rs`, `agent_chat/` | CAD content | Rebuild what you need |
| `update.rs` | Version logic is generic; `SolveApp` appears from about line 688 down. `REPO` and `LATEST_API` are hard-coded. | Copy the version logic, rebuild the UI |

### UI rules to adopt as written

From `skills/solidcraft-ui/SKILL.md` and `docs/ui-kit.md`, which is 1081 lines: see "Composition and rules" at line 190 and "Extending and checking the kit" at line 564.

- **One outer container.** Every panel, sheet, menu, overlay and window goes through `ui_kit::Container`, either directly or through `Panel`, `Window`, `Confirm` or `Menu`. Feature code never builds raw egui frames, popups or shadows. This is enforced by `cargo xtask ui-containers`.
- **Tokens only.** "No literal colours in feature code."
- **Kit controls only.**
  - Sliders only through `ui_kit::slider`.
  - Form rows only through `row`, `expr_row`, `text_row`, `choice_row` and `count_row`, which align at `control_height` 28.
  - Disclosure arrows only through `ui_kit::disclosure`.
  - Confirmations only through `Confirm`.
- **Warnings** use a coloured outline plus `warning_caption`, so colour is never the only cue.
- **Text** goes through the catalogs by stable id. New strings go in `assets/locales/pending/<slug>.json`.
- **Extending the kit:** add one implementation, document its tokens, re-export it in `mod.rs`, and add one entry to `gallery::ELEMENTS`.

Other design documents: `docs/theme-colour-audit.md`, `docs/ui-container-audit.md`, `docs/panel-foundation.md`, `docs/ribbon-layout.md`, `docs/fonts.md`, `docs/ui-icon-design-proposal.md`, `assets/icons/ui/STYLE_GUIDE.md`.

**Typography** (`docs/fonts.md`):
- Interface font is IBM Plex Sans (variable) in Regular, Medium, Semibold and Bold.
- Semibold: titles and selected labels.
- Bold: compass letters and the primary OK action.
- Medium: selection fields and status messages.
- IBM Plex Mono Regular: numeric fields and dimensions.

These come from `ui-egui/src/fonts.rs`: `base_font_definitions()`, `setup_bundled_fonts(ctx)`, and `medium()`, `semibold()`, `bold()`, which return `FontId`. The font files are in `assets/fonts/ibm-plex-*` under the OFL licence.

## 3. How the app shell is built (`apps/solidcraft/src`)

`main.rs` is a thin host. The `App { solve: SolveApp, .. }` struct implements eframe 0.37's split API: `logic`, `raw_input_hook`, `ui`, `save`, `on_exit`. Everything visible is drawn by `SolveApp::ui` in `ui-egui/src/lib.rs:3419`.

### Window (`app_viewport`, main.rs:909)

- Title, plus a 64x64 procedural icon (`assets/icons/app/shared/source/app_icon.rs`).
- Size 1600x1000, minimum 960x600, drag and drop on.
- **`with_decorations(false)` on Windows and Linux.** `CUSTOM_TITLEBAR = !macOS`. On macOS: `fullsize_content_view` + `titlebar_shown(false)`.
- `with_app_id`.

### `NativeOptions` (main.rs:1077)

- `depth_buffer` 0, `multisampling` 1. The 3D view renders offscreen.
- `persistence_path = settings_dir/app.ron`, `persist_window = true`.
- wgpu backend order D3D12, then Vulkan, then GL, with an automatic re-exec fallback if no window opens.

### Settings

- Location: `%APPDATA%\SolidCraft\data\` (`eframe::storage_dir`).
- `app.ron` holds eframe's own keys, plus one JSON string under `"solidcraft.prefs"` (`prefs::STORAGE_KEY`).
- `graphics.json` holds the backend and title-bar choice; it is read before the window exists.
- **`PrefsWriter`** (main.rs:118–168) flushes preferences at most once a second after real input, instead of eframe's 30 s timer. Worth copying.
- `startup.rs`:
  - `check_settings` validates `app.ron` before eframe reads it, and moves a corrupt file aside as `*.bad-<ts>`.
  - `begin`/`started` count failed starts; after 2 the app starts in safe mode.
  - Command-line arguments are parsed by hand.

### Per-frame order

`SolveApp::logic`:
1. `prefs::apply`, which resolves system or explicit dark/light and sends `ViewportCommand::SetTheme`.
2. Only when the theme changed: `solidcraft_theme::set_dark` + `set_vision` + `apply(ctx)`.
3. `toolbar::shortcuts`: Ctrl+N/O/, always; Ctrl+W/Z/Y/S when a document is open; then `keymap::press`.
4. `update::tick`.
5. Dropped files.

`SolveApp::ui`:
1. `solidcraft_ui_text::scope(lang)`.
2. **App bar** (`toolbar::app_bar`). One dark 34 px strip that is title bar and menu bar at once: drag area, logo, Home, a "File" button opening a custom `egui::Area` menu (not `egui::MenuBar`), Save, Undo, Redo, document tabs, update pill, assistant, settings gear, help, "Report a bug", version, then `titlebar::caption_buttons`.
3. Modals.
4. If `home.open`: `home::show` and return.
5. Otherwise:
   - `status_bar` (off by default)
   - `toolbar::toolbar` (ribbon: tab row plus icon groups)
   - assistant side panel
   - `timeline` (bottom history strip)
   - `browser` (left tree)
   - `Container::new(ctx, Surface::Viewport).central()` (the viewport)
6. Dialogs.
7. `titlebar::resize_zones`, used when the Win32 border is not installed.

### Custom title bar on Windows (`native_frame.rs`, 250 lines, `unsafe`)

- Subclasses winit's window procedure with `SetWindowLongPtrW(GWLP_WNDPROC)`.
- `WM_NCHITTEST` returns `HTLEFT` / `HTTOPRIGHT` / ... within `GetSystemMetricsForDpi(SM_CXSIZEFRAME)+SM_CXPADDEDBORDER`, with doubled corners, and is skipped when maximized.
- `WM_GETDPISCALEDSIZE` keeps the logical size when the window moves between monitors.
- `set_class_icon` loads `.ico` resource 1, so the taskbar icon is right from the start.
- `work_area()`.
- egui draws the caption:
  - `titlebar::drag_area` sends `ViewportCommand::StartDrag`, so Aero Snap works.
  - Double-click maximizes.
  - Three 46 px caption buttons.
- Dependencies: `windows-sys 0.61` (`Win32_Foundation`, `Win32_Graphics_Gdi`, `Win32_System_LibraryLoader`, `Win32_UI_HiDpi`, `Win32_UI_WindowsAndMessaging`), `raw-window-handle 0.6`.
- Copy it together with the pure functions `resize_hit`, `zones`, `corner_size`, `resize_border_px` and `dpi_scaled_size` from `ui-egui/src/titlebar.rs`.

### Other shell modules

| Module | What it does | Verdict |
|---|---|---|
| `desktop_theme.rs` | Linux only: XDG portal or `gsettings` for dark mode. Windows and macOS use `ctx.system_theme()`. | — |
| `desktop_transparency.rs` | Windows `EnableTransparency` registry value or the macOS reduce-transparency setting; only matters for blur themes | Copy verbatim if needed |
| `native_menu.rs` | macOS NSMenu through `muda`, sharing the `FileAction` enum with the egui File menu | Copy the pattern only |
| `window_fit.rs` | Pure: clamps the window to the monitor's work area | Copy |
| `come_forward.rs` | Brings the window to the front for the "save changes?" prompt | Copy |
| `device_lost.rs` | GPU reset recovery; needs the vendored egui-wgpu | Optional |
| `control_server`, `nav_bridge`, `space_mouse*`, `native_crash*`, `open_documents`, `desktop_chooser` | — | Skip |

### i18n

- `assets/locales/*.json`: 35 flat `{ "id": "text" }` files. About 6,200 keys in `en`, mixing English-as-key entries and dotted ids.
- Placeholders are `{name}`; plurals use separate `_one`/`_other` keys.
- `ui-egui/build.rs merged_locales()` merges `assets/locales/pending/*.json` into each catalog and writes `$OUT_DIR/locales/*.json`.
- `i18n.rs` declares `static X: Catalog = Catalog::new("de", "Deutsch", include_str!(...))` for each language; `install()` calls `solidcraft_ui_text::install(LOCALES)`.
- DarkBurn copies this pattern (about 40 lines plus the build step) and writes its own catalogs. SolidCraft's text is CAD wording, so reusing it only helps for generic strings such as OK and Cancel.

### Update check

- Host HTTP code: `main.rs` `http_call` / `http_get` / `http_download`, using ureq 3, rustls with ring, and the platform certificate verifier.
- Logic: `ui-egui/update.rs` checks GitHub `releases/latest`, compares with semver and verifies SHA256SUMS plus minisign.
- The logic can be copied; the UI must be rebuilt.

### Shell pieces DarkBurn wants

| Piece | Source |
|---|---|
| Frame | `native_frame` + `titlebar` geometry + `window_fit` |
| Theme | `solidcraft-theme` + `theme_files` |
| App bar and menus | rebuild with `ui_kit::Menu` |
| Panels | `ui_kit::Panel` / `Container` |
| Ribbon | rebuild using `fit()` and the constants |
| i18n | `solidcraft-ui-text` + `build.rs` pattern |
| Icons | Section 4 |
| Preferences | `Window::show_pages` + `PrefsWriter` + `STORAGE_KEY` pattern |
| Startup safety | `startup.rs` |

## 4. Icons (`assets/icons/ui/`)

**How icons are made:**
- **SVG files are the source of truth.** They are hand-edited, or started with `design/tools/author/new_icon.py <id> --base cube|slab|empty`.
- Folders under `design/svg/`:

| Folder | Count | What it holds |
|---|---|---|
| `model/` | 297 | coloured |
| `flat/` | 66 | `currentColor` controls (save, undo, chevrons) |
| `workspace/` | 4 | — |
| `small/` | 8 | variants used at 24 px or less |

- `cargo xtask icons` only runs the Python lint and review checks. Nothing generates SVGs.
- `source/*.rs` (`glyphs`, `markers`, `navigation`, `pointer`) is egui `Painter` code for viewport glyphs and caption buttons, included with `#[path]`.

**Build and render pipeline:**
1. `ui-egui/build.rs icon_data()` reads `palette.toml` and `aliases.toml` and embeds every SVG with `include_str!` into `$OUT_DIR/icon_data.rs`.
2. `icon_svg.rs` has its own small SVG-subset parser:
   - path commands M L C Q A Z
   - `circle`, `g`, `clipPath`, `mask`
3. `render(doc, Look, px)` rasterises with **tiny-skia**. It maps each fixed source hex colour to a palette role, then to that role's light or dark value or an accent ramp.
4. `icons.rs` `paint_icon(painter, rect, name, opacity)` caches one texture per (icon, look, pixel size) and snaps to whole pixels.
5. Variants: `paint_ink`, `paint_accented`, `paint_danger`, `paint_app_bar`.

**Style:**
- 64x64 viewBox, centred, at most 58 units across.
- Isometric 30° cube base: corners (32,5), (8.62,18.5), (55.38,18.5), (32,32), (8.62,45.5), (55.38,45.5), (32,59).
- Strokes, all round caps and joins:

| Use | Width |
|---|---|
| outline | 2.55 |
| action / sketch | 4.25 |
| navigation | 5.6 |

- Not allowed: gradients, shadows, text, transforms, CSS.

**Palette roles:**
- `ink`, `line` (`currentColor`), `passive-1/2/3`, `gap` (erases), `guide`, `active-1/2/3` (blue ramp)
- `component-orange-1..3`, `component-yellow-1..3`
- Accents: surface, form, mesh, sketch, constraint, construction
- Colour-vision variants
- `[icon-accents]`
- `aliases.toml` lets an icon reuse another's artwork in a different accent.

**Licence:** the icons are original SolidCraft artwork under MIT OR Apache-2.0, with about 375 rows in `ATTRIBUTION.md`. `glyphs.rs` caption geometry is adapted from VectorCraft, also MIT/Apache. DarkBurn may reuse them if it keeps the notices. The repo's "never copy icons" rule is about other products' artwork.

**How DarkBurn adds its own icons:**
1. Copy `design/palette.toml`, `aliases.toml`, `tools/author/*.py` and optionally `lint_icons.py` and `icon_status.py`, keeping the `svg/{model,flat,workspace,small}` layout.
2. Port `build.rs icon_data()`, `icon_svg.rs` (about 750 lines; needs `tiny-skia 0.12`, `toml` at build time, `log`, `egui`) and `icons.rs`. Their `solidcraft_theme` calls keep working because DarkBurn depends on that crate.
3. Write each new 64x64 SVG using only the source hex colours, for example a laser head, air assist, frame, origin, job or material.
4. Re-colour icons only by editing `palette.toml`, or through a theme file's `icon_palette`.

## 5. Dependency strategy

### Can DarkBurn depend on these crates from outside the workspace?

- **Path:** yes. `solidcraft-theme = { path = "../solidcraft/crates/theme" }` works. The member manifests use `version.workspace`, `egui = { workspace = true }` and `[lints] workspace = true`. Cargo resolves those through `../solidcraft/Cargo.toml`, which a path or git dependency is allowed to do.
- **Git:** yes. `{ git = "https://github.com/phobicdotno/solidcraft", rev = "<sha>" }` works the same way. Cargo clones the repo and loads only the named package.
  - Pin a `rev`.
  - Version is `0.40.0-rc.3`, a pre-release; fine for git and path dependencies.
  - Note: the fork's `main` follows upstream and may rename paths. Upstream is `tangency-labs/solidcraft`; the workspace `repository` field still says `bherbruck/solidcraft`.
- **crates.io:** no. Nothing is published.
  - None of the four Shared crates sets `publish`.
  - `ui-egui` has no `publish = false`, but it is not published either.
  - `apps/solidcraft` sets `publish = false`.

### Heavy internal dependencies

| Crate | Internal deps | Pulls in |
|---|---|---|
| solidcraft-theme | `solidcraft-color` | egui, serde, serde_json |
| solidcraft-ui-text | none | serde_json, log, unicode-bidi 0.3, sys-locale 0.3.2 |
| solidcraft-ui-registry | none | egui |
| solidcraft-color | none | nothing |
| **solidcraft-ui-egui** | **engine, gpu, space-mouse, solidcore-record** (no features to turn off) | wgpu 30, egui-wgpu, image, zip, the whole CAD kernel. **Do not depend on it.** |

### Vendored crates and patches

- The workspace root has `[patch.crates-io] egui-wgpu = { path = "vendor/egui-wgpu" }` and `winit = { path = "vendor/winit" }`.
- **Patches in a dependency's workspace are ignored by consumers.** DarkBurn therefore builds with stock egui-wgpu 0.37.0 and winit 0.30.13 unless it adds its own patch.
- The four Shared crates need neither patch.
- DarkBurn only needs the vendored copies for these patched egui-wgpu features:
  - device-lost survival (`DeviceLost`)
  - `set_startup_hook`
  - MSAA clamping
  - D3D12 surface-configure retry
  - the Wayland pinch gesture fix in winit
- To get them: copy `vendor/egui-wgpu` and `vendor/winit` (MIT/Apache) and add the same `[patch.crates-io]` lines. This is recommended later for robustness on Windows with NVIDIA (#1935, #4172, #7007).

### Pinned versions (root `[workspace.dependencies]` and `Cargo.lock`)

| Crate | Declared | Locked |
|---|---|---|
| egui | `"0.37"`, features `["default_fonts"]` | 0.37.0 |
| eframe | `"0.37"`, `default-features = false`, `["persistence"]`; app adds `default_fonts`, `wgpu`, `accesskit`, `wayland`, `x11`, `links` | 0.37.0 |
| egui-wgpu | `"0.37"` (patched) | 0.37.0 |
| egui-winit / epaint / emath | — | 0.37.0 |
| wgpu | `"30"`, `default-features = false`, `["std", "wgsl"]` | 30.0.1 |
| naga | `"30"` | — |
| winit | 0.30 (patched) | 0.30.13 |
| tiny-skia | `"0.12"`, no default features, `["std", "simd"]` | 0.12.0 |
| toml | `"1"` (build) | — |
| rfd | `"0.17"` | 0.17.2 |
| ron | `"0.12"` | 0.12.2 |
| accesskit | — | 0.25.2 |
| serde | `1` (derive, rc) | — |
| serde_json | `1` (float_roundtrip) | — |
| pollster | 0.4 | — |
| web-time | 1 | — |
| windows-sys | 0.61 | — |
| raw-window-handle | 0.6 | — |
| embed-resource | 3.0.11 | — |
| ureq | 3.4 | — |
| rustls | 0.23 | — |

### Recommended `Cargo.toml` for DarkBurn

```toml
[workspace]
resolver = "3"
members = ["crates/*", "apps/*"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.95"
license = "MIT OR Apache-2.0"

[workspace.dependencies]
# SolidCraft shared UI crates (path while developing side by side; switch to git+rev for CI)
solidcraft-theme       = { path = "../solidcraft/crates/theme" }
solidcraft-ui-text     = { path = "../solidcraft/crates/ui-text" }
solidcraft-ui-registry = { path = "../solidcraft/crates/ui-registry" }
solidcraft-color       = { path = "../solidcraft/crates/color" }
# solidcraft-theme     = { git = "https://github.com/phobicdotno/solidcraft", rev = "be6e43955" }  # same for the other three

# Must match SolidCraft exactly (one egui in the graph)
egui      = { version = "=0.37.0", features = ["default_fonts"] }
eframe    = { version = "=0.37.0", default-features = false, features = ["persistence", "default_fonts", "wgpu", "accesskit", "wayland", "x11", "links"] }
egui-wgpu = "=0.37.0"
wgpu      = { version = "30", default-features = false, features = ["std", "wgsl"] }
tiny-skia = { version = "0.12", default-features = false, features = ["std", "simd"] }
serde      = { version = "1", features = ["derive", "rc"] }
serde_json = { version = "1", features = ["float_roundtrip"] }
log = "0.4"
web-time = "1"
rfd = "0.17"
ron = "0.12"
pollster = "0.4"
semver = "1"
toml = "1"            # build.rs: icon palette
unicode-bidi = "0.3"

[workspace.lints.rust]
unsafe_code = "forbid"   # app crate relaxes to "deny" for native_frame.rs, as SolidCraft does

[profile.dev.package."*"]
opt-level = 2
[profile.dev]
opt-level = 1
[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 16
debug = "line-tables-only"

# Optional, later: copy ../solidcraft/vendor/{egui-wgpu,winit} into DarkBurn/vendor and
# [patch.crates-io]
# egui-wgpu = { path = "vendor/egui-wgpu" }
# winit     = { path = "vendor/winit" }
```

**Proposed layout:**
- `crates/darkburn-ui`: the copied `ui_kit` plus fonts, icons, icon_svg, frame, the blur stub and theme_files. Its `build.rs` holds `icon_data`, `merged_locales` and `builtin_themes`. Assets go in `DarkBurn/assets/{icons/ui/design, fonts, locales, styles/ui/themes}`.
- `apps/darkburn`:
  - shell code: `main.rs`, `native_frame.rs`, `window_fit.rs`, `startup.rs`
  - Windows-only dependencies `windows-sys` and `raw-window-handle`
  - build dependency `embed-resource`
- Keep the `ATTRIBUTION`/`NOTICE` rows for copied MIT/Apache code, the icons, and the OFL fonts.

## 6. Toolchain and build evidence

```
cargo 1.99.1 (5f94df478 2026-08-27)
rustc 1.99.1 (58cf8f9c3 2026-10-13)
Default host: x86_64-pc-windows-msvc; toolchain stable-x86_64-pc-windows-msvc (only); target x86_64-pc-windows-msvc
```

- There is no `rust-toolchain.toml` in the repo. 1.99.1 is above the required 1.95.
- MSVC Build Tools are present (per the earlier session notes).

**What is in `target\`:**
- `release\solidcraft-cli.exe` (161 MB). This proves the toolchain works for the full engine.
- No `solidcraft.exe` GUI build. The running app is the installed copy.
- `debug\` has fingerprints for theme, ui-text, ui-registry, color and ui-egui, from a test build.

**Rough build times, read from fingerprint timestamps:**

| Build | Units | Duration |
|---|---|---|
| Release `solidcraft-cli` from clean | 271 | about 9 min |
| Debug build of ui-egui and tests | 625 | about 19 min |

Expect a release build of `apps/solidcraft` to take about 10–15 min on the same PC, a 16-core desktop.

DarkBurn depends only on the four small crates, egui/eframe/wgpu and tiny-skia, so a clean build should be a few minutes.

**Design documentation in the repo:** `CLAUDE.md` routes to `AGENTS.md`. `AGENTS.md` sets the hard rule: one outer UI container, colours from theme tokens. It routes UI work to `skills/solidcraft-ui/SKILL.md` and `docs/ui-kit.md`. `CONTRIBUTING.md` has no visual style guide. The look is described in words in:
- `crates/theme/src/lib.rs` header: "a blue-grey in the spirit of desktop CAD tools, viewport #2e3440"
- `docs/ui-kit.md`
- `docs/fonts.md`
- `docs/theme-colour-audit.md`
- `docs/ribbon-layout.md`
- `assets/icons/ui/STYLE_GUIDE.md`
- `docs/design/ux-redesign/` and `docs/design/preferences/`

## 7. The look, from the screenshot of the running app

The screenshot showed the dark theme with the Inspect tab active. The picture itself is not kept in this repository.

- **Chrome.** No OS frame. A single very dark (#1C1F26) app bar about 34 pt tall runs the full width. From left to right it holds:
  - the small blue isometric logo and "SolidCraft"
  - a home icon, a "File" text menu, then Save, Undo and Redo
  - a document tab (the document's name, slightly lifted surface, close ×) and a "+"
  - on the right: small status and settings icons, a "Report a bug" pill, the version "v0.40.0-rc.3"
  - flat minimize, maximize and close glyphs, Windows-style
- **Ribbon.** Below the app bar sits a mid-dark (#272B34) band:
  - On the far left, a "Design / Workspace" switcher with a large icon.
  - A row of text tabs: Model, Surface, Assemble, Insert, Inspect, Manage, View, Sheet Metal, Weldments, "+". The active tab is a filled blue pill.
  - Underneath, groups of icon-over-label buttons with icons about 20–24 px and labels about 11 pt, such as Measure, Mass Center, Properties, Section, Interference, Draft, Access, Contact, Zebra, Curvature, Curv. Comb, Isocurve, Play History, Dependents, Inspect.
  - Each group is captioned underneath in small dim text with a dropdown chevron: "Measure ▾", "Section ▾", "Check ▾", "Surface Quality ▾", "History ▾".
  - A "Search commands" field sits at the top right.
- **Panels.**
  - Left: a "Browser" card about 150 pt wide with a header ("Browser · N bodies ▾"). It holds a dense tree, about 15 pt per row, with disclosure triangles, small blue and grey icons, eye toggles in a right-hand column, and folders for Bodies, Sketches and Features.
  - Right: an "Assistant" card with a header, dropdown rows, chat bubbles, "done" status lines with green checks, and a prompt field with a round blue send button.
  - Bottom: a full-width "History" strip with transport controls (|◀ ◀ ▶ ▶|), an "All components" chip and a long row of small feature icons.
  - Floating over the viewport: a compact "Measure" card with a pencil icon, a title, a × close, an "Items | 1 selected" blue chip field, a result line and a "Close" button.
- **Viewport.** A flat slate background (#2E3440) with a faint grid. The model is light grey with dark edge lines and an orange-highlighted selection. A navigation cube and triad (red, green, blue) are at the top right, and a tiny translucent row of view tools sits at the bottom centre.
- **Colour mood.** Cool, low-contrast blue-greys across three or four steps of dark surfaces. One saturated accent, blue #3A88EE, used for the active tab, chips, icon fills and the send button. Status colours are used sparingly (green check, orange selection). There are no gradients and almost no shadows. Separation comes from flat surface steps and 1 px borders (#3F4552) with radii of 4–6 pt.
- **Type.** IBM Plex Sans, small: about 11–12 pt for UI text and about 10.5 pt for group captions and dim labels. Semibold is used for panel titles. Text is light grey (#DEE2E8), with dim grey (#949BA8) for secondary text.
- **Density.** High, like desktop CAD tools: 28 pt controls, 6/4 pt item spacing and narrow side panels. The viewport gets most of the window.
- **Icons.** Small isometric 3D-style icons with flat blue and grey faces and rounded strokes. Single-colour line glyphs are used for chrome (save, undo, close, chevrons). Icons are colourful only in the active (blue) ramp.

LightBurn's layout (top menu and toolbar, left tool column, centre canvas, right Cuts/Layers, Move and Laser panels) maps directly onto:
- the app bar
- a ribbon, or a slimmer single-row toolbar
- `Panel` cards on the left, right and bottom
- `Surface::Viewport` for the canvas

## 8. What must be rebuilt (not reusable)

1. **The app shell.** `DarkBurnApp` with `logic` (prefs → theme → shortcuts → update tick) and `ui` (app bar → modals → home/return → status bar → toolbar → side panels → central canvas → dialogs → resize zones). This follows `SolveApp::logic`/`ui`.
2. **App bar and menus.** Brand, File/Edit/Arrange/Laser Tools/Window/Help menus built on `ui_kit::Menu`, document tabs, and caption buttons. Copy titlebar geometry, but the close action calls DarkBurn's unsaved-changes flow.
3. **Toolbar or ribbon.** DarkBurn's own command registry, in place of `solidcraft_engine::CommandSpec`. Reuse the `fit()` and label helpers and the constants (34/30/86).
4. **Keymap and shortcuts.** A new keymap keyed to DarkBurn commands. `key_name` and `display` can be copied.
5. **Preferences window.** A new `Prefs` struct and pages on `Window::show_pages`, `settings_search` and `preferences_cards`, plus the JSON-in-`app.ron` storage key and `PrefsWriter`.
6. **Home / start page.** New, using `ui_kit::file_view`.
7. **Status bar.** About 30 lines on `Container` + `Surface::ApplicationBar`, 24 pt tall.
8. **Docked panels.** Fixed `Panel` cards. The draggable `panel_runtime` depends on the engine; port it only if tab-group docking is needed.
9. **Canvas.** The 2D design canvas (pan, zoom, rulers, grid, selection handles) is entirely new. SolidCraft's `navigation::handle` and viewport are 3D and engine-bound. Use the tokens `viewport_*`, `grid*`, `sel_*`, `box_window/crossing`, `rubber_band`.
10. **Laser-specific panels.** Cuts/Layers table (`table.rs`), Move/jog, Laser/console, Library, Camera, job progress (`status.rs`, `StageBar`).
11. **`Expressions` implementation.** For units (mm/in, mm/s vs mm/min, %).
12. **Locale catalogs.** DarkBurn's own `assets/locales/*.json`; SolidCraft's are CAD wording.
13. **App icon and laser-specific UI icons.** New SVGs in the same style, plus the Windows `.ico`, `build.rs` with `embed-resource`, and procedural `app_icon.rs`.
14. **Update check.** Constants (repo and asset names) and UI. HTTP and version logic can be copied.
15. **Optional.** Device-lost restart and crash journal (needs vendored egui-wgpu), and the macOS native menu (muda pattern).
