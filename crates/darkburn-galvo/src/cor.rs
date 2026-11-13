//! `.cor` lens correction files.
//!
//! A `.cor` file holds a 65 x 65 grid of (dx, dy) offsets, one node every 1024 galvo units
//! over the 0..65535 field. Two layouts exist (both as read by MeerK40t's `balormk`, MIT):
//!
//! * `LMC1COR_1.0`: 22-byte UTF-16 label, 0x1FA header bytes, then 4225 pairs of `f64`. Header
//!   byte offset 2 starts 63 doubles; element 43 is the lens scale.
//! * older files: 22-byte label, 14 header bytes (6 unknown, then an `f64` scale), then 4225
//!   pairs of little-endian `i32`.
//!
//! On the board a negative offset is sent as `0x8000 + |v|` (see [`encode_offset`]).
//! The board applies the table itself once uploaded ([`CorTable::upload_commands`]); the host
//! side [`CorTable::correct`] exists for previews, tests and boards without a table.

use std::fmt;

use crate::codec::SingleCmd;

/// Nodes per axis.
pub const GRID: usize = 65;
/// Distance between nodes in galvo units (65535 / 64).
pub const NODE_SPACING: f64 = 65535.0 / 64.0;

const LABEL_LEN: usize = 0x16;
const FLOAT_HEADER: usize = 0x1FA;
const INT_HEADER: usize = 0xE;

/// Errors from reading a `.cor` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CorError {
    /// File shorter than its layout needs.
    Truncated { needed: usize, got: usize },
    /// A node holds an offset the board has no way to take: more than 15 bits, or (in the
    /// float layout) not a number. `row` and `col` count from 0.
    OutOfRange { row: usize, col: usize },
}

impl fmt::Display for CorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { needed, got } => write!(f, "The lens correction file is too short ({got} bytes, needs {needed})."),
            Self::OutOfRange { row, col } => {
                write!(f, "The lens correction file holds an offset the board cannot take (row {}, column {}).", row + 1, col + 1)
            }
        }
    }
}
impl std::error::Error for CorError {}

/// The two on-disk layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CorFormat {
    /// `LMC1COR_1.0`, `f64` nodes.
    Float,
    /// Older, `i32` nodes.
    Int,
}

/// Largest offset the board takes, either way: 15 bits beside the sign bit.
pub const OFFSET_MAX: i32 = 0x7FFF;

/// Board encoding of one signed offset: negatives become `0x8000 + |v|`. An offset beyond
/// [`OFFSET_MAX`] is sent as the largest one of its sign, never with its top bits cut off
/// (which would turn its sign round).
pub fn encode_offset(v: i32) -> u16 {
    let size = v.unsigned_abs().min(OFFSET_MAX as u32) as u16;
    if v >= 0 { size } else { 0x8000 | size }
}

/// Inverse of [`encode_offset`] for values written by it.
pub fn decode_offset(w: u16) -> i32 {
    if w & 0x8000 != 0 { -((w & 0x7FFF) as i32) } else { w as i32 }
}

/// A 65 x 65 lens correction table.
#[derive(Clone, Debug, PartialEq)]
pub struct CorTable {
    /// Row-major, `nodes[row * 65 + col]`, offsets in galvo units. Row = Y, column = X.
    nodes: Vec<(i32, i32)>,
    /// Lens scale stored in the file, when present.
    pub scale: Option<f64>,
    /// Layout the table was read from (used when writing it back).
    pub format: CorFormat,
}

impl CorTable {
    /// A table that changes nothing.
    pub fn identity() -> Self {
        Self { nodes: vec![(0, 0); GRID * GRID], scale: None, format: CorFormat::Float }
    }

    /// Builds a table from a generator called with `(row, col)`.
    pub fn from_fn(mut f: impl FnMut(usize, usize) -> (i32, i32)) -> Self {
        let mut nodes = Vec::with_capacity(GRID * GRID);
        for r in 0..GRID {
            for c in 0..GRID {
                nodes.push(f(r, c));
            }
        }
        Self { nodes, scale: None, format: CorFormat::Float }
    }

    pub fn node(&self, row: usize, col: usize) -> (i32, i32) {
        self.nodes[row * GRID + col]
    }

    pub fn set_node(&mut self, row: usize, col: usize, v: (i32, i32)) {
        self.nodes[row * GRID + col] = v;
    }

    pub fn is_identity(&self) -> bool {
        self.nodes.iter().all(|&n| n == (0, 0))
    }

    /// Parses a `.cor` file.
    pub fn parse(bytes: &[u8]) -> Result<Self, CorError> {
        let need = |n: usize| if bytes.len() < n { Err(CorError::Truncated { needed: n, got: bytes.len() }) } else { Ok(()) };
        need(LABEL_LEN)?;
        let label: Vec<u16> = bytes[..LABEL_LEN].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let is_float = String::from_utf16_lossy(&label) == "LMC1COR_1.0";
        let f64_at = |o: usize| f64::from_le_bytes(bytes[o..o + 8].try_into().expect("8 bytes"));
        let out_of_range = |i: usize| CorError::OutOfRange { row: i / GRID, col: i % GRID };
        let mut nodes = Vec::with_capacity(GRID * GRID);
        if is_float {
            let start = LABEL_LEN + FLOAT_HEADER;
            need(start + GRID * GRID * 16)?;
            let scale = f64_at(LABEL_LEN + 2 + 43 * 8);
            for i in 0..GRID * GRID {
                let o = start + i * 16;
                // "Not a number" would read as 0 and anything huge as the largest number.
                let (dx, dy) = (f64_at(o).round(), f64_at(o + 8).round());
                if !(dx.abs() <= OFFSET_MAX as f64 && dy.abs() <= OFFSET_MAX as f64) {
                    return Err(out_of_range(i));
                }
                nodes.push((dx as i32, dy as i32));
            }
            Ok(Self { nodes, scale: scale.is_finite().then_some(scale), format: CorFormat::Float })
        } else {
            let start = LABEL_LEN + INT_HEADER;
            need(start + GRID * GRID * 8)?;
            let scale = f64_at(LABEL_LEN + 6);
            let i32_at = |o: usize| i32::from_le_bytes(bytes[o..o + 4].try_into().expect("4 bytes"));
            for i in 0..GRID * GRID {
                let o = start + i * 8;
                let (dx, dy) = (i32_at(o), i32_at(o + 4));
                if dx.unsigned_abs() > OFFSET_MAX as u32 || dy.unsigned_abs() > OFFSET_MAX as u32 {
                    return Err(out_of_range(i));
                }
                nodes.push((dx, dy));
            }
            Ok(Self { nodes, scale: scale.is_finite().then_some(scale), format: CorFormat::Int })
        }
    }

    /// Writes the table in its own layout.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self.format {
            CorFormat::Float => {
                for u in "LMC1COR_1.0".encode_utf16() {
                    out.extend_from_slice(&u.to_le_bytes());
                }
                let mut header = vec![0u8; FLOAT_HEADER];
                if let Some(s) = self.scale {
                    header[2 + 43 * 8..2 + 44 * 8].copy_from_slice(&s.to_le_bytes());
                }
                out.extend_from_slice(&header);
                for &(dx, dy) in &self.nodes {
                    out.extend_from_slice(&(dx as f64).to_le_bytes());
                    out.extend_from_slice(&(dy as f64).to_le_bytes());
                }
            }
            CorFormat::Int => {
                out.extend_from_slice(&[0u8; LABEL_LEN]);
                let mut header = vec![0u8; INT_HEADER];
                if let Some(s) = self.scale {
                    header[6..14].copy_from_slice(&s.to_le_bytes());
                }
                out.extend_from_slice(&header);
                for &(dx, dy) in &self.nodes {
                    out.extend_from_slice(&dx.to_le_bytes());
                    out.extend_from_slice(&dy.to_le_bytes());
                }
            }
        }
        out
    }

    /// The board words for every node, in upload order.
    pub fn hardware_words(&self) -> Vec<(u16, u16)> {
        self.nodes.iter().map(|&(dx, dy)| (encode_offset(dx), encode_offset(dy))).collect()
    }

    /// Commands that load this table: `WriteCorTable(true)` then 4225 `WriteCorLine`.
    pub fn upload_commands(&self) -> Vec<SingleCmd> {
        let mut v = vec![SingleCmd::WriteCorTable(true)];
        for (i, (dx, dy)) in self.hardware_words().into_iter().enumerate() {
            v.push(SingleCmd::WriteCorLine { dx, dy, non_first: (i != 0) as u16 });
        }
        v
    }

    /// Commands that clear the lens table (blank table).
    pub fn clear_commands() -> Vec<SingleCmd> {
        vec![SingleCmd::WriteCorTable(false)]
    }

    /// Offset at a galvo position, bilinear between the four surrounding nodes.
    pub fn offset_at(&self, x: f64, y: f64) -> (f64, f64) {
        let fx = (x / NODE_SPACING).clamp(0.0, (GRID - 1) as f64);
        let fy = (y / NODE_SPACING).clamp(0.0, (GRID - 1) as f64);
        let c0 = (fx.floor() as usize).min(GRID - 2);
        let r0 = (fy.floor() as usize).min(GRID - 2);
        let tx = fx - c0 as f64;
        let ty = fy - r0 as f64;
        let n = |r: usize, c: usize| {
            let (a, b) = self.node(r, c);
            (a as f64, b as f64)
        };
        let (a, b, c, d) = (n(r0, c0), n(r0, c0 + 1), n(r0 + 1, c0), n(r0 + 1, c0 + 1));
        let lerp = |p: f64, q: f64, t: f64| p + (q - p) * t;
        let dx = lerp(lerp(a.0, b.0, tx), lerp(c.0, d.0, tx), ty);
        let dy = lerp(lerp(a.1, b.1, tx), lerp(c.1, d.1, tx), ty);
        (dx, dy)
    }

    /// Applies the correction on the host: `position + offset`.
    pub fn correct(&self, x: f64, y: f64) -> (f64, f64) {
        let (dx, dy) = self.offset_at(x, y);
        (x + dx, y + dy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_encoding_round_trips() {
        for v in [-300, -1, 0, 1, 300, 0x7FFF, -0x7FFF] {
            assert_eq!(decode_offset(encode_offset(v)), v);
        }
        assert_eq!(encode_offset(-5), 0x8005);
        assert_eq!(encode_offset(5), 5);
    }

    #[test]
    fn float_file_round_trip() {
        let mut t = CorTable::from_fn(|r, c| (r as i32 - 32, -(c as i32) + 7));
        t.scale = Some(1.0123);
        let bytes = t.to_bytes();
        assert_eq!(bytes.len(), 22 + 0x1FA + 4225 * 16);
        let back = CorTable::parse(&bytes).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn int_file_round_trip() {
        let mut t = CorTable::from_fn(|r, c| (c as i32, -(r as i32)));
        t.format = CorFormat::Int;
        t.scale = Some(0.998);
        let bytes = t.to_bytes();
        assert_eq!(bytes.len(), 22 + 14 + 4225 * 8);
        let back = CorTable::parse(&bytes).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn truncated_files_are_rejected() {
        let t = CorTable::identity().to_bytes();
        assert!(matches!(CorTable::parse(&t[..t.len() - 1]), Err(CorError::Truncated { .. })));
        assert!(CorTable::parse(&[1, 2, 3]).is_err());
    }

    #[test]
    fn upload_sequence_shape() {
        let cmds = CorTable::identity().upload_commands();
        assert_eq!(cmds.len(), 1 + 4225);
        assert_eq!(cmds[0], SingleCmd::WriteCorTable(true));
        assert_eq!(cmds[1], SingleCmd::WriteCorLine { dx: 0, dy: 0, non_first: 0 });
        assert_eq!(cmds[2], SingleCmd::WriteCorLine { dx: 0, dy: 0, non_first: 1 });
        assert_eq!(CorTable::clear_commands(), vec![SingleCmd::WriteCorTable(false)]);
    }

    #[test]
    fn bilinear_correction() {
        // dx grows 10 units per column, dy 20 per row.
        let t = CorTable::from_fn(|r, c| (c as i32 * 10, r as i32 * 20));
        let (dx, dy) = t.offset_at(0.0, 0.0);
        assert_eq!((dx, dy), (0.0, 0.0));
        let (dx, dy) = t.offset_at(NODE_SPACING * 2.5, NODE_SPACING * 4.0);
        assert!((dx - 25.0).abs() < 1e-6 && (dy - 80.0).abs() < 1e-6);
        // Far corner clamps to the last node.
        let (dx, dy) = t.offset_at(65535.0, 65535.0);
        assert!((dx - 640.0).abs() < 1e-6 && (dy - 1280.0).abs() < 1e-6);
        let (x, y) = t.correct(NODE_SPACING, NODE_SPACING);
        assert!((x - (NODE_SPACING + 10.0)).abs() < 1e-6 && (y - (NODE_SPACING + 20.0)).abs() < 1e-6);
        assert!(CorTable::identity().is_identity() && !t.is_identity());
    }
}
