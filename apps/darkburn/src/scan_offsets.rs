//! Scanning offset (backlash) table and fast blank-skip settings for image scans [IMG-22,
//! IMG-23].
//!
//! A bidirectional scan burns the lines of one direction slightly shifted against the other at
//! speed, because the laser fires a little late. The table lists, per scan speed, how far the
//! two directions are apart; [`shift_plans`] moves the lines of a [`darkburn_raster::ScanPlan`]
//! by half the offset each way. Fast blank-skip lets long gaps of blank image be rapided over.
//!
//! These are device-level settings, so they live here, process-wide, until the device settings
//! window arrives: [`current`] and [`set`] read and write them, [`load_from`] and [`save_to`]
//! keep them in `scan_offsets.dbso` in the settings folder, and `images::scan_plans_for_layer`
//! applies them. The sign of the shift (forward lines go back by half the offset, reverse lines
//! forward by half) still has to be confirmed on a real machine.

use std::path::Path;
use std::sync::RwLock;

use darkburn_raster::ScanPlan;
use serde::{Deserialize, Serialize};

/// File extension of an exported table.
pub const EXTENSION: &str = "dbso";
const FORMAT: &str = "darkburn-scan-offsets";
const VERSION: u32 = 1;
/// The most rows a table holds.
pub const MAX_ROWS: usize = 64;
/// File name in the settings folder.
pub const FILE: &str = "scan_offsets.dbso";

/// One row: at `speed` (mm/s) the two directions are `offset` mm apart.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OffsetEntry {
    pub speed: f64,
    pub offset: f64,
}

/// Offsets by scan speed, sorted by speed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanOffsetTable {
    pub entries: Vec<OffsetEntry>,
}

impl ScanOffsetTable {
    /// Sorts by speed, drops rows that are not numbers, merges rows with the same speed (the
    /// later one wins) and keeps at most [`MAX_ROWS`].
    pub fn normalise(&mut self) {
        self.entries.retain(|e| e.speed.is_finite() && e.speed > 0.0 && e.offset.is_finite());
        for e in &mut self.entries {
            e.offset = e.offset.clamp(-10.0, 10.0);
        }
        let mut merged: Vec<OffsetEntry> = Vec::new();
        for e in std::mem::take(&mut self.entries) {
            match merged.iter_mut().find(|m| (m.speed - e.speed).abs() < 1e-9) {
                Some(m) => m.offset = e.offset,
                None => merged.push(e),
            }
        }
        merged.sort_by(|a, b| a.speed.total_cmp(&b.speed));
        merged.truncate(MAX_ROWS);
        self.entries = merged;
    }

    /// Sets the offset at `speed` (adds the row, or changes the one there).
    pub fn set(&mut self, speed: f64, offset: f64) {
        self.entries.push(OffsetEntry { speed, offset });
        self.normalise();
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.entries.len() {
            self.entries.remove(index);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The offset at `speed`: linear between the neighbouring rows, the end rows' value beyond
    /// them, zero for an empty table.
    pub fn offset_at(&self, speed: f64) -> f64 {
        let e = &self.entries;
        match e.len() {
            0 => 0.0,
            1 => e[0].offset,
            _ => {
                if speed <= e[0].speed {
                    return e[0].offset;
                }
                if speed >= e[e.len() - 1].speed {
                    return e[e.len() - 1].offset;
                }
                let i = e.iter().position(|r| r.speed >= speed).unwrap_or(e.len() - 1).max(1);
                let (a, b) = (e[i - 1], e[i]);
                let t = (speed - a.speed) / (b.speed - a.speed);
                a.offset + (b.offset - a.offset) * t
            }
        }
    }

    /// The table as the text of a `.dbso` file.
    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(&serde_json::json!({ "format": FORMAT, "version": VERSION, "entries": self.entries })).expect("table serialises")
    }

    /// Reads a table from the text of a `.dbso` file.
    pub fn from_text(text: &str) -> Result<ScanOffsetTable, String> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not a scanning offset file: {e}"))?;
        if v.get("format").and_then(|f| f.as_str()) != Some(FORMAT) {
            return Err("not a scanning offset file".into());
        }
        if v.get("version").and_then(serde_json::Value::as_u64).is_some_and(|n| n > u64::from(VERSION)) {
            return Err("this scanning offset file is from a newer DarkBurn".into());
        }
        let entries: Vec<OffsetEntry> =
            serde_json::from_value(v.get("entries").cloned().unwrap_or_default()).map_err(|e| format!("the rows could not be read: {e}"))?;
        let mut t = ScanOffsetTable { entries };
        t.normalise();
        Ok(t)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.to_text()).map_err(|e| format!("could not write {}: {e}", path.display()))
    }

    pub fn load(path: &Path) -> Result<ScanOffsetTable, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        Self::from_text(&text)
    }
}

/// How image scans are tuned for the device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanTuning {
    /// Rapid over blank stretches instead of scanning through them.
    pub fast_skip: bool,
    /// Blank stretches longer than this (mm) are skipped.
    pub skip_mm: f64,
    pub offsets: ScanOffsetTable,
}

impl Default for ScanTuning {
    fn default() -> Self {
        ScanTuning { fast_skip: true, skip_mm: 5.0, offsets: ScanOffsetTable::default() }
    }
}

impl ScanTuning {
    pub fn blank_skip_mm(&self) -> Option<f64> {
        self.fast_skip.then_some(self.skip_mm.clamp(0.1, 1000.0))
    }

    /// Saves to `dir/scan_offsets.dbso`.
    pub fn save_to(&self, dir: &Path) -> Result<(), String> {
        let _ = std::fs::create_dir_all(dir);
        let text = serde_json::to_string_pretty(&serde_json::json!({
            "format": FORMAT, "version": VERSION, "fast_skip": self.fast_skip, "skip_mm": self.skip_mm, "entries": self.offsets.entries,
        }))
        .expect("tuning serialises");
        std::fs::write(dir.join(FILE), text).map_err(|e| format!("could not save the scanning offsets: {e}"))
    }

    /// Reads `dir/scan_offsets.dbso`; the defaults when there is none.
    pub fn load_from(dir: &Path) -> ScanTuning {
        let Ok(text) = std::fs::read_to_string(dir.join(FILE)) else { return ScanTuning::default() };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return ScanTuning::default() };
        let mut t = ScanTuning::default();
        if let Some(b) = v.get("fast_skip").and_then(serde_json::Value::as_bool) {
            t.fast_skip = b;
        }
        if let Some(n) = v.get("skip_mm").and_then(serde_json::Value::as_f64).filter(|n| n.is_finite()) {
            t.skip_mm = n.clamp(0.1, 1000.0);
        }
        if let Ok(table) = ScanOffsetTable::from_text(&text) {
            t.offsets = table;
        }
        t
    }
}

static CURRENT: RwLock<Option<ScanTuning>> = RwLock::new(None);

/// The tuning scans use now.
pub fn current() -> ScanTuning {
    CURRENT.read().ok().and_then(|g| g.clone()).unwrap_or_default()
}

/// Changes the tuning scans use.
pub fn set(t: ScanTuning) {
    if let Ok(mut g) = CURRENT.write() {
        *g = Some(t);
    }
}

/// Moves the lines of `plans` for the offset at `speed` (mm/s): forward lines back by half the
/// offset, reverse lines forward by half. Does nothing for unidirectional plans or a zero offset.
pub fn shift_plans(plans: &mut [ScanPlan], speed: f64, table: &ScanOffsetTable) {
    let half = table.offset_at(speed) / 2.0;
    if half == 0.0 {
        return;
    }
    for plan in plans {
        for line in &mut plan.lines {
            let d = if line.reverse { half } else { -half };
            for span in &mut line.spans {
                span.travel_start += d;
                span.travel_end += d;
                for r in &mut span.runs {
                    r.x0 += d;
                    r.x1 += d;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_raster::{ScanLine, ScanRun, ScanSpan};

    fn table() -> ScanOffsetTable {
        let mut t = ScanOffsetTable::default();
        t.set(300.0, 0.2);
        t.set(100.0, 0.05);
        t.set(200.0, 0.1);
        t
    }

    #[test]
    fn rows_are_sorted_and_interpolated() {
        let t = table();
        assert_eq!(t.entries.iter().map(|e| e.speed).collect::<Vec<_>>(), vec![100.0, 200.0, 300.0]);
        assert!((t.offset_at(150.0) - 0.075).abs() < 1e-9);
        assert_eq!(t.offset_at(10.0), 0.05);
        assert_eq!(t.offset_at(9000.0), 0.2);
        assert_eq!(ScanOffsetTable::default().offset_at(100.0), 0.0);
    }

    #[test]
    fn same_speed_replaces_and_junk_is_dropped() {
        let mut t = table();
        t.set(200.0, 0.5);
        assert_eq!(t.entries.len(), 3);
        assert_eq!(t.offset_at(200.0), 0.5);
        t.set(f64::NAN, 1.0);
        t.set(-5.0, 1.0);
        assert_eq!(t.entries.len(), 3);
    }

    #[test]
    fn files_round_trip_and_bad_ones_are_refused() {
        let t = table();
        assert_eq!(ScanOffsetTable::from_text(&t.to_text()).unwrap(), t);
        assert!(ScanOffsetTable::from_text("{}").is_err());
        assert!(ScanOffsetTable::from_text("nonsense").is_err());
        assert!(ScanOffsetTable::from_text(r#"{"format":"darkburn-scan-offsets","version":99,"entries":[]}"#).is_err());
        let dir = std::env::temp_dir().join(format!("darkburn-so-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(format!("a.{EXTENSION}"));
        t.save(&f).unwrap();
        assert_eq!(ScanOffsetTable::load(&f).unwrap(), t);
        let tuning = ScanTuning { fast_skip: false, skip_mm: 8.0, offsets: t };
        tuning.save_to(&dir).unwrap();
        assert_eq!(ScanTuning::load_from(&dir), tuning);
        assert_eq!(ScanTuning::load_from(&dir.join("none")), ScanTuning::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plans_shift_by_direction() {
        let line = |reverse| ScanLine {
            row: 0,
            y: 0.0,
            reverse,
            spans: vec![ScanSpan { runs: vec![ScanRun { x0: 1.0, x1: 2.0, power: 50.0 }], travel_start: 0.0, travel_end: 3.0 }],
        };
        let mut plans = vec![ScanPlan { angle_deg: 0.0, interval_mm: 0.1, lines: vec![line(false), line(true)] }];
        shift_plans(&mut plans, 200.0, &table());
        assert!((plans[0].lines[0].spans[0].runs[0].x0 - 0.95).abs() < 1e-9);
        assert!((plans[0].lines[1].spans[0].runs[0].x0 - 1.05).abs() < 1e-9);
        assert!((plans[0].lines[1].spans[0].travel_end - 3.05).abs() < 1e-9);
        let mut same = plans.clone();
        shift_plans(&mut same, 200.0, &ScanOffsetTable::default());
        assert_eq!(same, plans);
    }

    #[test]
    fn skip_setting_follows_the_switch() {
        let mut t = ScanTuning::default();
        assert_eq!(t.blank_skip_mm(), Some(5.0));
        t.fast_skip = false;
        assert_eq!(t.blank_skip_mm(), None);
    }
}
