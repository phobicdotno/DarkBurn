//! The Gweike network test window: one row per device with a coloured state pill, a latency
//! sparkline over the last 60 samples and the numbers. Opened from the Gweike machine page;
//! closing it stops the test.

use darkburn_ui::ui_kit::{self, Action};
use egui::{Color32, Context, CornerRadius, Id, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use solidcraft_theme::Tokens;

use crate::i18n::tr;
use crate::net_test::{HISTORY, Health, NetTest, SLOW_MS, Target, Track};
use crate::shell::DarkBurn;

/// Temp flag the Gweike page sets to open the window.
pub const OPEN: &str = "db_net_test_open";

const SPARK: Vec2 = Vec2::new(110.0, 20.0);

fn device_name(t: Target) -> String {
    match t {
        Target::Card => tr("net.dev_card", "Controller card (ping)"),
        Target::CardProtocol => tr("net.dev_protocol", "Controller card (status read)"),
        Target::Follower => tr("net.dev_follower", "External Z follower"),
        Target::Fiber => tr("net.dev_fiber", "Fiber source"),
    }
}

fn health_colour(h: Health) -> Color32 {
    let t = Tokens::get();
    match h {
        Health::Waiting | Health::NotFitted => t.text_dim,
        Health::Ok => t.agent_ok,
        Health::Slow => t.warning,
        Health::Lost => t.error,
    }
}

fn pill(ui: &mut Ui, h: Health, id: &str) {
    let t = Tokens::get();
    let colour = health_colour(h);
    let galley = ui.painter().layout_no_wrap(h.text(), egui::FontId::proportional(t.font_small), colour);
    let size = Vec2::new(galley.size().x + 14.0, galley.size().y + 4.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(size.x.max(70.0), size.y), Sense::hover());
    let r = Rect::from_min_size(rect.min, size);
    ui.painter().rect(r, CornerRadius::same(t.radius_lg), colour.gamma_multiply(0.15), Stroke::new(1.0, colour), egui::StrokeKind::Inside);
    ui.painter().galley(r.center() - galley.size() / 2.0, galley, colour);
    solidcraft_ui_registry::publish_rect(&format!("net_test:pill:{id}"), r);
}

/// Latency over the last samples, scaled to the slowest one (at least the "slow" limit); a
/// missing answer is a short red tick at the bottom.
fn sparkline(ui: &mut Ui, track: &Track, colour: Color32, id: &str) {
    let t = Tokens::get();
    let (rect, _) = ui.allocate_exact_size(SPARK, Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(t.radius_sm), t.panel_header);
    let top = track.samples.iter().flatten().fold(SLOW_MS, |a, b| a.max(*b));
    let step = rect.width() / (HISTORY.saturating_sub(1).max(1)) as f32;
    let offset = HISTORY.saturating_sub(track.samples.len());
    let mut line: Vec<Pos2> = Vec::new();
    for (i, s) in track.samples.iter().enumerate() {
        let x = rect.left() + (offset + i) as f32 * step;
        match s {
            Some(ms) => line.push(Pos2::new(x, rect.bottom() - 2.0 - (*ms / top) as f32 * (rect.height() - 4.0))),
            None => {
                if line.len() > 1 {
                    p.add(egui::Shape::line(std::mem::take(&mut line), Stroke::new(1.5, colour)));
                }
                line.clear();
                p.line_segment([Pos2::new(x, rect.bottom()), Pos2::new(x, rect.bottom() - 6.0)], Stroke::new(1.5, t.error));
            }
        }
    }
    if line.len() > 1 {
        p.add(egui::Shape::line(line, Stroke::new(1.5, colour)));
    } else if let Some(pt) = line.first() {
        p.circle_filled(*pt, 1.5, colour);
    }
    solidcraft_ui_registry::publish_rect(&format!("net_test:spark:{id}"), rect);
}

fn ms(v: Option<f64>) -> String {
    v.map_or_else(|| "-".to_owned(), |v| format!("{v:.1}"))
}

fn rows(ui: &mut Ui, n: &NetTest) {
    let t = Tokens::get();
    egui::Grid::new("db_net_test_grid").num_columns(8).spacing([t.space_8, t.space_6]).striped(true).show(ui, |ui| {
        for h in [
            tr("net.col_device", "Device"),
            tr("net.col_ip", "Address"),
            tr("net.col_state", "State"),
            tr("net.col_latency", "Latency"),
            tr("net.col_min_avg_max", "Min / avg / max ms"),
            tr("net.col_jitter", "Jitter ms"),
            tr("net.col_loss", "Loss"),
            tr("net.col_bad", "Bad frames"),
        ] {
            ui.label(RichText::new(h).size(t.font_small).color(t.text_dim));
        }
        ui.end_row();
        for target in Target::ALL {
            let track = n.track(target);
            let s = track.stats();
            let h = Health::of(target, &s);
            ui.label(RichText::new(device_name(target)).color(t.text));
            ui.label(RichText::new(target.ip().to_string()).monospace().color(t.text_dim));
            pill(ui, h, target.id());
            let line = if h == Health::Waiting || h == Health::NotFitted { t.text_dim } else { t.accent };
            sparkline(ui, track, line, target.id());
            ui.label(RichText::new(format!("{} / {} / {}", ms(s.min_ms), ms(s.avg_ms), ms(s.max_ms))).monospace());
            ui.label(RichText::new(ms(s.jitter_ms)).monospace());
            let loss = if s.sent == 0 { "-".to_owned() } else { format!("{:.0} %", s.loss_pct()) };
            ui.label(RichText::new(loss).monospace());
            if target == Target::CardProtocol {
                let colour = if track.bad_frames > 0 { t.error } else { t.text };
                ui.label(RichText::new(track.bad_frames.to_string()).monospace().color(colour));
            } else {
                ui.label("");
            }
            ui.end_row();
        }
    });
}

pub fn window(ctx: &Context, app: &mut DarkBurn) {
    if ctx.data_mut(|d| d.remove_temp::<bool>(Id::unique(OPEN))).unwrap_or(false) {
        app.machine.ext.net.open = true;
    }
    let n = &mut app.machine.ext.net;
    if !n.open {
        return;
    }
    let running = n.running();
    let go = if running { tr("net.stop", "Stop") } else { tr("net.start", "Start") };
    let close = tr("net.close", "Close");
    let verdict = n.verdict();
    let (closed, action) = ui_kit::Window::new(Id::unique("db_net_test"), "settings", &tr("net.title", "Laser network test"))
        .dockable("db_net_test")
        .body_width(820.0)
        .actions(&go, &close, None)
        .show(ctx, true, |ui| {
            let t = Tokens::get();
            let r = ui.label(RichText::new(&verdict).size(t.font_heading).strong().color(t.text));
            solidcraft_ui_registry::publish_rect("net_test:verdict", r.rect);
            ui.add_space(t.space_6);
            rows(ui, n);
            ui.add_space(t.space_4);
            ui_kit::status_line(
                ui,
                &tr("net.note", "Read only: 1000-byte pings once a second and one status read of the card. Nothing is written to the machine."),
                ui_kit::StatusTone::Dim,
            );
        });
    match action {
        Action::Ok if running => n.stop(),
        Action::Ok => n.start(),
        _ => {}
    }
    if closed || action == Action::Cancel {
        n.stop();
        n.open = false;
    }
}
