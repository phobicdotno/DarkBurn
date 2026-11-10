//! The text interface: the text tool (place, type in place with an I-beam, double-click to
//! edit), the Text Options toolbar, the font picker, the Edit Text Shapes and QR code dialogs, the
//! Variable Text panel and the text settings. Document changes go through `text_cmds.rs`; typing
//! in place edits the shape directly inside one undo step.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use darkburn_core::kurbo::{Affine, BezPath, Point};
use darkburn_core::shape::{AlignX, AlignY, VariableText};
use darkburn_core::{CodeSpec, Editor, Shape, ShapeId, ShapeKind, TextShape};
use darkburn_text::{FontPrefs, MAX_QR_BYTES, QrContent, WifiSecurity, geomutil};
use darkburn_ui::ui_kit;
use egui::{Color32, Context, CursorIcon, Id, Key, Pos2, Rect, Response, RichText, Sense, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};
use solidcraft_theme::Tokens;

use crate::numeric::{self, Num};
use crate::render::Xf;
use crate::shell::DarkBurn;
use crate::text_cmds;
use crate::text_render as render;
use crate::tools::Tool;

pub const HINT: &str = "Click to place text and type; the I-beam shows where. Enter starts a new line, Esc finishes. Click text, or double-click it with the Select tool, to edit it.";

// ================================================================ typing

/// What a keystroke did to the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Done {
    Nothing,
    Changed,
    Finish,
}

/// A caret in a text, in characters, with an optional selection anchor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Caret {
    pub at: usize,
    pub anchor: Option<usize>,
}

fn byte_of(s: &str, chars: usize) -> usize {
    s.char_indices().nth(chars).map(|(i, _)| i).unwrap_or(s.len())
}

impl Caret {
    pub fn end_of(s: &str) -> Caret {
        Caret { at: s.chars().count(), anchor: None }
    }

    fn selection(&self) -> Option<(usize, usize)> {
        self.anchor.filter(|a| *a != self.at).map(|a| (a.min(self.at), a.max(self.at)))
    }

    pub fn selected_text(&self, s: &str) -> Option<String> {
        self.selection().map(|(a, b)| s[byte_of(s, a)..byte_of(s, b)].to_string())
    }

    fn delete_selection(&mut self, s: &mut String) -> bool {
        let Some((a, b)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        s.replace_range(byte_of(s, a)..byte_of(s, b), "");
        self.at = a;
        self.anchor = None;
        true
    }

    /// Type `text` at the caret (control characters are dropped, newlines kept).
    pub fn insert(&mut self, s: &mut String, text: &str) -> bool {
        let clean: String = text.chars().filter(|c| *c == '\n' || !c.is_control()).collect();
        if clean.is_empty() {
            return false;
        }
        self.delete_selection(s);
        s.insert_str(byte_of(s, self.at), &clean);
        self.at += clean.chars().count();
        true
    }

    pub fn backspace(&mut self, s: &mut String) -> bool {
        if self.delete_selection(s) {
            return true;
        }
        if self.at == 0 {
            return false;
        }
        let (a, b) = (byte_of(s, self.at - 1), byte_of(s, self.at));
        s.replace_range(a..b, "");
        self.at -= 1;
        true
    }

    pub fn delete(&mut self, s: &mut String) -> bool {
        if self.delete_selection(s) {
            return true;
        }
        if self.at >= s.chars().count() {
            return false;
        }
        let (a, b) = (byte_of(s, self.at), byte_of(s, self.at + 1));
        s.replace_range(a..b, "");
        true
    }

    fn line_bounds(s: &str, at: usize) -> (usize, usize) {
        let chars: Vec<char> = s.chars().collect();
        let mut start = at.min(chars.len());
        while start > 0 && chars[start - 1] != '\n' {
            start -= 1;
        }
        let mut end = at.min(chars.len());
        while end < chars.len() && chars[end] != '\n' {
            end += 1;
        }
        (start, end)
    }

    /// Move the caret; `select` extends the selection.
    pub fn go(&mut self, s: &str, key: Key, select: bool) {
        if select {
            self.anchor.get_or_insert(self.at);
        } else if let Some((a, b)) = self.selection()
            && matches!(key, Key::ArrowLeft | Key::ArrowRight)
        {
            self.at = if key == Key::ArrowLeft { a } else { b };
            self.anchor = None;
            return;
        } else {
            self.anchor = None;
        }
        let len = s.chars().count();
        let (ls, le) = Self::line_bounds(s, self.at);
        match key {
            Key::ArrowLeft => self.at = self.at.saturating_sub(1),
            Key::ArrowRight => self.at = (self.at + 1).min(len),
            Key::Home => self.at = ls,
            Key::End => self.at = le,
            Key::ArrowUp => {
                if ls > 0 {
                    let col = self.at - ls;
                    let (ps, pe) = Self::line_bounds(s, ls - 1);
                    self.at = (ps + col).min(pe);
                } else {
                    self.at = 0;
                }
            }
            Key::ArrowDown => {
                if le < len {
                    let col = self.at - ls;
                    let (ns, ne) = Self::line_bounds(s, le + 1);
                    self.at = (ns + col).min(ne);
                } else {
                    self.at = len;
                }
            }
            _ => {}
        }
    }

    pub fn select_all(&mut self, s: &str) {
        self.anchor = Some(0);
        self.at = s.chars().count();
    }
}

/// Apply one input event to a text. Pure, so it can be tested.
pub fn apply_event(caret: &mut Caret, s: &mut String, ev: &egui::Event, clipboard: &mut Option<String>) -> Done {
    match ev {
        egui::Event::Text(t) | egui::Event::Paste(t) => {
            if caret.insert(s, t) {
                Done::Changed
            } else {
                Done::Nothing
            }
        }
        egui::Event::Ime(egui::ImeEvent::Commit(t)) => {
            if caret.insert(s, t) {
                Done::Changed
            } else {
                Done::Nothing
            }
        }
        egui::Event::Copy => {
            *clipboard = caret.selected_text(s);
            Done::Nothing
        }
        egui::Event::Cut => {
            *clipboard = caret.selected_text(s);
            if caret.delete_selection(s) { Done::Changed } else { Done::Nothing }
        }
        egui::Event::Key { key, pressed: true, modifiers, .. } => match key {
            Key::Escape => Done::Finish,
            Key::Enter if modifiers.command => Done::Finish,
            Key::Enter => {
                if caret.insert(s, "\n") {
                    Done::Changed
                } else {
                    Done::Nothing
                }
            }
            Key::Backspace => {
                if caret.backspace(s) {
                    Done::Changed
                } else {
                    Done::Nothing
                }
            }
            Key::Delete => {
                if caret.delete(s) {
                    Done::Changed
                } else {
                    Done::Nothing
                }
            }
            Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown | Key::Home | Key::End => {
                caret.go(s, *key, modifiers.shift);
                Done::Nothing
            }
            Key::A if modifiers.command => {
                caret.select_all(s);
                Done::Nothing
            }
            _ => Done::Nothing,
        },
        _ => Done::Nothing,
    }
}

/// A text being typed on the canvas.
#[derive(Clone, Debug)]
pub struct Typing {
    pub id: ShapeId,
    pub caret: Caret,
    /// The shape was made by this session (an empty one is taken back).
    pub new_shape: bool,
    pub original: String,
    /// The editor revision after our last change; another change ends the session.
    rev: u64,
    frames: u32,
}

fn typing_id() -> Id {
    Id::unique("darkburn_text_typing")
}

// ================================================================ state

/// The QR code and barcode dialog.
#[derive(Default)]
pub struct QrDialog {
    pub open: bool,
    /// The shape being edited (None: a new one).
    pub editing: Option<ShapeId>,
    barcode: bool,
    kind: usize,
    text: String,
    url: String,
    ssid: String,
    password: String,
    security: usize,
    hidden: bool,
    name: String,
    phone: String,
    email: String,
    website: String,
    address: String,
    note: String,
    ecc: usize,
    size: f64,
    variable: bool,
    code: usize,
    data: String,
    height: f64,
    init: bool,
}

impl QrDialog {
    pub fn open_new(&mut self) {
        *self = QrDialog { open: true, size: 30.0, ecc: 1, height: 15.0, ..Default::default() };
        self.init = true;
    }

    fn open_for(&mut self, id: ShapeId, c: &darkburn_core::CodeShape, size: f64) {
        *self = QrDialog { open: true, editing: Some(id), size, variable: c.variable, init: true, ..Default::default() };
        match &c.spec {
            CodeSpec::Qr(q) => {
                self.ecc = q.ecc as usize;
                match &q.content {
                    QrContent::Text(t) => self.text = t.clone(),
                    QrContent::Url(u) => {
                        self.kind = 1;
                        self.url = u.clone();
                    }
                    QrContent::Wifi { ssid, password, security, hidden } => {
                        self.kind = 2;
                        self.ssid = ssid.clone();
                        self.password = password.clone();
                        self.security = match security {
                            WifiSecurity::Wpa => 0,
                            WifiSecurity::Wep => 1,
                            WifiSecurity::Open => 2,
                        };
                        self.hidden = *hidden;
                    }
                    QrContent::Contact(k) => {
                        self.kind = 3;
                        self.name = k.name.clone();
                        self.phone = k.phone.clone();
                        self.email = k.email.clone();
                        self.website = k.url.clone();
                        self.address = k.address.clone();
                        self.note = k.note.clone();
                    }
                }
            }
            CodeSpec::Barcode(b) => {
                self.barcode = true;
                self.data = b.data.clone();
                self.height = b.height_mm;
                self.code = BARCODES.iter().position(|(_, k)| *k == b.kind).unwrap_or(0);
            }
        }
    }

    fn args(&self) -> Value {
        if self.barcode {
            return json!({ "barcode": BARCODES[self.code.min(BARCODES.len() - 1)].0, "data": self.data, "height": self.height, "variable": self.variable });
        }
        let ecc = ["L", "M", "Q", "H"][self.ecc.min(3)];
        let mut v = json!({ "ecc": ecc, "variable": self.variable });
        if self.size > 0.0 {
            v["size"] = json!(self.size);
        }
        match self.kind {
            0 => {
                v["kind"] = json!("text");
                v["text"] = json!(self.text);
            }
            1 => {
                v["kind"] = json!("url");
                v["url"] = json!(self.url);
            }
            2 => {
                v["kind"] = json!("wifi");
                v["ssid"] = json!(self.ssid);
                v["password"] = json!(self.password);
                v["security"] = json!(["wpa", "wep", "open"][self.security.min(2)]);
                v["hidden"] = json!(self.hidden);
            }
            _ => {
                v["kind"] = json!("contact");
                for (k, f) in [
                    ("name", &self.name),
                    ("phone", &self.phone),
                    ("email", &self.email),
                    ("website", &self.website),
                    ("address", &self.address),
                    ("note", &self.note),
                ] {
                    v[k] = json!(f);
                }
            }
        }
        v
    }
}

const BARCODES: [(&str, darkburn_text::BarcodeKind); 6] = [
    ("code128", darkburn_text::BarcodeKind::Code128),
    ("code39", darkburn_text::BarcodeKind::Code39),
    ("ean13", darkburn_text::BarcodeKind::Ean13),
    ("upca", darkburn_text::BarcodeKind::UpcA),
    ("itf", darkburn_text::BarcodeKind::Itf),
    ("datamatrix", darkburn_text::BarcodeKind::DataMatrix),
];
const BARCODE_NAMES: [&str; 6] = ["Code 128", "Code 39", "EAN-13", "UPC-A", "ITF", "Data Matrix"];

/// The Edit Text Shapes dialog.
#[derive(Default)]
pub struct EditDialog {
    pub open: bool,
    drafts: Vec<(ShapeId, TextShape)>,
    current: usize,
    zoom: f32,
    pan: egui::Vec2,
}

/// State of the text interface.
#[derive(Default)]
pub struct TextUi {
    pub qr: QrDialog,
    pub edit: EditDialog,
    pub font_picker_open: bool,
    pub typing: Option<Typing>,
    /// Values shown by the Variable Text panel's Test button.
    pub test_values: Vec<String>,
    rendered_rev: Option<u64>,
    rendered_at: f64,
}

impl TextUi {
    pub fn open_edit_dialog(&mut self, editor: &Editor) {
        let mut drafts = Vec::new();
        for id in editor.selected() {
            collect_text(editor.doc.get(id), &mut drafts);
        }
        if drafts.is_empty() {
            return;
        }
        self.edit = EditDialog { open: true, drafts, current: 0, zoom: 1.0, pan: egui::Vec2::ZERO };
    }
}

fn collect_text(s: Option<&Shape>, out: &mut Vec<(ShapeId, TextShape)>) {
    let Some(s) = s else { return };
    match &s.kind {
        ShapeKind::Text(t) => out.push((s.id, t.clone())),
        ShapeKind::Group { children } if s.array.is_none() => children.iter().for_each(|c| collect_text(Some(c), out)),
        _ => {}
    }
}

/// Once a frame: render text and codes whose inputs changed (undo, array edits, a new variable
/// text state, the clock for date and time text).
pub fn tick(app: &mut DarkBurn, ctx: &Context) {
    let now = ctx.input(|i| i.time);
    let rev = app.editor.revision();
    if app.text.rendered_rev == Some(rev) && now - app.text.rendered_at < 1.0 {
        return;
    }
    app.text.rendered_rev = Some(rev);
    app.text.rendered_at = now;
    let was_modified = app.editor.is_modified();
    if render::refresh(&mut app.editor.doc) > 0 {
        app.editor.touch();
        if let Some(t) = &mut app.text.typing {
            t.rev = app.editor.revision();
        }
        app.text.rendered_rev = Some(app.editor.revision());
        if !was_modified {
            app.editor.mark_saved();
        }
    }
}

// ================================================================ the text tool

fn text_at(app: &DarkBurn, p: Point, tol: f64) -> Option<ShapeId> {
    darkburn_core::select::hits_at(&app.editor.doc, p, tol)
        .into_iter()
        .find(|id| app.editor.doc.get(*id).is_some_and(|s| matches!(s.kind, ShapeKind::Text(_) | ShapeKind::Code(_))))
}

fn text_mut(app: &mut DarkBurn, id: ShapeId) -> Option<&mut TextShape> {
    match &mut app.editor.doc.get_mut(id)?.kind {
        ShapeKind::Text(t) => Some(t),
        _ => None,
    }
}

/// Start typing into the text shape `id` (or a new one made at `at`).
pub fn begin_typing(app: &mut DarkBurn, id: ShapeId, new_shape: bool) {
    let Some(text) = text_mut(app, id).map(|t| t.text.clone()) else { return };
    app.editor.selection.set([id]);
    app.text.typing = Some(Typing { id, caret: Caret::end_of(&text), new_shape, original: text, rev: app.editor.revision(), frames: 0 });
}

/// Place a new, empty text at `at` and start typing.
pub fn place_text(app: &mut DarkBurn, at: Point) {
    finish_typing(app);
    let t = TextShape {
        font: text_cmds::default_font(app),
        height: app.prefs.fonts.default_size.max(0.1),
        welded: app.prefs.opts.auto_weld,
        ..Default::default()
    };
    let layer = app.editor.current_layer;
    app.editor.checkpoint("Add text");
    let id = app.editor.doc.add(Shape::new(ShapeKind::Text(t), Affine::translate(at.to_vec2()), layer));
    app.editor.touch();
    begin_typing(app, id, true);
}

/// End the typing session. An empty new text is taken back; an unchanged one leaves no undo
/// step; a text that was typed away to nothing is taken out of the design (undo brings it back).
pub fn finish_typing(app: &mut DarkBurn) {
    let Some(t) = app.text.typing.take() else { return };
    let text = text_mut(app, t.id).map(|x| x.text.clone());
    let alone = app.editor.revision() == t.rev;
    let empty = text.as_deref().is_none_or(|s| s.trim().is_empty());
    let unchanged = !t.new_shape && text.as_deref() == Some(t.original.as_str());
    if alone && (empty && t.new_shape || unchanged) {
        app.editor.drop_checkpoint();
        app.editor.selection.retain_existing(&app.editor.doc);
    } else if empty && text.is_some() {
        // Nothing left to draw: an empty text would stay as a shape nobody can see or click.
        app.editor.doc.remove(&[t.id]);
        app.editor.touch();
        app.editor.selection.retain_existing(&app.editor.doc);
    } else if let Some(font) = text_mut(app, t.id).map(|x| x.font.clone()) {
        app.prefs.fonts.touch(&font);
    }
}

fn type_events(ctx: &Context, app: &mut DarkBurn) {
    let Some(mut ty) = app.text.typing.take() else { return };
    let events = ctx.input(|i| i.events.clone());
    let mut clipboard = None;
    let mut finish = false;
    let mut changed = false;
    if let Some(t) = text_mut(app, ty.id) {
        let mut s = t.text.clone();
        for ev in &events {
            match apply_event(&mut ty.caret, &mut s, ev, &mut clipboard) {
                Done::Changed => changed = true,
                Done::Finish => {
                    finish = true;
                    break;
                }
                Done::Nothing => {}
            }
        }
        if changed {
            t.text = s;
        }
    } else {
        finish = true;
    }
    if let Some(c) = clipboard {
        ctx.copy_text(c);
    }
    if changed {
        render::refresh(&mut app.editor.doc);
        app.editor.touch();
        ty.rev = app.editor.revision();
        app.text.rendered_rev = Some(ty.rev);
    }
    app.text.typing = Some(ty);
    if finish {
        finish_typing(app);
    }
}

/// The text tool on the canvas.
pub fn interact(ui: &Ui, app: &mut DarkBurn, response: &Response, xf: &Xf, raw: Option<Point>) {
    let ctx = ui.ctx().clone();
    ctx.set_cursor_icon(CursorIcon::Text);
    let tol = xf.mm(6.0);
    if response.clicked_by(egui::PointerButton::Primary)
        && let Some(p) = raw
    {
        match text_at(app, p, tol) {
            Some(id) if matches!(app.editor.doc.get(id).map(|s| &s.kind), Some(ShapeKind::Text(_))) => {
                if app.text.typing.as_ref().map(|t| t.id) != Some(id) {
                    finish_typing(app);
                    app.editor.checkpoint("Edit text");
                    app.editor.touch();
                    begin_typing(app, id, false);
                    if let Some(t) = &mut app.text.typing {
                        t.rev = app.editor.revision();
                    }
                }
            }
            Some(id) => open_code_dialog(app, id),
            None => place_text(app, p),
        }
    }
    // A dialog, window or menu over the canvas has the keyboard: what is typed there is not for
    // the text behind it. Typing goes on when it has closed.
    if crate::escape::window_open(&ctx) {
        return;
    }
    if let Some(ty) = &mut app.text.typing {
        ty.frames += 1;
        let id = typing_id();
        ui.interact(response.rect, id, Sense::focusable_noninteractive());
        ctx.memory_mut(|m| {
            m.request_focus(id);
            m.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true });
        });
        let lost = ty.frames > 2 && !ctx.memory(|m| m.has_focus(id));
        let other_change = app.editor.revision() != ty.rev;
        if lost || other_change {
            finish_typing(app);
        } else {
            type_events(&ctx, app);
        }
    }
}

/// Double-click with the Select tool: edit the text or code under the pointer.
pub fn on_double_click(app: &mut DarkBurn, raw: Option<Point>, tol: f64) -> bool {
    let Some(p) = raw else { return false };
    let Some(id) = text_at(app, p, tol) else { return false };
    if matches!(app.editor.doc.get(id).map(|s| &s.kind), Some(ShapeKind::Text(_))) {
        app.set_tool(Tool::Text);
        finish_typing(app);
        app.editor.checkpoint("Edit text");
        app.editor.touch();
        begin_typing(app, id, false);
        if let Some(t) = &mut app.text.typing {
            t.rev = app.editor.revision();
        }
    } else {
        open_code_dialog(app, id);
    }
    true
}

fn open_code_dialog(app: &mut DarkBurn, id: ShapeId) {
    let Some(s) = app.editor.doc.get(id) else { return };
    if let ShapeKind::Code(c) = &s.kind {
        let size = s.bbox().width();
        let c = c.clone();
        app.editor.selection.set([id]);
        app.text.qr.open_for(id, &c, size);
    }
}

/// Where the caret is, in the text shape's own coordinates: (x, bottom, top).
pub fn caret_geometry(t: &TextShape, caret: usize) -> Option<(f64, f64, f64)> {
    let text: String = t.text.clone();
    let spec = render::spec_of(t, text.clone());
    let (full, _) = render::layout_of(t, &spec)?;
    let before: String = text.chars().take(caret).collect();
    let line = before.matches('\n').count();
    let prefix_line = before.rsplit('\n').next().unwrap_or("");
    let left = darkburn_text::TextSpec { text: prefix_line.to_string(), h_align: darkburn_text::HAlign::Left, ..spec.clone() };
    let (pre, _) = render::layout_of(t, &left)?;
    let pw = pre.lines.first().map(|l| l.width).unwrap_or(0.0);
    let m = full.metrics;
    let (x0, base) = match full.lines.get(line) {
        Some(l) => (l.x_start, l.baseline_y),
        None => (0.0, -(line as f64) * m.line_height_mm),
    };
    Some((x0 + pw, base + m.descent_mm.min(0.0), base + m.cap_height_mm * 1.15))
}

/// The caret and the box round the text being typed.
pub fn paint_overlay(app: &DarkBurn, xf: &Xf, painter: &egui::Painter, accent: Color32, time: f64) {
    let Some(ty) = &app.text.typing else { return };
    let Some(s) = app.editor.doc.get(ty.id) else { return };
    let ShapeKind::Text(t) = &s.kind else { return };
    let bb = s.bbox();
    if bb.width() > 0.0 || bb.height() > 0.0 {
        let r = xf.rect(bb.inflate(1.0, 1.0));
        let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
        painter.extend(egui::Shape::dashed_line(&corners, Stroke::new(1.0, accent.gamma_multiply(0.8)), 4.0, 3.0));
        if ty.caret.selection().is_some_and(|(a, b)| a == 0 && b == t.text.chars().count()) {
            painter.rect_filled(xf.rect(bb), 0.0, accent.gamma_multiply(0.25));
        }
    }
    if (time * 2.0).floor() as i64 % 2 == 0 {
        let (x, y0, y1) = caret_geometry(t, ty.caret.at).unwrap_or((0.0, 0.0, t.height));
        let a = xf.to_screen(s.transform * Point::new(x, y0));
        let b = xf.to_screen(s.transform * Point::new(x, y1));
        painter.line_segment([a, b], Stroke::new(1.5, accent));
    }
    painter.ctx().request_repaint_after(std::time::Duration::from_millis(250));
}

// ================================================================ font picker

type Preview = Arc<Vec<Vec<[f32; 2]>>>;

fn previews() -> &'static Mutex<HashMap<(String, String), Preview>> {
    static P: OnceLock<Mutex<HashMap<(String, String), Preview>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// The outline of `text` in `font`, as polylines in a 1 x 1 box (x right, y down), cached.
pub fn preview_lines(font: &str, text: &str) -> Preview {
    let key = (font.to_string(), text.to_string());
    if let Some(p) = previews().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return p.clone();
    }
    let t = TextShape { font: font.to_string(), height: 10.0, ..Default::default() };
    let mut spec = render::spec_of(&t, text.to_string());
    spec.v_align = darkburn_text::VAlign::Bottom;
    let lines: Vec<Vec<[f32; 2]>> = match render::layout_of(&t, &spec) {
        Some((layout, _)) => {
            let paths = layout.outlines(false);
            let bb = geomutil::union_bbox(paths.iter()).unwrap_or_default();
            let s = bb.width().max(bb.height() * 2.0).max(1e-6);
            paths
                .iter()
                .flat_map(|p| geomutil::flatten_polylines(p, 0.05))
                .map(|(pts, closed)| {
                    let mut v: Vec<[f32; 2]> = pts.iter().map(|q| [((q.x - bb.x0) / s) as f32, (1.0 - (q.y - bb.y0) / s) as f32]).collect();
                    if closed && let Some(f) = v.first().copied() {
                        v.push(f);
                    }
                    v
                })
                .collect()
        }
        None => Vec::new(),
    };
    let p = Arc::new(lines);
    let mut m = previews().lock().unwrap_or_else(|e| e.into_inner());
    if m.len() > 600 {
        m.clear();
    }
    m.insert(key, p.clone());
    p
}

fn paint_preview(painter: &egui::Painter, rect: Rect, lines: &[Vec<[f32; 2]>], ink: Color32) {
    // Fit the 1 x 0.5 preview box into `rect`, left aligned and centred vertically.
    let s = rect.width().min(rect.height() * 2.0);
    let top = rect.center().y - s / 4.0;
    for l in lines {
        let pts: Vec<Pos2> = l.iter().map(|p| pos2(rect.left() + p[0] * s, top + p[1] * s)).collect();
        painter.add(egui::Shape::line(pts, Stroke::new(1.0, ink)));
    }
}

/// The font name shown for a font setting (SHX files by file name).
pub fn font_label(font: &str) -> String {
    match font.strip_prefix(render::SHX_PREFIX) {
        Some(path) => format!("{} (SHX)", std::path::Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or(path)),
        None => font.to_string(),
    }
}

/// A font drop-down with search, favourites, recent fonts and a live preview. Returns the
/// font picked this frame.
pub fn font_picker(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    current: &str,
    width: f32,
    prefs: &mut FontPrefs,
    shx: &[String],
    sample: &str,
) -> Option<String> {
    let t = Tokens::get();
    let mut picked = None;
    let sample = if sample.trim().is_empty() { "Sample text" } else { sample.lines().next().unwrap_or("Sample").trim() };
    let sample: String = sample.chars().take(24).collect();
    let id = Id::unique(("font_picker", ui.scope_id(), format!("{salt:?}")));
    let hovered_id = id.with("hover");
    let shown = font_label(current);
    ui.scope(|ui| {
        ui.set_max_width(width);
        ui_kit::dropdown_menu(ui, ("font_picker_menu", id), &shown, |ui| {
            ui.set_min_width(width.max(280.0));
            let mut query: String = ui.data_mut(|d| d.get_temp(id.with("q"))).unwrap_or_default();
            let edit = ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search fonts").desired_width(f32::INFINITY));
            crate::a11y::text(ui, &edit, "Search fonts");
            if edit.changed() {
                ui.data_mut(|d| d.insert_temp(id.with("q"), query.clone()));
            }
            let hovered: Option<String> = ui.data(|d| d.get_temp(hovered_id));
            let shown_font = hovered.clone().unwrap_or_else(|| current.to_string());
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
            ui.painter().rect_filled(rect, t.radius_sm, t.control_fill);
            paint_preview(ui.painter(), rect.shrink(6.0), &preview_lines(&shown_font, &sample), t.text);
            let (favs, recent, all) = {
                let lib = render::lib();
                let installed = darkburn_text::list_families(&lib);
                let l = prefs.listing(&installed, &query, false);
                (l.favourites, l.recent, l.all)
            };
            let shx_list: Vec<String> =
                shx.iter().filter(|s| query.is_empty() || font_label(s).to_lowercase().contains(&query.to_lowercase())).cloned().collect();
            let mut new_hover = None;
            let mut toggle = None;
            egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                let mut section = |ui: &mut Ui, title: &str, list: &[String]| {
                    if list.is_empty() {
                        return;
                    }
                    ui_kit::section_heading(ui, title);
                    for f in list {
                        ui.horizontal(|ui| {
                            let star = if prefs.is_favourite(f) { "\u{2605}" } else { "\u{2606}" };
                            if ui.add(egui::Button::new(star).frame(false)).on_hover_text("Favourite").clicked() {
                                toggle = Some(f.clone());
                            }
                            let r = ui.selectable_label(f == current, font_label(f));
                            if r.hovered() {
                                new_hover = Some(f.clone());
                            }
                            if r.clicked() {
                                picked = Some(f.clone());
                            }
                            let (pr, _) = ui.allocate_exact_size(vec2(90.0, 16.0), Sense::hover());
                            if ui.is_rect_visible(pr) {
                                paint_preview(ui.painter(), pr, &preview_lines(f, "Abc"), t.text_dim);
                            }
                        });
                    }
                };
                section(ui, "Favourites", &favs);
                section(ui, "Recent", &recent);
                section(ui, "Single-line (SHX)", &shx_list);
                section(ui, "All fonts", &all);
            });
            ui.data_mut(|d| match new_hover {
                Some(h) => {
                    d.insert_temp(hovered_id, h);
                }
                None => {
                    d.remove_temp::<String>(hovered_id);
                }
            });
            if let Some(f) = toggle {
                prefs.toggle_favourite(&f);
            }
        });
    });
    if let Some(p) = &picked {
        prefs.touch(p);
    }
    picked
}

// ================================================================ ribbon: the Text Options toolbar

fn selected_texts(app: &DarkBurn) -> (Vec<ShapeId>, Option<TextShape>) {
    let ids = text_cmds::selected_text(app, &app.editor.selected());
    let first = ids.first().and_then(|i| text_cmds::find_text(&app.editor.doc, *i)).cloned();
    (ids, first)
}

fn apply(app: &mut DarkBurn, ids: &[ShapeId], mut args: Value) {
    args["ids"] = json!(ids.iter().map(|i| i.0).collect::<Vec<_>>());
    if let Err(e) = crate::api::execute_headless(app, "text.set", &args) {
        app.note = Some(e);
    }
}

/// The Text Options toolbar groups drawn in the ribbon: `kind` is text_font, text_style,
/// text_spacing, text_align or text_variable. Returns the right edge.
pub fn ribbon_item(ui: &mut Ui, kind: &str, at: Pos2, height: f32, app: &mut DarkBurn) -> f32 {
    let tk = Tokens::get();
    let row_h = tk.control_height;
    let (ids, first) = selected_texts(app);
    let live = first.is_some();
    let t =
        first.clone().unwrap_or_else(|| TextShape { font: text_cmds::default_font(app), height: app.prefs.fonts.default_size, ..Default::default() });
    let label = |ui: &mut Ui, rect: Rect, s: &str| {
        ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, s, egui::FontId::proportional(tk.font_small), tk.text_dim);
    };
    match kind {
        "text_font" => {
            let area = Rect::from_min_size(at, vec2(210.0, height));
            let mut next = None;
            ui.scope_builder(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.horizontal(|ui| {
                    ui.set_height(row_h);
                    let sample = t.text.clone();
                    let shx = app.prefs.shx_fonts.clone();
                    let mut prefs = app.prefs.fonts.clone();
                    let font = font_picker(ui, "ribbon", &t.font, 150.0, &mut prefs, &shx, &sample);
                    app.prefs.fonts = prefs;
                    if let Some(f) = font {
                        next = Some(json!({ "font": f }));
                    }
                });
                ui.horizontal(|ui| {
                    ui.set_height(row_h);
                    ui.label(RichText::new("Height").size(tk.font_small).color(tk.text_dim));
                    let mut h = t.height;
                    let ed = ui
                        .push_id("text_height", |ui| numeric::field_sized(ui, &mut h, &Num::new(0.1..=10_000.0, 2, " mm").speed(0.1), Some(90.0)))
                        .inner;
                    crate::a11y::number(ui, &ed.response, "Text height");
                    if ed.changed {
                        if live {
                            next = Some(json!({ "height": h.max(0.1) }));
                        } else {
                            app.prefs.fonts.default_size = h.max(0.1);
                        }
                    }
                });
            });
            if let Some(a) = next {
                if live {
                    apply(app, &ids, a);
                } else if let Some(f) = a.get("font").and_then(Value::as_str) {
                    app.prefs.fonts.default_family = f.to_string();
                }
            }
            area.right()
        }
        "text_style" => {
            let area = Rect::from_min_size(pos2(at.x, at.y + 4.0), vec2(250.0, height));
            let mut next = None;
            ui.scope_builder(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::left_to_right(egui::Align::Min)), |ui| {
                ui.add_enabled_ui(live, |ui| {
                    let mut col = |ui: &mut Ui, items: &mut [(&str, bool, &str)]| {
                        ui.vertical(|ui| {
                            for (label, value, key) in items.iter_mut() {
                                if ui_kit::checkbox(ui, label, value).changed() {
                                    next = Some(json!({ *key: *value }));
                                }
                            }
                        });
                    };
                    col(ui, &mut [("Bold", t.bold, "bold"), ("Italic", t.italic, "italic")]);
                    col(ui, &mut [("Upper case", t.upper_case, "upper_case"), ("Welded", t.welded, "welded")]);
                    col(ui, &mut [("Distort", t.distort, "distort")]);
                });
            });
            if let Some(a) = next {
                apply(app, &ids, a);
            }
            area.right()
        }
        "text_spacing" => {
            let area = Rect::from_min_size(at, vec2(150.0, height));
            let mut next = None;
            ui.scope_builder(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
                ui.add_enabled_ui(live, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for (name, key, v) in [("Letters", "letter_spacing", t.letter_spacing), ("Lines", "line_spacing", t.line_spacing)] {
                        ui.horizontal(|ui| {
                            ui.set_height(row_h);
                            let lr = ui.allocate_exact_size(vec2(48.0, row_h), Sense::hover()).0;
                            label(ui, lr, name);
                            let mut x = v;
                            let ed = ui
                                .push_id(("text_spacing", key), |ui| {
                                    numeric::field_sized(ui, &mut x, &Num::new(-100.0..=1000.0, 2, " mm").speed(0.05), Some(90.0))
                                })
                                .inner;
                            crate::a11y::number(ui, &ed.response, &format!("{name} spacing"));
                            if ed.changed {
                                next = Some(json!({ key: x }));
                            }
                        });
                    }
                });
            });
            if let Some(a) = next {
                apply(app, &ids, a);
            }
            area.right()
        }
        "text_align" => {
            let area = Rect::from_min_size(at, vec2(150.0, height));
            let mut next = None;
            ui.scope_builder(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
                ui.add_enabled_ui(live, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.horizontal(|ui| {
                        ui.set_height(row_h);
                        let lr = ui.allocate_exact_size(vec2(16.0, row_h), Sense::hover()).0;
                        label(ui, lr, "X");
                        let mut i = [AlignX::Left, AlignX::Middle, AlignX::Right].iter().position(|a| *a == t.align_x).unwrap_or(0);
                        if ui_kit::dropdown(ui, "text_align_x", &mut i, &["Left", "Middle", "Right"]).changed() {
                            let v = ["left", "middle", "right"][i];
                            next = Some(json!({ "align_x": v }));
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.set_height(row_h);
                        let lr = ui.allocate_exact_size(vec2(16.0, row_h), Sense::hover()).0;
                        label(ui, lr, "Y");
                        let mut i = [AlignY::Baseline, AlignY::Top, AlignY::Middle, AlignY::Bottom].iter().position(|a| *a == t.align_y).unwrap_or(0);
                        if ui_kit::dropdown(ui, "text_align_y", &mut i, &["Baseline", "Top", "Middle", "Bottom"]).changed() {
                            let v = ["baseline", "top", "middle", "bottom"][i];
                            next = Some(json!({ "align_y": v }));
                        }
                    });
                });
            });
            if let Some(a) = next {
                apply(app, &ids, a);
            }
            area.right()
        }
        "text_variable" => {
            let area = Rect::from_min_size(at, vec2(190.0, height));
            let mut next = None;
            ui.scope_builder(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
                ui.add_enabled_ui(live, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.horizontal(|ui| {
                        ui.set_height(row_h);
                        let lr = ui.allocate_exact_size(vec2(40.0, row_h), Sense::hover()).0;
                        label(ui, lr, "Type");
                        let mut i = VAR_KINDS.iter().position(|k| *k == t.variable).unwrap_or(0);
                        if ui_kit::dropdown(ui, "text_var_type", &mut i, &VAR_NAMES).changed() {
                            next = Some(json!({ "variable": VAR_IDS[i] }));
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.set_height(row_h);
                        let lr = ui.allocate_exact_size(vec2(40.0, row_h), Sense::hover()).0;
                        label(ui, lr, "Offset");
                        let mut o = t.offset as f64;
                        let ed = ui
                            .push_id("text_var_offset", |ui| numeric::field_sized(ui, &mut o, &Num::new(-100_000.0..=100_000.0, 0, ""), Some(90.0)))
                            .inner;
                        crate::a11y::number(ui, &ed.response, "Index offset");
                        if ed.changed {
                            next = Some(json!({ "offset": numeric::whole(o) }));
                        }
                    });
                });
            });
            if let Some(a) = next {
                apply(app, &ids, a);
            }
            area.right()
        }
        _ => at.x,
    }
}

const VAR_KINDS: [VariableText; 5] =
    [VariableText::Normal, VariableText::DateTime, VariableText::Merge, VariableText::Serial, VariableText::CutSetting];
const VAR_NAMES: [&str; 5] = ["Normal", "Date and time", "Merge", "Serial number", "Cut setting"];
const VAR_IDS: [&str; 5] = ["normal", "date_time", "merge", "serial", "cut_setting"];

// ================================================================ Shape Properties additions

/// Extra rows of the text section of Shape Properties: the font picker, distort, variable text type.
/// Returns nothing; changes go through `text.set`.
pub fn properties_extra(ui: &mut Ui, app: &mut DarkBurn) {
    let (ids, first) = selected_texts(app);
    let Some(t) = first else { return };
    let mut next = None;
    ui_kit::row(ui, "Font", |ui| {
        let shx = app.prefs.shx_fonts.clone();
        let mut prefs = app.prefs.fonts.clone();
        let w = ui.available_width();
        let f = font_picker(ui, "properties", &t.font, w, &mut prefs, &shx, &t.text);
        app.prefs.fonts = prefs;
        if let Some(f) = f {
            next = Some(json!({ "font": f }));
        }
    });
    let mut d = t.distort;
    if ui_kit::checkbox(ui, "Distort", &mut d).changed() {
        next = Some(json!({ "distort": d }));
    }
    ui_kit::row(ui, "Variable text", |ui| {
        let mut i = VAR_KINDS.iter().position(|k| *k == t.variable).unwrap_or(0);
        if ui_kit::dropdown(ui, "sp_var_type", &mut i, &VAR_NAMES).changed() {
            next = Some(json!({ "variable": VAR_IDS[i] }));
        }
    });
    ui_kit::row(ui, "Index offset", |ui| {
        let mut o = t.offset as f64;
        let ed = ui.push_id("sp_var_offset", |ui| numeric::field(ui, &mut o, &Num::new(-100_000.0..=100_000.0, 0, ""))).inner;
        if ed.changed {
            next = Some(json!({ "offset": numeric::whole(o) }));
        }
    });
    if ui_kit::button(ui, "Edit text shapes...", ui_kit::Button::Small).clicked() {
        app.text.open_edit_dialog(&app.editor);
    }
    if t.on_path.is_some() && ui_kit::button(ui, "Take off the path", ui_kit::Button::Small).clicked() {
        let _ = crate::api::execute_headless(app, "text.detach_path", &json!({ "ids": ids.iter().map(|i| i.0).collect::<Vec<_>>() }));
    }
    if let Some(a) = next {
        apply(app, &ids, a);
    }
}

/// The QR code / barcode section of Shape Properties.
pub fn code_properties(ui: &mut Ui, app: &mut DarkBurn) {
    let ids: Vec<ShapeId> =
        app.editor.selected().into_iter().filter(|i| app.editor.doc.get(*i).is_some_and(|s| matches!(s.kind, ShapeKind::Code(_)))).collect();
    let Some(id) = ids.first().copied() else { return };
    let Some(s) = app.editor.doc.get(id) else { return };
    let ShapeKind::Code(c) = &s.kind else { return };
    ui_kit::row(ui, "Size", |ui| ui.label(format!("{:.2} x {:.2} mm", s.bbox().width(), s.bbox().height())));
    if let Some(e) = &c.error {
        ui_kit::status_line(ui, e, ui_kit::StatusTone::Dim);
    }
    if ui_kit::button(ui, "Edit code...", ui_kit::Button::Small).clicked() {
        open_code_dialog(app, id);
    }
}

// ================================================================ dialogs

/// The windows of this area, drawn every frame.
pub fn show(ctx: &Context, app: &mut DarkBurn) {
    let time = ctx.input(|i| i.time);
    let _ = time;
    edit_dialog(ctx, app);
    qr_dialog(ctx, app);
}

fn drafts_to_commands(app: &mut DarkBurn, drafts: &[(ShapeId, TextShape)]) {
    text_cmds::apply_drafts(app, drafts);
}

fn edit_dialog(ctx: &Context, app: &mut DarkBurn) {
    if !app.text.edit.open {
        return;
    }
    let mut d = std::mem::take(&mut app.text.edit);
    let tk = Tokens::get();
    let (closed, action) = ui_kit::Window::new(Id::unique("db_text_edit"), "text", "Edit Text Shapes")
        .cards(3.2)
        .actions("OK", "Cancel", Some("Apply"))
        .show(ctx, true, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(150.0);
                    ui_kit::section_heading(ui, "Text shapes");
                    egui::ScrollArea::vertical().max_height(260.0).id_salt("edit_text_list").show(ui, |ui| {
                        for (i, (_, t)) in d.drafts.iter().enumerate() {
                            let name: String = t.text.lines().next().unwrap_or("").chars().take(22).collect();
                            if ui.selectable_label(i == d.current, if name.is_empty() { "(empty)".to_string() } else { name }).clicked() {
                                d.current = i;
                            }
                        }
                    });
                });
                ui.vertical(|ui| {
                    let cur = d.current.min(d.drafts.len() - 1);
                    let shx = app.prefs.shx_fonts.clone();
                    let mut fonts = app.prefs.fonts.clone();
                    let t = &mut d.drafts[cur].1;
                    ui_kit::text_area(ui, "Text", Id::unique("edit_text_area"), &mut t.text, 5, 100_000);
                    ui_kit::row(ui, "Font", |ui| {
                        let w = ui.available_width();
                        if let Some(f) = font_picker(ui, "edit_dialog", &t.font, w, &mut fonts, &shx, &t.text) {
                            t.font = f;
                        }
                    });
                    ui_kit::row(ui, "Height", |ui| {
                        ui_kit::number_field(ui, &mut t.height, 0.1..=10_000.0, 0.1, " mm");
                    });
                    ui.horizontal(|ui| {
                        ui_kit::checkbox(ui, "Bold", &mut t.bold);
                        ui_kit::checkbox(ui, "Italic", &mut t.italic);
                        ui_kit::checkbox(ui, "Upper case", &mut t.upper_case);
                        ui_kit::checkbox(ui, "Welded", &mut t.welded);
                    });
                    ui_kit::row(ui, "Letter spacing", |ui| {
                        ui_kit::number_field(ui, &mut t.letter_spacing, -100.0..=1000.0, 0.05, " mm");
                    });
                    ui_kit::row(ui, "Line spacing", |ui| {
                        ui_kit::number_field(ui, &mut t.line_spacing, -100.0..=1000.0, 0.05, " mm");
                    });
                    ui_kit::row(ui, "Align X", |ui| {
                        let mut i = [AlignX::Left, AlignX::Middle, AlignX::Right].iter().position(|a| *a == t.align_x).unwrap_or(0);
                        if ui_kit::dropdown(ui, "edit_ax", &mut i, &["Left", "Middle", "Right"]).changed() {
                            t.align_x = [AlignX::Left, AlignX::Middle, AlignX::Right][i];
                        }
                    });
                    ui_kit::row(ui, "Align Y", |ui| {
                        let mut i = [AlignY::Baseline, AlignY::Top, AlignY::Middle, AlignY::Bottom].iter().position(|a| *a == t.align_y).unwrap_or(0);
                        if ui_kit::dropdown(ui, "edit_ay", &mut i, &["Baseline", "Top", "Middle", "Bottom"]).changed() {
                            t.align_y = [AlignY::Baseline, AlignY::Top, AlignY::Middle, AlignY::Bottom][i];
                        }
                    });
                    ui_kit::row(ui, "Variable text", |ui| {
                        let mut i = VAR_KINDS.iter().position(|k| *k == t.variable).unwrap_or(0);
                        if ui_kit::dropdown(ui, "edit_var", &mut i, &VAR_NAMES).changed() {
                            t.variable = VAR_KINDS[i];
                        }
                    });
                    app.prefs.fonts = fonts;
                });
            });
            ui_kit::section_heading(ui, "Preview");
            let cur = d.current.min(d.drafts.len() - 1);
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 200.0), Sense::click_and_drag());
            ui.painter().rect_filled(rect, tk.radius_sm, tk.control_fill);
            if resp.dragged() {
                d.pan += resp.drag_delta();
            }
            if resp.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    d.zoom = (d.zoom * (1.0 + scroll * 0.002)).clamp(0.1, 50.0);
                }
            }
            if resp.double_clicked() {
                d.zoom = 1.0;
                d.pan = egui::Vec2::ZERO;
            }
            let paths = preview_paths(&app.editor.doc, &d.drafts[cur].1);
            let all: BezPath = paths.iter().fold(BezPath::new(), |mut a, p| {
                a.extend(p.elements().iter().copied());
                a
            });
            if !all.elements().is_empty() {
                use darkburn_core::kurbo::Shape as _;
                let bb = all.bounding_box();
                let fit = (rect.width() as f64 * 0.9 / bb.width().max(1e-6)).min(rect.height() as f64 * 0.8 / bb.height().max(1e-6)) * d.zoom as f64;
                let centre = bb.center();
                let mid = rect.center() + d.pan;
                let painter = ui.painter().with_clip_rect(rect);
                for (pts, closed) in geomutil::flatten_polylines(&all, 0.05 / fit.max(1e-6)) {
                    let mut v: Vec<Pos2> =
                        pts.iter().map(|q| pos2(mid.x + ((q.x - centre.x) * fit) as f32, mid.y - ((q.y - centre.y) * fit) as f32)).collect();
                    if closed && let Some(f) = v.first().copied() {
                        v.push(f);
                    }
                    painter.add(egui::Shape::line(v, Stroke::new(1.2, tk.text)));
                }
            }
            ui_kit::caption(ui, "Preview", "Scroll to zoom, drag to pan, double-click to fit.");
        });
    match action {
        ui_kit::Action::Ok => {
            drafts_to_commands(app, &d.drafts);
            d.open = false;
        }
        ui_kit::Action::Apply => drafts_to_commands(app, &d.drafts),
        ui_kit::Action::Cancel => d.open = false,
        ui_kit::Action::None => {
            if closed {
                d.open = false;
            }
        }
    }
    app.text.edit = d;
}

/// The outlines a draft text would have, for the dialog's preview.
pub fn preview_paths(doc: &darkburn_core::Document, t: &TextShape) -> Vec<BezPath> {
    let mut t = t.clone();
    t.render_key = 0;
    t.backup = None;
    let ctx = darkburn_text::EvalContext::local_now();
    let text = render::evaluated_text(doc, 0, &t, &ctx);
    let spec = render::spec_of(&t, text);
    match render::layout_of(&t, &spec) {
        Some((layout, _)) => render::outlines_of(&t, &layout),
        None => Vec::new(),
    }
}

fn qr_dialog(ctx: &Context, app: &mut DarkBurn) {
    if !app.text.qr.open {
        return;
    }
    let mut q = std::mem::take(&mut app.text.qr);
    let tk = Tokens::get();
    let title = if q.editing.is_some() { "Edit Code" } else { "Create QR Code" };
    let args = q.args();
    let (payload_len, error, matrix) = qr_preview(&q);
    let (closed, action) =
        ui_kit::Window::new(Id::unique("db_qr_dialog"), "db_qr", title).cards(2.4).actions("OK", "Cancel", None).show(ctx, error.is_none(), |ui| {
            if q.editing.is_none() {
                let mut i = q.barcode as usize;
                if ui_kit::segmented(ui, &mut i, &["QR code", "Barcode"], ui_kit::Segments::default()) {
                    q.barcode = i == 1;
                }
            }
            if q.barcode {
                ui_kit::row(ui, "Type", |ui| {
                    ui_kit::dropdown(ui, "barcode_kind", &mut q.code, &BARCODE_NAMES);
                });
                ui_kit::row(ui, "Data", |ui| {
                    ui_kit::field(ui, &mut q.data);
                });
                ui_kit::row(ui, "Bar height", |ui| {
                    ui_kit::number_field(ui, &mut q.height, 1.0..=1000.0, 0.5, " mm");
                });
            } else {
                ui_kit::row(ui, "Content", |ui| {
                    ui_kit::dropdown(ui, "qr_kind", &mut q.kind, &["Text", "URL", "Wi-Fi", "Contact"]);
                });
                match q.kind {
                    0 => {
                        ui_kit::text_area(ui, "Text", Id::unique("qr_text"), &mut q.text, 4, 100_000);
                    }
                    1 => ui_kit::row(ui, "Address", |ui| {
                        ui_kit::hint_field(ui, &mut q.url, "example.com");
                    }),
                    2 => {
                        ui_kit::row(ui, "Network", |ui| {
                            ui_kit::field(ui, &mut q.ssid);
                        });
                        ui_kit::row(ui, "Password", |ui| {
                            ui_kit::field(ui, &mut q.password);
                        });
                        ui_kit::row(ui, "Security", |ui| {
                            ui_kit::dropdown(ui, "qr_sec", &mut q.security, &["WPA", "WEP", "None"]);
                        });
                        ui_kit::checkbox(ui, "Hidden network", &mut q.hidden);
                    }
                    _ => {
                        for (label, field) in [
                            ("Name", &mut q.name),
                            ("Phone", &mut q.phone),
                            ("Email", &mut q.email),
                            ("Website", &mut q.website),
                            ("Address", &mut q.address),
                            ("Note", &mut q.note),
                        ] {
                            ui_kit::row(ui, label, |ui| {
                                ui_kit::field(ui, field);
                            });
                        }
                    }
                }
                ui_kit::row(ui, "Error correction", |ui| {
                    ui_kit::dropdown(ui, "qr_ecc", &mut q.ecc, &["Low (7 %)", "Medium (15 %)", "Quartile (25 %)", "High (30 %)"]);
                });
                ui_kit::row(ui, "Size", |ui| {
                    ui_kit::number_field(ui, &mut q.size, 1.0..=2000.0, 0.5, " mm");
                });
                ui_kit::caption(ui, "Size", &format!("{payload_len} of {MAX_QR_BYTES} bytes used."));
            }
            ui_kit::checkbox(ui, "Variable text ({serial}, {csv:1}, {date} ...)", &mut q.variable);
            if let Some(e) = &error {
                ui_kit::status_line(ui, e, ui_kit::StatusTone::Dim);
            }
            let (rect, _) = ui.allocate_exact_size(vec2(160.0, 160.0), Sense::hover());
            ui.painter().rect_filled(rect, tk.radius_sm, Color32::WHITE);
            paint_matrix(ui.painter(), rect.shrink(6.0), &matrix);
        });
    match action {
        ui_kit::Action::Ok => {
            let cmd = match (q.barcode, q.editing) {
                (false, None) => "qr.add",
                (false, Some(_)) => "qr.set",
                (true, None) => "barcode.add",
                (true, Some(_)) => "barcode.set",
            };
            let mut a = args;
            if let Some(id) = q.editing {
                a["ids"] = json!([id.0]);
            } else {
                // A new code goes in the middle of the visible work area.
                let page = app.prefs.page();
                a["x"] = json!(f64::from(page.x) / 2.0);
                a["y"] = json!(f64::from(page.y) / 2.0);
            }
            if let Err(e) = crate::api::execute_headless(app, cmd, &a) {
                app.note = Some(e);
            }
            q.open = false;
        }
        ui_kit::Action::Cancel => q.open = false,
        _ => {
            if closed {
                q.open = false;
            }
        }
    }
    app.text.qr = q;
}

type Matrix = Vec<Vec<bool>>;

/// Payload size, an error text if the code cannot be made, and the dark modules for the preview.
fn qr_preview(q: &QrDialog) -> (usize, Option<String>, Matrix) {
    if q.barcode {
        let kind = BARCODES[q.code.min(BARCODES.len() - 1)].1;
        if q.variable {
            // Tokens turn into something else when the code is made: do not judge the data here.
            return (q.data.len(), None, Vec::new());
        }
        let spec = darkburn_text::BarcodeSpec { kind, data: q.data.clone(), ..Default::default() };
        return match spec.generate() {
            Ok(r) => {
                let mut row = Vec::new();
                for (dark, w) in &r.runs {
                    let n = (w / spec.module_mm).round().max(1.0) as usize;
                    row.extend(std::iter::repeat_n(*dark, n));
                }
                let m = if row.is_empty() { Vec::new() } else { vec![row; 1] };
                (q.data.len(), None, m)
            }
            Err(e) => (q.data.len(), Some(e.to_string()), Vec::new()),
        };
    }
    let a = q.args();
    let content = match q.kind {
        0 => QrContent::Text(q.text.clone()),
        1 => QrContent::Url(q.url.clone()),
        2 => QrContent::Wifi {
            ssid: q.ssid.clone(),
            password: q.password.clone(),
            security: [WifiSecurity::Wpa, WifiSecurity::Wep, WifiSecurity::Open][q.security.min(2)],
            hidden: q.hidden,
        },
        _ => QrContent::Contact(darkburn_text::ContactCard {
            name: q.name.clone(),
            phone: q.phone.clone(),
            email: q.email.clone(),
            url: q.website.clone(),
            address: q.address.clone(),
            note: q.note.clone(),
            ..Default::default()
        }),
    };
    let _ = a;
    let ecc = [darkburn_text::EccLevel::L, darkburn_text::EccLevel::M, darkburn_text::EccLevel::Q, darkburn_text::EccLevel::H][q.ecc.min(3)];
    let spec = darkburn_text::QrSpec { content, ecc, ..Default::default() };
    let len = spec.content.payload().len();
    if q.variable {
        // Tokens turn into something else when the code is made: do not judge the size or the text here.
        return (len, None, Vec::new());
    }
    match spec.generate() {
        Ok(r) => (len, None, r.matrix),
        Err(e) => (len, Some(e.to_string()), Vec::new()),
    }
}

fn paint_matrix(painter: &egui::Painter, rect: Rect, m: &Matrix) {
    let rows = m.len();
    let cols = m.iter().map(Vec::len).max().unwrap_or(0);
    if rows == 0 || cols == 0 {
        return;
    }
    let cell = (rect.width() / cols as f32).min(rect.height() / rows.max(1) as f32 * if rows == 1 { 4.0 } else { 1.0 });
    let h = if rows == 1 { rect.height() } else { cell };
    let origin = rect.min;
    for (y, row) in m.iter().enumerate() {
        for (x, dark) in row.iter().enumerate() {
            if *dark {
                let r = Rect::from_min_size(pos2(origin.x + x as f32 * cell, origin.y + y as f32 * h), vec2(cell, h));
                painter.rect_filled(r, 0.0, Color32::BLACK);
            }
        }
    }
}

// ================================================================ the Variable Text panel

/// The Variable Text panel.
pub fn variable_panel(ui: &mut Ui, app: &mut DarkBurn) {
    let t = Tokens::get();
    let v = app.editor.doc.variables.clone();
    ui.label(RichText::new("Serial numbers and CSV rows used by variable text, QR codes and barcodes.").size(t.font_small).color(t.text_dim));
    ui.add_space(t.space_6);
    let mut set: Option<Value> = None;
    let mut int_row = |ui: &mut Ui, label: &str, key: &str, value: i64, range: std::ops::RangeInclusive<f64>| {
        let mut x = value as f64;
        let ed = ui.push_id(("vt", key), |ui| numeric::row(ui, label, &mut x, &Num::new(range, 0, ""))).inner;
        if ed.changed {
            set = Some(json!({ key: numeric::whole(x) }));
        }
    };
    int_row(ui, "Start", "start", v.serial.start, -1e12..=1e12);
    int_row(ui, "End", "end", v.serial.end, -1e12..=1e12);
    int_row(ui, "Current", "current", v.serial.current, -1e12..=1e12);
    int_row(ui, "Advance by", "advance_by", v.serial.advance_by, -1e9..=1e9);
    int_row(ui, "Zero pad to", "padding", v.serial.padding as i64, 0.0..=64.0);
    let mut auto = v.auto_advance;
    if ui_kit::checkbox(ui, "Auto-advance after a job", &mut auto).changed() {
        set = Some(json!({ "auto_advance": auto }));
    }
    if let Some(a) = set {
        run_cmd(app, "variable.set", &a);
    }
    ui.horizontal_wrapped(|ui| {
        for (label, cmd) in [("Previous", "variable.previous"), ("Next", "variable.next"), ("Reset", "variable.reset"), ("Bake", "variable.bake")] {
            if ui_kit::button(ui, label, ui_kit::Button::Small).clicked() {
                run_cmd(app, cmd, &Value::Null);
            }
        }
        if ui_kit::button(ui, "Test", ui_kit::Button::Small).clicked() {
            match crate::api::execute_headless(app, "variable.test", &json!({ "count": 6 })) {
                Ok(r) => {
                    app.text.test_values =
                        r["values"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
                }
                Err(e) => {
                    app.text.test_values = vec![e];
                }
            }
        }
    });
    if !app.text.test_values.is_empty() {
        ui_kit::section_heading(ui, "Next values");
        for s in app.text.test_values.clone() {
            ui.label(RichText::new(s.replace('\n', " / ")).size(t.font_small));
        }
    }
    ui_kit::section_heading(ui, "CSV file");
    let path = v.merge.path.clone().unwrap_or_default();
    ui_kit::row(ui, "File", |ui| {
        let mut p = if path.is_empty() { String::new() } else { path.clone() };
        ui_kit::hint_field(ui, &mut p, "No file");
    });
    let mut header = v.merge.header;
    if ui_kit::checkbox(ui, "First row holds column names", &mut header).changed() && !path.is_empty() {
        run_cmd(app, "variable.csv_load", &json!({ "path": path, "header": header }));
    } else if header != v.merge.header {
        app.editor.edit("CSV header", |d, _| {
            d.variables.merge.header = header;
            true
        });
    }
    ui.horizontal(|ui| {
        if ui_kit::button(ui, "Browse", ui_kit::Button::Small).clicked()
            && let Some(f) = rfd::FileDialog::new().add_filter("CSV and text files", &["csv", "tsv", "txt"]).pick_file()
        {
            run_cmd(app, "variable.csv_load", &json!({ "path": f.to_string_lossy(), "header": header }));
        }
        if ui_kit::button(ui, "Clear", ui_kit::Button::Small).clicked() {
            run_cmd(app, "variable.csv_clear", &Value::Null);
        }
    });
    if v.merge.row_count() > 0 {
        ui.label(
            RichText::new(format!("Row {} of {}. Columns: {}", v.merge.current_row + 1, v.merge.row_count(), v.merge.column_names().join(", ")))
                .size(t.font_small)
                .color(t.text_dim),
        );
    }
}

fn run_cmd(app: &mut DarkBurn, id: &str, args: &Value) {
    if let Err(e) = crate::api::execute_headless(app, id, args) {
        app.note = Some(e);
    }
}

// ================================================================ settings

/// Default font and size for new text, SHX fonts, in the Settings window.
pub fn settings_section(ui: &mut Ui, prefs: &mut crate::prefs::Prefs) {
    ui_kit::section_heading(ui, "Text");
    ui_kit::row(ui, "Default font", |ui| {
        let shx = prefs.shx_fonts.clone();
        let w = ui.available_width();
        let current =
            if prefs.fonts.default_family.is_empty() { render::lib().default_family().to_string() } else { prefs.fonts.default_family.clone() };
        let mut fp = prefs.fonts.clone();
        if let Some(f) = font_picker(ui, "settings", &current, w, &mut fp, &shx, "Sample text") {
            fp.default_family = f;
        }
        prefs.fonts = fp;
    });
    ui_kit::row(ui, "Default size", |ui| {
        ui_kit::number_field(ui, &mut prefs.fonts.default_size, 0.1..=10_000.0, 0.1, " mm");
    });
    ui_kit::row(ui, "Single-line fonts", |ui| {
        if ui_kit::button(ui, "Add SHX font", ui_kit::Button::Small).clicked()
            && let Some(f) = rfd::FileDialog::new().add_filter("SHX fonts", &["shx"]).pick_file()
        {
            let name = format!("{}{}", render::SHX_PREFIX, f.to_string_lossy());
            render::forget_shx(&f.to_string_lossy());
            if render::shx_font(&f.to_string_lossy()).is_some() && !prefs.shx_fonts.contains(&name) {
                prefs.shx_fonts.push(name);
            }
        }
    });
    let mut remove = None;
    for (i, f) in prefs.shx_fonts.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(font_label(f));
            if ui_kit::button(ui, "Remove", ui_kit::Button::Small).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        prefs.shx_fonts.remove(i);
    }
}

// ================================================================ tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Typing a text away to nothing takes the text out; it does not stay as a shape nobody can see.
    #[test]
    fn a_text_emptied_while_typing_is_taken_out() {
        let mut app = DarkBurn::new(crate::prefs::Prefs::default());
        let out =
            crate::api::execute_headless(&mut app, "text.add", &json!({ "text": "Ab", "x": 10, "y": 10, "height": 10, "font": "IBM Plex Sans" }))
                .unwrap();
        let id = ShapeId(out["id"].as_u64().unwrap());
        // the text tool clicked on it
        app.editor.checkpoint("Edit text");
        app.editor.touch();
        begin_typing(&mut app, id, false);
        // Backspace twice
        let mut ty = app.text.typing.take().unwrap();
        let mut s = text_mut(&mut app, id).unwrap().text.clone();
        ty.caret.backspace(&mut s);
        ty.caret.backspace(&mut s);
        assert!(s.is_empty());
        text_mut(&mut app, id).unwrap().text = s;
        render::refresh(&mut app.editor.doc);
        app.editor.touch();
        ty.rev = app.editor.revision();
        app.text.typing = Some(ty);
        finish_typing(&mut app);
        assert!(app.editor.doc.get(id).is_none(), "the emptied text is gone");
        assert!(app.editor.selected().is_empty());
        assert!(app.editor.undo(), "one undo step");
        assert_eq!(text_cmds::find_text(&app.editor.doc, id).map(|t| t.text.as_str()), Some("Ab"), "undo brings the text back");
    }

    /// "Weld text automatically" (Settings, Text) also holds for text made with the text tool.
    #[test]
    fn the_text_tool_follows_weld_text_automatically() {
        for auto in [false, true] {
            let mut app = DarkBurn::new(crate::prefs::Prefs::default());
            app.prefs.opts.auto_weld = auto;
            place_text(&mut app, Point::new(20.0, 20.0));
            let id = app.text.typing.as_ref().unwrap().id;
            assert_eq!(text_cmds::find_text(&app.editor.doc, id).unwrap().welded, auto);
        }
    }

    /// The Text Options toolbar changes text through `apply` (-> `text.set`); every control's
    /// arguments are exercised on a selected text shape, then the ribbon is drawn with them.
    #[test]
    fn text_options_controls_change_the_selected_text() {
        let mut app = DarkBurn::new(crate::prefs::Prefs::default());
        let out = crate::api::execute_headless(
            &mut app,
            "text.add",
            &json!({ "text": "Ab12", "x": 10, "y": 10, "height": 10, "font": "IBM Plex Sans", "layer": 0 }),
        )
        .unwrap();
        let id = ShapeId(out["id"].as_u64().unwrap());
        app.editor.selection.set([id]);
        let (ids, first) = selected_texts(&app);
        assert_eq!(ids, vec![id]);
        assert!(first.is_some(), "the toolbar sees a live text");
        let get = |app: &DarkBurn| text_cmds::find_text(&app.editor.doc, id).cloned().unwrap();
        for args in [
            json!({ "height": 14.0 }),
            json!({ "bold": true }),
            json!({ "italic": true }),
            json!({ "upper_case": true }),
            json!({ "welded": true }),
            json!({ "distort": true }),
            json!({ "letter_spacing": 2.0 }),
            json!({ "line_spacing": 3.0 }),
            json!({ "align_x": "right" }),
            json!({ "align_y": "middle" }),
            json!({ "variable": "serial" }),
            json!({ "offset": 5 }),
        ] {
            apply(&mut app, &ids, args);
            assert!(app.note.is_none(), "{:?}", app.note);
        }
        let t = get(&app);
        assert_eq!((t.height, t.bold, t.italic, t.upper_case, t.welded, t.distort), (14.0, true, true, true, true, true));
        assert_eq!((t.letter_spacing, t.line_spacing, t.align_x, t.align_y), (2.0, 3.0, AlignX::Right, AlignY::Middle));
        assert_eq!((t.variable, t.offset), (VariableText::Serial, 5));
        // The font picker remembers recent and favourite fonts.
        app.prefs.fonts.toggle_favourite("IBM Plex Sans");
        assert!(app.prefs.fonts.favourites.iter().any(|f| f == "IBM Plex Sans"));
        // The Text tab draws with the text selected.
        app.prefs.ribbon_tab = "text".into();
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1500.0, 900.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| app.ui(ui));
            out.textures_delta.clear();
        }
    }

    fn key(k: Key, shift: bool) -> egui::Event {
        egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers { shift, ..Default::default() } }
    }

    fn type_all(c: &mut Caret, s: &mut String, evs: &[egui::Event]) {
        let mut clip = None;
        for e in evs {
            apply_event(c, s, e, &mut clip);
        }
    }

    #[test]
    fn typing_multi_line_and_navigation() {
        let mut s = String::new();
        let mut c = Caret::default();
        type_all(&mut c, &mut s, &[egui::Event::Text("ab".into()), key(Key::Enter, false), egui::Event::Text("cde".into())]);
        assert_eq!(s, "ab\ncde");
        type_all(&mut c, &mut s, &[key(Key::ArrowUp, false)]);
        assert_eq!(c.at, 2, "up keeps the column, clipped to the shorter line");
        type_all(&mut c, &mut s, &[key(Key::Home, false), egui::Event::Text("X".into())]);
        assert_eq!(s, "Xab\ncde");
        type_all(&mut c, &mut s, &[key(Key::Delete, false), key(Key::End, false), key(Key::Backspace, false)]);
        assert_eq!(s, "X\ncde");
    }

    #[test]
    fn non_latin_text_and_selection() {
        let mut s = String::new();
        let mut c = Caret::default();
        type_all(
            &mut c,
            &mut s,
            &[egui::Event::Text("\u{5e9}\u{5dc}\u{5d5}\u{5dd}".into()), egui::Event::Ime(egui::ImeEvent::Commit("\u{4f60}\u{597d}".into()))],
        );
        assert_eq!(s.chars().count(), 6);
        type_all(&mut c, &mut s, &[key(Key::Backspace, false)]);
        assert_eq!(s.chars().count(), 5);
        c.select_all(&s);
        type_all(&mut c, &mut s, &[egui::Event::Text("x".into())]);
        assert_eq!(s, "x");
        let mut clip = None;
        c.select_all(&s);
        apply_event(&mut c, &mut s, &egui::Event::Cut, &mut clip);
        assert_eq!((s.as_str(), clip.as_deref()), ("", Some("x")));
    }

    #[test]
    fn escape_finishes() {
        let mut s = "a".to_string();
        let mut c = Caret::end_of(&s);
        let mut clip = None;
        assert_eq!(apply_event(&mut c, &mut s, &key(Key::Escape, false), &mut clip), Done::Finish);
    }

    #[test]
    fn typing_in_place_makes_one_undo_step_and_an_empty_text_vanishes() {
        let mut app = crate::shape_ops::tests::app();
        let before = app.editor.history.len();
        place_text(&mut app, Point::new(20.0, 20.0));
        assert!(app.text.typing.is_some());
        // Type through the same path the canvas uses.
        let id = app.text.typing.as_ref().unwrap().id;
        for ch in ["H", "i"] {
            let ty = app.text.typing.as_mut().unwrap();
            let t = match &mut app.editor.doc.get_mut(id).unwrap().kind {
                ShapeKind::Text(t) => t,
                _ => panic!(),
            };
            let mut s = t.text.clone();
            let mut clip = None;
            apply_event(&mut ty.caret, &mut s, &egui::Event::Text(ch.into()), &mut clip);
            t.text = s;
            render::refresh(&mut app.editor.doc);
            app.editor.touch();
            app.text.typing.as_mut().unwrap().rev = app.editor.revision();
        }
        finish_typing(&mut app);
        assert_eq!(app.editor.history.len(), before + 1, "one undo step for the whole text");
        assert!(app.editor.doc.get(id).unwrap().bbox().width() > 3.0);
        // An empty new text leaves nothing behind.
        let n = app.editor.doc.len();
        place_text(&mut app, Point::new(50.0, 50.0));
        finish_typing(&mut app);
        assert_eq!(app.editor.doc.len(), n);
        assert_eq!(app.editor.history.len(), before + 1);
    }

    #[test]
    fn caret_sits_after_the_typed_text() {
        let t = TextShape { text: "AB\nC".into(), font: "IBM Plex Sans".into(), ..Default::default() };
        let (x0, ..) = caret_geometry(&t, 0).unwrap();
        let (x2, ..) = caret_geometry(&t, 2).unwrap();
        assert!(x2 > x0 + 5.0);
        let (x3, y0, _) = caret_geometry(&t, 3).unwrap();
        assert!((x3 - x0).abs() < 1e-6, "start of the second line");
        let (_, y_first, _) = caret_geometry(&t, 0).unwrap();
        assert!(y0 < y_first - 5.0, "second line is lower");
    }

    #[test]
    fn previews_are_cached_and_scaled_to_a_box() {
        let p = preview_lines("IBM Plex Sans", "Abc");
        assert!(!p.is_empty());
        assert!(p.iter().flatten().all(|q| q[0] >= -0.01 && q[0] <= 1.01 && q[1] >= -0.5 && q[1] <= 1.01));
        assert!(Arc::ptr_eq(&p, &preview_lines("IBM Plex Sans", "Abc")));
    }

    #[test]
    fn the_edit_dialog_opens_for_selected_text_and_applies_in_one_step() {
        use crate::shape_ops::tests::{app, run};
        let mut a = app();
        let r = run(&mut a, "text.add", json!({ "text": "One", "font": "IBM Plex Sans" })).unwrap();
        let id = ShapeId(r["id"].as_u64().unwrap());
        a.text.open_edit_dialog(&a.editor);
        assert!(a.text.edit.open);
        a.text.edit.drafts[0].1.text = "Changed".into();
        a.text.edit.drafts[0].1.height = 4.0;
        let before = a.editor.history.len();
        let drafts = a.text.edit.drafts.clone();
        text_cmds::apply_drafts(&mut a, &drafts);
        assert_eq!(a.editor.history.len(), before + 1);
        match &a.editor.doc.get(id).unwrap().kind {
            ShapeKind::Text(t) => assert_eq!((t.text.as_str(), t.height), ("Changed", 4.0)),
            _ => panic!(),
        }
        assert!(a.editor.doc.get(id).unwrap().bbox().height() < 7.0);
    }

    #[test]
    fn the_dialog_describes_a_code_back_into_its_fields() {
        use crate::shape_ops::tests::{app, run};
        let mut a = app();
        let r = run(&mut a, "qr.add", json!({ "kind": "wifi", "ssid": "Home", "password": "pw", "ecc": "Q", "size": 25 })).unwrap();
        let id = ShapeId(r["id"].as_u64().unwrap());
        open_code_dialog(&mut a, id);
        let q = &a.text.qr;
        assert!(q.open && q.editing == Some(id));
        assert_eq!((q.kind, q.ssid.as_str(), q.ecc), (2, "Home", 2));
        assert!((q.size - 25.0).abs() < 0.1);
        let args = q.args();
        assert_eq!(args["ssid"], "Home");
        let (len, err, matrix) = qr_preview(q);
        assert!(len > 10 && err.is_none() && !matrix.is_empty());
    }

    fn frame(ctx: &Context, app: &mut DarkBurn, events: Vec<egui::Event>) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| app.ui(ui));
        out.textures_delta.clear();
    }

    #[test]
    fn typing_on_the_canvas_across_frames_keeps_the_focus() {
        let ctx = Context::default();
        let mut app = DarkBurn::new(crate::prefs::Prefs::default());
        app.prefs.fonts.default_family = "IBM Plex Sans".into();
        app.set_tool(Tool::Text);
        frame(&ctx, &mut app, vec![]);
        place_text(&mut app, Point::new(50.0, 50.0));
        for ch in ["H", "e", "l", "l", "o"] {
            frame(&ctx, &mut app, vec![egui::Event::Text(ch.into())]);
        }
        let id = app.text.typing.as_ref().expect("still typing after several frames").id;
        assert_eq!(text_cmds::find_text(&app.editor.doc, id).unwrap().text, "Hello");
        frame(&ctx, &mut app, vec![key(Key::Enter, false), egui::Event::Text("x".into())]);
        assert_eq!(text_cmds::find_text(&app.editor.doc, id).unwrap().text, "Hello\nx");
        assert!(app.editor.doc.get(id).unwrap().bbox().height() > 10.0);
        frame(&ctx, &mut app, vec![key(Key::Escape, false)]);
        assert!(app.text.typing.is_none());
        assert_eq!(app.tool, Tool::Text, "the text tool stays on");
        assert_eq!(app.editor.history.len(), 1, "typing a text is one undo step");
        // The window code runs with text selected: toolbar, properties, dialogs, the panel.
        app.editor.selection.set([id]);
        app.prefs.ribbon_tab = "text".into();
        app.text.open_edit_dialog(&app.editor);
        app.text.qr.open_new();
        for _ in 0..3 {
            frame(&ctx, &mut app, vec![]);
        }
        assert!(app.text.edit.open && app.text.qr.open);
    }

    #[test]
    fn double_click_with_the_select_tool_starts_editing() {
        let ctx = Context::default();
        let mut app = DarkBurn::new(crate::prefs::Prefs::default());
        let r = crate::shape_ops::tests::run(&mut app, "text.add", json!({ "text": "Edit me", "x": 30, "y": 30, "font": "IBM Plex Sans" })).unwrap();
        let id = ShapeId(r["id"].as_u64().unwrap());
        frame(&ctx, &mut app, vec![]);
        assert!(on_double_click(&mut app, Some(Point::new(35.0, 33.0)), 1.0));
        assert_eq!(app.tool, Tool::Text);
        assert_eq!(app.text.typing.as_ref().map(|t| t.id), Some(id));
        finish_typing(&mut app);
        assert!(app.editor.doc.get(id).is_some(), "an unchanged text stays and leaves no undo step");
    }
}
