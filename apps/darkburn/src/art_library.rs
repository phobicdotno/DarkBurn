//! The Art Library: collections of artwork saved as `.dbart` files. Several can be open at once,
//! each holds folders of items, each item is shown as a thumbnail with a larger preview on hover.
//! Items are added from the selection or by dragging files in; a click puts the item on the page.
//! The files are plain JSON, so running copies of DarkBurn share a library through the file: an
//! open library that changed on disk (and has no unsaved changes) is reloaded.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use darkburn_core::clipboard::{Clipboard, PasteAt};
use darkburn_core::kurbo::Point;
use darkburn_ui::ui_kit::{self, BackingTooltipExt as _, Menu, MenuItem};
use egui::{Rect, RichText, Sense, Stroke, Ui, vec2};
use serde::{Deserialize, Serialize};
use solidcraft_theme::Tokens;

use crate::i18n::tr;
use crate::shell::DarkBurn;

pub const FILE_FORMAT: &str = "darkburn-art-library";
pub const FILE_EXTENSION: &str = "dbart";
const CELL: f32 = 64.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Item {
    pub name: String,
    /// The artwork as DarkBurn clipboard text (shapes and their images).
    pub payload: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Folder {
    pub name: String,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtLibrary {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub folders: Vec<Folder>,
}

impl Default for ArtLibrary {
    fn default() -> Self {
        ArtLibrary {
            format: FILE_FORMAT.into(),
            version: 1,
            name: "Untitled art".into(),
            folders: vec![Folder { name: "General".into(), items: Vec::new() }],
        }
    }
}

/// What adding an item with an existing name does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnDuplicate {
    Replace,
    KeepBoth,
}

impl ArtLibrary {
    pub fn from_json(text: &str) -> Result<ArtLibrary, String> {
        let bad = |e: String| format!("{}: {e}", tr("art.bad_file", "This is not an art library"));
        let value: serde_json::Value = serde_json::from_str(text).map_err(|e| bad(e.to_string()))?;
        if value.get("format").and_then(|f| f.as_str()) != Some(FILE_FORMAT) {
            return Err(tr("art.bad_file", "This is not an art library"));
        }
        serde_json::from_value(value).map_err(|e| bad(e.to_string()))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, self.to_json()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.to_string()
        })
    }

    /// Add `item` to folder `folder` (created if the index is past the end). An item with the same
    /// name is replaced or kept beside the new one with a number after its name.
    pub fn add(&mut self, folder: usize, mut item: Item, on_duplicate: OnDuplicate) -> usize {
        if self.folders.is_empty() {
            self.folders.push(Folder { name: "General".into(), items: Vec::new() });
        }
        let folder = folder.min(self.folders.len() - 1);
        let items = &mut self.folders[folder].items;
        match (items.iter().position(|i| i.name == item.name), on_duplicate) {
            (Some(at), OnDuplicate::Replace) => {
                items[at] = item;
                at
            }
            (Some(_), OnDuplicate::KeepBoth) => {
                let base = item.name.clone();
                let mut n = 2;
                while items.iter().any(|i| i.name == item.name) {
                    item.name = format!("{base} {n}");
                    n += 1;
                }
                items.push(item);
                items.len() - 1
            }
            (None, _) => {
                items.push(item);
                items.len() - 1
            }
        }
    }

    pub fn has_item(&self, folder: usize, name: &str) -> bool {
        self.folders.get(folder).is_some_and(|f| f.items.iter().any(|i| i.name == name))
    }

    /// Items whose name contains every word of `search` (case-insensitive): (folder, item).
    pub fn search(&self, search: &str) -> Vec<(usize, usize)> {
        let words: Vec<String> = search.split_whitespace().map(str::to_lowercase).collect();
        let mut out = Vec::new();
        for (f, folder) in self.folders.iter().enumerate() {
            for (i, item) in folder.items.iter().enumerate() {
                let hay = format!("{} {}", folder.name, item.name).to_lowercase();
                if words.iter().all(|w| hay.contains(w)) {
                    out.push((f, i));
                }
            }
        }
        out
    }
}

/// An item drawn small: outlines scaled into the unit square (longest side 1).
#[derive(Clone, Debug, Default)]
pub struct Thumb {
    /// Polylines with the layer they came from.
    pub lines: Vec<(usize, Vec<[f32; 2]>)>,
    /// Width over height.
    pub aspect: f32,
}

/// The outlines of an item's artwork, ready to draw at any size.
pub fn thumb_of(payload: &str) -> Thumb {
    let Some(clip) = Clipboard::from_text(payload) else { return Thumb::default() };
    let Some(bb) = clip.shapes.iter().map(|s| s.bbox()).reduce(|a, b| a.union(b)) else { return Thumb::default() };
    let side = bb.width().max(bb.height()).max(1e-6);
    let tol = side / 200.0;
    let mut lines = Vec::new();
    for shape in &clip.shapes {
        for leaf in shape.leaves() {
            for poly in darkburn_geom::path::flatten_to_polylines(&leaf.outline(), tol) {
                let pts: Vec<[f32; 2]> = poly.points.iter().map(|p| [((p.x - bb.x0) / side) as f32, ((bb.y1 - p.y) / side) as f32]).collect();
                let mut pts = pts;
                if poly.closed
                    && let Some(first) = pts.first().copied()
                {
                    pts.push(first);
                }
                lines.push((leaf.layer, pts));
            }
        }
    }
    Thumb { lines, aspect: (bb.width() / bb.height().max(1e-6)) as f32 }
}

fn paint_thumb(painter: &egui::Painter, rect: Rect, thumb: &Thumb, dark: bool) {
    let side = rect.width().min(rect.height());
    let (w, h) = if thumb.aspect >= 1.0 { (side, side / thumb.aspect.max(1e-3)) } else { (side * thumb.aspect, side) };
    let origin = rect.center() - vec2(w, h) / 2.0;
    let scale = w.max(h);
    for (layer, pts) in &thumb.lines {
        let screen: Vec<egui::Pos2> = pts.iter().map(|p| origin + vec2(p[0] * scale, p[1] * scale)).collect();
        painter.add(egui::Shape::line(screen, Stroke::new(1.0, crate::render::ink(*layer, dark))));
    }
}

/// Where an item goes when it is inserted.
pub fn insert_item(app: &mut DarkBurn, library: usize, folder: usize, item: usize) -> Result<usize, String> {
    let payload = app
        .extras
        .art
        .libs
        .get(library)
        .and_then(|l| l.lib.folders.get(folder))
        .and_then(|f| f.items.get(item))
        .map(|i| i.payload.clone())
        .ok_or_else(|| tr("art.no_item", "No such item"))?;
    let mut clip = Clipboard::from_text(&payload).ok_or_else(|| tr("art.damaged", "This item is damaged"))?;
    let centre = Point::new(app.prefs.page_width as f64 / 2.0, app.prefs.page_height as f64 / 2.0);
    let mut new = Vec::new();
    app.editor.edit("Insert art", |d, sel| {
        new = clip.paste(d, PasteAt::Centre(centre));
        sel.set(new.iter().copied());
        !new.is_empty()
    });
    Ok(new.len())
}

/// Add the selected shapes to a library folder.
pub fn add_selection(app: &mut DarkBurn, library: usize, folder: usize, name: &str, on_duplicate: OnDuplicate) -> Result<usize, String> {
    let ids = app.editor.selected();
    if ids.is_empty() {
        return Err(tr("art.nothing_selected", "Nothing is selected"));
    }
    let mut clip = Clipboard::default();
    clip.copy(&app.editor.doc, &ids);
    let art = &mut app.extras.art;
    let open = art.libs.get_mut(library).ok_or_else(|| tr("art.no_library", "No such library"))?;
    let at = open.lib.add(folder, Item { name: name.to_string(), payload: clip.to_text() }, on_duplicate);
    open.dirty = true;
    Ok(at)
}

/// A library that is open.
pub struct Open {
    pub lib: ArtLibrary,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub mtime: Option<SystemTime>,
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Open {
    pub fn load(path: &Path) -> Result<Open, String> {
        let lib = ArtLibrary::from_json(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)?;
        Ok(Open { lib, path: Some(path.to_path_buf()), dirty: false, mtime: mtime(path) })
    }

    pub fn save_as(&mut self, path: &Path) -> Result<(), String> {
        let path = path.with_extension(FILE_EXTENSION);
        self.lib.save(&path)?;
        self.mtime = mtime(&path);
        self.path = Some(path);
        self.dirty = false;
        Ok(())
    }

    pub fn save(&mut self) -> Result<(), String> {
        let path = self.path.clone().ok_or_else(|| tr("art.no_file", "The library has no file yet: use Save as"))?;
        self.save_as(&path)
    }
}

#[derive(Default)]
pub struct ArtState {
    pub libs: Vec<Open>,
    pub search: String,
    /// Where "Add selection" puts items: library and folder.
    pub target: (usize, usize),
    pub renaming: Option<(usize, usize, usize, String)>,
    /// A drop or add that met an item of the same name: (library, folder, item, how to ask).
    pub pending: Option<(usize, usize, Item)>,
    pub status: String,
    pub thumbs: HashMap<u64, Thumb>,
    /// The panel's screen rectangle from the last frame, so dropped files can be told apart.
    pub panel_rect: Option<Rect>,
    pub last_check: Option<std::time::Instant>,
}

fn hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

impl ArtState {
    pub fn open(&mut self, path: &Path) -> Result<usize, String> {
        if let Some(i) = self.libs.iter().position(|l| l.path.as_deref() == Some(path)) {
            return Ok(i);
        }
        self.libs.push(Open::load(path)?);
        Ok(self.libs.len() - 1)
    }

    pub fn new_library(&mut self) -> usize {
        self.libs.push(Open { lib: ArtLibrary::default(), path: None, dirty: false, mtime: None });
        self.libs.len() - 1
    }

    /// Reload libraries another running copy changed (those with no unsaved changes here).
    pub fn refresh(&mut self, now: std::time::Instant) {
        if self.last_check.is_some_and(|t| now.duration_since(t).as_secs() < 3) {
            return;
        }
        self.last_check = Some(now);
        for open in &mut self.libs {
            if let Some(p) = &open.path
                && !open.dirty
                && let Some(m) = mtime(p)
                && open.mtime != Some(m)
                && let Ok(fresh) = Open::load(p)
            {
                *open = fresh;
            }
        }
    }

    /// Files dropped on the panel: libraries open, artwork files become items of the target folder.
    pub fn drop_files(&mut self, paths: &[PathBuf], opts: &crate::import::Options) -> usize {
        let mut done = 0;
        for path in paths {
            if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(FILE_EXTENSION)) {
                match self.open(path) {
                    Ok(_) => done += 1,
                    Err(e) => self.status = e,
                }
                continue;
            }
            let name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| crate::import::import_bytes(path, &b, opts, None)) {
                Ok(imp) if !imp.is_empty() => {
                    if self.libs.is_empty() {
                        self.new_library();
                    }
                    let (l, f) = (self.target.0.min(self.libs.len() - 1), self.target.1);
                    let item = Item { name, payload: imp.into_clipboard().to_text() };
                    let open = &mut self.libs[l];
                    if open.lib.has_item(f, &item.name) {
                        self.pending = Some((l, f, item));
                    } else {
                        open.lib.add(f, item, OnDuplicate::KeepBoth);
                        open.dirty = true;
                    }
                    done += 1;
                }
                Ok(_) => self.status = format!("{}: {}", name, tr("art.empty_file", "nothing to import")),
                Err(e) => self.status = format!("{name}: {e}"),
            }
        }
        done
    }
}

/// Dropped files that belong to the Art Library panel (the pointer is over it). True when taken.
pub fn handle_drop(app: &mut DarkBurn, ctx: &egui::Context, paths: &[PathBuf]) -> bool {
    let Some(rect) = app.extras.art.panel_rect else { return false };
    if !ctx.pointer_latest_pos().is_some_and(|p| rect.contains(p)) {
        return false;
    }
    let opts = crate::files::import_options(app);
    app.extras.art.drop_files(paths, &opts);
    true
}

fn btn(ui: &mut Ui, text: &str, tip: &str, enabled: bool) -> bool {
    // In a wrapped row a button that does not fit moves to the next row; left alone it would be
    // squeezed into what is left of this one and break its label letter by letter.
    let t = Tokens::get();
    let need = ui.painter().layout_no_wrap(text.to_owned(), egui::FontId::proportional(t.font_label), t.text).size().x + 2.0 * t.space_12;
    if ui.available_size_before_wrap().x < need {
        ui.end_row();
    }
    ui.add_enabled_ui(enabled, |ui| ui_kit::button(ui, text, ui_kit::Button::Small).backing_on_hover_text(tip).clicked()).inner
}

/// The Art Library panel.
pub fn show(ui: &mut Ui, app: &mut DarkBurn) {
    let t = Tokens::get();
    let dark = ui.visuals().dark_mode;
    app.extras.art.panel_rect = Some(ui.ctx().content_rect().intersect(ui.max_rect().expand(8.0)));
    app.extras.art.refresh(std::time::Instant::now());
    let mut art = std::mem::take(&mut app.extras.art);
    let mut insert: Option<(usize, usize, usize)> = None;
    let mut add_sel = false;

    ui.horizontal_wrapped(|ui| {
        if btn(ui, &tr("art.new", "New"), &tr("art.new_tip", "Start an empty art library"), true) {
            let i = art.new_library();
            art.target = (i, 0);
        }
        if btn(ui, &tr("art.load", "Load library"), &tr("art.load_tip", "Open an art library file; several can be open"), true)
            && let Some(p) = rfd::FileDialog::new().add_filter("Art library", &[FILE_EXTENSION]).pick_file()
        {
            match art.open(&p) {
                Ok(i) => art.target = (i, 0),
                Err(e) => art.status = e,
            }
        }
        let can_add = !app.editor.selection.is_empty() && !art.libs.is_empty();
        if btn(ui, &tr("art.add_selection", "Add selection"), &tr("art.add_selection_tip", "Add the selected shapes to the chosen folder"), can_add) {
            add_sel = true;
        }
    });
    ui_kit::hint_field(ui, &mut art.search, &tr("art.search", "Search art"));
    ui.add_space(t.space_6);

    if art.libs.is_empty() {
        ui.label(
            RichText::new(tr("art.empty", "No art library is open. Load one, make a new one, or drop a .dbart file here."))
                .size(t.font_small)
                .color(t.text_dim),
        );
    }
    let search = art.search.clone();
    let mut close: Option<usize> = None;
    let mut action: Option<Act> = None;
    for li in 0..art.libs.len() {
        let hits: Vec<(usize, usize)> = art.libs[li].lib.search(&search);
        let title = format!("{}{}", art.libs[li].lib.name, if art.libs[li].dirty { " *" } else { "" });
        let header = egui::CollapsingHeader::new(RichText::new(title).strong()).id_salt(("art_lib", li)).default_open(true);
        header.show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if btn(ui, &tr("art.save", "Save"), "", true) {
                    let r = if art.libs[li].path.is_some() {
                        art.libs[li].save()
                    } else if let Some(p) = rfd::FileDialog::new()
                        .add_filter("Art library", &[FILE_EXTENSION])
                        .set_file_name(format!("{}.{FILE_EXTENSION}", art.libs[li].lib.name))
                        .save_file()
                    {
                        art.libs[li].save_as(&p)
                    } else {
                        Ok(())
                    };
                    if let Err(e) = r {
                        art.status = e;
                    }
                }
                if btn(ui, &tr("art.add_folder", "New folder"), "", true) {
                    action = Some(Act::AddFolder(li));
                }
                if btn(ui, &tr("art.close", "Close"), "", true) {
                    close = Some(li);
                }
            });
            let folders = art.libs[li].lib.folders.clone();
            for (fi, folder) in folders.iter().enumerate() {
                let shown: Vec<usize> = hits.iter().filter(|(f, _)| *f == fi).map(|(_, i)| *i).collect();
                if !search.trim().is_empty() && shown.is_empty() {
                    continue;
                }
                let target = art.target == (li, fi);
                let label = format!("{}{}", if target { "> " } else { "" }, folder.name);
                let resp = egui::CollapsingHeader::new(label).id_salt(("art_folder", li, fi)).default_open(true).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                        for &ii in &shown {
                            let item = &folder.items[ii];
                            let key = hash(&item.payload);
                            let thumb = art.thumbs.entry(key).or_insert_with(|| thumb_of(&item.payload)).clone();
                            ui.vertical(|ui| {
                                let (rect, resp) = ui.allocate_exact_size(vec2(CELL, CELL), Sense::click());
                                ui.painter().rect(
                                    rect,
                                    t.radius_sm,
                                    if resp.hovered() { t.hover } else { t.control_fill },
                                    Stroke::new(1.0, t.border),
                                    egui::StrokeKind::Inside,
                                );
                                paint_thumb(&ui.painter().with_clip_rect(rect), rect.shrink(5.0), &thumb, dark);
                                let renaming = matches!(&art.renaming, Some((a, b, c, _)) if (*a, *b, *c) == (li, fi, ii));
                                if renaming {
                                    if let Some((_, _, _, text)) = &mut art.renaming {
                                        let e = ui.add(egui::TextEdit::singleline(text).desired_width(CELL));
                                        if !e.has_focus() && !e.lost_focus() {
                                            e.request_focus();
                                        }
                                        if e.lost_focus() {
                                            let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
                                            action = Some(Act::Rename(li, fi, ii, if cancelled { None } else { Some(text.clone()) }));
                                        }
                                    }
                                } else {
                                    ui.add_sized(
                                        [CELL, 14.0],
                                        egui::Label::new(RichText::new(&item.name).size(t.font_micro).color(t.text_dim)).truncate(),
                                    );
                                }
                                if resp.clicked() {
                                    insert = Some((li, fi, ii));
                                }
                                resp.clone().on_hover_ui(|ui| {
                                    let (r, _) = ui.allocate_exact_size(vec2(220.0, 220.0), Sense::hover());
                                    ui.painter().rect_filled(r, t.radius_sm, t.control_fill);
                                    paint_thumb(ui.painter(), r.shrink(8.0), &thumb, dark);
                                    ui.label(&item.name);
                                });
                                Menu::context(&resp, |ui| {
                                    ui.set_min_width(180.0);
                                    if MenuItem::new("art_insert", &tr("art.insert", "Put on the page"), "").show(ui).clicked() {
                                        insert = Some((li, fi, ii));
                                    }
                                    if MenuItem::new("art_rename", &tr("art.rename", "Rename"), "").show(ui).clicked() {
                                        action = Some(Act::StartRename(li, fi, ii));
                                    }
                                    let replace_text = tr("art.replace", "Replace with selection");
                                    let mut replace = MenuItem::new("art_replace", &replace_text, "");
                                    replace.enabled = !app.editor.selection.is_empty();
                                    if replace.show(ui).clicked() && replace.enabled {
                                        action = Some(Act::Replace(li, fi, ii));
                                    }
                                    Menu::separator(ui);
                                    let delete_text = tr("art.delete", "Delete");
                                    let mut del = MenuItem::new("art_delete", &delete_text, "");
                                    del.danger = true;
                                    if del.show(ui).clicked() {
                                        action = Some(Act::Delete(li, fi, ii));
                                    }
                                });
                            });
                        }
                    });
                });
                if resp.header_response.clicked() {
                    art.target = (li, fi);
                }
            }
        });
    }
    if let Some((l, f, item)) = art.pending.clone() {
        ui.add_space(t.space_6);
        ui_kit::status_line(ui, &format!("\"{}\" {}", item.name, tr("art.exists", "is already in this folder")), ui_kit::StatusTone::Dim);
        ui.horizontal(|ui| {
            if btn(ui, &tr("art.replace_it", "Replace it"), "", true) {
                art.libs[l].lib.add(f, item.clone(), OnDuplicate::Replace);
                art.libs[l].dirty = true;
                art.pending = None;
            }
            if btn(ui, &tr("art.keep_both", "Keep both"), "", true) {
                art.libs[l].lib.add(f, item.clone(), OnDuplicate::KeepBoth);
                art.libs[l].dirty = true;
                art.pending = None;
            }
            if btn(ui, &tr("art.skip", "Skip"), "", true) {
                art.pending = None;
            }
        });
    }
    if !art.status.is_empty() {
        ui_kit::status_line(ui, &art.status, ui_kit::StatusTone::Dim);
    }
    if let Some(i) = close
        && i < art.libs.len()
    {
        art.libs.remove(i);
        art.target = (0, 0);
    }
    app.extras.art = art;

    // Things that need the document.
    if add_sel {
        let (l, f) = app.extras.art.target;
        let name =
            format!("{} {}", tr("art.item", "Item"), app.extras.art.libs.get(l).and_then(|o| o.lib.folders.get(f)).map_or(1, |f| f.items.len() + 1));
        if app.extras.art.libs.get(l).is_some_and(|o| o.lib.has_item(f, &name)) {
            let _ = add_selection(app, l, f, &name, OnDuplicate::KeepBoth);
        } else if let Err(e) = add_selection(app, l, f, &name, OnDuplicate::KeepBoth) {
            app.extras.art.status = e;
        }
    }
    if let Some((l, f, i)) = insert
        && let Err(e) = insert_item(app, l, f, i)
    {
        app.extras.art.status = e;
    }
    match action {
        Some(Act::AddFolder(l)) => {
            let n = app.extras.art.libs[l].lib.folders.len() + 1;
            app.extras.art.libs[l].lib.folders.push(Folder { name: format!("{} {n}", tr("art.folder", "Folder")), items: Vec::new() });
            app.extras.art.libs[l].dirty = true;
        }
        Some(Act::StartRename(l, f, i)) => {
            let name = app.extras.art.libs[l].lib.folders[f].items[i].name.clone();
            app.extras.art.renaming = Some((l, f, i, name));
        }
        Some(Act::Rename(l, f, i, name)) => {
            app.extras.art.renaming = None;
            if let Some(name) = name.filter(|n| !n.trim().is_empty()) {
                rename_item(&mut app.extras.art, l, f, i, &name);
            }
        }
        Some(Act::Delete(l, f, i)) => {
            let open = &mut app.extras.art.libs[l];
            open.lib.folders[f].items.remove(i);
            open.dirty = true;
        }
        Some(Act::Replace(l, f, i)) => {
            let name = app.extras.art.libs[l].lib.folders[f].items[i].name.clone();
            if let Err(e) = add_selection(app, l, f, &name, OnDuplicate::Replace) {
                app.extras.art.status = e;
            }
        }
        None => {}
    }
}

enum Act {
    AddFolder(usize),
    StartRename(usize, usize, usize),
    Rename(usize, usize, usize, Option<String>),
    Delete(usize, usize, usize),
    Replace(usize, usize, usize),
}

/// Rename an item; a name another item of the folder has is refused.
pub fn rename_item(art: &mut ArtState, l: usize, f: usize, i: usize, name: &str) -> bool {
    let Some(open) = art.libs.get_mut(l) else { return false };
    let Some(folder) = open.lib.folders.get_mut(f) else { return false };
    if folder.items.iter().enumerate().any(|(k, it)| k != i && it.name == name) || i >= folder.items.len() {
        art.status = tr("art.name_taken", "Another item has that name");
        return false;
    }
    folder.items[i].name = name.to_string();
    open.dirty = true;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::Prefs;
    use darkburn_core::Shape;
    use darkburn_core::kurbo::Rect as KRect;

    fn app_with_rect() -> DarkBurn {
        let mut app = DarkBurn::new(Prefs::default());
        let mut id = darkburn_core::ShapeId(0);
        app.editor.edit("add", |d, sel| {
            id = d.add(Shape::rect(KRect::new(10.0, 10.0, 30.0, 20.0), 0.0, 1));
            sel.set([id]);
            true
        });
        app
    }

    #[test]
    fn add_selection_then_insert_round_trips_the_artwork() {
        let mut app = app_with_rect();
        app.extras.art.new_library();
        add_selection(&mut app, 0, 0, "Box", OnDuplicate::KeepBoth).unwrap();
        assert_eq!(app.extras.art.libs[0].lib.folders[0].items.len(), 1);
        let before = app.editor.doc.len();
        assert_eq!(insert_item(&mut app, 0, 0, 0).unwrap(), 1);
        assert_eq!(app.editor.doc.len(), before + 1);
        let t = thumb_of(&app.extras.art.libs[0].lib.folders[0].items[0].payload);
        assert!(!t.lines.is_empty() && (t.aspect - 2.0).abs() < 1e-3, "{t:?}");
    }

    #[test]
    fn duplicates_replace_or_keep_both() {
        let mut lib = ArtLibrary::default();
        let item = |p: &str| Item { name: "Star".into(), payload: p.into() };
        lib.add(0, item("a"), OnDuplicate::KeepBoth);
        lib.add(0, item("b"), OnDuplicate::Replace);
        assert_eq!(lib.folders[0].items.len(), 1);
        assert_eq!(lib.folders[0].items[0].payload, "b");
        lib.add(0, item("c"), OnDuplicate::KeepBoth);
        assert_eq!(lib.folders[0].items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["Star", "Star 2"]);
    }

    #[test]
    fn files_round_trip_and_open_libraries_are_shared_through_the_file() {
        let dir = std::env::temp_dir().join(format!("darkburn-art-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shapes.dbart");
        let mut a = ArtState::default();
        let i = a.new_library();
        a.libs[i].lib.add(0, Item { name: "One".into(), payload: "x".into() }, OnDuplicate::KeepBoth);
        a.libs[i].lib.folders.push(Folder { name: "Extra".into(), items: vec![] });
        a.libs[i].save_as(&path).unwrap();
        // A second "running copy" opens the file, changes it and saves.
        let mut b = ArtState::default();
        b.open(&path).unwrap();
        assert_eq!(b.libs[0].lib, a.libs[0].lib);
        b.libs[0].lib.add(0, Item { name: "Two".into(), payload: "y".into() }, OnDuplicate::KeepBoth);
        std::thread::sleep(std::time::Duration::from_millis(50));
        b.libs[0].save().unwrap();
        // The first copy sees the change on its next check (it has no unsaved changes).
        a.libs[0].mtime = Some(SystemTime::UNIX_EPOCH);
        a.refresh(std::time::Instant::now());
        assert_eq!(a.libs[0].lib.folders[0].items.len(), 2);
        assert!(ArtLibrary::from_json("{}").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_finds_by_folder_or_name_and_rename_refuses_clashes() {
        let mut art = ArtState::default();
        art.new_library();
        let lib = &mut art.libs[0].lib;
        lib.add(0, Item { name: "Heart".into(), payload: String::new() }, OnDuplicate::KeepBoth);
        lib.add(0, Item { name: "Star".into(), payload: String::new() }, OnDuplicate::KeepBoth);
        assert_eq!(lib.search("hea"), [(0, 0)]);
        assert_eq!(lib.search("general").len(), 2);
        assert!(lib.search("zzz").is_empty());
        assert!(!rename_item(&mut art, 0, 0, 0, "Star"));
        assert!(rename_item(&mut art, 0, 0, 0, "Love"));
        assert_eq!(art.libs[0].lib.folders[0].items[0].name, "Love");
    }

    #[test]
    fn dropped_files_open_libraries_and_become_items() {
        let dir = std::env::temp_dir().join(format!("darkburn-art-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let svg = dir.join("box.svg");
        std::fs::write(
            &svg,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="10mm"><rect width="20" height="10" fill="none" stroke="red"/></svg>"#,
        )
        .unwrap();
        let mut art = ArtState::default();
        let opts = crate::files::import_options(&DarkBurn::new(Prefs::default()));
        assert_eq!(art.drop_files(std::slice::from_ref(&svg), &opts), 1);
        assert_eq!(art.libs[0].lib.folders[0].items[0].name, "box");
        assert_eq!(art.drop_files(&[svg], &opts), 1);
        assert!(art.pending.is_some(), "the same name asks before replacing");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
