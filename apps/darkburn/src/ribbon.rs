//! The ribbon: a row of tabs and, under it, groups of icon-over-label buttons, as in SolidCraft.
//! DarkBurn's toolbars (Main, Arrange, Modifiers, …) are its groups; hiding a toolbar in the
//! Window menu hides its groups, and a tab with nothing left disappears.

use darkburn_ui::ui_kit::{self, BackingTooltipExt as _, Container, Edge, Menu, MenuItem, Surface};
use darkburn_ui::{fonts, icons};
use egui::{Align2, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use solidcraft_theme::Tokens;

use crate::commands;
use crate::prefs::Prefs;

pub const TAB_ROW_HEIGHT: f32 = 30.0;
pub const GROUP_HEIGHT: f32 = 86.0;
pub const RIBBON_HEIGHT: f32 = TAB_ROW_HEIGHT + GROUP_HEIGHT + 1.0;
const BUTTON_HEIGHT: f32 = 60.0;
const BUTTON_MIN_WIDTH: f32 = 54.0;
const BUTTON_MAX_WIDTH: f32 = 76.0;
const BUTTON_GAP: f32 = 3.0;
const GROUP_PAD: f32 = 6.0;
const CAPTION_HEIGHT: f32 = 20.0;
const COMPACT_W: f32 = 32.0;
const CHEVRON_W: f32 = 30.0;

/// One thing in a ribbon group.
#[derive(Clone, Copy)]
enum Item {
    /// A command button.
    Cmd(&'static str),
    /// A small form: rows of (label, value) fields, `width` points wide each.
    Fields(&'static [(&'static str, &'static str)], f32),
    /// A column of check boxes.
    Checks(&'static [&'static str]),
    /// A column of drop-downs: (label, options).
    Choices(&'static [(&'static str, &'static [&'static str])]),
    /// The numeric edits: position, size, scale and rotation of the selection.
    Numeric,
    /// A live control drawn by `shape_ui.rs`: "dock_padding", "dock_checks" or "corner_radius".
    Live(&'static str),
}

struct Group {
    caption: &'static str,
    toolbar: &'static str,
    items: &'static [Item],
}

use Item::{Checks, Choices, Cmd, Fields, Live, Numeric};

const fn g(caption: &'static str, toolbar: &'static str, items: &'static [Item]) -> Group {
    Group { caption, toolbar, items }
}

const TABS: &[(&str, &str, &[Group])] = &[
    (
        "home",
        "Home",
        &[
            g("File", "main", &[Cmd("file.new"), Cmd("file.open"), Cmd("file.save"), Cmd("file.import")]),
            g("Edit", "main", &[Cmd("edit.undo"), Cmd("edit.redo"), Cmd("edit.cut"), Cmd("edit.copy"), Cmd("edit.paste"), Cmd("edit.delete")]),
            g("View", "main", &[Cmd("view.pan"), Cmd("view.zoom_page"), Cmd("view.zoom_in"), Cmd("view.zoom_out"), Cmd("view.frame_selection")]),
            g("Laser", "main", &[Cmd("laser.capture"), Cmd("window.preview")]),
            g("Setup", "main", &[Cmd("edit.settings"), Cmd("edit.device_settings")]),
            g("Position and size", "numeric", &[Numeric]),
        ],
    ),
    (
        "arrange",
        "Arrange",
        &[
            g("Group", "arrange", &[Cmd("arrange.group"), Cmd("arrange.ungroup")]),
            g("Flip", "arrange", &[Cmd("arrange.flip_v"), Cmd("arrange.flip_h"), Cmd("arrange.mirror")]),
            g("Align", "arrange", &[Cmd("arrange.align_centres"), Cmd("arrange.align_v_centres"), Cmd("arrange.align_h_centres")]),
            g("Distribute", "arrange", &[Cmd("arrange.distribute_v"), Cmd("arrange.distribute_h")]),
            g("Size", "arrange", &[Cmd("arrange.same_width"), Cmd("arrange.same_height"), Cmd("arrange.move_selection")]),
            g(
                "Docking",
                "docking",
                &[
                    Cmd("arrange.dock_left"),
                    Cmd("arrange.dock_right"),
                    Cmd("arrange.dock_up"),
                    Cmd("arrange.dock_down"),
                    Live("dock_checks"),
                    Live("dock_padding"),
                ],
            ),
        ],
    ),
    (
        "modify",
        "Modify",
        &[
            g("Offset", "modifiers", &[Cmd("tools.offset")]),
            g("Boolean", "modifiers", &[Cmd("tools.weld"), Cmd("tools.union"), Cmd("tools.difference"), Cmd("tools.intersection")]),
            g("Array", "modifiers", &[Cmd("arrange.grid_array"), Cmd("arrange.circular_array")]),
            g("Corners", "modifiers", &[Cmd("tools.start_points"), Cmd("tools.round_corners"), Live("corner_radius")]),
        ],
    ),
    (
        "text",
        "Text",
        &[
            g("Font", "text", &[Choices(&[("Font", &["IBM Plex Sans"])]), Fields(&[("Height", "10.00 mm")], 120.0)]),
            g("Style", "text", &[Checks(&["Bold", "Italic"]), Checks(&["Upper case", "Welded"]), Checks(&["Distort"])]),
            g("Spacing", "text", &[Fields(&[("Letters", "0.00"), ("Lines", "0.00")], 110.0)]),
            g("Alignment", "text", &[Choices(&[("X", &["Left", "Middle", "Right"]), ("Y", &["Baseline", "Top", "Middle", "Bottom"])])]),
            g(
                "Variable text",
                "text",
                &[Choices(&[("Type", &["Normal", "Date and time", "Merge", "Serial number", "Cut setting"])]), Fields(&[("Offset", "0")], 90.0)],
            ),
        ],
    ),
    ("modes", "Modes", &[g("Modes", "modes", &[Cmd("laser.rotary"), Cmd("laser.print_and_cut"), Cmd("laser.cylinder")])]),
];

#[cfg(test)]
/// Each tab's id, English label and group captions (for the translation table).
pub fn tab_texts() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    TABS.iter().map(|(id, label, groups)| (*id, *label, groups.iter().map(|g| g.caption).collect())).collect()
}

/// Every command id a ribbon button runs.
pub fn command_ids() -> Vec<&'static str> {
    TABS.iter()
        .flat_map(|(_, _, groups)| groups.iter())
        .flat_map(|g| g.items.iter())
        .filter_map(|i| if let Cmd(id) = i { Some(*id) } else { None })
        .collect()
}

fn tab_visible(prefs: &Prefs, groups: &[Group]) -> bool {
    groups.iter().any(|g| prefs.toolbar(g.toolbar))
}

/// The ribbon, docked at the top under the application bar.
pub fn show(ui: &mut Ui, app: &mut crate::shell::DarkBurn, clicked: &mut Option<String>) {
    let pan_tool = app.pan_tool;
    let tabs: Vec<_> = TABS.iter().filter(|(_, _, groups)| tab_visible(&app.prefs, groups)).collect();
    if tabs.is_empty() {
        return;
    }
    if !tabs.iter().any(|(id, _, _)| *id == app.prefs.ribbon_tab) {
        app.prefs.ribbon_tab = tabs[0].0.to_string();
    }
    let t = Tokens::get();
    let host = Container::new(ui.ctx(), Surface::Chrome);
    let panel = host.edge("db_ribbon", Edge::Top).exact_size(RIBBON_HEIGHT).show_separator_line(false);
    let frame = host.edge_frame(t.toolbar);
    let shown = host.show_edge(ui, panel, frame, |ui| {
        let r = frame.outer_rect(ui.max_rect());
        // Tab row.
        let mut x = r.left() + 8.0;
        for (id, tab_label, _) in &tabs {
            let active = app.prefs.ribbon_tab == *id;
            let font = if active { fonts::semibold(t.font_button) } else { FontId::proportional(t.font_button) };
            let g = ui.painter().layout_no_wrap(crate::i18n::tr(&format!("ribbon.tab.{id}"), tab_label), font, t.text);
            let tr = Rect::from_min_size(pos2(x, r.top() + 4.0), vec2(g.size().x + 18.0, TAB_ROW_HEIGHT - 8.0));
            x = tr.right() + 2.0;
            let resp = ui.interact(tr, ui.scope_id().with(("ribbon_tab", *id)), Sense::click());
            let (fill, ink) = if active {
                (t.accent_soft, t.selected_ink)
            } else if resp.hovered() {
                (t.hover, t.text)
            } else {
                (egui::Color32::TRANSPARENT, t.text)
            };
            ui.painter().rect_filled(tr, t.radius_sm, fill);
            ui.painter().galley(pos2(tr.left() + 9.0, tr.center().y - g.size().y / 2.0), g, ink);
            solidcraft_ui_registry::publish_target(&format!("ribbon:tab:{id}"), tr);
            if resp.clicked() {
                app.prefs.ribbon_tab = id.to_string();
            }
        }
        // Groups.
        let Some((_, _, groups)) = tabs.iter().find(|(id, _, _)| app.prefs.ribbon_tab == *id) else {
            return;
        };
        let row = Rect::from_min_max(pos2(r.left(), r.top() + TAB_ROW_HEIGHT), pos2(r.right(), r.top() + TAB_ROW_HEIGHT + GROUP_HEIGHT));
        let mut shown_groups: Vec<&Group> = groups.iter().filter(|g| app.prefs.toolbar(g.toolbar)).collect();
        let avail = row.width() - 8.0;
        let total = |compact: bool, gs: &[&Group]| -> f32 { gs.iter().map(|g| group_width(ui, g, compact)).sum() };
        let compact = total(false, &shown_groups) > avail;
        let mut hidden: Vec<&Group> = Vec::new();
        if compact {
            // Still too wide: the groups furthest right go behind the chevron first; the
            // position and size edits stay as long as anything else can go instead.
            while total(true, &shown_groups) + CHEVRON_W > avail && shown_groups.len() > 1 {
                let pos = shown_groups.iter().rposition(|g| !g.items.iter().any(|i| matches!(i, Numeric))).unwrap_or(shown_groups.len() - 1);
                hidden.insert(0, shown_groups.remove(pos));
            }
        }
        let mut gx = row.left() + 4.0;
        for group in shown_groups {
            let start = gx;
            gx += GROUP_PAD;
            for item in group.items {
                gx = match item {
                    Numeric => crate::numeric_edits::show(ui, pos2(gx, row.top() + 4.0), BUTTON_HEIGHT, app),
                    Live(kind) => crate::shape_ui::ribbon_item(ui, kind, pos2(gx, row.top() + 4.0), BUTTON_HEIGHT, app),
                    _ => item_at(ui, *item, gx, row, pan_tool, compact, &mut app.hint, clicked),
                } + BUTTON_GAP;
            }
            gx += GROUP_PAD - BUTTON_GAP;
            let caption = Rect::from_min_max(pos2(start, row.bottom() - CAPTION_HEIGHT), pos2(gx, row.bottom()));
            let label = crate::i18n::tr(&format!("ribbon.group.{}", group.caption), group.caption);
            ui.painter().with_clip_rect(caption).text(caption.center(), Align2::CENTER_CENTER, label, fonts::semibold(t.font_micro), t.text_dim);
            ui.painter().vline(gx, (row.top() + 6.0)..=(row.bottom() - 6.0), Stroke::new(1.0, t.border));
            gx += 1.0;
        }
        if !hidden.is_empty() {
            overflow_menu(ui, row, &hidden, clicked);
        }
    });
    let _ = shown;
}

/// Width of one item.
fn item_width(ui: &Ui, item: Item, compact: bool) -> f32 {
    let t = Tokens::get();
    match item {
        Cmd(id) => match commands::find(id) {
            None => 0.0,
            Some(_) if compact => COMPACT_W,
            Some(c) => {
                let w = ui.painter().layout_no_wrap(crate::i18n::cmd_label(c), FontId::proportional(t.font_body), t.text).size().x;
                (w + 10.0).clamp(BUTTON_MIN_WIDTH, BUTTON_MAX_WIDTH)
            }
        },
        Fields(_, w) => w,
        Checks(labels) => {
            labels.iter().map(|l| ui.painter().layout_no_wrap(l.to_string(), FontId::proportional(t.font_label), t.text).size().x).fold(0.0, f32::max)
                + 28.0
        }
        Choices(_) => 170.0,
        Numeric => crate::numeric_edits::WIDTH,
        Live("dock_checks") => 140.0,
        Live("dock_padding") => 128.0,
        Live(_) => 124.0,
    }
}

/// Width of a group as laid out by [`show`], with its separator.
fn group_width(ui: &Ui, g: &Group, compact: bool) -> f32 {
    2.0 * GROUP_PAD + g.items.iter().map(|i| item_width(ui, *i, compact) + BUTTON_GAP).sum::<f32>() - BUTTON_GAP + 1.0
}

/// The chevron at the right of the group row: the groups that did not fit, as a menu.
fn overflow_menu(ui: &mut Ui, row: Rect, hidden: &[&Group], clicked: &mut Option<String>) {
    let t = Tokens::get();
    let br = Rect::from_center_size(pos2(row.right() - CHEVRON_W / 2.0 - 2.0, row.top() + 34.0), vec2(CHEVRON_W - 4.0, 44.0));
    let id = ui.scope_id().with("ribbon_overflow");
    Menu::button_with(
        ui,
        |ui, _| {
            let resp = ui.interact(br, id, Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(br, t.radius_sm, t.hover);
            }
            ui.painter().text(br.center(), Align2::CENTER_CENTER, "\u{bb}", FontId::proportional(t.font_heading), t.text);
            solidcraft_ui_registry::publish_target("ribbon:overflow", br);
            resp.backing_on_hover_text(crate::i18n::tr("ui.more_commands", "More commands"))
        },
        |ui| {
            ui.set_min_width(ui_kit::LIST_W);
            for g in hidden {
                Menu::heading(ui, &crate::i18n::tr(&format!("ribbon.group.{}", g.caption), g.caption));
                for item in g.items {
                    if let Cmd(cid) = item
                        && let Some(c) = commands::find(cid)
                    {
                        let text = crate::i18n::cmd_label(c);
                        let mut mi = MenuItem::new(c.id, &text, c.icon);
                        mi.enabled = commands::works(cid);
                        if mi.show(ui).clicked() && mi.enabled {
                            *clicked = Some(cid.to_string());
                        }
                    }
                }
            }
        },
    );
}

/// Lay out one item from `x` in the group row `row`; returns its right edge.
fn item_at(
    ui: &mut Ui,
    item: Item,
    x: f32,
    row: Rect,
    pan_tool: bool,
    compact: bool,
    hint: &mut Option<String>,
    clicked: &mut Option<String>,
) -> f32 {
    let t = Tokens::get();
    let top = row.top() + 4.0;
    match item {
        Cmd(id) => {
            let Some(c) = commands::find(id) else {
                return x;
            };
            let label_w = ui.painter().layout_no_wrap(crate::i18n::cmd_label(c), FontId::proportional(t.font_body), t.text).size().x;
            let w = if compact { COMPACT_W } else { (label_w + 10.0).clamp(BUTTON_MIN_WIDTH, BUTTON_MAX_WIDTH) };
            let whole = Rect::from_min_size(pos2(x, top), vec2(w, BUTTON_HEIGHT));
            let enabled = commands::works(id);
            let resp = ui.interact(whole, ui.scope_id().with(("ribbon", id)), if enabled { Sense::click() } else { Sense::hover() });
            let pressed = id == "view.pan" && pan_tool;
            if pressed || (enabled && resp.is_pointer_button_down_on()) {
                ui.painter().rect(whole, t.radius_sm, t.accent_soft, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
            } else if enabled && resp.hovered() {
                ui.painter().rect_filled(whole, t.radius_sm, t.hover);
            }
            let icon_y = if compact { whole.center().y } else { whole.top() + 16.0 };
            let icon_px = crate::options::get().icon_size.points();
            let icon = Rect::from_center_size(pos2(whole.center().x, icon_y), vec2(icon_px, icon_px));
            icons::paint_icon(ui.painter(), icon, c.icon, if enabled { 1.0 } else { 0.35 });
            let ink = if enabled { t.text } else { t.text_dim.gamma_multiply(0.75) };
            let label = crate::i18n::cmd_label(c);
            let mut job = egui::text::LayoutJob::simple(label.clone(), FontId::proportional(t.font_body), ink, w - 4.0);
            job.halign = egui::Align::Center;
            if let Some(s) = job.sections.first_mut() {
                s.format.line_height = Some(t.font_body + 2.0);
            }
            job.wrap.max_rows = 2;
            let galley = ui.painter().layout_job(job);
            if !compact {
                ui.painter().galley(pos2(whole.center().x, whole.top() + 32.0), galley, ink);
            }
            solidcraft_ui_registry::publish_target(&format!("ribbon:{id}"), whole);
            let tip = match (c.key.is_empty(), enabled) {
                (true, true) => label.clone(),
                (false, true) => format!("{} ({})", label, crate::keymap::key_for(c)),
                (_, false) => format!("{} - {}", label, crate::i18n::tr("ui.later_version", "arrives in a later version")),
            };
            if resp.hovered() {
                *hint = Some(tip.clone());
            }
            if resp.backing_on_hover_text(tip).clicked() && enabled {
                *clicked = Some(id.to_string());
            }
            whole.right()
        }
        Fields(rows, width) => {
            let area = Rect::from_min_size(pos2(x, top), vec2(width, BUTTON_HEIGHT));
            ui.scope_builder(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
                ui.add_enabled_ui(false, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for (i, (label, value)) in rows.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.set_height(t.control_height);
                            ui.label(egui::RichText::new(*label).size(t.font_small).color(t.text_dim));
                            let mut v = value.to_string();
                            ui.push_id(i, |ui| ui_kit::field(ui, &mut v));
                        });
                    }
                });
            });
            area.right()
        }
        Checks(labels) => {
            let width = labels
                .iter()
                .map(|l| ui.painter().layout_no_wrap(l.to_string(), FontId::proportional(t.font_label), t.text).size().x)
                .fold(0.0, f32::max)
                + 28.0;
            let area = Rect::from_min_size(pos2(x, top + 4.0), vec2(width, BUTTON_HEIGHT));
            ui.scope_builder(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
                ui.add_enabled_ui(false, |ui| {
                    for l in labels {
                        let mut on = false;
                        ui_kit::checkbox(ui, l, &mut on);
                    }
                });
            });
            area.right()
        }
        Choices(rows) => {
            let width = 170.0;
            let area = Rect::from_min_size(pos2(x, top), vec2(width, BUTTON_HEIGHT));
            ui.scope_builder(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
                ui.add_enabled_ui(false, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for (label, options) in rows {
                        ui.horizontal(|ui| {
                            ui.set_height(t.control_height);
                            ui.label(egui::RichText::new(*label).size(t.font_small).color(t.text_dim));
                            let mut i = 0;
                            ui_kit::dropdown(ui, &format!("ribbon_{label}"), &mut i, options);
                        });
                    }
                });
            });
            area.right()
        }
        Numeric | Live(_) => x,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ribbon_buttons_name_real_commands() {
        for (_, _, groups) in TABS {
            for g in *groups {
                assert!(crate::prefs::TOOLBARS.iter().any(|(id, _)| *id == g.toolbar), "{} is not a toolbar", g.toolbar);
                for item in g.items {
                    if let Cmd(id) = item {
                        assert!(commands::find(id).is_some(), "{id}");
                    }
                }
            }
        }
    }

    #[test]
    fn hiding_every_toolbar_of_a_tab_hides_the_tab() {
        let mut prefs = Prefs::default();
        let (_, _, arrange) = TABS.iter().find(|(id, _, _)| *id == "arrange").unwrap();
        assert!(tab_visible(&prefs, arrange));
        prefs.toolbars.insert("arrange".into(), false);
        assert!(tab_visible(&prefs, arrange), "docking is still shown");
        prefs.toolbars.insert("docking".into(), false);
        assert!(!tab_visible(&prefs, arrange));
    }
}
