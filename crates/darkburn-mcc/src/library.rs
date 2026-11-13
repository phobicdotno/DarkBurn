//! A folder scanner for an Mlaser machine package: finds recipes, machine settings, compensation
//! tables and the small settings files below a folder and reads them into a [`Library`].
//!
//! The vendor's layout is `Cutting parameters/<head>/<material>/<name>.xml`, where `<head>` is the
//! fibre source power (`1200W`), the same with `_air` (recipes cut with air fed on the nitrogen
//! line) or `CO2`. Cleaning and quenching recipes sit in a folder of their own and are kept apart
//! ([`RecipeKind`]). Machine settings (`Bk*Para.xml`, a full backup) are merged per folder into a
//! [`MachineProfile`].
//!
//! The scanner only reads files. It does not follow symbolic links, stops at [`MAX_DEPTH`] folders
//! deep and [`MAX_ENTRIES`] directory entries, and skips files larger than [`MAX_FILE_BYTES`].
//! A file it cannot read lands in [`Library::unreadable`] with the reason; nothing panics.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::craft::{Craft, CraftIssue, GasType};
use crate::mlaser_files::{CompensationTable, IniFile, ScFile, read_statistics, starts_with_sc_magic};
use crate::paramxml::{MlaserError, ParamDoc};
use crate::profile::MachineProfile;
use crate::recipe::{LayerKind, NamedGas, RecipeFile, RecipeName, recipe_from_doc, validate_craft};

/// Deepest folder level scanned below the root.
pub const MAX_DEPTH: usize = 12;
/// Most directory entries looked at in one scan.
pub const MAX_ENTRIES: usize = 50_000;
/// Largest file read, bytes.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// What a recipe is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecipeKind {
    /// A cutting recipe.
    Cutting,
    /// Laser cleaning (surface) recipe.
    Cleaning,
    /// Quenching (surface hardening) recipe.
    Quenching,
}

/// The head folder a recipe sits in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeadFolder {
    /// `<watts>W`, or `<watts>W_air` when `air` is true.
    Fibre {
        /// Source power, W.
        watts: u32,
        /// The `_air` folder.
        air: bool,
    },
    /// `CO2`.
    Co2,
}

impl HeadFolder {
    /// Read a folder name (`1200W`, `1200W_air`, `CO2`; case is ignored).
    pub fn parse(name: &str) -> Option<HeadFolder> {
        let lower = name.trim().to_ascii_lowercase();
        if lower == "co2" {
            return Some(HeadFolder::Co2);
        }
        let (body, air) = match lower.strip_suffix("_air") {
            Some(b) => (b, true),
            None => (lower.as_str(), false),
        };
        let digits = body.strip_suffix('w')?;
        if digits.is_empty() || digits.len() > 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(HeadFolder::Fibre { watts: digits.parse().ok()?, air })
    }

    /// The layer kind the folder is for.
    pub fn kind(&self) -> LayerKind {
        match self {
            HeadFolder::Fibre { .. } => LayerKind::Fibre,
            HeadFolder::Co2 => LayerKind::Co2,
        }
    }
}

/// How much a check finding matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CheckLevel {
    /// Expected for the vendor's files; shown for information.
    Info,
    /// Something does not add up.
    Warning,
}

/// One finding of the name-against-content checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// Level.
    pub level: CheckLevel,
    /// What was found.
    pub message: String,
}

/// A recipe found in the library.
#[derive(Clone, Debug, PartialEq)]
pub struct RecipeEntry {
    /// Path below the scan root.
    pub path: PathBuf,
    /// Cutting, cleaning or quenching.
    pub kind: RecipeKind,
    /// Head folder, when the recipe sits below one.
    pub head: Option<HeadFolder>,
    /// Material folder (the folder directly below the head folder).
    pub material: Option<String>,
    /// What the file name says.
    pub name: RecipeName,
    /// The file, read.
    pub recipe: RecipeFile,
    /// Name-against-content findings.
    pub checks: Vec<Check>,
    /// Findings of the craft checks ([`crate::craft::FibreCraft::validate`]).
    pub issues: Vec<CraftIssue>,
}

impl RecipeEntry {
    /// The craft, ready to put on a layer.
    pub fn craft(&self) -> &Craft {
        &self.recipe.layer.craft
    }

    /// The file stem.
    pub fn title(&self) -> String {
        self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

/// Machine settings merged per folder.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileEntry {
    /// Folder below the scan root.
    pub folder: PathBuf,
    /// Files merged, in merge order (the first wins on a conflict).
    pub files: Vec<PathBuf>,
    /// The merged profile.
    pub profile: MachineProfile,
}

/// A compensation table found in the library.
#[derive(Clone, Debug, PartialEq)]
pub struct CompensationEntry {
    /// Path below the scan root.
    pub path: PathBuf,
    /// Head (`Co2_pwmCompensation.txt` is CO2, `pwmCompensation.txt` fibre).
    pub kind: LayerKind,
    /// The table.
    pub table: CompensationTable,
}

/// A file that could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unreadable {
    /// Path below the scan root.
    pub path: PathBuf,
    /// Why.
    pub reason: String,
}

/// Everything found below a folder.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Library {
    /// Recipes of all kinds, sorted by path.
    pub recipes: Vec<RecipeEntry>,
    /// Machine settings, one profile per folder that holds settings files.
    pub profiles: Vec<ProfileEntry>,
    /// Compensation tables.
    pub compensation: Vec<CompensationEntry>,
    /// Plain `.ini` files.
    pub ini_files: Vec<(PathBuf, IniFile)>,
    /// `scFlie` value lists (`.dat`, `.pcf`, some `.ini`).
    pub value_lists: Vec<(PathBuf, ScFile)>,
    /// Job statistics files, rows of fields.
    pub statistics: Vec<(PathBuf, Vec<Vec<String>>)>,
    /// `.chf` job files found (not read here; see [`crate::chf`]).
    pub jobs: Vec<PathBuf>,
    /// Files that could not be read, with the reason.
    pub unreadable: Vec<Unreadable>,
    /// Library-wide findings (limits reached, tables that disagree).
    pub notes: Vec<Check>,
}

impl Library {
    /// Recipes of one kind.
    pub fn recipes_of(&self, kind: RecipeKind) -> impl Iterator<Item = &RecipeEntry> {
        self.recipes.iter().filter(move |r| r.kind == kind)
    }

    /// The craft of the recipe whose file stem is `title` (case-insensitive), when exactly one
    /// recipe has it.
    pub fn preset_craft(&self, title: &str) -> Option<&Craft> {
        let mut found = self.recipes.iter().filter(|r| r.title().eq_ignore_ascii_case(title));
        let first = found.next()?;
        found.next().is_none().then(|| first.craft())
    }

    /// Recipes the craft checks flag, with their findings.
    pub fn flagged(&self) -> Vec<(&RecipeEntry, &[CraftIssue])> {
        self.recipes.iter().filter(|r| !r.issues.is_empty()).map(|r| (r, r.issues.as_slice())).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileClass {
    Xml,
    Compensation(LayerKind),
    Statistics,
    Ini,
    ValueList,
    Job,
}

fn classify_name(name: &str) -> Option<FileClass> {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "xml" => Some(FileClass::Xml),
        "ini" => Some(FileClass::Ini),
        "dat" | "pcf" => Some(FileClass::ValueList),
        "chf" => Some(FileClass::Job),
        "txt" if lower.contains("pwmcompensation") => {
            Some(FileClass::Compensation(if lower.starts_with("co2") { LayerKind::Co2 } else { LayerKind::Fibre }))
        }
        "txt" if lower.starts_with("processesstatistic") => Some(FileClass::Statistics),
        _ => None,
    }
}

fn recipe_kind(rel: &Path) -> RecipeKind {
    let text = rel.to_string_lossy().to_ascii_lowercase();
    let stem = rel.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if stem.contains("quench") {
        RecipeKind::Quenching
    } else if stem.contains("clean") {
        RecipeKind::Cleaning
    } else if text.contains("quench") || text.contains("clean") {
        // A folder for both; the file name did not say which.
        if text.contains("clean") && !text.contains("quench") { RecipeKind::Cleaning } else { RecipeKind::Quenching }
    } else {
        RecipeKind::Cutting
    }
}

fn head_and_material(rel: &Path) -> (Option<HeadFolder>, Option<String>) {
    let parts: Vec<String> = rel.parent().map(|p| p.iter().map(|c| c.to_string_lossy().into_owned()).collect()).unwrap_or_default();
    for (i, p) in parts.iter().enumerate().rev() {
        if let Some(h) = HeadFolder::parse(p) {
            return (Some(h), parts.get(i + 1).cloned());
        }
    }
    (None, None)
}

fn fmt_num(v: f64) -> String {
    format!("{v}")
}

/// Compare what a recipe's file name and folders say with what the file holds.
pub fn check_recipe(head: Option<&HeadFolder>, stem: &str, name: &RecipeName, recipe: &RecipeFile, kind: RecipeKind) -> Vec<Check> {
    let mut out = Vec::new();
    let info = |m: String| Check { level: CheckLevel::Info, message: m };
    let warn = |m: String| Check { level: CheckLevel::Warning, message: m };
    let layer = &recipe.layer;
    if let Some(h) = head
        && h.kind() != layer.kind
    {
        out.push(warn(format!("sits in a {:?} folder but holds a {:?} layer", h.kind(), layer.kind)));
    }
    if layer.slot != crate::recipe::RECIPE_SLOT {
        out.push(info(format!("stored in layer slot {}, not {}", layer.slot, crate::recipe::RECIPE_SLOT)));
    }
    for n in &layer.notes {
        out.push(warn(format!("{}: {}", n.attr, n.message)));
    }
    let (gas, focus, pressure, inner_name) = match &layer.craft {
        Craft::Fibre(c) => (c.cut.gas, Some(c.cut.focus_mm), c.cut.gas_pressure_bar, c.name.as_str()),
        Craft::Co2(c) => (c.gas, None, c.gas_pressure_bar, c.name.as_str()),
    };
    if let Some(g) = name.gas
        && !g.matches(gas)
    {
        if g == NamedGas::Air && matches!(gas, GasType::N2 | GasType::HighN2) {
            out.push(info(format!("named air, stored as {gas:?}: air fed on the nitrogen line")));
        } else {
            out.push(warn(format!("named {g:?} but the gas is {gas:?}")));
        }
    }
    if let Some(HeadFolder::Fibre { air: true, .. }) = head
        && name.gas.is_some_and(|g| g != NamedGas::Air)
    {
        out.push(warn("in an air folder but named for another gas".to_string()));
    }
    if let (Some(nf), Some(ff)) = (name.focus_mm, focus)
        && nf != ff
    {
        out.push(info(format!("name says focus {}, the file has {}: the name's focus is set by hand on the head", fmt_num(nf), fmt_num(ff))));
    }
    if let Some(p) = name.pressure_mpa
        && p != pressure
    {
        out.push(info(format!("name says {} MPa, the file's gas pressure is {}", fmt_num(p), fmt_num(pressure))));
    }
    if !inner_name.is_empty() && inner_name.trim() != stem.trim() {
        let inner = RecipeName::parse(inner_name);
        let mut what = format!("the name stored inside is {inner_name:?}");
        if kind == RecipeKind::Cutting {
            if let (Some(a), Some(b)) = (name.thickness_mm, inner.thickness_mm)
                && a != b
            {
                let _ = write!(what, "; thickness {} against {}", fmt_num(a), fmt_num(b));
            }
            if let (Some(a), Some(b)) = (&name.nozzle, &inner.nozzle)
                && a != b
            {
                let _ = write!(what, "; nozzle {a} against {b}");
            }
        }
        out.push(info(what));
    }
    if kind == RecipeKind::Cutting && head.is_some_and(|h| h.kind() == LayerKind::Fibre) {
        if name.thickness_mm.is_none() {
            out.push(info("no thickness in the name".to_string()));
        }
        if name.nozzle.is_none() {
            out.push(info("no nozzle in the name".to_string()));
        }
    }
    out
}

fn rel_to(root: &Path, p: &Path) -> PathBuf {
    p.strip_prefix(root).map(Path::to_path_buf).unwrap_or_else(|_| p.to_path_buf())
}

fn fail(lib: &mut Library, rel: &Path, e: MlaserError) {
    lib.unreadable.push(Unreadable { path: rel.to_path_buf(), reason: e.to_string() });
}

struct Walker {
    root: PathBuf,
    entries: usize,
    files: Vec<(PathBuf, FileClass)>,
    notes: Vec<Check>,
    unreadable: Vec<Unreadable>,
}

impl Walker {
    fn walk(&mut self, dir: &Path, depth: usize) {
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) => {
                self.unreadable.push(Unreadable { path: rel_to(&self.root, dir), reason: format!("folder cannot be listed: {e}") });
                return;
            }
        };
        let mut children: Vec<PathBuf> = Vec::new();
        for item in rd {
            self.entries += 1;
            if self.entries > MAX_ENTRIES {
                if self.entries == MAX_ENTRIES + 1 {
                    self.notes.push(Check { level: CheckLevel::Warning, message: format!("stopped after {MAX_ENTRIES} folder entries") });
                }
                return;
            }
            if let Ok(item) = item {
                children.push(item.path());
            }
        }
        children.sort();
        for p in children {
            let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if depth >= MAX_DEPTH {
                    self.notes.push(Check {
                        level: CheckLevel::Warning,
                        message: format!("{} is deeper than {MAX_DEPTH} folders; not scanned", rel_to(&self.root, &p).display()),
                    });
                } else {
                    self.walk(&p, depth + 1);
                }
            } else if meta.is_file()
                && let Some(class) = p.file_name().and_then(|n| classify_name(&n.to_string_lossy()))
            {
                if meta.len() > MAX_FILE_BYTES {
                    self.unreadable
                        .push(Unreadable { path: rel_to(&self.root, &p), reason: format!("{} bytes, more than {MAX_FILE_BYTES}", meta.len()) });
                } else {
                    self.files.push((p, class));
                }
            }
        }
    }
}

fn settings_rank(name: &str) -> u8 {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("bk") {
        0
    } else if lower.contains("bk") {
        1
    } else {
        2
    }
}

/// Scan a folder.
pub fn scan(root: &Path) -> Library {
    let mut w = Walker { root: root.to_path_buf(), entries: 0, files: Vec::new(), notes: Vec::new(), unreadable: Vec::new() };
    w.walk(root, 0);
    let mut lib = Library { notes: w.notes, unreadable: w.unreadable, ..Library::default() };
    let mut settings: Vec<(PathBuf, PathBuf, ParamDoc)> = Vec::new();
    for (abs, class) in w.files {
        let rel = rel_to(root, &abs);
        let bytes = match std::fs::read(&abs) {
            Ok(b) => b,
            Err(e) => {
                lib.unreadable.push(Unreadable { path: rel, reason: format!("cannot be read: {e}") });
                continue;
            }
        };
        match class {
            FileClass::Job => lib.jobs.push(rel),
            FileClass::Compensation(kind) => match CompensationTable::parse(&bytes) {
                Ok(table) => lib.compensation.push(CompensationEntry { path: rel, kind, table }),
                Err(e) => fail(&mut lib, &rel, e),
            },
            FileClass::Statistics => match read_statistics(&bytes) {
                Ok(rows) => lib.statistics.push((rel, rows)),
                Err(e) => fail(&mut lib, &rel, e),
            },
            FileClass::Ini | FileClass::ValueList if starts_with_sc_magic(&bytes) => match ScFile::parse(&bytes) {
                Ok(f) => lib.value_lists.push((rel, f)),
                Err(e) => fail(&mut lib, &rel, e),
            },
            FileClass::ValueList => fail(&mut lib, &rel, MlaserError::new("not an scFlie value list")),
            FileClass::Ini => match IniFile::parse(&bytes) {
                Ok(f) => lib.ini_files.push((rel, f)),
                Err(e) => fail(&mut lib, &rel, e),
            },
            FileClass::Xml => match ParamDoc::parse(&bytes) {
                Err(e) => fail(&mut lib, &rel, e),
                Ok(doc) if doc.root != crate::recipe::ROOT => {
                    fail(&mut lib, &rel, MlaserError::new(format!("root element is <{}>, not a parameter document", doc.root)))
                }
                Ok(doc) if doc.sections.is_empty() => fail(&mut lib, &rel, MlaserError::new("parameter document with no sections")),
                Ok(doc) => {
                    let layers = doc.sections.iter().filter(|s| LayerKind::from_section_name(&s.name).is_some()).count();
                    if layers == 1 && doc.sections.len() == 1 {
                        match recipe_from_doc(doc) {
                            Ok(recipe) => lib.recipes.push(recipe_entry(rel, recipe)),
                            Err(e) => fail(&mut lib, &rel, e),
                        }
                    } else {
                        let folder = rel.parent().map(Path::to_path_buf).unwrap_or_default();
                        settings.push((folder, rel, doc));
                    }
                }
            },
        }
    }
    settings.sort_by(|a, b| {
        let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        a.0.cmp(&b.0).then(settings_rank(&name(&a.1)).cmp(&settings_rank(&name(&b.1)))).then(a.1.cmp(&b.1))
    });
    for (folder, rel, doc) in settings {
        let label = rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match lib.profiles.last_mut() {
            Some(p) if p.folder == folder => {
                p.profile.merge(&label, &doc);
                p.files.push(rel);
            }
            _ => {
                let profile = MachineProfile::from_docs(&[(&label, &doc)]);
                lib.profiles.push(ProfileEntry { folder, files: vec![rel], profile });
            }
        }
    }
    lib.recipes.sort_by(|a, b| a.path.cmp(&b.path));
    compare_compensation(&mut lib);
    lib
}

fn recipe_entry(rel: PathBuf, recipe: RecipeFile) -> RecipeEntry {
    let stem = rel.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name = RecipeName::parse(&stem);
    let kind = recipe_kind(&rel);
    let (head, material) = head_and_material(&rel);
    let checks = check_recipe(head.as_ref(), &stem, &name, &recipe, kind);
    let issues = validate_craft(&recipe.layer.craft);
    RecipeEntry { path: rel, kind, head, material, name, recipe, checks, issues }
}

fn compare_compensation(lib: &mut Library) {
    let mut notes = Vec::new();
    for c in &lib.compensation {
        for p in &lib.profiles {
            match p.profile.scan_fly_table(c.kind) {
                Some(Ok(t)) => {
                    let d = c.table.differences(&t);
                    if !d.is_empty() {
                        notes.push(Check {
                            level: CheckLevel::Info,
                            message: format!(
                                "{} differs from the table stored in the settings of {} in {} rows",
                                c.path.display(),
                                p.folder.display(),
                                d.len()
                            ),
                        });
                    }
                }
                Some(Err(e)) => {
                    notes.push(Check { level: CheckLevel::Warning, message: format!("scan table in the settings of {}: {e}", p.folder.display()) })
                }
                None => {}
            }
        }
    }
    notes.dedup();
    lib.notes.extend(notes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::write_recipe;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            let p = std::env::temp_dir().join(format!("darkburn-mcc-lib-{tag}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn put(&self, rel: &str, bytes: &[u8]) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fibre(name: &str, gas: GasType, focus: f64) -> Vec<u8> {
        let mut c = crate::recipe::fibre_from_entry(&crate::recipe::fibre_template()).0;
        c.name = name.to_string();
        c.cut.gas = gas;
        c.cut.focus_mm = focus;
        c.cut.gas_pressure_bar = 0.8;
        write_recipe(&Craft::Fibre(c)).unwrap()
    }

    fn co2(name: &str) -> Vec<u8> {
        let mut c = crate::recipe::co2_from_entry(&crate::recipe::co2_template()).0;
        c.name = name.to_string();
        write_recipe(&Craft::Co2(c)).unwrap()
    }

    const SETTINGS: &str = "<ParameterRoot>\r\n<PManuParam>\r\n<MC JogFastSpeed=\"120\"/>\r\n</PManuParam>\r\n<PGraphParam>\r\n<GRP FiberScanFlyCompensateStr=\"100#0.25,200#0.5\"/>\r\n</PGraphParam>\r\n</ParameterRoot>\r\n";
    const SETTINGS2: &str =
        "<ParameterRoot>\r\n<PManuParam>\r\n<MC JogFastSpeed=\"90\" JogSlowSpeed=\"20\"/>\r\n</PManuParam>\r\n</ParameterRoot>\r\n";

    fn sample() -> TempDir {
        let t = TempDir::new("sample");
        t.put("Cutting parameters/800W/Mild steel/Mild 2.0mm 1.5S F-2 O2.xml", &fibre("Mild 2.0mm 1.5S F-2 O2", GasType::O2, -2.0));
        t.put("Cutting parameters/800W_air/Brass/Brass 1mm 1.5S F-1 air.xml", &fibre("Brass 1mm 1.5S F-1 air", GasType::HighN2, 0.0));
        t.put("Cutting parameters/800W/Mild steel/Mild 3mm 2.0D N2.xml", &fibre("other name", GasType::O2, 0.0));
        t.put("Cutting parameters/CO2/Birch/Birch 4mm air.xml", &co2("Birch 4mm air"));
        t.put("Cutting parameters/CO2/Birch/Wrong head.xml", &fibre("Wrong head", GasType::Air, 0.0));
        t.put("Surface/Cleaning 1.0MPA.xml", &fibre("Surface demo", GasType::HighN2, 0.0));
        t.put("Surface/quenching 0.5 MPA.xml", &fibre("Surface demo", GasType::HighN2, 0.0));
        t.put("pwmCompensation.txt", b"100,0.25\r\n200,0.75");
        t.put("Co2_pwmCompensation.txt", b"100,0.1\r\n300,0.2");
        t.put("pkg/File/BkManuPara.xml", SETTINGS.as_bytes());
        t.put("pkg/File/SecondBkManuPara.xml", SETTINGS2.as_bytes());
        t.put("pkg/File/softPara.ini", b"[SC2000]\r\nNormalExit=1\r\n");
        t.put("pkg/File/Temp/tempIsBreak.ini", b"scFlie\r\n0\r\neof\r\n");
        t.put("pkg/File/ManuContour.dat", b"scFlie\r\n2\r\n4\r\n5\r\neof\r\n");
        t.put("pkg/File/blob.dat", b"\x00\x01\x02");
        t.put("pkg/File/ProcessesStatistic.txt", b"demo,1,2,3,4,5,6\r\n");
        t.put("pkg/Graph/job.chf", b"scFlie\r\n");
        t.put("pkg/broken.xml", b"<ParameterRoot><PLayerParam11><GP a=\"1\"");
        t.put("pkg/notes.txt", b"ignored");
        t.put("pkg/other.xml", b"<Config/>");
        t
    }

    #[test]
    fn scans_a_package() {
        let t = sample();
        let lib = scan(&t.0);
        let titles: Vec<String> = lib.recipes.iter().map(|r| r.title()).collect();
        assert_eq!(
            titles,
            [
                "Mild 2.0mm 1.5S F-2 O2",
                "Mild 3mm 2.0D N2",
                "Brass 1mm 1.5S F-1 air",
                "Birch 4mm air",
                "Wrong head",
                "Cleaning 1.0MPA",
                "quenching 0.5 MPA"
            ]
        );
        assert_eq!(lib.recipes_of(RecipeKind::Cleaning).count(), 1);
        assert_eq!(lib.recipes_of(RecipeKind::Quenching).count(), 1);
        assert_eq!(lib.recipes_of(RecipeKind::Cutting).count(), 5);

        let mild = lib.recipes.iter().find(|r| r.title().starts_with("Mild 2.0")).unwrap();
        assert_eq!(mild.head, Some(HeadFolder::Fibre { watts: 800, air: false }));
        assert_eq!(mild.material.as_deref(), Some("Mild steel"));
        assert_eq!(mild.name.thickness_mm, Some(2.0));
        assert_eq!(mild.name.nozzle.as_deref(), Some("1.5S"));
        assert_eq!(mild.name.focus_mm, Some(-2.0));
        assert!(mild.checks.is_empty(), "{:?}", mild.checks);
        assert!(lib.preset_craft("mild 2.0mm 1.5s f-2 o2").is_some());
        assert!(lib.preset_craft("nothing").is_none());

        let brass = lib.recipes.iter().find(|r| r.title().starts_with("Brass")).unwrap();
        assert_eq!(brass.head, Some(HeadFolder::Fibre { watts: 800, air: true }));
        let msgs: Vec<(CheckLevel, &str)> = brass.checks.iter().map(|c| (c.level, c.message.as_str())).collect();
        assert!(msgs.iter().any(|(l, m)| *l == CheckLevel::Info && m.contains("nitrogen line")), "{msgs:?}");
        assert!(msgs.iter().any(|(l, m)| *l == CheckLevel::Info && m.contains("focus -1")), "{msgs:?}");
        assert!(msgs.iter().all(|(l, _)| *l == CheckLevel::Info), "{msgs:?}");

        let other = lib.recipes.iter().find(|r| r.title().starts_with("Mild 3mm")).unwrap();
        assert!(other.checks.iter().any(|c| c.level == CheckLevel::Warning && c.message.contains("named N2")), "{:?}", other.checks);
        assert!(other.checks.iter().any(|c| c.message.contains("\"other name\"")));

        let wrong = lib.recipes.iter().find(|r| r.title() == "Wrong head").unwrap();
        assert!(wrong.checks.iter().any(|c| c.level == CheckLevel::Warning && c.message.contains("Co2 folder")), "{:?}", wrong.checks);

        let clean = lib.recipes_of(RecipeKind::Cleaning).next().unwrap();
        assert!(clean.checks.iter().any(|c| c.message.contains("1 MPa")), "{:?}", clean.checks);

        assert_eq!(lib.compensation.len(), 2);
        assert_eq!(lib.compensation.iter().find(|c| c.kind == LayerKind::Co2).unwrap().table.points.len(), 2);
        assert_eq!(lib.profiles.len(), 1);
        let p = &lib.profiles[0];
        assert_eq!(p.folder, Path::new("pkg/File"));
        assert_eq!(p.files, [PathBuf::from("pkg/File/BkManuPara.xml"), PathBuf::from("pkg/File/SecondBkManuPara.xml")]);
        assert_eq!(p.profile.machine_params().0.jog_fast_speed, 120.0);
        assert_eq!(p.profile.machine_params().0.jog_slow_speed, 20.0);
        assert_eq!(p.profile.conflicts.len(), 1);
        assert!(lib.notes.iter().any(|n| n.message.contains("pwmCompensation.txt differs") && n.message.contains("1 rows")), "{:?}", lib.notes);

        assert_eq!(lib.ini_files.len(), 1);
        assert_eq!(lib.value_lists.len(), 2);
        assert_eq!(lib.statistics.len(), 1);
        assert_eq!(lib.jobs, [PathBuf::from("pkg/Graph/job.chf")]);
        let bad: Vec<&Path> = lib.unreadable.iter().map(|u| u.path.as_path()).collect();
        assert_eq!(bad, [Path::new("pkg/File/blob.dat"), Path::new("pkg/broken.xml"), Path::new("pkg/other.xml")]);
        assert!(lib.unreadable.iter().all(|u| !u.reason.is_empty()));
    }

    #[test]
    fn written_recipes_pass_the_craft_checks() {
        let t = sample();
        let lib = scan(&t.0);
        for r in lib.recipes.iter().filter(|r| r.title() != "Wrong head") {
            assert!(r.issues.iter().all(|i| i.severity != crate::craft::Severity::Error), "{}: {:?}", r.title(), r.issues);
        }
        let _ = lib.flagged();
    }

    #[test]
    fn head_folders() {
        assert_eq!(HeadFolder::parse("1200W"), Some(HeadFolder::Fibre { watts: 1200, air: false }));
        assert_eq!(HeadFolder::parse("1200w_AIR"), Some(HeadFolder::Fibre { watts: 1200, air: true }));
        assert_eq!(HeadFolder::parse("co2"), Some(HeadFolder::Co2));
        for bad in ["W", "_air", "12x0W", "9999999W", "Copper", ""] {
            assert_eq!(HeadFolder::parse(bad), None, "{bad}");
        }
        assert_eq!(recipe_kind(Path::new("Cleaning and quenching/x.xml")), RecipeKind::Quenching);
        assert_eq!(recipe_kind(Path::new("Cleaning and quenching/Cleaning 1MPa.xml")), RecipeKind::Cleaning);
        assert_eq!(recipe_kind(Path::new("Cleaning/x.xml")), RecipeKind::Cleaning);
        assert_eq!(recipe_kind(Path::new("1200W/Steel/x.xml")), RecipeKind::Cutting);
    }

    #[test]
    fn hostile_files_land_in_unreadable() {
        let t = TempDir::new("hostile");
        let huge = format!(
            "<ParameterRoot>\r\n<PLayerParam11>\r\n<GP CutSpeed=\"1e400\" CutPower=\"nan\" CutFrequency=\"{}\" ManuType=\"99\"/>\r\n</PLayerParam11>\r\n</ParameterRoot>\r\n",
            "9".repeat(5000)
        );
        t.put("1200W/Steel/huge.xml", huge.as_bytes());
        t.put("1200W/Steel/empty.xml", b"");
        t.put("1200W/Steel/root.xml", b"<ParameterRoot/>");
        t.put("1200W/Steel/two.xml", b"<ParameterRoot><PLayerParam1><GP/></PLayerParam1><PLayerParam2><GP/></PLayerParam2></ParameterRoot>");
        t.put("1200W/Steel/nogp.xml", b"<ParameterRoot><PLayerParam11><XX/></PLayerParam11></ParameterRoot>");
        t.put("1200W/Steel/deep.xml", b"<ParameterRoot><PLayerParam11><GP><a/></GP></PLayerParam11></ParameterRoot>");
        t.put("1200W/Steel/bin.xml", &[0xFF, 0xFE, 0x00, 0x3C, 0x00]);
        t.put("pwmCompensation.txt", b"fast,slow");
        t.put("x/ManuContour.dat", b"scFlie\r\n5\r\n");
        let mut deep = String::new();
        for i in 0..(MAX_DEPTH + 3) {
            deep.push_str(&format!("d{i}/"));
        }
        t.put(&format!("{deep}deep.ini"), b"[a]\r\nb=c\r\n");
        let lib = scan(&t.0);
        // The huge-number recipe is read with notes; the rest are unreadable or settings.
        let huge = lib.recipes.iter().find(|r| r.title() == "huge").unwrap();
        assert!(huge.checks.iter().any(|c| c.level == CheckLevel::Warning), "{:?}", huge.checks);
        let bad: Vec<String> = lib.unreadable.iter().map(|u| u.path.to_string_lossy().into_owned()).collect();
        for want in ["empty.xml", "root.xml", "nogp.xml", "deep.xml", "bin.xml", "pwmCompensation.txt", "ManuContour.dat"] {
            assert!(bad.iter().any(|b| b.ends_with(want)), "{want} not in {bad:?}");
        }
        assert_eq!(lib.profiles.len(), 1);
        assert!(lib.notes.iter().any(|n| n.message.contains("deeper than")));
        assert!(lib.ini_files.is_empty());
        let missing = scan(&t.0.join("no such folder"));
        assert_eq!(missing.unreadable.len(), 1);
    }
}
