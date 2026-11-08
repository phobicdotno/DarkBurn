//! Pure ordered option packing. Composite descendants belong to their parent's single cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Width {
    Narrow,
    Composite,
    Wide,
}

pub struct Entry {
    pub width: Width,
    pub advanced: bool,
    /// Measured minimum row width, including translated labels and controls.
    pub minimum: f32,
}

#[derive(Debug, PartialEq)]
pub struct Plan {
    pub rows: Vec<Vec<usize>>,
    pub more: Vec<Vec<usize>>,
    pub more_count: usize,
    pub section_labels: bool,
}

pub fn choice_width(values: usize) -> Width {
    if (4..=5).contains(&values) { Width::Wide } else { Width::Narrow }
}

/// `width` is the usable width after the caller subtracts the inter-column gutter.
pub fn layout(entries: &[Entry], width: f32) -> Plan {
    let mut main = Vec::new();
    let mut more = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if entry.advanced || main.len() >= 10 {
            more.push(i);
        } else {
            main.push(i);
        }
    }
    let pack = |indices: &[usize]| {
        let mut rows: Vec<Vec<usize>> = Vec::new();
        let mut pending = Vec::new();
        for &i in indices {
            let Some(entry) = entries.get(i) else {
                continue;
            };
            let wide = entry.width == Width::Wide || !width.is_finite() || entry.minimum > width.max(0.0) / 2.0;
            if wide {
                if !pending.is_empty() {
                    rows.push(std::mem::take(&mut pending));
                }
                rows.push(vec![i]);
            } else {
                pending.push(i);
                if pending.len() == 2 {
                    rows.push(std::mem::take(&mut pending));
                }
            }
        }
        if !pending.is_empty() {
            rows.push(pending);
        }
        rows
    };
    Plan { rows: pack(&main), more: pack(&more), more_count: more.len(), section_labels: entries.len() > 6 }
}
