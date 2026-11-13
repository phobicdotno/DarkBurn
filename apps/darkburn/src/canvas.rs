//! The design canvas: rulers, the work area with its grid, and zoom and pan.

use egui::{Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};
use solidcraft_theme::Tokens;

use crate::prefs::Prefs;
use crate::render::{self, Xf};
use crate::shell::DarkBurn;
use crate::view::{self, View};

/// Width of the rulers along the top and left edges.
pub const RULER: f32 = 20.0;

/// The canvas colours: the theme's own, or always light or always dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backdrop {
    #[default]
    Theme,
    Light,
    Dark,
}

impl Backdrop {
    pub const ALL: [Backdrop; 3] = [Backdrop::Theme, Backdrop::Light, Backdrop::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Backdrop::Theme => "Follow the theme",
            Backdrop::Light => "Light",
            Backdrop::Dark => "Dark",
        }
    }

    fn tokens(self) -> Tokens {
        match self {
            Backdrop::Theme => Tokens::get(),
            Backdrop::Light => Tokens::light(),
            Backdrop::Dark => Tokens::dark(),
        }
    }
}

/// A view change asked for from a menu, button or key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewCmd {
    ZoomIn,
    ZoomOut,
    ZoomPage,
    /// Fit this document rectangle (mm, Y up) in the view.
    Frame(darkburn_core::kurbo::Rect),
}

/// What the canvas reports back each frame.
#[derive(Default)]
pub struct Output {
    /// The pointer's position in machine coordinates (mm), when it is over the canvas.
    pub cursor: Option<Pos2>,
}

fn mix(a: Color32, b: Color32, k: f32) -> Color32 {
    let k = k.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Draw the canvas in `ui`'s whole rect and handle zoom, pan and the tools.
pub fn show(ui: &mut Ui, app: &mut DarkBurn, cmd: Option<ViewCmd>) -> Output {
    let pan_tool = app.pan_tool;
    let full = ui.max_rect();
    let canvas = Rect::from_min_max(full.min + vec2(RULER, RULER), full.max);
    let page = app.prefs.page();
    let mut out = Output::default();
    if !(canvas.width() > 1.0 && canvas.height() > 1.0) {
        return out;
    }
    solidcraft_ui_registry::publish_rect("canvas", canvas);
    let id = ui.scope_id().with("canvas");
    let response = ui.interact(canvas, id, Sense::click_and_drag());
    // The pointer on the canvas itself: not on a window, menu or list in front of it (the wheel
    // there scrolls that, and must not zoom the drawing behind).
    let hover = ui.input(|i| i.pointer.hover_pos()).filter(|p| canvas.contains(*p) && response.contains_pointer());
    let pan_mode = {
        let prefs = &mut app.prefs;
        let view = &mut prefs.view;
        if !view.placed {
            view.zoom_to_page(canvas, page);
        }

        // Zoom: wheel (plain or with Ctrl) and pinch about the pointer, keys and buttons about the
        // centre.
        if let Some(at) = hover {
            let invert = prefs.invert_wheel;
            let (mut factor, pinch) = ui.input(|i| {
                let mut f = 1.0;
                for e in &i.events {
                    if let egui::Event::MouseWheel { unit, delta, .. } = e {
                        let points = match unit {
                            egui::MouseWheelUnit::Point => delta.y,
                            egui::MouseWheelUnit::Line => delta.y * 50.0,
                            egui::MouseWheelUnit::Page => delta.y * 500.0,
                        };
                        f *= view::wheel_factor(points, invert);
                    }
                }
                (f, i.multi_touch().map(|t| t.zoom_delta))
            });
            if let Some(pinch) = pinch {
                factor *= pinch;
            }
            if factor != 1.0 {
                view.zoom_about(canvas, at, factor);
            }
        }
        match cmd {
            Some(ViewCmd::ZoomIn) => view.zoom_about(canvas, canvas.center(), view::ZOOM_STEP),
            Some(ViewCmd::ZoomOut) => view.zoom_about(canvas, canvas.center(), 1.0 / view::ZOOM_STEP),
            Some(ViewCmd::ZoomPage) => view.zoom_to_page(canvas, page),
            Some(ViewCmd::Frame(r)) => {
                // Document Y is up, page v is down.
                let min = pos2(r.x0 as f32, page.y - r.y1 as f32);
                view.zoom_to_rect(canvas, Rect::from_min_size(min, vec2(r.width() as f32, r.height() as f32)));
            }
            None => {}
        }

        // Pan: middle drag, Space + drag, or any drag with the pan tool.
        let space = ui.input(|i| i.key_down(egui::Key::Space)) && ui.memory(|m| m.focused().is_none());
        let panning = response.dragged_by(egui::PointerButton::Middle) || ((space || pan_tool) && response.dragged_by(egui::PointerButton::Primary));
        if panning {
            view.pan(response.drag_delta());
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if hover.is_some() && (space || pan_tool) {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        } else if hover.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
        space || panning
    };

    // The tools, then the picture with the view as they left it (auto-pan may have moved it).
    let xf = Xf::new(canvas, &app.prefs.view, page.y);
    crate::canvas_input::interact(ui, app, &response, canvas, &xf, pan_mode);
    let view = app.prefs.view;
    let xf = Xf::new(canvas, &view, page.y);
    let ct = app.prefs.backdrop.tokens();
    let dark = luminance(ct.field) < 128.0;
    paint_backdrop(ui, &app.prefs, &view, canvas);
    let painter = ui.painter().with_clip_rect(canvas);
    crate::vision_ui::paint_overlay(ui.ctx(), app, &xf, &painter);
    let now = ui.input(|i| i.time);
    let flash = app.flash_layer.map(|(layer, t0)| (layer, ((now - t0) * 6.0).floor() as i64 % 2 == 0));
    let selected = app.editor.selected();
    let scene = render::Scene {
        doc: &app.editor.doc,
        selected: &selected,
        filled: app.prefs.filled,
        smooth: app.prefs.smooth,
        dark,
        highlight: ct.accent,
        page: darkburn_core::kurbo::Rect::new(0.0, 0.0, page.x as f64, page.y as f64),
        flash,
    };
    render::paint(&painter, ui.ctx(), &xf, &scene);
    crate::canvas_input::paint_overlays(ui, app, &xf, &painter, ct.accent, dark);
    if let (true, Some(p)) = (app.prefs.opts.fullscreen_cursor, hover) {
        let line = Stroke::new(1.0, ct.accent.gamma_multiply(0.45));
        let gap = app.prefs.opts.cursor_size;
        painter.line_segment([pos2(canvas.left(), p.y), pos2(p.x - gap, p.y)], line);
        painter.line_segment([pos2(p.x + gap, p.y), pos2(canvas.right(), p.y)], line);
        painter.line_segment([pos2(p.x, canvas.top()), pos2(p.x, p.y - gap)], line);
        painter.line_segment([pos2(p.x, p.y + gap), pos2(p.x, canvas.bottom())], line);
    }
    crate::job_ui::paint_overlay(&painter, &xf, app, ct.accent);
    crate::text_ui::paint_overlay(app, &xf, &painter, ct.accent, now);
    crate::tangent_tool::paint_overlay(ui.ctx(), app, &xf, &painter, ct.accent);
    paint_rulers(ui, &app.prefs, &view, full, canvas);
    out.cursor = hover.map(|p| app.prefs.origin.to_machine(view.to_page(canvas, p), page));
    out
}

fn luminance(c: Color32) -> f32 {
    0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32
}

/// The surroundings, the work area, its grid, outline, centre cross and origin corner.
fn paint_backdrop(ui: &Ui, prefs: &Prefs, view: &View, canvas: Rect) {
    let ct = prefs.backdrop.tokens();
    let page = prefs.page();
    let painter = ui.painter().with_clip_rect(canvas);
    let page_rect = view.page_rect(canvas, page);

    // Surroundings, then the work area.
    painter.rect_filled(canvas, 0.0, ct.viewport_bottom);
    let page_fill = ct.field;
    painter.rect_filled(page_rect, 0.0, page_fill);

    // Grid: lines at the chosen spacing (or following the zoom), every fifth one stronger.
    let (major, minor) = view::grid_steps_for(view.zoom, prefs.grid_spacing);
    if prefs.show_grid {
        let k = 0.25 + 0.75 * prefs.grid_contrast;
        let major_ink = mix(page_fill, ct.grid_major, k * 1.4);
        let minor_ink = mix(page_fill, ct.grid, k * 1.25);
        let grid = painter.with_clip_rect(canvas.intersect(page_rect));
        let visible = Rect::from_min_max(view.to_page(canvas, canvas.min), view.to_page(canvas, canvas.max));
        let lines = |step: f32, ink: Color32, skip_major: bool| {
            for u in view::ticks(visible.left().max(0.0), visible.right().min(page.x), step) {
                if skip_major && (u / major).round() * major == u {
                    continue;
                }
                let x = view.to_screen(canvas, pos2(u, 0.0)).x;
                grid.vline(x, page_rect.y_range(), Stroke::new(1.0, ink));
            }
            for v in view::ticks(visible.top().max(0.0), visible.bottom().min(page.y), step) {
                if skip_major && (v / major).round() * major == v {
                    continue;
                }
                let y = view.to_screen(canvas, pos2(0.0, v)).y;
                grid.hline(page_rect.x_range(), y, Stroke::new(1.0, ink));
            }
        };
        if let Some(minor) = minor {
            lines(minor, minor_ink, true);
        }
        lines(major, major_ink, false);
    }

    // Page outline, centre cross and the origin corner.
    painter.rect_stroke(page_rect, 0.0, Stroke::new(1.0, mix(ct.border, ct.text_dim, 0.4)), egui::StrokeKind::Outside);
    let c = page_rect.center();
    let opts = crate::options::get();
    if opts.centre_cross {
        let arm = opts.cursor_size;
        let cross = Stroke::new(1.0, ct.accent);
        painter.line_segment([c - vec2(arm, 0.0), c + vec2(arm, 0.0)], cross);
        painter.line_segment([c - vec2(0.0, arm), c + vec2(0.0, arm)], cross);
    }
    let o = prefs.origin;
    let corner = pos2(if o.flips_x() { page_rect.right() } else { page_rect.left() }, if o.flips_y() { page_rect.bottom() } else { page_rect.top() });
    painter.circle_filled(corner, 3.5, ct.accent);
}

/// The rulers along the top and left, in machine coordinates.
fn paint_rulers(ui: &Ui, prefs: &Prefs, view: &View, full: Rect, canvas: Rect) {
    let t = Tokens::get();
    let page = prefs.page();
    let o = prefs.origin;
    let (major, minor) = view::grid_steps(view.zoom);
    // Rulers in machine coordinates.
    let rp = ui.painter().with_clip_rect(full);
    let top = Rect::from_min_max(pos2(canvas.left(), full.top()), pos2(full.right(), canvas.top()));
    let left = Rect::from_min_max(pos2(full.left(), canvas.top()), pos2(canvas.left(), full.bottom()));
    let corner_box = Rect::from_min_max(full.min, canvas.min);
    for r in [top, left, corner_box] {
        rp.rect_filled(r, 0.0, t.toolbar);
    }
    rp.hline(full.x_range(), canvas.top() - 0.5, Stroke::new(1.0, t.border));
    rp.vline(canvas.left() - 0.5, full.y_range(), Stroke::new(1.0, t.border));
    let font = FontId::proportional(t.font_micro);
    let ink = t.text_dim;
    let tick = Stroke::new(1.0, ink);
    let small_step = minor.unwrap_or(major / 5.0);
    let a = view.to_page(canvas, canvas.min);
    let b = view.to_page(canvas, canvas.max);
    // Horizontal ruler.
    {
        let clip = rp.with_clip_rect(top);
        for u in view::ticks(a.x, b.x, small_step) {
            let x = view.to_screen(canvas, pos2(u, 0.0)).x;
            let big = ((u / major).round() * major - u).abs() < small_step * 0.01;
            let h = if big { RULER * 0.55 } else { RULER * 0.22 };
            clip.vline(x, (top.bottom() - h)..=top.bottom(), tick);
            if big {
                let m = if o.flips_x() { page.x - u } else { u };
                clip.text(pos2(x + 3.0, top.top() + 1.0), Align2::LEFT_TOP, view::format_mm(m, major), font.clone(), ink);
            }
        }
    }
    // Vertical ruler, labels written top to bottom.
    {
        let clip = rp.with_clip_rect(left);
        for v in view::ticks(a.y, b.y, small_step) {
            let y = view.to_screen(canvas, pos2(0.0, v)).y;
            let big = ((v / major).round() * major - v).abs() < small_step * 0.01;
            let w = if big { RULER * 0.55 } else { RULER * 0.22 };
            clip.hline((left.right() - w)..=left.right(), y, tick);
            if big {
                let m = if o.flips_y() { page.y - v } else { v };
                let galley = clip.layout_no_wrap(view::format_mm(m, major), font.clone(), ink);
                let at = pos2(left.left() + 2.0, y + 3.0 + galley.size().x);
                clip.add(egui::epaint::TextShape::new(at, galley, ink).with_angle(-std::f32::consts::FRAC_PI_2));
            }
        }
    }
    rp.text(corner_box.center(), Align2::CENTER_CENTER, "mm", font, ink);
}
