//! The machine dialogs: Devices, New Device wizard, Find My Laser, Device Settings, Machine
//! Settings, controller info, the Connect confirmation with the REAL MACHINE warning, the close
//! warning, the pre-job checklist, the overwrite question and the macro editor.
//! The logic is in `devices.rs` and `machine_ui.rs`; these functions only draw and call it.

use darkburn_grbl::{SettingInfo, setting_info};
use darkburn_job::Corner;
use darkburn_job::gcode::{AirCommand, Flavor, MarlinPower};
use darkburn_ruida::settings::{self, Group, Unit};
use darkburn_ui::ui_kit::{self, Action, BackingTooltipExt as _, Button, Confirm, StatusTone, Tone};
use egui::{Context, Id, RichText, Ui};
use serde_json::json;
use solidcraft_theme::Tokens;

use crate::devices::{ConnectionKind, Device, DeviceType, RUIDA_DISCOVERY_NOTE, SortBy, Wizard, WizardStep, find_my_laser};
use crate::i18n::tr;
use crate::job_build::Transfer;
use crate::machine_api::{Family, SettingsBackup};
use crate::machine_ui;
use crate::shell::DarkBurn;

fn small(ui: &mut Ui, label: &str, tip: &str, enabled: bool) -> bool {
    ui.add_enabled_ui(enabled, |ui| ui_kit::button(ui, label, Button::Small).backing_on_hover_text(tip).clicked()).inner
}

fn note(ui: &mut Ui, text: &str) {
    let t = Tokens::get();
    ui.label(RichText::new(text).size(t.font_small).color(t.text_dim));
}

/// All the machine dialogs, once per frame.
pub fn dialogs(ctx: &Context, app: &mut DarkBurn) {
    devices_dialog(ctx, app);
    wizard_dialog(ctx, app);
    find_dialog(ctx, app);
    settings_dialog(ctx, app);
    machine_settings_dialog(ctx, app);
    controller_info_dialog(ctx, app);
    confirm_connect_dialog(ctx, app);
    confirm_close_dialog(ctx, app);
    checklist_dialog(ctx, app);
    overwrite_dialog(ctx, app);
    macro_dialog(ctx, app);
    crate::galvo_ui::dialogs(ctx, app);
    crate::gweike_ui::dialogs(ctx, app);
}

// ----- Devices [DEV-01, DEV-04, DEV-05] ---------------------------------------------------------

fn devices_dialog(ctx: &Context, app: &mut DarkBurn) {
    if !app.machine.dlg.devices {
        return;
    }
    let (closed, action) =
        ui_kit::Window::new(Id::unique("db_devices"), "settings", "Devices").cards(3.0).actions("", "Close", None).show(ctx, true, |ui| {
            let t = Tokens::get();
            ui.horizontal_wrapped(|ui| {
                if small(ui, "New", "Add a device with the wizard", true) {
                    app.machine.dlg.wizard = Some(Wizard::new());
                }
                if small(ui, "Find my laser", "List the serial ports that look like a laser. Nothing is opened.", true) {
                    app.machine.rescan_ports();
                    app.machine.dlg.find = Some(find_my_laser(&app.machine.ports));
                }
                if small(ui, "Import", "Add a device from a .dbdev file", true)
                    && let Some(p) = rfd::FileDialog::new().add_filter("DarkBurn device", &["dbdev"]).pick_file()
                {
                    match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| app.machine.devices.import_profile(&t)) {
                        Ok(id) => app.machine.dlg.devices_sel = Some(id),
                        Err(e) => app.machine.dlg.message = Some(e),
                    }
                }
                if small(ui, "Sort by name", "", true) {
                    app.machine.devices.sort(SortBy::Name);
                }
                if small(ui, "Sort by type", "", true) {
                    app.machine.devices.sort(SortBy::Type);
                }
            });
            if let Some(m) = app.machine.dlg.message.clone() {
                ui_kit::status_line(ui, &m, StatusTone::Error);
            }
            ui.separator();
            let sel = app.machine.dlg.devices_sel.or(app.machine.devices.active);
            let mut reorder: Option<(usize, usize)> = None;
            let n = app.machine.devices.devices.len();
            for i in 0..n {
                let d = app.machine.devices.devices[i].clone();
                let active = app.machine.devices.active == Some(d.id);
                ui.horizontal(|ui| {
                    let item = ui.dnd_drag_source(ui.make_persistent_id(("db_dev_row", d.id)), i, |ui| {
                        let label = format!(
                            "{}{}   ({}, {} x {} mm)",
                            if active { "\u{25CF} " } else { "" },
                            d.name,
                            d.kind.label(),
                            d.width_mm,
                            d.height_mm
                        );
                        ui.selectable_label(sel == Some(d.id), label)
                    });
                    if item.inner.clicked() {
                        app.machine.dlg.devices_sel = Some(d.id);
                    }
                    if item.inner.double_clicked() {
                        app.machine.select_device(d.id);
                    }
                    if let Some(from) = item.response.dnd_release_payload::<usize>() {
                        reorder = Some((*from, i));
                    }
                    if item.response.dnd_hover_payload::<usize>().is_some() {
                        ui.painter().hline(item.response.rect.x_range(), item.response.rect.bottom(), egui::Stroke::new(2.0, t.accent));
                    }
                    if small(ui, ui_kit::MOVE_UP, "Move up", i > 0) {
                        reorder = Some((i, i - 1));
                    }
                    if small(ui, ui_kit::MOVE_DOWN, "Move down", i + 1 < n) {
                        reorder = Some((i, i + 1));
                    }
                });
            }
            if let Some((from, to)) = reorder {
                app.machine.devices.move_device(from, to);
            }
            ui.separator();
            let sel = app.machine.dlg.devices_sel.or(app.machine.devices.active);
            ui.horizontal_wrapped(|ui| {
                let has = sel.is_some();
                if small(ui, "Use", "Make it the device the panels use", has)
                    && let Some(id) = sel
                {
                    app.machine.select_device(id);
                }
                if small(ui, "Edit", "Open its settings", has)
                    && let Some(d) = sel.and_then(|id| app.machine.devices.get(id).cloned())
                {
                    app.machine.dlg.settings = Some((d, 0));
                }
                if small(ui, "Duplicate", "Make a copy", has)
                    && let Some(id) = sel
                {
                    app.machine.dlg.devices_sel = app.machine.devices.duplicate(id);
                }
                if small(ui, "Remove", "Remove it from the list (the last device stays)", has && n > 1)
                    && let Some(id) = sel
                {
                    app.machine.devices.remove(id);
                    app.machine.dlg.devices_sel = None;
                }
                if small(ui, "Export", "Save it as a .dbdev file", has)
                    && let Some(id) = sel
                {
                    export_device(app, id);
                }
            });
        });
    if closed || action != Action::None {
        app.machine.dlg.devices = false;
        app.machine.dlg.message = None;
    }
}

fn export_device(app: &mut DarkBurn, id: u64) {
    let Some(text) = app.machine.devices.export_profile(id) else { return };
    let name = app.machine.devices.get(id).map(|d| d.name.clone()).unwrap_or_else(|| "device".into());
    if let Some(p) = rfd::FileDialog::new().add_filter("DarkBurn device", &["dbdev"]).set_file_name(format!("{name}.dbdev")).save_file()
        && let Err(e) = std::fs::write(&p, text)
    {
        app.machine.dlg.message = Some(format!("Could not save {}: {e}", p.display()));
    }
}

// ----- Wizard [DEV-03] -------------------------------------------------------------------------------

fn wizard_dialog(ctx: &Context, app: &mut DarkBurn) {
    let Some(mut w) = app.machine.dlg.wizard.take() else { return };
    let last = w.step == WizardStep::Summary;
    let title = format!("New device: {}", w.step.label());
    let (closed, action) = ui_kit::Window::new(Id::unique("db_device_wizard"), "settings", &title)
        .cards(3.0)
        .fields()
        .actions(if last { "Add" } else { "Next" }, "Cancel", None)
        .show(ctx, w.can_next() || (last && w.problems().is_empty()), |ui| {
            let steps: Vec<&str> = WizardStep::ALL.iter().map(|s| s.label()).collect();
            ui_kit::steps(ui, "wizard_steps", &steps, w.step.index(), None);
            match w.step {
                WizardStep::Type => {
                    note(ui, "What kind of laser is it? Pick a simulator to try DarkBurn with no hardware.");
                    egui::ScrollArea::vertical().id_salt("wizard_types").max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                        for k in DeviceType::ALL {
                            match k.needs_capture() {
                                None => {
                                    if ui.selectable_label(w.draft.kind == k, k.label()).clicked() {
                                        w.set_type(k);
                                    }
                                }
                                // Planned, but nothing can be built without a capture from real hardware.
                                Some(why) => {
                                    ui.add_enabled(false, egui::Button::new(format!("{} (needs hardware capture)", k.label())))
                                        .on_disabled_hover_text(why);
                                    ui.label(RichText::new(why).size(Tokens::get().font_small).color(Tokens::get().text_dim));
                                }
                            }
                        }
                    });
                    note(
                        ui,
                        "Greyed entries need a capture from real hardware before DarkBurn can support them. The simulators run with no hardware.",
                    );
                    if w.draft.kind == DeviceType::GweikeCardSimulator {
                        note(ui, crate::gweike_workflow::DIRECT_CONTROL_NOTE);
                    }
                    if w.draft.kind == DeviceType::LinuxCnc {
                        note(ui, "LinuxCNC is file output only: DarkBurn writes the .ngc program and you run it in LinuxCNC.");
                    }
                }
                WizardStep::Connection => {
                    let kinds = w.draft.kind.connections();
                    let labels: Vec<&str> = kinds.iter().map(|k| k.label()).collect();
                    let mut idx = kinds.iter().position(|k| *k == w.draft.connection).unwrap_or(0);
                    ui_kit::row(ui, "Connection", |ui| {
                        ui_kit::dropdown(ui, "wiz_conn", &mut idx, &labels);
                    });
                    w.draft.connection = kinds[idx];
                    connection_fields(ui, &mut w.draft);
                    note(ui, "DarkBurn does not open the connection until you press Connect in the Laser panel.");
                }
                WizardStep::NameAndSize => {
                    ui_kit::row(ui, "Name", |ui| {
                        ui_kit::field(ui, &mut w.draft.name);
                    });
                    if !w.draft.is_design_only() || true {
                        ui_kit::row(ui, "Width", |ui| {
                            ui_kit::number_field(ui, &mut w.draft.width_mm, 1.0..=10_000.0, 1.0, " mm");
                        });
                        ui_kit::row(ui, "Height", |ui| {
                            ui_kit::number_field(ui, &mut w.draft.height_mm, 1.0..=10_000.0, 1.0, " mm");
                        });
                    }
                }
                WizardStep::OriginAndHoming => {
                    origin_fields(ui, &mut w.draft);
                    ui_kit::checkbox(ui, "Home the machine when a job starts", &mut w.draft.auto_home);
                }
                WizardStep::Summary => {
                    for (k, v) in w.summary() {
                        ui_kit::row(ui, &k, |ui| {
                            ui.label(v);
                        });
                    }
                }
            }
            for p in w.problems() {
                ui_kit::status_line(ui, &p, StatusTone::Error);
            }
            if w.step != WizardStep::Type && small(ui, "Back", "", true) {
                w.back();
            }
        });
    match action {
        Action::Ok => {
            if last {
                match w.finish(&mut app.machine.devices) {
                    Ok(id) => {
                        app.machine.dlg.devices_sel = Some(id);
                        app.machine.dlg.devices = true;
                    }
                    Err(e) => {
                        app.machine.dlg.message = Some(e);
                        app.machine.dlg.wizard = Some(w);
                    }
                }
            } else {
                w.next();
                app.machine.dlg.wizard = Some(w);
            }
        }
        _ if closed || action == Action::Cancel => {}
        _ => app.machine.dlg.wizard = Some(w),
    }
}

fn connection_fields(ui: &mut Ui, d: &mut Device) {
    match d.connection {
        ConnectionKind::Serial => {
            ui_kit::row(ui, "Baud rate", |ui| {
                ui_kit::number_field(ui, &mut d.settings.gcode.baud_rate, 1200..=2_000_000, 100.0, "");
            });
            note(ui, "The port is chosen in the Laser panel (automatic by default).");
        }
        ConnectionKind::Tcp => {
            ui_kit::row(ui, "Host", |ui| {
                ui_kit::field(ui, &mut d.host);
            });
            ui_kit::row(ui, "Port", |ui| {
                ui_kit::number_field(ui, &mut d.tcp_port, 1..=65_535, 1.0, "");
            });
        }
        ConnectionKind::Udp => {
            ui_kit::row(ui, "IP address", |ui| {
                ui_kit::field(ui, &mut d.host);
            });
            note(ui, RUIDA_DISCOVERY_NOTE);
        }
        ConnectionKind::Usb => {
            ui_kit::row(ui, "Port or adapter serial", |ui| {
                ui_kit::hint_field(ui, &mut d.usb_serial, "first one found");
            });
            note(
                ui,
                "The controller's USB chip (FTDI) shows up as a serial port, like COM5. Leave the field empty to use the first one found. The Fire button and continuous panel keys need the Ethernet connection.",
            );
        }
        ConnectionKind::BoardUsb => {
            note(
                ui,
                "The galvo board's USB link is not built into this version. Use the galvo simulator until it has been checked on real hardware (docs/hardware-checks-galvo.md).",
            );
        }
        ConnectionKind::None => {}
    }
}

fn origin_fields(ui: &mut Ui, d: &mut Device) {
    let corners = [Corner::BottomLeft, Corner::BottomRight, Corner::TopLeft, Corner::TopRight];
    let labels = ["Front left (bottom left)", "Front right (bottom right)", "Rear left (top left)", "Rear right (top right)"];
    let mut idx = corners.iter().position(|c| *c == d.origin).unwrap_or(0);
    ui_kit::row(ui, "Origin corner", |ui| {
        ui_kit::dropdown(ui, "dev_origin", &mut idx, &labels);
    });
    d.origin = corners[idx];
}

// ----- Find my laser [DEV-02] ----------------------------------------------------------------------

fn find_dialog(ctx: &Context, app: &mut DarkBurn) {
    let Some(cands) = app.machine.dlg.find.clone() else { return };
    let (closed, action) =
        ui_kit::Window::new(Id::unique("db_find_laser"), "settings", "Find my laser").cards(3.0).actions("", "Close", None).show(ctx, true, |ui| {
            note(ui, "These are the serial ports on this computer. DarkBurn only lists them. To check what is on one, add it and press Connect.");
            if cands.is_empty() {
                ui_kit::status_line(ui, "No serial ports found. Plug the laser in with its USB cable and press Rescan.", StatusTone::Warning);
            }
            for c in &cands {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&c.label).strong());
                    if small(ui, "Add", "Add this port as a device", true) {
                        app.machine.dlg.wizard = Some(Wizard::from_candidate(c));
                    }
                });
                note(ui, &c.note);
            }
            note(ui, RUIDA_DISCOVERY_NOTE);
            if small(ui, "Rescan", "List the ports again", true) {
                app.machine.rescan_ports();
                app.machine.dlg.find = Some(find_my_laser(&app.machine.ports));
            }
        });
    if closed || action != Action::None {
        app.machine.dlg.find = None;
    }
}

// ----- Device Settings [DS-01..DS-17, DS-19] -----------------------------------------------------

const PAGES: [&str; 5] = ["Basic", "GCode", "DSP", "Additional", "Galvo"];

fn settings_dialog(ctx: &Context, app: &mut DarkBurn) {
    let Some((mut d, mut page)) = app.machine.dlg.settings.take() else { return };
    let errs = d.validate();
    let (closed, action) = ui_kit::Window::new(Id::unique("db_device_settings"), "settings", &format!("Device Settings: {}", d.name))
        .cards(4.0)
        .fields()
        .actions("OK", "Cancel", None)
        .show(ctx, errs.is_empty(), |ui| {
            ui.horizontal(|ui| {
                for (i, p) in PAGES.iter().enumerate() {
                    let hidden =
                        (i == 1 && d.family() != Family::Grbl) || (i == 2 && d.family() != Family::Ruida) || (i == 4 && d.family() != Family::Galvo);
                    if !hidden && ui_kit::choice_chip(ui, &format!("ds_page_{i}"), p, page == i).clicked() {
                        page = i;
                    }
                }
            });
            match page {
                0 => basic_page(ui, &mut d, app),
                1 => gcode_page(ui, &mut d),
                2 => dsp_page(ui, &mut d),
                4 => crate::galvo_ui::settings_page(ui, &mut d, &mut app.machine.ext.galvo_message),
                _ => additional_page(ui, &mut d, app),
            }
            for e in &errs {
                ui_kit::status_line(ui, e, StatusTone::Error);
            }
        });
    match action {
        Action::Ok => {
            let id = d.id;
            app.machine.devices.update(d);
            if app.machine.devices.active == Some(id) {
                // Pick up the new size and origin right away.
                app.machine.applied_reset();
                machine_ui::adopt_device_state(app);
            }
        }
        Action::Cancel => {}
        _ if closed => {}
        _ => app.machine.dlg.settings = Some((d, page)),
    }
}

fn basic_page(ui: &mut Ui, d: &mut Device, app: &mut DarkBurn) {
    ui_kit::row(ui, "Name", |ui| {
        ui_kit::field(ui, &mut d.name);
    });
    ui_kit::row(ui, "Type", |ui| {
        ui.label(d.kind.label());
    });
    if !d.kind.is_simulator() && d.kind != DeviceType::DesignOnly {
        let kinds = d.kind.connections();
        let labels: Vec<&str> = kinds.iter().map(|k| k.label()).collect();
        let mut idx = kinds.iter().position(|k| *k == d.connection).unwrap_or(0);
        ui_kit::row(ui, "Connection", |ui| {
            ui_kit::dropdown(ui, "ds_conn", &mut idx, &labels);
        });
        d.connection = kinds[idx];
        connection_fields(ui, d);
    }
    ui_kit::section_heading(ui, "Work area");
    if d.family() == Family::Galvo {
        note(ui, "The work area of a galvo is its square lens field. Set the size on the Galvo page.");
    } else {
        ui_kit::row(ui, "Width", |ui| {
            ui_kit::number_field(ui, &mut d.width_mm, 1.0..=10_000.0, 1.0, " mm");
        });
        ui_kit::row(ui, "Height", |ui| {
            ui_kit::number_field(ui, &mut d.height_mm, 1.0..=10_000.0, 1.0, " mm");
        });
    }
    origin_fields(ui, d);
    ui_kit::checkbox(ui, "Home when a job starts", &mut d.auto_home);
    ui_kit::checkbox(ui, "Negative coordinates (zero at the far corner)", &mut d.settings.negative_coords);
    ui_kit::section_heading(ui, "Red dot pointer");
    ui_kit::row(ui, "Offset X", |ui| {
        ui_kit::number_field(ui, &mut d.settings.red_dot_x, -500.0..=500.0, 0.1, " mm");
    });
    ui_kit::row(ui, "Offset Y", |ui| {
        ui_kit::number_field(ui, &mut d.settings.red_dot_y, -500.0..=500.0, 0.1, " mm");
    });
    ui_kit::section_heading(ui, "Z axis");
    ui_kit::checkbox(ui, "Z axis enabled", &mut d.settings.z_enabled);
    ui_kit::checkbox(ui, "Reverse Z", &mut d.settings.z_reverse);
    ui_kit::checkbox(ui, "Relative Z moves only", &mut d.settings.gcode.z_relative_only);
    ui_kit::checkbox(ui, "Optimize Z moves", &mut d.settings.gcode.z_optimize);
    ui_kit::row(ui, "Tab pulse width", |ui| {
        ui_kit::number_field(ui, &mut d.settings.tab_pulse_width, 0.001..=10.0, 0.01, " mm");
    });
    ui_kit::section_heading(ui, "Fire button");
    ui_kit::row(ui, "Highest power", |ui| {
        ui_kit::number_field(ui, &mut d.settings.fire_max_percent, 0.1..=100.0, 0.1, " %");
    });
    ui_kit::section_heading(ui, "Before every job");
    ui_kit::checkbox(ui, "Ask me to tick a checklist", &mut d.settings.checklist_on);
    if d.settings.checklist_on {
        let mut remove = None;
        for (i, item) in d.settings.checklist.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui_kit::field(ui, item);
                if small(ui, "Delete", "Remove this item", true) {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            d.settings.checklist.remove(i);
        }
        if small(ui, "Add item", "", true) {
            d.settings.checklist.push("New item".into());
        }
    }
    ui_kit::checkbox(ui, "Overwrite files on the controller without asking", &mut d.settings.overwrite_files);
    ui_kit::section_heading(ui, "Scanning offset");
    note(ui, "At each speed, shift the return pass of raster scans by this much.");
    let mut remove = None;
    for (i, row) in d.settings.scan.offsets.entries.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui_kit::number_field(ui, &mut row.speed, 0.0..=2000.0, 1.0, " mm/s");
            ui_kit::number_field(ui, &mut row.offset, -5.0..=5.0, 0.001, " mm");
            if small(ui, "Delete", "", true) {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        d.settings.scan.offsets.remove(i);
    }
    if small(ui, "Add row", "", true) {
        d.settings.scan.offsets.set(100.0, 0.0);
    }
    ui_kit::section_heading(ui, "This device remembers");
    ui_kit::checkbox(ui, "Inches", &mut d.state.inches);
    ui_kit::checkbox(ui, "Show the grid", &mut d.state.show_grid);
    ui_kit::row(ui, "Grid spacing", |ui| {
        ui_kit::number_field(ui, &mut d.state.grid_spacing, 0.1..=1000.0, 0.5, " mm");
    });
    ui.horizontal(|ui| {
        if small(ui, "Use current layers as defaults", "", true) {
            machine_ui::save_default_layers(app);
            if let Some(saved) = app.machine.devices.get(d.id) {
                d.state.default_layers = saved.state.default_layers.clone();
            }
        }
        if small(ui, "Apply default layers", "", d.state.default_layers.is_some()) {
            machine_ui::apply_default_layers(app);
        }
    });
}

fn gcode_page(ui: &mut Ui, d: &mut Device) {
    let g = &mut d.settings.gcode;
    let flavors = ["GRBL", "grblHAL", "FluidNC", "Marlin", "Smoothieware", "LinuxCNC"];
    let all = [Flavor::Grbl, Flavor::GrblHal, Flavor::FluidNc, Flavor::Marlin, Flavor::Smoothieware, Flavor::LinuxCnc];
    let mut fi = all.iter().position(|f| *f == g.flavor).unwrap_or(0);
    ui_kit::row(ui, "Dialect", |ui| {
        ui_kit::dropdown(ui, "ds_flavor", &mut fi, &flavors);
    });
    g.flavor = all[fi];
    ui_kit::checkbox(ui, "Home when a job starts", &mut g.auto_home);
    ui_kit::checkbox(ui, "Swap X and Y", &mut g.swap_xy);
    ui_kit::checkbox(ui, "Fast scan over white space", &mut g.fast_whitespace_scan);
    if g.fast_whitespace_scan {
        ui_kit::row(ui, "White space speed", |ui| {
            ui_kit::number_field(ui, &mut g.fast_whitespace_speed, 1.0..=2000.0, 1.0, " mm/s");
        });
    }
    ui_kit::checkbox(ui, "Use $J for jogging", &mut g.jog_with_dollar_j);
    ui_kit::checkbox(ui, "Pulse DTR when connecting", &mut g.dtr);
    ui_kit::checkbox(ui, "Fire button", &mut g.fire_button);
    ui_kit::checkbox(ui, "Laser on while framing", &mut g.laser_on_when_framing);
    let mut ai = usize::from(g.air == AirCommand::M7);
    ui_kit::row(ui, "Air assist", |ui| {
        ui_kit::dropdown(ui, "ds_air", &mut ai, &["M8 on, M9 off", "M7 on, M9 off"]);
    });
    g.air = if ai == 1 { AirCommand::M7 } else { AirCommand::M8 };
    ui_kit::row(ui, "S value for full power", |ui| {
        ui_kit::number_field(ui, &mut g.s_max, 1.0..=100_000.0, 1.0, "");
    });
    ui_kit::row(ui, "Baud rate", |ui| {
        ui_kit::number_field(ui, &mut g.baud_rate, 1200..=2_000_000, 100.0, "");
    });
    let mut ti = usize::from(g.transfer == Transfer::Synchronous);
    ui_kit::row(ui, "Transfer", |ui| {
        ui_kit::dropdown(ui, "ds_transfer", &mut ti, &["Buffered", "Synchronous"]);
    });
    g.transfer = if ti == 1 { Transfer::Synchronous } else { Transfer::Buffered };
    ui_kit::checkbox(ui, "Use G0 for overscan moves", &mut g.overscan_as_g0);
    ui_kit::checkbox(ui, "Write S with every G1", &mut g.s_every_g1);
    if matches!(g.flavor, Flavor::Marlin) {
        let mut mi = match g.marlin_power {
            MarlinPower::M3 => 0,
            MarlinPower::M106 => 1,
            MarlinPower::Inline => 2,
        };
        ui_kit::row(ui, "Marlin power", |ui| {
            ui_kit::dropdown(ui, "ds_marlin", &mut mi, &["M3 S", "M106 S", "Inline"]);
        });
        g.marlin_power = [MarlinPower::M3, MarlinPower::M106, MarlinPower::Inline][mi];
    }
    ui_kit::text_area(ui, "Start script", Id::unique("ds_start_script"), &mut g.start_script, 3, 10_000);
    ui_kit::text_area(ui, "End script", Id::unique("ds_end_script"), &mut g.end_script, 3, 10_000);
    d.settings.gcode.s_max = d.settings.gcode.s_max.max(1.0);
    if d.kind.is_translated() {
        gcode_firmware_fields(ui, d);
    }
}

/// What the G-code firmwares of `darkburn-gcodebots` add to the GCode page [DS-12, GCODE-01..05].
fn gcode_firmware_fields(ui: &mut Ui, d: &mut Device) {
    use darkburn_gcodebots::profile::{DeviceKind, MarlinPowerMode};
    let kind = d.kind.lib_kind();
    ui_kit::section_heading(ui, &tr("gcode.firmware", "This firmware"));
    note(ui, "The job is written as plain G-code and rewritten for this controller with the settings above. The dialect box does not apply.");
    let e = &mut d.settings.gcode_ext;
    ui_kit::row(ui, "Air assist on", |ui| {
        ui_kit::hint_field(ui, &mut e.air_on, "none");
    });
    ui_kit::row(ui, "Air assist off", |ui| {
        ui_kit::hint_field(ui, &mut e.air_off, "none");
    });
    ui_kit::row(ui, "Longest fire", |ui| {
        ui_kit::number_field(ui, &mut e.max_fire_seconds, 0.1..=600.0, 0.5, " s");
    });
    if matches!(kind, Some(DeviceKind::Marlin | DeviceKind::Snapmaker)) {
        let modes = [MarlinPowerMode::M3, MarlinPowerMode::M4, MarlinPowerMode::Inline, MarlinPowerMode::Fan];
        let mut mi = modes.iter().position(|m| *m == e.marlin_power).unwrap_or(0);
        ui_kit::row(ui, "Laser power", |ui| {
            ui_kit::dropdown(ui, "ds_marlin_mode", &mut mi, &["M3 S (constant)", "M4 S (dynamic)", "Inline (M3 I)", "Fan output (M106)"]);
        });
        e.marlin_power = modes[mi];
        ui_kit::row(ui, "Fan or tool number", |ui| {
            ui_kit::number_field(ui, &mut e.tool_index, 0..=9, 1.0, "");
        });
    }
    if kind == Some(DeviceKind::Smoothie) {
        ui_kit::checkbox(ui, "Merge raster moves into longer ones (clustering)", &mut e.clustering);
        ui_kit::checkbox(ui, "Shorter pause delay", &mut e.reduce_smoothie_pause_delay);
    }
    if kind == Some(DeviceKind::LinuxCnc) {
        ui_kit::checkbox(ui, "Power on the spindle output (M3 S) instead of M67", &mut e.constant_power);
        note(ui, "LinuxCNC is file output only. Use Save G-code and open the .ngc file in LinuxCNC.");
    }
    if kind == Some(DeviceKind::Snapmaker) {
        let s = &mut e.snapmaker;
        ui_kit::row(ui, "Model", |ui| {
            ui_kit::field(ui, &mut s.model);
        });
        ui_kit::row(ui, "Laser module", |ui| {
            ui_kit::field(ui, &mut s.tool_head);
        });
        ui_kit::checkbox(ui, "Write the file header", &mut s.header);
        ui_kit::row(ui, "Focus height", |ui| {
            ui_kit::number_field(ui, &mut s.focus_height_mm, -100.0..=300.0, 0.1, " mm");
        });
        ui_kit::row(ui, "Material height", |ui| {
            ui_kit::number_field(ui, &mut s.material_height_mm, 0.0..=300.0, 0.1, " mm");
        });
    }
    if kind == Some(DeviceKind::Custom) {
        ui_kit::section_heading(ui, "Command templates");
        note(ui, "Placeholders: {dx} {dy} {dz} {feed} for jogging, {s} {percent} for the fire button. Empty means not supported.");
        let c = &mut e.custom;
        for (label, field, rows) in [
            ("Home", &mut c.home, 1),
            ("Jog", &mut c.jog, 3),
            ("Pause", &mut c.pause, 1),
            ("Resume", &mut c.resume, 1),
            ("Stop", &mut c.stop, 1),
            ("Fire on", &mut c.fire_on, 1),
            ("Fire off", &mut c.fire_off, 1),
            ("Status request", &mut c.status, 1),
            ("Unlock", &mut c.unlock, 1),
            ("Laser off", &mut c.laser_off, 1),
            ("After the last line", &mut c.finish_sync, 1),
        ] {
            ui_kit::text_area(ui, label, Id::unique(("ds_custom", label)), field, rows, 1000);
        }
    }
}

fn dsp_page(ui: &mut Ui, d: &mut Device) {
    let s = &mut d.settings.dsp;
    ui_kit::checkbox(ui, "U axis (rotary)", &mut s.u_axis);
    ui_kit::checkbox(ui, "Dual tube (laser 2)", &mut s.dual_tube);
    ui_kit::checkbox(ui, "Add a number to the file name on every send", &mut s.filename_increment);
    ui_kit::checkbox(ui, "Hide Start (run from the controller)", &mut s.disable_start);
    ui_kit::checkbox(ui, "Check the bounds when framing", &mut s.check_bounds_when_framing);
    ui_kit::checkbox(ui, "Hardware overscan (the controller adds it)", &mut s.hardware_overscan);
    if s.hardware_overscan {
        note(
            ui,
            "Fill and image layers that scan along X or Y are sent without overscan moves; the controller runs past the line ends by the distance set in its own settings. Scans at other angles keep DarkBurn's overscan.",
        );
    }
    ui_kit::row(ui, "Network timeout", |ui| {
        ui_kit::number_field(ui, &mut s.network_timeout_ms, 100..=60_000, 100.0, " ms");
    });
    connection_fields(ui, d);
    note(
        ui,
        "Job start and end delays are set on the controller's own panel. No public source says where the controller keeps them, so DarkBurn cannot read or change them yet.",
    );
    note(ui, &crate::devices::known_boards_note());
}

fn additional_page(ui: &mut Ui, d: &mut Device, app: &mut DarkBurn) {
    note(ui, "Acceleration and speed numbers used for the time estimate and the preview. Machine Settings shows what the controller itself uses.");
    let a = &mut d.settings.additional;
    ui_kit::row(ui, "Cut acceleration", |ui| {
        ui_kit::number_field(ui, &mut a.accel, 1.0..=100_000.0, 10.0, " mm/s\u{b2}");
    });
    ui_kit::row(ui, "Engrave acceleration", |ui| {
        ui_kit::number_field(ui, &mut a.engrave_accel, 1.0..=100_000.0, 10.0, " mm/s\u{b2}");
    });
    ui_kit::row(ui, "Rapid speed", |ui| {
        ui_kit::number_field(ui, &mut a.rapid_speed, 1.0..=5000.0, 1.0, " mm/s");
    });
    ui_kit::row(ui, "Top cut speed", |ui| {
        ui_kit::number_field(ui, &mut a.max_speed, 1.0..=5000.0, 1.0, " mm/s");
    });
    ui_kit::row(ui, "Junction deviation", |ui| {
        ui_kit::number_field(ui, &mut a.junction_deviation, 0.0001..=5.0, 0.001, " mm");
    });
    ui_kit::row(ui, "Z speed", |ui| {
        ui_kit::number_field(ui, &mut a.z_speed, 0.1..=500.0, 0.1, " mm/s");
    });
    if app.machine.handle.is_connected() {
        // Read-only: takes the numbers from the settings already read from the controller.
        if ui_kit::button(ui, "Read from the controller", ui_kit::Button::Small).clicked() {
            match crate::motion_read::from_snapshot(app.machine.handle.snapshot(), d.settings.additional) {
                Some(new) => d.settings.additional = new,
                None => app.machine.fail("The controller has not reported its settings yet. Open Machine Settings first."),
            }
        }
    }
}

// ----- Machine Settings [EDIT-31, GRBL-07, RUIDA-07, RUIDA-09, RUIDA-10] -----------------------------

/// Settings whose change can lift a protection: laser mode, power range, soft and hard limits, homing.
pub fn risky_grbl_setting(n: u16) -> bool {
    matches!(n, 20 | 21 | 22 | 30 | 31 | 32)
}

fn machine_settings_dialog(ctx: &Context, app: &mut DarkBurn) {
    if !app.machine.dlg.machine_settings {
        return;
    }
    let family = machine_ui::family_of(&app.machine);
    let (closed, action) = ui_kit::Window::new(Id::unique("db_machine_settings"), "settings", "Machine Settings")
        .cards(5.0)
        .fields()
        .actions("", "Close", None)
        .show(ctx, true, |ui| {
            banner(ui, app);
            if !app.machine.handle.is_connected() {
                ui_kit::status_line(ui, "Connect to the laser to read and change its settings.", StatusTone::Warning);
            }
            match family {
                Family::Grbl if app.machine.handle.snapshot().gcode_firmware.is_some() => gcode_settings_body(ui, app),
                Family::Grbl => grbl_settings_body(ui, app),
                Family::Ruida => ruida_settings_body(ui, app),
                Family::Galvo => note(ui, "A galvo board is set up in Device Settings (Galvo page)."),
                Family::DesignOnly | Family::FileOnly => note(ui, "This device has no machine settings."),
            }
            backups_body(ui, app, family);
        });
    if closed || action != Action::None {
        app.machine.dlg.machine_settings = false;
        app.machine.dlg.setting_edits.clear();
    }
    risky_confirm(ctx, app);
    restore_confirm(ctx, app);
}

fn banner(ui: &mut Ui, app: &DarkBurn) {
    crate::laser_panel::banner(ui, app);
}

/// Machine Settings of a Smoothieware, Marlin, Snapmaker or custom controller [GCODE-01..05]:
/// the settings it reports by name, edited and written one at a time.
fn gcode_settings_body(ui: &mut Ui, app: &mut DarkBurn) {
    let snap = app.machine.handle.snapshot().clone();
    let connected = snap.connected();
    ui.horizontal(|ui| {
        if small(ui, &tr("gcode.read", "Read from laser"), "Ask the controller for its settings", connected) {
            machine_ui::run_with(app, "machine.read_settings", &json!({}));
        }
        let n = app.machine.ext.gcode_edits.len();
        if small(ui, &format!("Write {n} changed"), "Write the changed values to the controller", connected && n > 0) {
            let edits = std::mem::take(&mut app.machine.ext.gcode_edits);
            for (k, v) in edits {
                machine_ui::run_with(app, "machine.write_gcode_setting", &json!({ "key": k, "value": v }));
            }
        }
        if small(ui, "Store in the controller", "Keep the settings after a restart (Marlin M500)", connected) {
            machine_ui::run_with(app, "machine.persist_settings", &json!({}));
        }
        if small(ui, "Discard changes", "", n > 0) {
            app.machine.ext.gcode_edits.clear();
        }
    });
    if let Some(f) = snap.gcode_firmware {
        note(ui, &format!("Controller: {}", f.label()));
    }
    if snap.gcode_settings.is_empty() {
        note(ui, "Press Read from laser to list the settings this controller reports.");
    }
    egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
        for (k, val) in snap.gcode_settings.iter() {
            let mut v = app.machine.ext.gcode_edits.get(k).cloned().unwrap_or_else(|| val.clone());
            ui_kit::row(ui, k, |ui| {
                ui_kit::field(ui, &mut v);
            });
            if &v != val {
                app.machine.ext.gcode_edits.insert(k.clone(), v);
            } else {
                app.machine.ext.gcode_edits.remove(k);
            }
        }
    });
}

fn grbl_settings_body(ui: &mut Ui, app: &mut DarkBurn) {
    let snap = app.machine.handle.snapshot().clone();
    let connected = snap.connected();
    ui.horizontal(|ui| {
        if small(ui, "Read from laser", "Ask the controller for its $$ settings", connected) {
            machine_ui::run_with(app, "machine.read_settings", &json!({}));
        }
        let n = app.machine.dlg.setting_edits.len();
        if small(ui, &format!("Write {n} changed"), "Write the changed values to the controller", connected && n > 0) {
            let risky = app.machine.dlg.setting_edits.keys().any(|k| risky_grbl_setting(*k as u16));
            if risky {
                app.machine.dlg.message = Some("risky".into());
            } else {
                write_edits(app);
            }
        }
        if small(ui, "Discard changes", "", n > 0) {
            app.machine.dlg.setting_edits.clear();
        }
    });
    // Homing speeds, front and centre.
    ui_kit::section_heading(ui, "Homing");
    for (n, name) in [(24u32, "Homing feed (slow, finds the switch exactly)"), (25, "Homing seek (fast approach)")] {
        let cur = app.machine.dlg.setting_edits.get(&n).cloned().or_else(|| snap.grbl_settings.get(&(n as u16)).cloned());
        if let Some(mut v) = cur.clone() {
            ui_kit::row(ui, name, |ui| {
                ui_kit::field(ui, &mut v);
            });
            if Some(&v) != snap.grbl_settings.get(&(n as u16)) {
                app.machine.dlg.setting_edits.insert(n, v);
            } else {
                app.machine.dlg.setting_edits.remove(&n);
            }
        }
    }
    ui_kit::section_heading(ui, "All settings");
    egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
        for (n, val) in snap.grbl_settings.iter() {
            let info: Option<&'static SettingInfo> = setting_info(*n);
            let id = u32::from(*n);
            let mut v = app.machine.dlg.setting_edits.get(&id).cloned().unwrap_or_else(|| val.clone());
            let label = info.map_or_else(|| format!("${n}"), |i| format!("${n}  {}", i.label));
            ui_kit::row(ui, &label, |ui| {
                if info.is_some_and(|i| i.is_bool) {
                    let mut on = v.trim() != "0" && !v.trim().is_empty();
                    if ui_kit::checkbox(ui, "", &mut on).changed() {
                        v = if on { "1".into() } else { "0".into() };
                    }
                } else {
                    ui_kit::field(ui, &mut v);
                    if let Some(i) = info.filter(|i| !i.unit.is_empty()) {
                        ui.label(i.unit);
                    }
                }
            });
            if let Some(i) = info {
                note(ui, i.description);
            }
            if &v != val {
                app.machine.dlg.setting_edits.insert(id, v);
            } else {
                app.machine.dlg.setting_edits.remove(&id);
            }
        }
        if snap.grbl_settings.is_empty() {
            note(ui, "No settings read yet.");
        }
    });
}

fn write_edits(app: &mut DarkBurn) {
    let edits = std::mem::take(&mut app.machine.dlg.setting_edits);
    for (id, v) in edits {
        machine_ui::run_with(app, "machine.write_setting", &json!({ "id": id, "value": v }));
    }
    machine_ui::run_with(app, "machine.read_settings", &json!({}));
}

/// The warning shown before a change that can lift a protection is written.
fn risky_confirm(ctx: &Context, app: &mut DarkBurn) {
    if app.machine.dlg.message.as_deref() != Some("risky") {
        return;
    }
    let (_, action) = Confirm::new(Id::unique("db_risky_settings"), "settings", "Change protection settings?")
        .tone(Tone::Danger)
        .actions("Write anyway", "Cancel", None)
        .show(ctx, true, |ui| {
            ui.label("You are changing settings that limit the laser's power or motion (laser mode, power range, limits or homing).");
            ui.label("Switching these off or loosening them can stop the machine from holding its safety class, for example an enclosed machine that is safe by design. Check the machine's manual and your lid and interlock first.");
        });
    match action {
        Action::Ok => {
            app.machine.dlg.message = None;
            write_edits(app);
        }
        Action::Cancel => app.machine.dlg.message = None,
        _ => {}
    }
}

fn unit_label(u: Unit) -> &'static str {
    match u {
        Unit::Micron => "\u{b5}m",
        Unit::MicronPerSecond => "\u{b5}m/s",
        Unit::MicronPerSecond2 => "\u{b5}m/s\u{b2}",
        Unit::Microsecond => "\u{b5}s",
        Unit::Millisecond => "ms",
        Unit::Second => "s",
        Unit::Hertz => "Hz",
        Unit::Permille => "0.1 %",
        Unit::Percent => "%",
        Unit::Bits => "bits",
        Unit::Count => "",
    }
}

fn ruida_settings_body(ui: &mut Ui, app: &mut DarkBurn) {
    let snap = app.machine.handle.snapshot().clone();
    let connected = snap.connected();
    ui.horizontal(|ui| {
        if small(ui, "Read from controller", "Read the writable settings", connected) {
            machine_ui::run_with(app, "machine.read_settings", &json!({}));
        }
        let n = app.machine.dlg.setting_edits.len();
        if small(ui, &format!("Write {n} changed"), "Write the changed values to the controller", connected && n > 0) {
            let edits = std::mem::take(&mut app.machine.dlg.setting_edits);
            for (id, v) in edits {
                machine_ui::run_with(app, "machine.write_setting", &json!({ "id": id, "value": v }));
            }
            machine_ui::run_with(app, "machine.read_settings", &json!({}));
        }
        if small(ui, "Discard changes", "", n > 0) {
            app.machine.dlg.setting_edits.clear();
        }
    });
    note(ui, "Values are the controller's own units. Several registers are not confirmed on real controllers yet: change only what you know.");
    let Some(cur) = snap.ruida.settings.clone() else {
        note(ui, "No settings read yet.");
        dsp_extras(ui, app, &snap);
        return;
    };
    let groups = [
        (Group::Motion, "Speed and motion"),
        (Group::Laser, "Laser 1"),
        (Group::XAxis, "X axis"),
        (Group::YAxis, "Y axis"),
        (Group::ZAxis, "Z axis"),
        (Group::System, "System"),
    ];
    egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
        for (g, title) in groups {
            ui_kit::section_heading(ui, title);
            for r in settings::writable(Some(g)) {
                let Some(val) = cur.get(r.addr) else { continue };
                let id = u32::from(r.addr);
                let mut v = app.machine.dlg.setting_edits.get(&id).cloned().unwrap_or_else(|| val.to_string());
                ui_kit::row(ui, r.name, |ui| {
                    ui_kit::field(ui, &mut v);
                    ui.label(unit_label(r.unit));
                });
                if v != val.to_string() {
                    app.machine.dlg.setting_edits.insert(id, v);
                } else {
                    app.machine.dlg.setting_edits.remove(&id);
                }
            }
        }
    });
    dsp_extras(ui, app, &snap);
}

/// Dual tube and U axis settings [RUIDA-09, RUIDA-10].
fn dsp_extras(ui: &mut Ui, app: &mut DarkBurn, snap: &crate::machine_api::Snapshot) {
    let connected = snap.connected();
    ui_kit::section_heading(ui, "Dual tube (laser 2)");
    if app.machine.dlg.laser2.is_none() {
        app.machine.dlg.laser2 = snap.ruida.laser2;
    }
    if let Some(l) = app.machine.dlg.laser2.as_mut() {
        ui_kit::row(ui, "PWM frequency", |ui| {
            ui_kit::number_field(ui, &mut l.pwm_freq_hz, 0..=200_000, 100.0, " Hz");
        });
        ui_kit::row(ui, "Minimum power", |ui| {
            ui_kit::number_field(ui, &mut l.min_power, 0..=1000, 1.0, " (0.1 %)");
        });
        ui_kit::row(ui, "Maximum power", |ui| {
            ui_kit::number_field(ui, &mut l.max_power, 0..=1000, 1.0, " (0.1 %)");
        });
    }
    ui.horizontal(|ui| {
        if small(ui, "Read laser 2", "", connected) {
            app.machine.dlg.laser2 = None;
            machine_ui::run_with(app, "machine.dsp_read", &json!({ "what": "laser2" }));
        }
        if small(ui, "Write laser 2", "", connected && app.machine.dlg.laser2.is_some())
            && let Some(v) = app.machine.dlg.laser2.and_then(|l| serde_json::to_value(l).ok())
        {
            machine_ui::run_with(app, "machine.dsp_write", &json!({ "what": "laser2", "values": v }));
        }
    });
    ui_kit::section_heading(ui, "U axis and rotary");
    if app.machine.dlg.u_axis.is_none() {
        app.machine.dlg.u_axis = snap.ruida.u_axis;
    }
    if let Some(u) = app.machine.dlg.u_axis.as_mut() {
        ui_kit::row(ui, "Step length", |ui| {
            ui_kit::number_field(ui, &mut u.step_length_um, 0..=1_000_000, 1.0, " \u{b5}m");
        });
        ui_kit::row(ui, "Range", |ui| {
            ui_kit::number_field(ui, &mut u.range_um, 0..=100_000_000, 1000.0, " \u{b5}m");
        });
        ui_kit::row(ui, "Maximum speed", |ui| {
            ui_kit::number_field(ui, &mut u.max_velocity, 0..=1_000_000_000, 1000.0, " \u{b5}m/s");
        });
        ui_kit::row(ui, "Pulses per rotation", |ui| {
            ui_kit::number_field(ui, &mut u.rotary_pulses_per_rotation, 0..=10_000_000, 1.0, "");
        });
        ui_kit::row(ui, "Rotary diameter", |ui| {
            ui_kit::number_field(ui, &mut u.rotary_diameter_um, 0..=1_000_000_000, 100.0, " \u{b5}m");
        });
    }
    ui.horizontal(|ui| {
        if small(ui, "Read U axis", "", connected) {
            app.machine.dlg.u_axis = None;
            machine_ui::run_with(app, "machine.dsp_read", &json!({ "what": "u_axis" }));
        }
        if small(ui, "Write U axis", "", connected && app.machine.dlg.u_axis.is_some())
            && let Some(v) = app.machine.dlg.u_axis.and_then(|l| serde_json::to_value(l).ok())
        {
            machine_ui::run_with(app, "machine.dsp_write", &json!({ "what": "u_axis", "values": v }));
        }
    });
}

/// Backups kept with the device, and `.dbset` files [GRBL-07, RUIDA-07].
fn backups_body(ui: &mut Ui, app: &mut DarkBurn, family: Family) {
    ui_kit::section_heading(ui, "Backups");
    let have = app.machine.handle.backup_settings().is_some();
    // The name field takes the width it is given: left to itself it pushed the buttons off a
    // narrow window. The row wraps when the window is narrower than the field and the buttons.
    ui.horizontal_wrapped(|ui| {
        ui.scope(|ui| {
            ui.set_max_width(ui.available_width().min(220.0));
            ui_kit::hint_field(ui, &mut app.machine.dlg.backup_name, "Backup name");
        });
        if small(ui, "Back up now", "Keep the settings read from the laser with this device", have)
            && let Some(b) = app.machine.handle.backup_settings()
        {
            let name = if app.machine.dlg.backup_name.trim().is_empty() {
                format!("Backup {}", app.machine.active().state.backups.len() + 1)
            } else {
                app.machine.dlg.backup_name.trim().to_owned()
            };
            if let Some(d) = app.machine.devices.active_mut() {
                d.state.backups.push(crate::devices::BackupEntry { name, family, text: b.to_text() });
            }
            app.machine.dlg.backup_name.clear();
        }
        if small(ui, "Save to file", "Save the settings read from the laser as a .dbset file", have)
            && let (Some(b), Some(p)) = (
                app.machine.handle.backup_settings(),
                rfd::FileDialog::new().add_filter("DarkBurn settings", &["dbset"]).set_file_name("settings.dbset").save_file(),
            )
            && let Err(e) = std::fs::write(&p, b.to_text())
        {
            app.machine.fail(format!("Could not save {}: {e}", p.display()));
        }
        if small(ui, "Load from file", "Read a .dbset file; you confirm before it is written", app.machine.handle.is_connected())
            && let Some(p) = rfd::FileDialog::new().add_filter("DarkBurn settings", &["dbset"]).pick_file()
        {
            match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| SettingsBackup::parse(family, &t)) {
                Ok(b) => app.machine.dlg.confirm_restore = Some(b),
                Err(e) => app.machine.fail(e),
            }
        }
    });
    let backups = app.machine.active().state.backups.clone();
    let mut remove = None;
    for (i, b) in backups.iter().enumerate().filter(|(_, b)| b.family == family) {
        ui.horizontal(|ui| {
            ui.label(&b.name);
            if small(ui, "Restore", "Write this backup to the laser (asks first)", app.machine.handle.is_connected()) {
                match SettingsBackup::parse(b.family, &b.text) {
                    Ok(sb) => app.machine.dlg.confirm_restore = Some(sb),
                    Err(e) => app.machine.fail(e),
                }
            }
            if small(ui, "Delete", "", true) {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove
        && let Some(d) = app.machine.devices.active_mut()
    {
        d.state.backups.remove(i);
    }
}

fn restore_confirm(ctx: &Context, app: &mut DarkBurn) {
    let Some(b) = app.machine.dlg.confirm_restore.clone() else { return };
    let (_, action) = Confirm::new(Id::unique("db_restore_settings"), "settings", "Write these settings to the laser?")
        .tone(Tone::Warning)
        .actions("Write", "Cancel", None)
        .show(ctx, true, |ui| {
            ui.label(format!("{} values will be written to the controller. This replaces what it has now.", b.len()));
        });
    match action {
        Action::Ok => {
            app.machine.dlg.confirm_restore = None;
            if let Err(e) = app.machine.handle.restore_settings(b) {
                app.machine.fail(e.to_string());
            }
        }
        Action::Cancel => app.machine.dlg.confirm_restore = None,
        _ => {}
    }
}

// ----- Controller info [LT-16, RUIDA-08] ---------------------------------------------------------------

fn controller_info_dialog(ctx: &Context, app: &mut DarkBurn) {
    if !app.machine.dlg.controller_info {
        return;
    }
    let snap = app.machine.handle.snapshot().clone();
    let (closed, action) = ui_kit::Window::new(Id::unique("db_controller_info"), "settings", "Controller info")
        .cards(3.0)
        .actions("", "Close", None)
        .show(ctx, true, |ui| {
            if !snap.connected() {
                ui_kit::status_line(ui, "Connect to the laser first.", StatusTone::Warning);
                return;
            }
            let row = |ui: &mut Ui, k: &str, v: String| {
                ui_kit::row(ui, k, |ui| {
                    ui.label(v);
                });
            };
            row(ui, "Port", snap.port.clone());
            row(ui, "Firmware", if snap.firmware.is_empty() { "unknown".into() } else { snap.firmware.clone() });
            if let Some(i) = &snap.ruida.info {
                row(ui, "Board", darkburn_ruida::settings::board_label(i.card_id));
                row(ui, "Total on time", machine_ui::format_duration(std::time::Duration::from_secs(u64::from(i.total_on_time_s))));
                row(ui, "Total job time", machine_ui::format_duration(std::time::Duration::from_secs(u64::from(i.total_job_time_s))));
                row(ui, "Laser on time", machine_ui::format_duration(std::time::Duration::from_secs(u64::from(i.total_laser_on_time_s))));
                row(ui, "Jobs run", i.jobs_run.to_string());
                row(ui, "Last job", format!("{:.1} s", f64::from(i.last_job_time_ms) / 1000.0));
            } else if snap.family == Family::Ruida {
                note(ui, "Waiting for the controller...");
            }
            if small(ui, "Refresh", "", snap.family == Family::Ruida) {
                machine_ui::run_with(app, "machine.controller_info", &json!({}));
            }
        });
    if closed || action != Action::None {
        app.machine.dlg.controller_info = false;
    }
}

// ----- Safety dialogs [LASER-24, LASER-28, REAL MACHINE] -----------------------------------------------

fn confirm_connect_dialog(ctx: &Context, app: &mut DarkBurn) {
    let Some(spec) = app.machine.dlg.confirm_connect.clone() else { return };
    let name = app.machine.active().name.clone();
    let (_, action) = Confirm::new(Id::unique("db_confirm_connect"), "settings", "Connect to a real machine?")
        .tone(Tone::Danger)
        .actions("Connect", "Cancel", None)
        .show(ctx, true, |ui| {
            let t = Tokens::get();
            ui.label(RichText::new("REAL MACHINE").strong().color(t.error));
            ui.label(format!("{name} via {}.", spec.link.label()));
            ui.label("DarkBurn will open the connection and may send commands that move the laser head and fire the laser. Make sure the area is clear, the lid is closed and you can reach the stop button.");
        });
    match action {
        Action::Ok => app.machine.confirm_connect(),
        Action::Cancel => app.machine.dlg.confirm_connect = None,
        _ => {}
    }
}

fn confirm_close_dialog(ctx: &Context, app: &mut DarkBurn) {
    if !app.machine.dlg.confirm_close {
        return;
    }
    let (closed, action) = Confirm::new(Id::unique("db_confirm_close"), "settings", "A job is running")
        .tone(Tone::Warning)
        .actions("Stop and close", "Keep running", None)
        .show(ctx, true, |ui| {
            ui.label("Closing DarkBurn now stops the job and turns the laser off. Do you want to stop and close?");
        });
    match action {
        Action::Ok => {
            app.machine.stop_and_close();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        Action::Cancel => app.machine.dlg.confirm_close = false,
        _ if closed => app.machine.dlg.confirm_close = false,
        _ => {}
    }
}

fn checklist_dialog(ctx: &Context, app: &mut DarkBurn) {
    let Some((job, mut ticks)) = app.machine.dlg.checklist.take() else { return };
    let items = app.machine.active().settings.checklist.clone();
    let ready = ticks.len() == items.len() && ticks.iter().all(|t| *t);
    let (closed, action) = ui_kit::Window::new(Id::unique("db_job_checklist"), "settings", "Before you start")
        .cards(3.0)
        .actions("Start", "Cancel", None)
        .show(ctx, ready, |ui| {
            note(ui, "Tick every item to start the job.");
            for (i, item) in items.iter().enumerate() {
                if let Some(t) = ticks.get_mut(i) {
                    ui_kit::checkbox(ui, item, t);
                }
            }
        });
    match action {
        Action::Ok => {
            app.machine.dlg.checklist = Some((job, ticks));
            if let Err(e) = app.machine.finish_checklist() {
                app.machine.fail(e);
            }
        }
        Action::Cancel => {}
        _ if closed => {}
        _ => app.machine.dlg.checklist = Some((job, ticks)),
    }
}

fn overwrite_dialog(ctx: &Context, app: &mut DarkBurn) {
    let Some(name) = app.machine.handle.snapshot().ruida.overwrite_prompt.clone() else { return };
    let (_, action) = Confirm::new(Id::unique("db_overwrite_prompt"), "settings", "File already on the controller")
        .tone(Tone::Warning)
        .actions("Overwrite", "Cancel", Some("Rename"))
        .show(ctx, true, |ui| {
            ui.label(format!("The controller already has a file called {name}."));
        });
    let choice = match action {
        Action::Ok => Some("overwrite"),
        Action::Cancel => Some("cancel"),
        Action::Apply => Some("rename"),
        Action::None => None,
    };
    if let Some(c) = choice {
        machine_ui::run_with(app, "machine.file_overwrite", &json!({ "choice": c }));
    }
}

fn macro_dialog(ctx: &Context, app: &mut DarkBurn) {
    let Some((i, mut text)) = app.machine.dlg.macro_edit.take() else { return };
    let (closed, action) = ui_kit::Window::new(Id::unique("db_macro_edit"), "settings", &format!("Macro {}", i + 1))
        .cards(3.0)
        .actions("OK", "Cancel", None)
        .show(ctx, true, |ui| {
            note(ui, "One G-code line per row. The lines are sent one after the other.");
            ui_kit::text_area(ui, "Lines", Id::unique("db_macro_text"), &mut text, 8, 10_000);
        });
    match action {
        Action::Ok => {
            if let Some(d) = app.machine.devices.active_mut() {
                d.state.macros[i] = text;
            }
        }
        Action::Cancel => {}
        _ if closed => {}
        _ => app.machine.dlg.macro_edit = Some((i, text)),
    }
}
