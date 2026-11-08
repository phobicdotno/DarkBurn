//! Every command DarkBurn has a menu item or button for: its id, label, shortcut and icon, and
//! the menus built from them. Commands whose feature arrives in a later phase are listed (so the
//! menus are complete) but disabled; [`works`] names the ones that run today.

#![allow(dead_code, reason = "the registry queries are used by the MCP backend, wired in the next step")]

/// A command: stable id, menu label, shortcut as shown, icon (empty for none).
#[derive(Clone, Copy, Debug)]
pub struct Cmd {
    pub id: &'static str,
    pub label: &'static str,
    pub key: &'static str,
    pub icon: &'static str,
    /// JSON Schema (as text) of the command's arguments; empty when it takes none.
    pub args: &'static str,
}

const fn c(id: &'static str, label: &'static str, key: &'static str, icon: &'static str) -> Cmd {
    Cmd { id, label, key, icon, args: "" }
}

/// A command that takes arguments (agents and scripts call these; it has no menu item).
const fn a(id: &'static str, label: &'static str, args: &'static str) -> Cmd {
    Cmd { id, label, key: "", icon: "", args }
}

pub const COMMANDS: &[Cmd] = &[
    // File
    c("file.new", "New", "Ctrl+N", "new"),
    c("file.new_window", "New Window", "", ""),
    c("file.open", "Open", "Ctrl+O", "open"),
    c("file.import", "Import", "Ctrl+I", "import"),
    c("file.notes", "Show Notes", "Ctrl+Alt+N", ""),
    c("file.save", "Save", "Ctrl+S", "save"),
    c("file.save_as", "Save As", "Ctrl+Shift+S", ""),
    c("file.save_gcode", "Save G-code", "Alt+Shift+L", ""),
    c("file.export", "Export", "Alt+X", "export"),
    c("file.print", "Print (black only)", "Ctrl+P", ""),
    c("file.print_colour", "Print (keep colours)", "Ctrl+Shift+P", ""),
    c("file.save_bitmap", "Save Processed Bitmap", "", ""),
    c("file.save_capture", "Save Background Capture", "", ""),
    c("file.prefs_folder", "Open Preferences Folder", "", "folder"),
    c("file.prefs_export", "Export Preferences", "", ""),
    c("file.prefs_import", "Import Preferences", "", ""),
    c("file.prefs_backup", "Load Preferences Backup", "", ""),
    c("file.hotkeys", "Edit Hotkeys", "", ""),
    c("file.exit", "Exit", "Ctrl+Q", ""),
    // Edit
    c("edit.undo", "Undo", "Ctrl+Z", "undo"),
    c("edit.redo", "Redo", "Ctrl+Shift+Z", "redo"),
    c("edit.select_all", "Select All", "Ctrl+A", ""),
    c("edit.invert_selection", "Invert Selection", "Ctrl+Shift+I", ""),
    c("edit.cut", "Cut", "Ctrl+X", "cut"),
    c("edit.copy", "Copy", "Ctrl+C", "copy"),
    c("edit.duplicate", "Duplicate", "Ctrl+D", ""),
    c("edit.paste", "Paste", "Ctrl+V", "paste"),
    c("edit.paste_in_place", "Paste in Place", "Alt+V", ""),
    c("edit.delete", "Delete", "Del", "delete"),
    c("edit.convert_path", "Convert to Path", "Ctrl+Shift+C", ""),
    c("edit.convert_bitmap", "Convert to Bitmap", "Ctrl+Shift+B", ""),
    c("edit.close_path", "Close Path", "", ""),
    c("edit.close_tolerance", "Close Selected Paths with Tolerance", "", ""),
    c("edit.auto_join", "Auto-Join Selected Shapes", "Alt+J", ""),
    c("edit.optimize", "Optimize Selected Shapes", "Alt+Shift+O", ""),
    c("edit.delete_duplicates", "Delete Duplicates", "Alt+D", ""),
    c("edit.select_open", "Select Open Shapes", "", ""),
    c("edit.select_open_fill", "Select Open Shapes Set to Fill", "", ""),
    c("edit.select_layer", "Select All Shapes in Current Layer", "", ""),
    c("edit.select_contained", "Select Contained Shapes", "", ""),
    c("edit.image_replace", "Replace Image", "", ""),
    c("edit.image_replace_fit", "Replace Image to Fit", "", ""),
    c("edit.image_refresh", "Refresh Image", "", ""),
    c("edit.image_crop", "Apply Crop Path to Image", "", ""),
    c("edit.image_flatten", "Flatten Image Crop", "", ""),
    c("edit.image_remove_crop", "Remove Crop from Image", "", ""),
    c("edit.image_remove_backup", "Remove Backup Path", "", ""),
    c("edit.settings", "Settings", "", "settings"),
    c("edit.device_settings", "Device Settings", "", "db_device_settings"),
    c("edit.machine_settings", "Machine Settings", "", ""),
    // Tools
    c("tools.select", "Select", "", "select"),
    c("tools.line", "Draw Lines", "Ctrl+L", "line"),
    c("tools.rect", "Rectangle", "Ctrl+R", "rect"),
    c("tools.ellipse", "Ellipse", "Ctrl+E", "ellipse"),
    c("tools.polygon", "Polygon", "", "polygon"),
    c("tools.nodes", "Edit Nodes", "Ctrl+`", "db_node_edit"),
    c("tools.tabs", "Add Tabs", "Ctrl+Tab", "db_tabs"),
    c("tools.text", "Edit Text", "Ctrl+T", "text"),
    c("tools.position_laser", "Position Laser", "Alt+L", "db_position_laser"),
    c("tools.measure", "Measure", "Alt+M", "measure"),
    c("tools.qr", "Create QR Code", "", "db_qr"),
    c("tools.offset", "Offset Shapes", "Alt+O", "offset"),
    c("tools.weld", "Weld Shapes", "Ctrl+W", "db_weld"),
    c("tools.union", "Boolean Union", "Alt++", "db_union"),
    c("tools.difference", "Boolean Difference", "Alt+-", "db_difference"),
    c("tools.intersection", "Boolean Intersection", "Alt+*", "db_intersection"),
    c("tools.boolean_assistant", "Boolean Assistant", "Ctrl+B", ""),
    c("tools.cut_shapes", "Cut Shapes", "Alt+Shift+C", "cut"),
    c("tools.adjust_image", "Adjust Image", "Alt+I", ""),
    c("tools.trace_image", "Trace Image", "Alt+T", "trace"),
    c("tools.path_to_text", "Apply Path to Text", "", ""),
    c("tools.mask_image", "Apply Mask to Image", "", ""),
    c("tools.resize_slots", "Resize Slots in Selection", "", ""),
    c("tools.select_smaller", "Select Shapes Smaller than Selected", "", ""),
    c("tools.warp", "Warp Selection (4 points)", "", ""),
    c("tools.deform", "Deform Selection (16 points)", "", ""),
    c("tools.start_points", "Start Points", "", "db_start_points"),
    c("tools.round_corners", "Round Corners", "", "db_round_corners"),
    // Arrange
    c("arrange.group", "Group", "Ctrl+G", "db_group"),
    c("arrange.ungroup", "Ungroup", "Ctrl+U", "db_ungroup"),
    c("arrange.auto_group", "Auto-Group", "", ""),
    c("arrange.flip_h", "Flip Horizontal", "Ctrl+Shift+H", "db_flip_h"),
    c("arrange.flip_v", "Flip Vertical", "Ctrl+Shift+V", "db_flip_v"),
    c("arrange.mirror", "Mirror Across Line", "Ctrl+Shift+M", "mirror"),
    c("arrange.rotate_cw", "Rotate 90° Clockwise", ".", ""),
    c("arrange.rotate_ccw", "Rotate 90° Counter-Clockwise", ",", ""),
    c("arrange.two_point", "Two-Point Rotate / Scale", "Ctrl+2", ""),
    c("arrange.align_left", "Align Left", "L", ""),
    c("arrange.align_right", "Align Right", "R", ""),
    c("arrange.align_top", "Align Top", "T", ""),
    c("arrange.align_bottom", "Align Bottom", "B", ""),
    c("arrange.align_h_centres", "Align Horizontal Centres", "E", "db_align_h"),
    c("arrange.align_v_centres", "Align Vertical Centres", "C", "db_align_v"),
    c("arrange.align_centres", "Align Centres", "", "db_align_centers"),
    c("arrange.distribute_h", "Distribute Horizontally", "", "db_distribute_h"),
    c("arrange.distribute_v", "Distribute Vertically", "", "db_distribute_v"),
    c("arrange.distribute_h_centres", "Distribute Horizontal Centres", "", ""),
    c("arrange.distribute_v_centres", "Distribute Vertical Centres", "", ""),
    c("arrange.distribute_h_spacing", "Distribute Horizontal Spacing", "", ""),
    c("arrange.distribute_v_spacing", "Distribute Vertical Spacing", "", ""),
    c("arrange.move_h_together", "Move Horizontally Together", "", ""),
    c("arrange.move_v_together", "Move Vertically Together", "", ""),
    c("arrange.same_width", "Make Same Width", "", "db_same_width"),
    c("arrange.same_height", "Make Same Height", "", "db_same_height"),
    c("arrange.move_selection", "Move Selection", "", "move"),
    c("arrange.nest", "Nest Selected", "", ""),
    c("arrange.dock_left", "Dock Left", "", "db_dock_left"),
    c("arrange.dock_right", "Dock Right", "", "db_dock_right"),
    c("arrange.dock_up", "Dock Up", "", "db_dock_up"),
    c("arrange.dock_down", "Dock Down", "", "db_dock_down"),
    c("arrange.page_centre", "Page Centre", "", ""),
    c("arrange.page_upper_left", "Upper Left", "", ""),
    c("arrange.page_top", "Top", "", ""),
    c("arrange.page_upper_right", "Upper Right", "", ""),
    c("arrange.page_left", "Left", "", ""),
    c("arrange.page_right", "Right", "", ""),
    c("arrange.page_lower_left", "Lower Left", "", ""),
    c("arrange.page_bottom", "Bottom", "", ""),
    c("arrange.page_lower_right", "Lower Right", "", ""),
    c("arrange.to_laser", "Move to Laser Position", "", ""),
    c("arrange.laser_centre", "Centre", "", ""),
    c("arrange.laser_upper_left", "Upper Left", "", ""),
    c("arrange.laser_top", "Top", "", ""),
    c("arrange.laser_upper_right", "Upper Right", "", ""),
    c("arrange.laser_left", "Left", "", ""),
    c("arrange.laser_right", "Right", "", ""),
    c("arrange.laser_lower_left", "Lower Left", "", ""),
    c("arrange.laser_bottom", "Bottom", "", ""),
    c("arrange.laser_lower_right", "Lower Right", "", ""),
    c("arrange.grid_array", "Grid Array", "", "pattern_rect"),
    c("arrange.circular_array", "Circular Array", "", "pattern_circ"),
    c("arrange.copy_along_path", "Copy Along Path", "", ""),
    c("arrange.rubber_band", "Create Rubber-Band Outline", "", ""),
    c("arrange.break_apart", "Break Apart", "Alt+B", ""),
    c("arrange.forward", "Push Forward", "PgUp", ""),
    c("arrange.backward", "Push Backward", "PgDn", ""),
    c("arrange.front", "Push to Front", "Ctrl+PgUp", ""),
    c("arrange.back", "Push to Back", "Ctrl+PgDn", ""),
    c("arrange.lock", "Lock Selected Shapes", "", ""),
    c("arrange.unlock", "Unlock Selected Shapes", "", ""),
    c("arrange.array_edit", "Edit Array", "", ""),
    c("arrange.array_add", "Add to Array", "", ""),
    c("arrange.array_remove", "Remove from Array", "", ""),
    c("arrange.array_exit", "Exit Array", "", ""),
    c("arrange.array_flatten", "Flatten Array", "", ""),
    c("arrange.array_duplicate", "Duplicate Array", "", ""),
    c("arrange.move_as_group", "Move as Group", "", ""),
    c("arrange.lock_inner", "Lock Inner Objects", "", ""),
    // Laser Tools
    c("laser.pnc_first", "Set First Target Position", "", ""),
    c("laser.pnc_second", "Set Second Target Position", "", ""),
    c("laser.pnc_align", "Align Output to Targets", "", ""),
    c("laser.pnc_align_noscale", "Align Output to Targets (no scaling)", "", ""),
    c("laser.pnc_reset", "Reset Print and Cut", "", ""),
    c("laser.pnc_wizard", "Print and Cut Wizard", "", ""),
    c("laser.camera_lens", "Calibrate Camera Lens", "", ""),
    c("laser.camera_alignment", "Calibrate Camera Alignment", "", ""),
    c("laser.rotary", "Rotary Setup", "Ctrl+Shift+R", "db_rotary"),
    c("laser.focus_test", "Focus Test", "", ""),
    c("laser.interval_test", "Interval Test", "", ""),
    c("laser.material_test", "Material Test", "", "db_material_test"),
    c("laser.centre_finder", "Centre Finder", "", ""),
    c("laser.feeder", "Feeder Setup", "Alt+Shift+F", ""),
    c("laser.cylinder", "Cylinder Correction", "", "db_cylinder"),
    c("laser.controller_info", "Get Controller Info", "", ""),
    c("laser.repeat_marking", "Repeat Marking (galvo)", "", ""),
    c("laser.lens_test", "Lens Correction Test (galvo)", "", ""),
    c("laser.material_test_galvo", "Material Test (galvo)", "", ""),
    c("laser.print_and_cut", "Print and Cut", "", "db_print_cut"),
    c("laser.capture", "Capture Camera Image", "", "db_capture"),
    // Window
    c("window.reset_layout", "Reset to Default Layout", "", ""),
    c("window.preview", "Preview", "Alt+P", "db_preview"),
    c("view.zoom_page", "Zoom to Page", "Ctrl+0", "db_zoom_page"),
    c("view.zoom_in", "Zoom In", "Ctrl+=", "db_zoom_in"),
    c("view.zoom_out", "Zoom Out", "Ctrl+-", "db_zoom_out"),
    c("view.frame_selection", "Frame Selection", "Ctrl+Shift+A", "db_frame_selection"),
    c("view.pan", "Pan View", "", "pan"),
    c("view.wire_coarse", "Wireframe, Coarse", "", ""),
    c("view.wire_smooth", "Wireframe, Smooth", "", ""),
    c("view.filled_coarse", "Filled, Coarse", "", ""),
    c("view.filled_smooth", "Filled, Smooth", "", ""),
    c("view.toggle_filled", "Toggle Wireframe / Filled", "Alt+Shift+W", ""),
    // Help
    c("help.about", "About DarkBurn", "", "help"),
    c("help.quick", "Quick Help", "F1", ""),
    c("help.docs", "Online Documentation", "", ""),
    c("help.support_data", "Generate Support Data", "", ""),
    c("help.camera", "Camera Selection Help", "", ""),
    c("help.updates", "Check for Updates", "", ""),
    c("help.debug_log", "Enable Debug Log", "", ""),
    // Selection and shapes, with arguments
    a(
        "selection.set",
        "Select Shapes by Id",
        r#"{"type":"object","properties":{"ids":{"type":"array","items":{"type":"integer"}}},"required":["ids"]}"#,
    ),
    a("selection.clear", "Clear Selection", ""),
    a(
        "shape.add_rect",
        "Add Rectangle",
        r#"{"type":"object","properties":{"x":{"type":"number","description":"left, mm"},"y":{"type":"number","description":"bottom, mm"},"width":{"type":"number"},"height":{"type":"number"},"radius":{"type":"number","description":"corner radius, negative rounds inwards"},"layer":{"type":"integer"}},"required":["x","y","width","height"]}"#,
    ),
    a(
        "shape.add_ellipse",
        "Add Ellipse",
        r#"{"type":"object","properties":{"x":{"type":"number"},"y":{"type":"number"},"width":{"type":"number"},"height":{"type":"number"},"layer":{"type":"integer"}},"required":["x","y","width","height"]}"#,
    ),
    a(
        "shape.add_polygon",
        "Add Polygon",
        r#"{"type":"object","properties":{"cx":{"type":"number"},"cy":{"type":"number"},"sides":{"type":"integer","minimum":3},"radius":{"type":"number"},"layer":{"type":"integer"}},"required":["cx","cy","sides","radius"]}"#,
    ),
    a(
        "shape.add_path",
        "Add Path",
        r#"{"type":"object","properties":{"svg_path":{"type":"string","description":"SVG path data in mm, Y up"},"layer":{"type":"integer"}},"required":["svg_path"]}"#,
    ),
    a(
        "shape.place",
        "Move Selection to a Point",
        r#"{"type":"object","properties":{"anchor":{"enum":["top_left","top","top_right","left","centre","right","bottom_left","bottom","bottom_right"]},"x":{"type":"number"},"y":{"type":"number"}},"required":["x","y"]}"#,
    ),
    a(
        "shape.translate",
        "Move Selection by",
        r#"{"type":"object","properties":{"dx":{"type":"number"},"dy":{"type":"number"}},"required":["dx","dy"]}"#,
    ),
    a(
        "shape.resize",
        "Resize Selection",
        r#"{"type":"object","properties":{"anchor":{"type":"string"},"width":{"type":"number"},"height":{"type":"number"}},"required":["width","height"]}"#,
    ),
    a(
        "shape.rotate",
        "Rotate Selection",
        r#"{"type":"object","properties":{"degrees":{"type":"number","description":"counter-clockwise"}},"required":["degrees"]}"#,
    ),
    a(
        "shape.set_layer",
        "Put Selection on a Layer",
        r#"{"type":"object","properties":{"layer":{"type":"integer","minimum":0,"maximum":31}},"required":["layer"]}"#,
    ),
    a(
        "shape.set_props",
        "Set Shape Properties",
        r#"{"type":"object","properties":{"name":{"type":"string"},"locked":{"type":"boolean"},"priority":{"type":"integer"},"power_scale":{"type":"number"}}}"#,
    ),
    // Layers, with arguments
    a("layer.select", "Make a Layer Current", r#"{"type":"object","properties":{"layer":{"type":"integer"}},"required":["layer"]}"#),
    a(
        "layer.assign",
        "Put Selection on a Layer and Make it Current",
        r#"{"type":"object","properties":{"layer":{"type":"integer"}},"required":["layer"]}"#,
    ),
    a("layer.select_shapes", "Select a Layer's Shapes", r#"{"type":"object","properties":{"layer":{"type":"integer"}},"required":["layer"]}"#),
    a(
        "layer.set_mode",
        "Set a Layer's Mode",
        r#"{"type":"object","properties":{"layer":{"type":"integer"},"mode":{"enum":["line","fill","fill_line","offset_fill","image"]}},"required":["layer","mode"]}"#,
    ),
    a(
        "layer.set_param",
        "Set a Layer Setting",
        r#"{"type":"object","properties":{"layer":{"type":"integer"},"field":{"enum":["name","speed","max_power","min_power","interval","passes","output","show","air"]},"value":{}},"required":["layer","field","value"]}"#,
    ),
    a(
        "layer.move",
        "Move a Layer in Output Order",
        r#"{"type":"object","properties":{"layer":{"type":"integer"},"delta":{"type":"integer"}},"required":["layer","delta"]}"#,
    ),
    a(
        "layer.delete",
        "Delete a Layer",
        r#"{"type":"object","properties":{"layer":{"type":"integer"},"with_shapes":{"type":"boolean"}},"required":["layer"]}"#,
    ),
    // Project and settings, with arguments
    a("project.info", "Project Summary", ""),
    a("project.open", "Open a Project File", r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#),
    a("project.save_as", "Save the Project to a File", r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#),
    a(
        "project.import",
        "Import Files",
        r#"{"type":"object","properties":{"paths":{"type":"array","items":{"type":"string"}},"x":{"type":"number"},"y":{"type":"number"}},"required":["paths"]}"#,
    ),
    a(
        "setting.set",
        "Change a Setting",
        r#"{"type":"object","properties":{"key":{"enum":["filled","smooth","show_grid","theme","page_width","page_height","autosave_minutes","svg_dpi","import_group","import_select","import_tool_layers"]},"value":{}},"required":["key","value"]}"#,
    ),
];

/// The commands that run in this build.
pub fn works(id: &str) -> bool {
    matches!(
        id,
        "file.exit"
            | "file.new"
            | "file.open"
            | "file.import"
            | "file.notes"
            | "file.save"
            | "file.save_as"
            | "file.prefs_folder"
            | "edit.settings"
            | "edit.undo"
            | "edit.redo"
            | "edit.select_all"
            | "edit.invert_selection"
            | "edit.cut"
            | "edit.copy"
            | "edit.duplicate"
            | "edit.paste"
            | "edit.paste_in_place"
            | "edit.delete"
            | "edit.select_open"
            | "edit.select_open_fill"
            | "edit.select_layer"
            | "edit.select_contained"
            | "tools.select"
            | "tools.line"
            | "tools.rect"
            | "tools.ellipse"
            | "tools.polygon"
            | "arrange.group"
            | "arrange.ungroup"
            | "arrange.flip_h"
            | "arrange.flip_v"
            | "arrange.rotate_cw"
            | "arrange.rotate_ccw"
            | "arrange.same_width"
            | "arrange.same_height"
            | "arrange.forward"
            | "arrange.backward"
            | "arrange.front"
            | "arrange.back"
            | "arrange.lock"
            | "arrange.unlock"
            | "window.reset_layout"
            | "view.zoom_page"
            | "view.zoom_in"
            | "view.zoom_out"
            | "view.frame_selection"
            | "view.pan"
            | "view.wire_coarse"
            | "view.wire_smooth"
            | "view.filled_coarse"
            | "view.filled_smooth"
            | "view.toggle_filled"
            | "help.about"
    ) || crate::machine_ui::handles(id)
        || id.starts_with("panel.")
        || id.starts_with("toolbar.")
        || id.starts_with("lang.")
        || id.starts_with("file.recent.")
        || id.starts_with("selection.")
        || id.starts_with("shape.")
        || id.starts_with("layer.")
        || id.starts_with("project.")
        || id.starts_with("setting.")
        || id.starts_with("arrange.align_")
        || id.starts_with("arrange.distribute_")
        || id.starts_with("arrange.move_h_")
        || id.starts_with("arrange.move_v_")
        || (id.starts_with("arrange.page_"))
}

/// The keyboard shortcut a menu label like "Ctrl+Shift+Z" names.
pub fn parse_key(text: &str) -> Option<egui::KeyboardShortcut> {
    use egui::{Key, Modifiers};
    if text.is_empty() {
        return None;
    }
    let (mods, key) = if let Some(rest) = text.strip_suffix("++") {
        (rest, "+")
    } else {
        match text.rfind('+') {
            Some(i) => (&text[..i], &text[i + 1..]),
            None => ("", text),
        }
    };
    let mut m = Modifiers::NONE;
    for part in mods.split('+').filter(|p| !p.is_empty()) {
        match part {
            "Ctrl" => m |= Modifiers::COMMAND,
            "Shift" => m |= Modifiers::SHIFT,
            "Alt" => m |= Modifiers::ALT,
            _ => return None,
        }
    }
    let key = match key {
        "Del" => Key::Delete,
        "PgUp" => Key::PageUp,
        "PgDn" => Key::PageDown,
        "." => Key::Period,
        "," => Key::Comma,
        "=" => Key::Equals,
        "-" => Key::Minus,
        "+" => Key::Plus,
        "`" => Key::Backtick,
        "Tab" => Key::Tab,
        other => Key::from_name(other)?,
    };
    Some(egui::KeyboardShortcut::new(m, key))
}

/// Every command shortcut, those with more modifier keys first so Ctrl+Shift+Z is not taken
/// for Ctrl+Z.
pub fn shortcut_table() -> &'static [(egui::KeyboardShortcut, &'static str)] {
    static TABLE: std::sync::OnceLock<Vec<(egui::KeyboardShortcut, &'static str)>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut v: Vec<_> = COMMANDS.iter().filter_map(|c| parse_key(c.key).map(|k| (k, c.id))).collect();
        let weight = |k: &egui::KeyboardShortcut| k.modifiers.command as u8 + k.modifiers.shift as u8 + k.modifiers.alt as u8;
        v.sort_by_key(|(k, _)| std::cmp::Reverse(weight(k)));
        v
    })
}

pub fn find(id: &str) -> Option<&'static Cmd> {
    COMMANDS.iter().chain(crate::machine_ui::COMMANDS).find(|c| c.id == id)
}

/// Commands that move axes, fire the laser, send a job, write machine settings or connect to a
/// device. An agent must ask before running one; none is implemented yet.
pub fn hardware(id: &str) -> bool {
    crate::machine_ui::is_hardware(id)
        || id.starts_with("laser.")
        || id.starts_with("arrange.laser_")
        || matches!(id, "file.save_gcode" | "edit.device_settings" | "edit.machine_settings" | "arrange.to_laser" | "tools.position_laser")
}

/// The checklist items a command implements (the ones that work today).
pub fn checklist(id: &str) -> &'static [&'static str] {
    match id {
        "file.new" => &["FILE-01"],
        "file.open" | "project.open" => &["FILE-04"],
        "file.import" | "project.import" => &["FILE-05", "IMP-13"],
        "file.notes" => &["FILE-06"],
        "file.save" => &["FILE-07"],
        "file.save_as" | "project.save_as" => &["FILE-08"],
        "edit.undo" => &["EDIT-01"],
        "edit.redo" => &["EDIT-02"],
        "edit.select_all" => &["EDIT-03"],
        "edit.invert_selection" => &["EDIT-04"],
        "edit.cut" => &["EDIT-05"],
        "edit.copy" => &["EDIT-06"],
        "edit.duplicate" => &["EDIT-07"],
        "edit.paste" => &["EDIT-08"],
        "edit.paste_in_place" => &["EDIT-09"],
        "edit.delete" => &["EDIT-10"],
        "edit.select_open" => &["EDIT-18"],
        "edit.select_open_fill" => &["EDIT-19"],
        "edit.select_layer" => &["EDIT-20"],
        "edit.select_contained" => &["EDIT-21"],
        "tools.select" => &["TOOLS-01"],
        "tools.line" => &["TOOLS-02", "DRAW-01", "DRAW-02"],
        "tools.rect" | "shape.add_rect" => &["TOOLS-03", "DRAW-04"],
        "tools.ellipse" | "shape.add_ellipse" => &["TOOLS-04", "DRAW-06"],
        "tools.polygon" | "shape.add_polygon" => &["TOOLS-05", "DRAW-07"],
        "arrange.group" => &["ARR-01"],
        "arrange.ungroup" => &["ARR-02"],
        "arrange.flip_h" => &["ARR-04"],
        "arrange.flip_v" => &["ARR-05"],
        "arrange.rotate_cw" => &["ARR-07"],
        "arrange.rotate_ccw" => &["ARR-08"],
        "arrange.align_left" => &["ARR-10"],
        "arrange.align_right" => &["ARR-11"],
        "arrange.align_top" => &["ARR-12"],
        "arrange.align_bottom" => &["ARR-13"],
        "arrange.align_h_centres" => &["ARR-14"],
        "arrange.align_v_centres" => &["ARR-15"],
        "arrange.align_centres" => &["ARR-16"],
        "arrange.distribute_h" => &["ARR-17"],
        "arrange.distribute_v" => &["ARR-18"],
        "arrange.distribute_h_centres" => &["ARR-19"],
        "arrange.distribute_v_centres" => &["ARR-20"],
        "arrange.distribute_h_spacing" => &["ARR-21"],
        "arrange.distribute_v_spacing" => &["ARR-22"],
        "arrange.move_h_together" => &["ARR-23"],
        "arrange.move_v_together" => &["ARR-24"],
        "arrange.forward" => &["ARR-35"],
        "arrange.backward" => &["ARR-36"],
        "arrange.front" => &["ARR-37"],
        "arrange.back" => &["ARR-38"],
        "arrange.lock" => &["ARR-39"],
        "arrange.unlock" => &["ARR-40"],
        "shape.place" | "shape.translate" | "shape.resize" | "shape.rotate" => &["TB-12", "TB-14"],
        "shape.set_props" => &["SP-06"],
        "shape.set_layer" | "layer.assign" => &["TB-20"],
        "layer.select_shapes" => &["TB-22"],
        "layer.set_mode" => &["MODE-01", "CUTS-01"],
        "layer.set_param" => &["CUTS-08", "CSE-01"],
        "layer.move" => &["CUTS-03"],
        "layer.delete" => &["CUTS-04"],
        "view.frame_selection" => &["WIN-06"],
        "view.wire_coarse" | "view.wire_smooth" | "view.filled_coarse" | "view.filled_smooth" => &["WIN-07"],
        "view.toggle_filled" => &["WIN-08"],
        "view.zoom_in" => &["WIN-04"],
        "view.zoom_out" => &["WIN-05"],
        "view.zoom_page" => &["WIN-03"],
        _ if crate::machine_ui::handles(id) => crate::machine_ui::checklist(id),
        _ if id.starts_with("file.recent.") => &["FILE-03"],
        _ if id.starts_with("arrange.page_") => &["ARR-27"],
        _ => &[],
    }
}

/// Everything known about one command, for lists and descriptions.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Info {
    pub id: String,
    pub title: String,
    pub group: String,
    /// The menu items that lead to it, outermost first (empty when it has no menu item).
    pub menu_path: Vec<String>,
    pub shortcut: String,
    /// JSON Schema of the arguments (`{}` when there are none).
    pub args_schema: serde_json::Value,
    pub hardware: bool,
    pub checklist: Vec<String>,
    /// It does something in this build; the rest are listed but do nothing yet.
    pub implemented: bool,
}

/// Every registered command: the table above plus the ones generated from the panels,
/// toolbars, languages and recent projects.
pub fn registry() -> &'static [Cmd] {
    static ALL: std::sync::OnceLock<Vec<Cmd>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
        let mut v: Vec<Cmd> = COMMANDS.to_vec();
        v.extend_from_slice(crate::machine_ui::COMMANDS);
        for p in crate::panels::PANELS {
            v.push(c(leak(format!("panel.{}", p.id)), leak(format!("Show or Hide {}", p.title)), "", ""));
        }
        for (id, label) in crate::prefs::TOOLBARS.iter().copied().chain([("status", "Status Bar")]) {
            v.push(c(leak(format!("toolbar.{id}")), leak(format!("Show or Hide {label}")), "", ""));
        }
        for (code, name) in LANGUAGES {
            v.push(c(leak(format!("lang.{code}")), name, "", ""));
        }
        for i in 0..24 {
            v.push(c(leak(format!("file.recent.{i}")), leak(format!("Open Recent Project {}", i + 1)), "", ""));
        }
        v
    })
}

fn menu_paths() -> &'static std::collections::HashMap<&'static str, Vec<String>> {
    static PATHS: std::sync::OnceLock<std::collections::HashMap<&'static str, Vec<String>>> = std::sync::OnceLock::new();
    PATHS.get_or_init(|| {
        fn walk(entries: &'static [Entry], path: &mut Vec<String>, out: &mut std::collections::HashMap<&'static str, Vec<String>>) {
            for e in entries {
                match e {
                    Entry::Cmd(id) => {
                        let mut p = path.clone();
                        if let Some(c) = find(id) {
                            p.push(c.label.to_string());
                        }
                        out.entry(id).or_insert(p);
                    }
                    Entry::Sub(name, inner) => {
                        path.push(name.to_string());
                        walk(inner, path, out);
                        path.pop();
                    }
                    _ => {}
                }
            }
        }
        let mut out = std::collections::HashMap::new();
        for (menu, entries) in MENUS {
            walk(entries, &mut vec![menu.to_string()], &mut out);
        }
        out
    })
}

/// The registered command `id`.
pub fn lookup(id: &str) -> Option<&'static Cmd> {
    registry().iter().find(|c| c.id == id)
}

/// Description of command `id`.
pub fn describe(id: &str) -> Option<Info> {
    let cmd = lookup(id)?;
    let schema = if cmd.args.is_empty() { None } else { serde_json::from_str(cmd.args).ok() };
    Some(Info {
        id: cmd.id.to_string(),
        title: cmd.label.to_string(),
        group: cmd.id.split('.').next().unwrap_or("").to_string(),
        menu_path: menu_paths().get(cmd.id).cloned().unwrap_or_default(),
        shortcut: cmd.key.to_string(),
        args_schema: schema.unwrap_or_else(|| serde_json::json!({})),
        hardware: hardware(cmd.id),
        checklist: checklist(cmd.id).iter().map(|s| s.to_string()).collect(),
        implemented: works(cmd.id),
    })
}

/// Every registered command.
pub fn list() -> Vec<Info> {
    registry().iter().filter_map(|c| describe(c.id)).collect()
}

/// Whether the command can run right now (it works in this build and has something to act on).
pub fn enabled(app: &crate::shell::DarkBurn, id: &str) -> bool {
    works(id) && app.can_run(id)
}

/// Every action id the interface renders: menu items, ribbon and tool buttons, panel, toolbar
/// and language entries and recent projects. A test checks that each one is registered.
pub fn ui_action_ids() -> Vec<String> {
    fn walk(entries: &[Entry], out: &mut Vec<String>) {
        for e in entries {
            match e {
                Entry::Cmd(id) => out.push(id.to_string()),
                Entry::Sub(_, inner) => walk(inner, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for (_, entries) in MENUS {
        walk(entries, &mut out);
    }
    out.extend(crate::ribbon::command_ids().into_iter().map(String::from));
    out.extend(crate::shell::tool_ids().iter().map(|s| s.to_string()));
    out.extend(crate::panels::PANELS.iter().map(|p| format!("panel.{}", p.id)));
    out.extend(crate::prefs::TOOLBARS.iter().map(|(id, _)| format!("toolbar.{id}")));
    out.push("toolbar.status".into());
    out.extend(LANGUAGES.iter().map(|(code, _)| format!("lang.{code}")));
    out.extend((0..24).map(|i| format!("file.recent.{i}")));
    out.sort();
    out.dedup();
    out
}

/// One row of a menu.
#[derive(Clone, Copy, Debug)]
pub enum Entry {
    Cmd(&'static str),
    Sep,
    Sub(&'static str, &'static [Entry]),
    /// Filled in by the app: panel and toolbar check items, recent files, languages.
    Dynamic(&'static str),
}

use Entry::{Cmd as C, Dynamic as D, Sep as S, Sub};

pub const MENUS: &[(&str, &[Entry])] = &[
    (
        "File",
        &[
            C("file.new"),
            C("file.new_window"),
            D("recent"),
            C("file.open"),
            C("file.import"),
            C("file.notes"),
            S,
            C("file.save"),
            C("file.save_as"),
            C("file.save_gcode"),
            C("file.export"),
            S,
            C("file.print"),
            C("file.print_colour"),
            S,
            C("file.save_bitmap"),
            C("file.save_capture"),
            S,
            C("file.prefs_folder"),
            C("file.prefs_export"),
            C("file.prefs_import"),
            C("file.prefs_backup"),
            S,
            C("file.hotkeys"),
            S,
            C("file.exit"),
        ],
    ),
    (
        "Edit",
        &[
            C("edit.undo"),
            C("edit.redo"),
            S,
            C("edit.select_all"),
            C("edit.invert_selection"),
            S,
            C("edit.cut"),
            C("edit.copy"),
            C("edit.duplicate"),
            C("edit.paste"),
            C("edit.paste_in_place"),
            C("edit.delete"),
            S,
            C("edit.convert_path"),
            C("edit.convert_bitmap"),
            C("edit.close_path"),
            C("edit.close_tolerance"),
            C("edit.auto_join"),
            C("edit.optimize"),
            C("edit.delete_duplicates"),
            S,
            C("edit.select_open"),
            C("edit.select_open_fill"),
            C("edit.select_layer"),
            C("edit.select_contained"),
            S,
            Sub(
                "Image Options",
                &[
                    C("edit.image_replace"),
                    C("edit.image_replace_fit"),
                    C("edit.image_refresh"),
                    C("edit.image_crop"),
                    C("edit.image_flatten"),
                    C("edit.image_remove_crop"),
                    C("edit.image_remove_backup"),
                ],
            ),
            S,
            C("edit.settings"),
            C("edit.device_settings"),
            C("edit.machine_settings"),
        ],
    ),
    (
        "Tools",
        &[
            C("tools.select"),
            C("tools.line"),
            C("tools.rect"),
            C("tools.ellipse"),
            C("tools.polygon"),
            C("tools.nodes"),
            C("tools.tabs"),
            C("tools.text"),
            C("tools.position_laser"),
            C("tools.measure"),
            C("tools.qr"),
            S,
            C("tools.offset"),
            C("tools.weld"),
            C("tools.union"),
            C("tools.difference"),
            C("tools.intersection"),
            C("tools.boolean_assistant"),
            S,
            C("tools.cut_shapes"),
            C("tools.adjust_image"),
            C("tools.trace_image"),
            S,
            C("tools.path_to_text"),
            C("tools.mask_image"),
            C("tools.resize_slots"),
            C("tools.select_smaller"),
            C("tools.warp"),
            C("tools.deform"),
        ],
    ),
    (
        "Arrange",
        &[
            C("arrange.group"),
            C("arrange.ungroup"),
            C("arrange.auto_group"),
            S,
            C("arrange.flip_h"),
            C("arrange.flip_v"),
            C("arrange.mirror"),
            C("arrange.rotate_cw"),
            C("arrange.rotate_ccw"),
            C("arrange.two_point"),
            S,
            Sub(
                "Align",
                &[
                    C("arrange.align_left"),
                    C("arrange.align_right"),
                    C("arrange.align_top"),
                    C("arrange.align_bottom"),
                    C("arrange.align_h_centres"),
                    C("arrange.align_v_centres"),
                    C("arrange.align_centres"),
                ],
            ),
            Sub(
                "Distribute",
                &[
                    C("arrange.distribute_h"),
                    C("arrange.distribute_v"),
                    C("arrange.distribute_h_centres"),
                    C("arrange.distribute_v_centres"),
                    C("arrange.distribute_h_spacing"),
                    C("arrange.distribute_v_spacing"),
                    C("arrange.move_h_together"),
                    C("arrange.move_v_together"),
                ],
            ),
            S,
            C("arrange.nest"),
            Sub("Dock", &[C("arrange.dock_left"), C("arrange.dock_right"), C("arrange.dock_up"), C("arrange.dock_down")]),
            Sub(
                "Move Selected Objects",
                &[
                    C("arrange.page_centre"),
                    C("arrange.page_upper_left"),
                    C("arrange.page_top"),
                    C("arrange.page_upper_right"),
                    C("arrange.page_left"),
                    C("arrange.page_right"),
                    C("arrange.page_lower_left"),
                    C("arrange.page_bottom"),
                    C("arrange.page_lower_right"),
                    S,
                    C("arrange.to_laser"),
                ],
            ),
            Sub(
                "Move Laser to Selection",
                &[
                    C("arrange.laser_centre"),
                    C("arrange.laser_upper_left"),
                    C("arrange.laser_top"),
                    C("arrange.laser_upper_right"),
                    C("arrange.laser_left"),
                    C("arrange.laser_right"),
                    C("arrange.laser_lower_left"),
                    C("arrange.laser_bottom"),
                    C("arrange.laser_lower_right"),
                ],
            ),
            S,
            C("arrange.grid_array"),
            C("arrange.circular_array"),
            C("arrange.copy_along_path"),
            S,
            C("arrange.rubber_band"),
            C("arrange.break_apart"),
            S,
            C("arrange.forward"),
            C("arrange.backward"),
            C("arrange.front"),
            C("arrange.back"),
            S,
            C("arrange.lock"),
            C("arrange.unlock"),
            S,
            Sub(
                "Virtual Array",
                &[
                    C("arrange.array_edit"),
                    C("arrange.array_add"),
                    C("arrange.array_remove"),
                    C("arrange.array_exit"),
                    C("arrange.array_flatten"),
                    C("arrange.array_duplicate"),
                ],
            ),
        ],
    ),
    (
        "Laser Tools",
        &[
            C("laser.devices"),
            S,
            Sub(
                "Print and Cut",
                &[
                    C("laser.pnc_first"),
                    C("laser.pnc_second"),
                    C("laser.pnc_align"),
                    C("laser.pnc_align_noscale"),
                    C("laser.pnc_reset"),
                    S,
                    C("laser.pnc_wizard"),
                ],
            ),
            S,
            C("laser.camera_lens"),
            C("laser.camera_alignment"),
            S,
            C("laser.rotary"),
            S,
            C("laser.focus_test"),
            C("laser.interval_test"),
            C("laser.material_test"),
            C("laser.centre_finder"),
            S,
            C("laser.feeder"),
            C("laser.cylinder"),
            C("laser.controller_info"),
            S,
            C("laser.repeat_marking"),
            C("laser.lens_test"),
            C("laser.material_test_galvo"),
        ],
    ),
    (
        "Window",
        &[
            C("window.reset_layout"),
            S,
            C("window.preview"),
            C("view.zoom_page"),
            C("view.zoom_in"),
            C("view.zoom_out"),
            C("view.frame_selection"),
            S,
            Sub("View Style", &[C("view.wire_coarse"), C("view.wire_smooth"), C("view.filled_coarse"), C("view.filled_smooth")]),
            C("view.toggle_filled"),
            S,
            D("panels"),
            S,
            D("toolbars"),
        ],
    ),
    ("Language", &[D("languages")]),
    (
        "Help",
        &[
            C("help.about"),
            S,
            C("help.quick"),
            C("help.docs"),
            S,
            C("help.support_data"),
            C("help.camera"),
            S,
            C("help.updates"),
            S,
            C("help.debug_log"),
        ],
    ),
];

/// Interface languages, each named in its own language. Only English has text so far.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("fr", "Français"),
    ("it", "Italiano"),
    ("nl", "Nederlands"),
    ("nb", "Norsk bokmål"),
    ("pl", "Polski"),
    ("pt", "Português"),
    ("sv", "Svenska"),
    ("da", "Dansk"),
    ("fi", "Suomi"),
    ("cs", "Čeština"),
    ("hu", "Magyar"),
    ("tr", "Türkçe"),
    ("el", "Ελληνικά"),
    ("ru", "Русский"),
    ("uk", "Українська"),
    ("ja", "日本語"),
    ("zh", "简体中文"),
    ("zh-TW", "繁體中文"),
    ("ko", "한국어"),
    ("he", "עברית"),
    ("ar", "العربية"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(entries: &[Entry], f: &mut impl FnMut(&str)) {
        for e in entries {
            match e {
                Entry::Cmd(id) => f(id),
                Entry::Sub(_, inner) => walk(inner, f),
                _ => {}
            }
        }
    }

    #[test]
    fn every_menu_command_exists_once() {
        let mut ids = std::collections::HashSet::new();
        for c in COMMANDS {
            assert!(ids.insert(c.id), "{} is listed twice", c.id);
        }
        for (_, entries) in MENUS {
            walk(entries, &mut |id| assert!(find(id).is_some(), "menu names unknown command {id}"));
        }
    }

    #[test]
    fn every_action_the_ui_renders_is_registered() {
        for id in ui_action_ids() {
            assert!(lookup(&id).is_some(), "{id} is shown in the interface but is not in the command registry");
        }
        let mut seen = std::collections::HashSet::new();
        for c in registry() {
            assert!(seen.insert(c.id), "{} registered twice", c.id);
            let info = describe(c.id).unwrap();
            assert!(info.args_schema.is_object(), "{} schema", c.id);
            if !c.args.is_empty() {
                assert!(serde_json::from_str::<serde_json::Value>(c.args).is_ok(), "{} schema is not JSON", c.id);
            }
        }
    }

    #[test]
    fn hardware_commands_are_flagged_and_do_nothing() {
        for info in list().into_iter().filter(|i| i.hardware) {
            // The machine commands run, but api::execute lets an agent reach only the simulators.
            assert!(!info.implemented || crate::machine_ui::handles(&info.id), "{} touches hardware and must not run yet", info.id);
        }
        assert!(describe("laser.rotary").unwrap().hardware);
        assert!(!describe("edit.undo").unwrap().hardware);
        assert_eq!(describe("shape.add_rect").unwrap().args_schema["required"].as_array().unwrap().len(), 4);
        assert_eq!(describe("arrange.align_left").unwrap().menu_path, ["Arrange", "Align", "Align Left"]);
        assert_eq!(describe("edit.undo").unwrap().checklist, ["EDIT-01"]);
        assert!(list().len() > 250);
    }

    #[test]
    fn all_top_level_menus_are_present() {
        let names: Vec<_> = MENUS.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["File", "Edit", "Tools", "Arrange", "Laser Tools", "Window", "Language", "Help"]);
    }

    #[test]
    fn working_commands_are_few_and_known() {
        for id in ["file.exit", "view.zoom_in", "help.about", "window.reset_layout"] {
            assert!(works(id));
        }
        for id in ["file.save_gcode", "tools.offset", "arrange.grid_array"] {
            assert!(!works(id), "{id} arrives in a later phase");
        }
    }

    #[test]
    fn shortcuts_parse_from_menu_labels() {
        use egui::{Key, Modifiers};
        assert_eq!(parse_key("Ctrl+Shift+Z"), Some(egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)));
        assert_eq!(parse_key("Alt++"), Some(egui::KeyboardShortcut::new(Modifiers::ALT, Key::Plus)));
        assert_eq!(parse_key("PgUp").map(|k| k.logical_key), Some(Key::PageUp));
        assert_eq!(parse_key(""), None);
        let table = shortcut_table();
        let undo = table.iter().position(|(_, id)| *id == "edit.undo").unwrap();
        let redo = table.iter().position(|(_, id)| *id == "edit.redo").unwrap();
        assert!(redo < undo, "Ctrl+Shift+Z is tried before Ctrl+Z");
        for c in COMMANDS.iter().filter(|c| !c.key.is_empty() && c.key != "Alt+*") {
            assert!(parse_key(c.key).is_some(), "{} does not parse", c.key);
        }
    }
}
