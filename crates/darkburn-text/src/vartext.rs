//! Variable text engine: pure functions over a serialisable [`VariableState`].
//!
//! A template is plain text with `{token}` placeholders (`{{` and `}}` give literal braces):
//!
//! | token | meaning |
//! |---|---|
//! | `{serial}` / `{serial:6}` | serial number, zero padded to the configured (or given) width |
//! | `{date}` `{time}` `{now}` + `:FORMAT` | current date / time through strftime codes, e.g. `{date:%d.%m.%Y}`, `{now:%Y-%m-%d %H:%M}` |
//! | `{csv:2}` `{csv:Name}` `{csv}` | column of the current merge row (0-based; header names when the file has a header; bare `{csv}` is column 0) |
//! | `{cut:S}` `P` `I` `N` `C` `L` | cut setting values: speed, power, interval, passes, layer, laser name (`L6,2` = 2 characters of the laser name starting at character 6, 0-based). Letters are case-insensitive |
//!
//! Unknown tokens are left in the output exactly as written so mistakes are visible.
//! For a whole text object of type [`VarKind::DateTime`] the entire text is a strftime format.

use crate::error::{Result, TextError};
use chrono::format::{Item, StrftimeItems};
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

/// Variable-text type of a text object (the "type" drop-down).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VarKind {
    #[default]
    Normal,
    /// The whole text is a strftime format.
    DateTime,
    Serial,
    Merge,
    CutSetting,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialConfig {
    pub start: i64,
    pub end: i64,
    pub current: i64,
    pub advance_by: i64,
    /// Minimum digits (zero padded).
    pub padding: usize,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self { start: 1, end: 9999, current: 1, advance_by: 1, padding: 0 }
    }
}

impl SerialConfig {
    fn wrap_next(&self) -> i64 {
        let c = self.current.saturating_add(self.advance_by);
        let (lo, hi) = (self.start.min(self.end), self.start.max(self.end));
        if self.advance_by >= 0 {
            if c > hi { lo } else { c }
        } else if c < lo {
            hi
        } else {
            c
        }
    }
    fn wrap_prev(&self) -> i64 {
        let c = self.current.saturating_sub(self.advance_by);
        let (lo, hi) = (self.start.min(self.end), self.start.max(self.end));
        if self.advance_by > 0 {
            if c < lo {
                // Last value reachable from start in whole steps.
                lo + (hi - lo) / self.advance_by * self.advance_by
            } else {
                c
            }
        } else if self.advance_by < 0 {
            if c > hi { hi + (lo - hi) / self.advance_by * self.advance_by } else { c }
        } else {
            c
        }
    }
}

/// Rows of a CSV merge file kept inside the project so it still works when the file is gone.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MergeSource {
    /// Where the data came from (shown in the UI, used by [`MergeSource::reload`]).
    pub path: Option<String>,
    pub header: bool,
    /// All rows including the header row when `header` is set.
    pub rows: Vec<Vec<String>>,
    /// Current data row (0 = first row after the header).
    pub current_row: usize,
}

impl MergeSource {
    /// Parse CSV text: quoted fields, doubled quotes, line breaks inside quotes; `,` `;` or tab
    /// separated (detected from the first line).
    pub fn from_csv_str(text: &str, header: bool) -> Result<MergeSource> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let first_line = text.lines().next().unwrap_or("");
        let count = |c: char| {
            let mut q = false;
            first_line
                .chars()
                .filter(|ch| {
                    if *ch == '"' {
                        q = !q;
                    }
                    !q && *ch == c
                })
                .count()
        };
        let delim = [(count(','), b','), (count(';'), b';'), (count('\t'), b'\t')].into_iter().max_by_key(|x| x.0).map(|x| x.1).unwrap_or(b',');
        let mut rdr = csv::ReaderBuilder::new().has_headers(false).flexible(true).delimiter(delim).from_reader(text.as_bytes());
        let mut rows = Vec::new();
        for rec in rdr.records() {
            let rec = rec.map_err(|e| TextError::Variable(format!("CSV: {e}")))?;
            let row: Vec<String> = rec.iter().map(|s| s.to_string()).collect();
            if row.len() == 1 && row[0].is_empty() {
                continue; // blank line
            }
            rows.push(row);
        }
        Ok(MergeSource { path: None, header, rows, current_row: 0 })
    }

    /// Load from a file (UTF-8, invalid bytes replaced).
    pub fn load(path: impl AsRef<std::path::Path>, header: bool) -> Result<MergeSource> {
        let p = path.as_ref();
        let bytes = std::fs::read(p)?;
        let mut m = Self::from_csv_str(&String::from_utf8_lossy(&bytes), header)?;
        m.path = Some(p.to_string_lossy().into_owned());
        Ok(m)
    }

    /// Re-read the stored path (keeps the current row when still in range).
    pub fn reload(&self) -> Result<MergeSource> {
        let p = self.path.as_ref().ok_or_else(|| TextError::Variable("no CSV file set".into()))?;
        let mut m = Self::load(p, self.header)?;
        m.current_row = self.current_row.min(m.row_count().saturating_sub(1));
        Ok(m)
    }

    /// Point at a moved file and load it (file relocation).
    pub fn relocate(&self, new_path: impl AsRef<std::path::Path>) -> Result<MergeSource> {
        let mut m = Self::load(new_path, self.header)?;
        m.current_row = self.current_row.min(m.row_count().saturating_sub(1));
        Ok(m)
    }

    /// Forget the file and its rows ("Clear").
    pub fn cleared(&self) -> MergeSource {
        MergeSource { header: self.header, ..Default::default() }
    }

    fn data_rows(&self) -> &[Vec<String>] {
        if self.header && !self.rows.is_empty() { &self.rows[1..] } else { &self.rows[..] }
    }

    /// Number of data rows.
    pub fn row_count(&self) -> usize {
        self.data_rows().len()
    }

    /// Header names (empty when the file has no header).
    pub fn column_names(&self) -> Vec<String> {
        if self.header { self.rows.first().cloned().unwrap_or_default() } else { Vec::new() }
    }

    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.column_names().iter().position(|n| n.trim().eq_ignore_ascii_case(name.trim()))
    }

    /// Value of column `col` in data row `row`.
    pub fn value(&self, row: usize, col: usize) -> Option<&str> {
        self.data_rows().get(row)?.get(col).map(|s| s.as_str())
    }
}

/// Cut setting values of the layer a text object sits on.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CutValues {
    /// mm/s
    pub speed: f64,
    /// percent
    pub power: f64,
    /// mm
    pub interval: f64,
    pub passes: u32,
    /// Layer number.
    pub layer: u32,
    pub laser_name: String,
}

/// Everything the evaluator needs besides the state: the clock and the cut settings (pure inputs).
#[derive(Clone, Debug)]
pub struct EvalContext {
    pub now: NaiveDateTime,
    pub cut: Option<CutValues>,
}

impl EvalContext {
    pub fn new(now: NaiveDateTime) -> Self {
        Self { now, cut: None }
    }
    /// Context using the local clock.
    pub fn local_now() -> Self {
        Self::new(chrono::Local::now().naive_local())
    }
    pub fn with_cut(mut self, cut: CutValues) -> Self {
        self.cut = Some(cut);
        self
    }
}

/// The variable-text state of a project (shared by all variable text objects).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VariableState {
    pub serial: SerialConfig,
    pub merge: MergeSource,
    /// Advance to the next index after a job is sent.
    pub auto_advance: bool,
}

fn fmt_num(v: f64) -> String {
    if v.fract().abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// strftime with validation: an invalid format yields the format text itself.
pub fn format_time(now: &NaiveDateTime, fmt: &str) -> String {
    let items: Vec<Item> = StrftimeItems::new(fmt).collect();
    if items.iter().any(|i| matches!(i, Item::Error)) {
        return fmt.to_string();
    }
    now.format_with_items(items.into_iter()).to_string()
}

impl VariableState {
    // ---- index navigation (all pure: return the new state) ----

    /// "Next": serial and merge row advance by one step, wrapping at the end.
    pub fn next(&self) -> VariableState {
        let mut s = self.clone();
        s.serial.current = self.serial.wrap_next();
        let n = self.merge.row_count();
        if n > 0 {
            s.merge.current_row = (self.merge.current_row + 1) % n;
        }
        s
    }

    /// "Previous".
    pub fn previous(&self) -> VariableState {
        let mut s = self.clone();
        s.serial.current = self.serial.wrap_prev();
        let n = self.merge.row_count();
        if n > 0 {
            s.merge.current_row = (self.merge.current_row + n - 1) % n;
        }
        s
    }

    /// Advance `n` times (copy `n` of an array; also used by virtual arrays).
    pub fn advanced(&self, n: usize) -> VariableState {
        (0..n).fold(self.clone(), |s, _| s.next())
    }

    /// "Reset": back to the start values and the first row.
    pub fn reset(&self) -> VariableState {
        let mut s = self.clone();
        s.serial.current = self.serial.start;
        s.merge.current_row = 0;
        s
    }

    /// Called after a job is sent: advances only when auto-advance is on and the job contains variable text.
    pub fn after_job(&self, job_contains_variable_text: bool) -> VariableState {
        if self.auto_advance && job_contains_variable_text { self.next() } else { self.clone() }
    }

    /// "Test": the next `count` outputs of `template` starting at the current index, without changing the state.
    pub fn test(&self, template: &str, ctx: &EvalContext, count: usize) -> Vec<String> {
        let mut s = self.clone();
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(s.evaluate(template, ctx));
            s = s.next();
        }
        out
    }

    /// "Bake": the plain text a variable text object turns into when converted to a normal shape.
    pub fn bake(&self, template: &str, ctx: &EvalContext) -> String {
        self.evaluate(template, ctx)
    }

    // ---- evaluation ----

    /// Evaluate a text object of a given kind.
    pub fn evaluate_kind(&self, kind: VarKind, text: &str, ctx: &EvalContext) -> String {
        match kind {
            VarKind::DateTime => format_time(&ctx.now, &self.evaluate(text, ctx)),
            _ => self.evaluate(text, ctx),
        }
    }

    /// Replace every `{token}` in `template`.
    pub fn evaluate(&self, template: &str, ctx: &EvalContext) -> String {
        let mut out = String::with_capacity(template.len());
        let mut it = template.char_indices().peekable();
        while let Some((i, c)) = it.next() {
            match c {
                '{' => {
                    if let Some((_, '{')) = it.peek() {
                        it.next();
                        out.push('{');
                        continue;
                    }
                    match template[i + 1..].find('}') {
                        Some(rel) => {
                            let tok = &template[i + 1..i + 1 + rel];
                            match self.token(tok, ctx) {
                                Some(v) => out.push_str(&v),
                                None => {
                                    out.push('{');
                                    out.push_str(tok);
                                    out.push('}');
                                }
                            }
                            // Skip the token text and the closing brace.
                            let end = i + 1 + rel + 1;
                            while it.peek().is_some_and(|(j, _)| *j < end) {
                                it.next();
                            }
                        }
                        None => out.push('{'),
                    }
                }
                '}' => {
                    if let Some((_, '}')) = it.peek() {
                        it.next();
                    }
                    out.push('}');
                }
                _ => out.push(c),
            }
        }
        out
    }

    fn token(&self, tok: &str, ctx: &EvalContext) -> Option<String> {
        let (name, arg) = match tok.split_once(':') {
            Some((n, a)) => (n.trim(), Some(a)),
            None => (tok.trim(), None),
        };
        match name.to_ascii_lowercase().as_str() {
            "serial" | "n" => {
                let width = match arg {
                    Some(a) => a.trim().parse::<usize>().ok()?,
                    None => self.serial.padding,
                };
                Some(format!("{:0width$}", self.serial.current, width = width.min(64)))
            }
            "date" => Some(format_time(&ctx.now, arg.unwrap_or("%Y-%m-%d"))),
            "time" => Some(format_time(&ctx.now, arg.unwrap_or("%H:%M:%S"))),
            "now" => Some(format_time(&ctx.now, arg.unwrap_or("%Y-%m-%d %H:%M:%S"))),
            "csv" | "merge" => {
                let col = match arg.map(str::trim) {
                    None | Some("") => 0,
                    Some(a) => match a.parse::<usize>() {
                        Ok(n) => n,
                        Err(_) => self.merge.column_index(a)?,
                    },
                };
                if self.merge.row_count() == 0 {
                    return Some(String::new());
                }
                Some(self.merge.value(self.merge.current_row, col).unwrap_or("").to_string())
            }
            "cut" => {
                let cut = ctx.cut.as_ref()?;
                let a = arg?.trim();
                let letter = a.chars().next()?.to_ascii_uppercase();
                let rest = &a[a.chars().next()?.len_utf8()..];
                match letter {
                    'S' => Some(fmt_num(cut.speed)),
                    'P' => Some(fmt_num(cut.power)),
                    'I' => Some(fmt_num(cut.interval)),
                    'N' => Some(cut.passes.to_string()),
                    'C' => Some(cut.layer.to_string()),
                    'L' => {
                        if rest.is_empty() {
                            Some(cut.laser_name.clone())
                        } else {
                            let (s, l) = rest.split_once(',')?;
                            let (s, l): (usize, usize) = (s.trim().parse().ok()?, l.trim().parse().ok()?);
                            Some(cut.laser_name.chars().skip(s).take(l).collect())
                        }
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
}
