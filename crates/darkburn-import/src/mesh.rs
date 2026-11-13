//! 3D model readers: STL (binary and ASCII) and Wavefront OBJ (vertices and faces).
//!
//! Both readers turn a file into a plain triangle soup in the file's own units (usually
//! millimetres). Materials, normals, texture coordinates, groups and colours are ignored.
//!
//! The readers are written for files from strangers: a [`MeshLimits`] budget caps the
//! triangles and vertices, no buffer is sized from a count in the file before that count is
//! checked against the budget and the file length, coordinates that are not finite numbers
//! are refused, and a file that stops in the middle of a record is an error.

use crate::ImportError;

/// One corner of a triangle: x, y, z.
pub type Vertex = [f32; 3];
/// Three corners.
pub type Triangle = [Vertex; 3];

/// How much a model file may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshLimits {
    /// The most triangles a model may have, after polygons are cut into triangles.
    pub max_triangles: usize,
    /// The most vertices an OBJ file may list.
    pub max_vertices: usize,
}

impl Default for MeshLimits {
    fn default() -> Self {
        MeshLimits { max_triangles: 4_000_000, max_vertices: 12_000_000 }
    }
}

/// A triangle soup.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub triangles: Vec<Triangle>,
}

impl Mesh {
    /// The smallest and largest x, y and z over every corner, or `None` for an empty mesh.
    pub fn bounds(&self) -> Option<(Vertex, Vertex)> {
        let first = self.triangles.first()?[0];
        let (mut lo, mut hi) = (first, first);
        for v in self.triangles.iter().flatten() {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
        Some((lo, hi))
    }

    /// Width, depth and height (x, y and z extent); zero for an empty mesh.
    pub fn size(&self) -> [f64; 3] {
        match self.bounds() {
            Some((lo, hi)) => [0, 1, 2].map(|k| f64::from(hi[k]) - f64::from(lo[k])),
            None => [0.0; 3],
        }
    }
}

/// The model formats read here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshFormat {
    Stl,
    Obj,
}

/// File extensions of 3D models, lower case.
pub const MESH_EXTENSIONS: [&str; 2] = ["stl", "obj"];

/// The model format for a file extension (any case), if it is one.
pub fn format_of(extension: &str) -> Option<MeshFormat> {
    match extension.to_ascii_lowercase().as_str() {
        "stl" => Some(MeshFormat::Stl),
        "obj" => Some(MeshFormat::Obj),
        _ => None,
    }
}

/// Reads a model of the given format.
pub fn read_mesh(format: MeshFormat, bytes: &[u8], limits: &MeshLimits) -> Result<Mesh, ImportError> {
    let mesh = match format {
        MeshFormat::Stl => read_stl(bytes, limits)?,
        MeshFormat::Obj => read_obj(bytes, limits)?,
    };
    if mesh.triangles.is_empty() {
        return Err(ImportError::Parse("the model has no triangles".into()));
    }
    Ok(mesh)
}

fn parse_err(msg: impl Into<String>) -> ImportError {
    ImportError::Parse(msg.into())
}

fn too_many(limit: usize) -> ImportError {
    ImportError::Unsupported(format!("the model has more than {limit} triangles; simplify it first"))
}

fn truncated() -> ImportError {
    parse_err("the file ends early; it is cut off or damaged")
}

fn finite(v: Vertex) -> Result<Vertex, ImportError> {
    if v.iter().all(|c| c.is_finite()) { Ok(v) } else { Err(parse_err("a coordinate is not a finite number")) }
}

// ---------------------------------------------------------------------------------------------
// STL

const STL_HEADER: usize = 80;
const STL_RECORD: u64 = 50;

/// Reads a binary or ASCII STL file.
///
/// A file whose length matches the triangle count in its header is binary, even when its
/// header starts with `solid` (many exporters write that). Other files that start with
/// `solid` are read as text; the rest are binary.
pub fn read_stl(bytes: &[u8], limits: &MeshLimits) -> Result<Mesh, ImportError> {
    let count = binary_count(bytes);
    let exact = count.is_some_and(|n| STL_HEADER as u64 + 4 + STL_RECORD * u64::from(n) == bytes.len() as u64);
    if !starts_with_solid(bytes) {
        return read_binary_stl(bytes, limits);
    }
    if exact {
        return read_binary_stl(bytes, limits).or_else(|e| read_ascii_stl(bytes, limits).map_err(|_| e));
    }
    match read_ascii_stl(bytes, limits) {
        Ok(m) => Ok(m),
        Err(e) => {
            // A binary file with "solid" in its header and padding after the last record.
            let fits = count.is_some_and(|n| n > 0 && STL_HEADER as u64 + 4 + STL_RECORD * u64::from(n) <= bytes.len() as u64);
            if fits && !looks_like_text(bytes) { read_binary_stl(bytes, limits) } else { Err(e) }
        }
    }
}

fn binary_count(bytes: &[u8]) -> Option<u32> {
    let b = bytes.get(STL_HEADER..STL_HEADER + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn starts_with_solid(bytes: &[u8]) -> bool {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    bytes[start..].get(..5).is_some_and(|w| w.eq_ignore_ascii_case(b"solid"))
}

fn looks_like_text(bytes: &[u8]) -> bool {
    bytes.iter().take(4096).all(|&b| b.is_ascii_graphic() || b.is_ascii_whitespace())
}

/// Reads a binary STL file: an 80-byte header, a little-endian triangle count, then 50 bytes per
/// triangle (normal, three corners, attribute word). Bytes after the last triangle are ignored.
pub fn read_binary_stl(bytes: &[u8], limits: &MeshLimits) -> Result<Mesh, ImportError> {
    let n = binary_count(bytes).ok_or_else(truncated)?;
    let n = usize::try_from(n).map_err(|_| too_many(limits.max_triangles))?;
    if n > limits.max_triangles {
        return Err(too_many(limits.max_triangles));
    }
    let need = STL_HEADER as u64 + 4 + STL_RECORD * n as u64;
    if (bytes.len() as u64) < need {
        return Err(truncated());
    }
    let mut triangles = Vec::with_capacity(n);
    for record in bytes[STL_HEADER + 4..].as_chunks::<{ STL_RECORD as usize }>().0.iter().take(n) {
        let f = |i: usize| {
            let o = 12 + i * 4;
            f32::from_le_bytes([record[o], record[o + 1], record[o + 2], record[o + 3]])
        };
        let corner = |c: usize| finite([f(c * 3), f(c * 3 + 1), f(c * 3 + 2)]);
        triangles.push([corner(0)?, corner(1)?, corner(2)?]);
    }
    Ok(Mesh { triangles })
}

/// Reads an ASCII STL file (`solid` ... `facet normal` ... `outer loop` ... `vertex x y z` ...
/// `endloop` `endfacet` ... `endsolid`). Several solids in one file are joined. A loop with
/// more than three corners is cut into a fan of triangles.
pub fn read_ascii_stl(bytes: &[u8], limits: &MeshLimits) -> Result<Mesh, ImportError> {
    #[derive(PartialEq)]
    enum At {
        Outside,
        Solid,
        Facet,
        Loop,
        LoopDone,
    }
    let mut at = At::Outside;
    let mut seen_solid = false;
    let mut corners: Vec<Vertex> = Vec::new();
    let mut triangles: Vec<Triangle> = Vec::new();
    for (number, raw) in bytes.split(|&b| b == b'\n').enumerate() {
        let line = String::from_utf8_lossy(raw);
        let mut words = line.split_ascii_whitespace();
        let Some(word) = words.next() else { continue };
        let word = word.to_ascii_lowercase();
        let bad = || parse_err(format!("unexpected \"{word}\" on line {}", number + 1));
        match (word.as_str(), &at) {
            ("solid", At::Outside) => {
                at = At::Solid;
                seen_solid = true;
            }
            ("facet", At::Solid) => at = At::Facet,
            ("outer", At::Facet) => {
                at = At::Loop;
                corners.clear();
            }
            ("vertex", At::Loop) => {
                let mut c = [0f32; 3];
                for slot in &mut c {
                    *slot = words
                        .next()
                        .and_then(|w| w.parse::<f32>().ok())
                        .ok_or_else(|| parse_err(format!("a vertex on line {} needs three numbers", number + 1)))?;
                }
                if triangles.len() + corners.len().saturating_sub(1) > limits.max_triangles {
                    return Err(too_many(limits.max_triangles));
                }
                corners.push(finite(c)?);
            }
            ("endloop", At::Loop) => {
                if corners.len() < 3 {
                    return Err(parse_err(format!("a facet ending on line {} has fewer than three corners", number + 1)));
                }
                if triangles.len() + corners.len() - 2 > limits.max_triangles {
                    return Err(too_many(limits.max_triangles));
                }
                for k in 1..corners.len() - 1 {
                    triangles.push([corners[0], corners[k], corners[k + 1]]);
                }
                at = At::LoopDone;
            }
            ("endfacet", At::LoopDone) => at = At::Solid,
            ("endsolid", At::Solid) => at = At::Outside,
            _ => return Err(bad()),
        }
    }
    if !seen_solid || at != At::Outside {
        return Err(truncated());
    }
    Ok(Mesh { triangles })
}

/// Writes a binary STL file (normals left at zero, as readers recompute them).
pub fn write_binary_stl(mesh: &Mesh) -> Vec<u8> {
    let mut out = Vec::with_capacity(84 + 50 * mesh.triangles.len());
    out.extend_from_slice(&[b' '; STL_HEADER]);
    out.extend_from_slice(&u32::try_from(mesh.triangles.len()).unwrap_or(u32::MAX).to_le_bytes());
    for t in mesh.triangles.iter().take(u32::MAX as usize) {
        out.extend_from_slice(&[0u8; 12]);
        for c in t.iter().flatten() {
            out.extend_from_slice(&c.to_le_bytes());
        }
        out.extend_from_slice(&[0u8; 2]);
    }
    out
}

/// Writes an ASCII STL file. Numbers are written so they read back exactly.
pub fn write_ascii_stl(mesh: &Mesh, name: &str) -> String {
    let mut out = format!("solid {name}\n");
    for t in &mesh.triangles {
        out.push_str("  facet normal 0 0 0\n    outer loop\n");
        for v in t {
            out.push_str(&format!("      vertex {:e} {:e} {:e}\n", v[0], v[1], v[2]));
        }
        out.push_str("    endloop\n  endfacet\n");
    }
    out.push_str(&format!("endsolid {name}\n"));
    out
}

// ---------------------------------------------------------------------------------------------
// OBJ

/// Reads the geometry of a Wavefront OBJ file: `v` lines and `f` lines. Faces with more than
/// three corners are cut into a fan of triangles; negative (relative) indices are understood;
/// texture and normal indices (`a/b/c`, `a//c`) and every other line are ignored.
pub fn read_obj(bytes: &[u8], limits: &MeshLimits) -> Result<Mesh, ImportError> {
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut triangles: Vec<Triangle> = Vec::new();
    let mut face: Vec<Vertex> = Vec::new();
    for (number, raw) in bytes.split(|&b| b == b'\n').enumerate() {
        let line = String::from_utf8_lossy(raw);
        let line = line.split('#').next().unwrap_or("");
        let mut words = line.split_ascii_whitespace();
        match words.next() {
            Some("v") => {
                if vertices.len() >= limits.max_vertices {
                    return Err(ImportError::Unsupported(format!("the model has more than {} vertices; simplify it first", limits.max_vertices)));
                }
                let mut c = [0f32; 3];
                for slot in &mut c {
                    *slot = words
                        .next()
                        .and_then(|w| w.parse::<f32>().ok())
                        .ok_or_else(|| parse_err(format!("the vertex on line {} needs three numbers", number + 1)))?;
                }
                vertices.push(finite(c)?);
            }
            Some("f") => {
                face.clear();
                for w in words {
                    let index = w.split('/').next().unwrap_or("");
                    let i: i64 = index.parse().map_err(|_| parse_err(format!("bad face index \"{w}\" on line {}", number + 1)))?;
                    let count = vertices.len() as i64;
                    let at = if i > 0 { i - 1 } else { count + i };
                    if i == 0 || at < 0 || at >= count {
                        return Err(parse_err(format!("face index {i} on line {} points at no vertex", number + 1)));
                    }
                    if face.len() > limits.max_triangles {
                        return Err(too_many(limits.max_triangles));
                    }
                    face.push(vertices[at as usize]);
                }
                if face.len() < 3 {
                    return Err(parse_err(format!("the face on line {} has fewer than three corners", number + 1)));
                }
                if triangles.len() + face.len() - 2 > limits.max_triangles {
                    return Err(too_many(limits.max_triangles));
                }
                for k in 1..face.len() - 1 {
                    triangles.push([face[0], face[k], face[k + 1]]);
                }
            }
            _ => {}
        }
    }
    Ok(Mesh { triangles })
}
