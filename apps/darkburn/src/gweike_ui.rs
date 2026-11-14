//! The Gweike window [MCC-02, MCC-04, MCC-05]: the layer to material and craft mapping, the
//! fibre and CO2 craft editor, the craft file export and the card simulator.
//!
//! This window never connects to a machine. The card simulator is a struct in this process.
//! Direct control waits for a read-only capture on the real card (see
//! `docs/research/mlaser-rea-findings.md`); the window says so.

use darkburn_mcc::{Co2Craft, FibreCraft, GasType, PierceStage, ProcessType};
use darkburn_ui::ui_kit::{self, Action, BackingTooltipExt as _, Button, StatusTone};
use egui::{Context, Id, RichText, Ui};
use serde_json::json;
use solidcraft_theme::Tokens;

use crate::gweike_workflow::{DIRECT_CONTROL_NOTE, GweikeSettings, Head};
use crate::i18n::tr;
use crate::machine_ui;
use crate::shell::DarkBurn;

const GASES: [GasType; 6] = [GasType::Air, GasType::O2, GasType::N2, GasType::HighAir, GasType::HighO2, GasType::HighN2];
const GAS_NAMES: [&str; 6] = ["Air", "Oxygen", "Nitrogen", "High-pressure air", "High-pressure oxygen", "High-pressure nitrogen"];
const PROCESSES: [ProcessType; 5] =
    [ProcessType::DirectCut, ProcessType::FixedHeight, ProcessType::Pierce1, ProcessType::Pierce2, ProcessType::Pierce3];
const PROCESS_NAMES: [&str; 5] = ["Direct cut", "Fixed height cut", "One pierce stage", "Two pierce stages", "Three pierce stages"];

fn small(ui: &mut Ui, label: &str, tip: &str, enabled: bool) -> bool {
    ui.add_enabled_ui(enabled, |ui| ui_kit::button(ui, label, Button::Small).backing_on_hover_text(tip).clicked()).inner
}

fn note(ui: &mut Ui, text: &str) {
    let t = Tokens::get();
    ui.label(RichText::new(text).size(t.font_small).color(t.text_dim));
}

fn gas_row(ui: &mut Ui, id: &str, gas: &mut GasType, bar: &mut f64) {
    let mut gi = GASES.iter().position(|g| g == gas).unwrap_or(0);
    ui_kit::row(ui, "Assist gas", |ui| {
        ui_kit::dropdown(ui, id, &mut gi, &GAS_NAMES);
    });
    *gas = GASES[gi];
    ui_kit::row(ui, "Gas pressure", |ui| {
        ui_kit::number_field(ui, bar, 0.0..=darkburn_mcc::craft::MAX_PRESSURE_BAR, 0.1, " bar");
    });
}

/// The fibre craft editor [MCC-04]: power, frequency, duty, speed, gas and pressure, pierce
/// stages, focus and follower height, nozzle.
pub fn fibre_editor(ui: &mut Ui, c: &mut FibreCraft) {
    ui_kit::row(ui, "Name", |ui| {
        ui_kit::field(ui, &mut c.name);
    });
    let mut nozzle = c.nozzle.clone().unwrap_or_default();
    ui_kit::row(ui, "Nozzle", |ui| {
        ui_kit::hint_field(ui, &mut nozzle, "like 1.5S");
    });
    c.nozzle = (!nozzle.trim().is_empty()).then(|| nozzle.trim().to_owned());
    let mut pi = PROCESSES.iter().position(|p| *p == c.process).unwrap_or(0);
    ui_kit::row(ui, "Process", |ui| {
        ui_kit::dropdown(ui, "gw_process", &mut pi, &PROCESS_NAMES);
    });
    c.process = PROCESSES[pi];
    ui_kit::section_heading(ui, "Cutting");
    let k = &mut c.cut;
    ui_kit::row(ui, "Speed", |ui| {
        ui_kit::number_field(ui, &mut k.speed_mm_s, 0.1..=3000.0, 1.0, " mm/s");
    });
    ui_kit::row(ui, "Duty (power)", |ui| {
        ui_kit::number_field(ui, &mut k.power_pct, 0.0..=100.0, 0.5, " %");
    });
    ui_kit::row(ui, "Frequency", |ui| {
        ui_kit::number_field(ui, &mut k.freq_hz, 1.0..=50_000.0, 10.0, " Hz");
    });
    ui_kit::row(ui, "Peak current", |ui| {
        ui_kit::number_field(ui, &mut k.peak_current_pct, 0.0..=100.0, 0.5, " %");
    });
    gas_row(ui, "gw_cut_gas", &mut k.gas, &mut k.gas_pressure_bar);
    ui_kit::row(ui, "Follower height", |ui| {
        ui_kit::number_field(ui, &mut k.height_mm, 0.0..=50.0, 0.05, " mm");
    });
    ui_kit::row(ui, "Focus", |ui| {
        ui_kit::number_field(ui, &mut k.focus_mm, -50.0..=50.0, 0.05, " mm");
    });
    ui_kit::row(ui, "Lift between contours", |ui| {
        ui_kit::number_field(ui, &mut k.up_height_mm, 0.0..=200.0, 0.5, " mm");
    });
    ui_kit::row(ui, "Delay after laser on", |ui| {
        ui_kit::number_field(ui, &mut k.laser_on_delay_ms, 0.0..=10_000.0, 1.0, " ms");
    });
    let stages = c.process.stages();
    while c.pierce.len() < stages {
        c.pierce.push(PierceStage::default());
    }
    for (i, p) in c.pierce.iter_mut().take(stages).enumerate() {
        ui_kit::section_heading(ui, &format!("Pierce stage {}", i + 1));
        ui_kit::row(ui, "Height", |ui| {
            ui_kit::number_field(ui, &mut p.height_mm, 0.0..=50.0, 0.05, " mm");
        });
        ui_kit::row(ui, "Focus", |ui| {
            ui_kit::number_field(ui, &mut p.focus_mm, -50.0..=50.0, 0.05, " mm");
        });
        ui_kit::row(ui, "Duty (power)", |ui| {
            ui_kit::number_field(ui, &mut p.power_pct, 0.0..=100.0, 0.5, " %");
        });
        ui_kit::row(ui, "Frequency", |ui| {
            ui_kit::number_field(ui, &mut p.freq_hz, 1.0..=50_000.0, 10.0, " Hz");
        });
        ui_kit::row(ui, "Peak current", |ui| {
            ui_kit::number_field(ui, &mut p.peak_current_pct, 0.0..=100.0, 0.5, " %");
        });
        gas_row(ui, &format!("gw_pierce_gas_{i}"), &mut p.gas, &mut p.gas_pressure_bar);
        ui_kit::row(ui, "Time", |ui| {
            ui_kit::number_field(ui, &mut p.duration_ms, 0.0..=60_000.0, 10.0, " ms");
        });
    }
    ui_kit::section_heading(ui, "Height follower");
    ui_kit::checkbox(ui, "Follow the material with the capacitive sensor", &mut c.follower.follow);
    ui_kit::row(ui, "Fixed height position", |ui| {
        ui_kit::number_field(ui, &mut c.follower.fixed_height_cut_pos_mm, 0.0..=200.0, 0.5, " mm");
    });
    note(ui, "The machine's own follower, piercing and gas logic stay in charge. These values are for you to enter in its software.");
}

pub fn co2_editor(ui: &mut Ui, c: &mut Co2Craft) {
    ui_kit::row(ui, "Name", |ui| {
        ui_kit::field(ui, &mut c.name);
    });
    ui_kit::row(ui, "Speed", |ui| {
        ui_kit::number_field(ui, &mut c.speed_mm_s, 0.1..=3000.0, 1.0, " mm/s");
    });
    ui_kit::row(ui, "Duty (power)", |ui| {
        ui_kit::number_field(ui, &mut c.duty_pct, 0.0..=100.0, 0.5, " %");
    });
    ui_kit::row(ui, "Frequency", |ui| {
        ui_kit::number_field(ui, &mut c.freq_hz, 1.0..=50_000.0, 10.0, " Hz");
    });
    gas_row(ui, "gw_co2_gas", &mut c.gas, &mut c.gas_pressure_bar);
    ui_kit::row(ui, "Delay after laser on", |ui| {
        ui_kit::number_field(ui, &mut c.laser_on_delay_ms, 0.0..=10_000.0, 1.0, " ms");
    });
    ui_kit::row(ui, "Fixed head position", |ui| {
        ui_kit::number_field(ui, &mut c.fixed_height_cut_pos_mm, 0.0..=200.0, 0.5, " mm");
    });
}

/// What the card simulator answered, as text for the window: the dissector's explanation, or the
/// answer's fields one per line.
pub fn card_answer(v: &serde_json::Value) -> String {
    if let Some(t) = v.get("text").and_then(|t| t.as_str()) {
        return t.to_owned();
    }
    match v.as_object() {
        Some(o) if !o.is_empty() => o.iter().map(|(k, x)| format!("{k}: {x}")).collect::<Vec<_>>().join("\n"),
        _ => v.to_string(),
    }
}

/// The card simulator's buttons [MCC-05]: each one names an `mcc.*` command; the registry runs it.
fn card_buttons(ui: &mut Ui, connected: bool, frame: &mut String, cmd: &mut Option<(&'static str, serde_json::Value)>) {
    ui.horizontal_wrapped(|ui| {
        if small(ui, &tr("gweike.card_connect", "Connect"), &tr("gweike.card_connect_tip", "Open the simulated card, read only"), !connected) {
            *cmd = Some(("mcc.connect", json!({})));
        }
        if small(ui, &tr("gweike.card_disconnect", "Disconnect"), "", connected) {
            *cmd = Some(("mcc.disconnect", json!({})));
        }
        if small(ui, &tr("gweike.card_status", "Status"), &tr("gweike.card_status_tip", "Read the simulated card's state and alarms"), connected) {
            *cmd = Some(("mcc.status", json!({})));
        }
        if small(ui, &tr("gweike.card_stop", "Stop"), "", true) {
            *cmd = Some(("mcc.stop", json!({})));
        }
        if small(ui, &tr("gweike.card_estop", "Emergency stop"), "", true) {
            *cmd = Some(("mcc.emergency_stop", json!({})));
        }
    });
    ui.horizontal(|ui| {
        ui_kit::hint_field(ui, frame, &tr("gweike.card_frame_hint", "frame bytes in hex"));
        if small(ui, &tr("gweike.card_explain", "Explain a frame"), "", !frame.trim().is_empty()) {
            *cmd = Some(("mcc.dissect", json!({ "hex": frame.trim() })));
        }
    });
}

pub fn dialogs(ctx: &Context, app: &mut DarkBurn) {
    if !app.machine.ext.gweike_open {
        return;
    }
    let id = app.machine.active().id;
    let mut s: GweikeSettings = app.machine.active().settings.gweike.clone();
    let used = app.editor.doc.used_layers();
    let mut head = app.machine.ext.gweike_craft_head;
    let mut slot = app.machine.ext.gweike_craft_slot.min(darkburn_mcc::craft::LAYERS_PER_HEAD - 1);
    if head == Head::Skip {
        head = Head::Fibre;
    }
    let mut cmd: Option<(&'static str, serde_json::Value)> = None;
    let msg = app.machine.ext.gweike_message.clone();
    let plan = app.machine.ext.gweike_plan.clone();
    let card = app.machine.ext.gweike_card.clone();
    let mut shelf = std::mem::take(&mut app.machine.ext.gweike_recipes);
    let connected = app.machine.ext.mcc.connected();
    let frame_id = Id::unique("db_gweike_card_frame");
    let mut frame: String = ctx.data_mut(|d| d.get_temp(frame_id)).unwrap_or_default();
    let doc_layers: Vec<(usize, String)> =
        used.iter().filter(|l| **l < darkburn_core::layer::COLOUR_LAYERS).map(|l| (*l, app.editor.doc.layer(*l).name.clone())).collect();
    let defaults: Vec<(usize, crate::gweike_workflow::LayerMap)> = doc_layers.iter().map(|(l, _)| (*l, s.map_of(*l, &app.editor.doc))).collect();
    let (closed, action) = ui_kit::Window::new(Id::unique("db_gweike"), "settings", &tr("gweike.title", "Gweike craft file and card simulator"))
        .dockable("db_gweike")
        .cards(5.0)
        .fields()
        .actions("", "Close", None)
        .show(ctx, true, |ui| {
            ui_kit::status_line(ui, DIRECT_CONTROL_NOTE, StatusTone::Warning);
            ui_kit::section_heading(ui, &tr("gweike.layers", "Layers to materials and crafts"));
            if doc_layers.is_empty() {
                note(ui, "Nothing is drawn yet.");
            }
            for ((l, name), (_, m0)) in doc_layers.iter().zip(defaults.iter()) {
                let mut m = m0.clone();
                ui.horizontal(|ui| {
                    ui.label(format!("{} {}", darkburn_core::layer::layer_label(*l), name));
                    ui_kit::field(ui, &mut m.material);
                    let mut hi = Head::ALL.iter().position(|h| *h == m.head).unwrap_or(0);
                    let labels: Vec<&str> = Head::ALL.iter().map(|h| h.label()).collect();
                    ui_kit::dropdown(ui, &format!("gw_head_{l}"), &mut hi, &labels);
                    m.head = Head::ALL[hi];
                    ui_kit::number_field(ui, &mut m.slot, 0..=10, 1.0, "");
                });
                if &m != m0 {
                    s.layers.insert(*l, m);
                }
            }
            ui_kit::section_heading(ui, &tr("gweike.craft", "Craft parameters"));
            ui.horizontal(|ui| {
                for h in [Head::Fibre, Head::Co2] {
                    if ui_kit::choice_chip(ui, &format!("gw_head_chip_{h:?}"), h.label(), head == h).clicked() {
                        head = h;
                    }
                }
                ui_kit::number_field(ui, &mut slot, 0..=10, 1.0, "");
            });
            note(ui, "The slot is the layer number a contour in the craft file carries.");
            // The craft editor and, beside it, the recipes of the vendor software for the same
            // head: one click puts a recipe on the slot open in the editor.
            ui.columns(2, |cols| {
                match head {
                    Head::Fibre => fibre_editor(&mut cols[0], &mut s.crafts.fibre[slot]),
                    _ => co2_editor(&mut cols[0], &mut s.crafts.co2[slot]),
                }
                ui_kit::section_heading(&mut cols[1], &tr("gweike.recipes", "Recipes from the Gweike software"));
                if let Some(c) = crate::gweike_recipes::section(&mut cols[1], &mut shelf, head, slot) {
                    cmd = Some(c);
                }
            });
            for p in s.craft_problems() {
                ui_kit::status_line(ui, &p, StatusTone::Warning);
            }
            ui_kit::section_heading(ui, &tr("gweike.export", "Files"));
            ui.horizontal_wrapped(|ui| {
                if small(ui, "Craft file (.chf)...", "Write the .chf and a craft table beside it", !doc_layers.is_empty())
                    && let Some(p) = rfd::FileDialog::new().add_filter("Gweike craft file", &["chf"]).set_file_name("job.chf").save_file()
                {
                    cmd = Some(("gweike.export_chf", json!({ "path": p.display().to_string() })));
                }
                if small(ui, "DXF per material...", "", !doc_layers.is_empty()) {
                    cmd = Some(("file.export_gweike_dxf", json!({})));
                }
                if small(ui, ".nc G-code...", "", !doc_layers.is_empty()) {
                    cmd = Some(("file.export_gweike_nc", json!({})));
                }
            });
            ui_kit::section_heading(ui, &tr("gweike.sim", "Card simulator"));
            note(ui, "Runs the planner over the design and streams it into an in-memory card. Nothing is sent anywhere and nothing moves.");
            ui.horizontal(|ui| {
                if small(ui, "Dry run", "Show the FIFO plan and a time estimate", !doc_layers.is_empty()) {
                    cmd = Some(("gweike.plan", json!({})));
                }
                if small(ui, "Run on the simulated card", "", !doc_layers.is_empty()) {
                    cmd = Some(("gweike.simulate", json!({})));
                }
            });
            if let Some(p) = &plan {
                note(
                    ui,
                    &format!(
                        "Plan: {} ticks in {} frames, estimate {}, cut {:.0} mm, travel {:.0} mm",
                        p.ticks,
                        p.frames,
                        machine_ui::format_duration(std::time::Duration::from_secs_f64(p.estimated_s)),
                        p.cut_length_mm,
                        p.rapid_length_mm
                    ),
                );
            }
            if let Some(c) = &card {
                note(
                    ui,
                    &format!(
                        "Simulated card: {} frames taken, {} ticks run, {} underruns, {} laser requests refused.",
                        c.frames_accepted, c.ticks_consumed, c.underruns, c.laser_refusals
                    ),
                );
            }
            card_buttons(ui, connected, &mut frame, &mut cmd);
            if let Some(m) = &msg {
                note(ui, m);
            }
        });
    ctx.data_mut(|d| d.insert_temp(frame_id, frame));
    app.machine.ext.gweike_craft_head = head;
    app.machine.ext.gweike_craft_slot = slot;
    app.machine.ext.gweike_recipes = shelf;
    if s != app.machine.active().settings.gweike
        && let Some(d) = app.machine.devices.get_mut(id)
    {
        d.settings.gweike = s;
    }
    if let Some((c, args)) = cmd {
        app.machine.ext.gweike_message = None;
        if c.starts_with("file.") {
            app.run(ctx, c);
            return;
        }
        match crate::api::execute_headless(app, c, &args) {
            Ok(v) if c.starts_with("gweike.recipe") => {
                if c == "gweike.recipes_scan" {
                    app.machine.ext.gweike_message = None;
                }
                let warnings: Vec<&str> =
                    v.get("warnings").and_then(|w| w.as_array()).map(|w| w.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
                if !warnings.is_empty() {
                    app.machine.ext.gweike_message =
                        Some(format!("{} (check: {})", app.machine.ext.gweike_message.clone().unwrap_or_default(), warnings.join("; ")));
                }
            }
            Ok(v) if c.starts_with("mcc.") => app.machine.ext.gweike_message = Some(card_answer(&v)),
            Ok(v) => {
                app.machine.ext.gweike_message =
                    Some(v.get("path").and_then(|p| p.as_str()).map_or_else(|| "Done.".to_owned(), |p| format!("Wrote {p}")))
            }
            Err(e) => app.machine.ext.gweike_message = Some(e),
        }
    }
    if closed || action != Action::None {
        app.machine.ext.gweike_open = false;
    }
}
