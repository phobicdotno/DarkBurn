//! Toolpath intermediate representation: a flat list of machine operations.

use kurbo::Point;
use serde::{Deserialize, Serialize};

use crate::input::CutMode;

/// Which coordinate system the toolpath is in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoordSpace {
    /// Design coordinates (mm, Y up, origin bottom-left of the bed).
    #[default]
    Design,
    /// Absolute machine coordinates.
    MachineAbsolute,
    /// Machine axes, relative to where the head is when the job starts (start = 0,0).
    RelativeToStart,
}

/// Settings of one layer (or sub-layer) as the machine needs them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IrLayer {
    /// Colour index of the source layer.
    pub layer_index: usize,
    /// 0 = main settings, 1.. = sub-layers.
    pub sublayer: usize,
    pub name: String,
    pub mode: CutMode,
    pub speed: f64,
    pub max_power: f64,
    pub min_power: f64,
    pub passes: u32,
    pub constant_power: bool,
    pub air: bool,
    pub laser: u8,
    /// Scanning layer (fill or image): uses engraving acceleration and raster decimals.
    pub raster: bool,
    /// DSP-only settings (PWM frequency, PPI, U offset, laser 2 power, dot mode).
    #[serde(default)]
    pub dsp: crate::input::DspSettings,
    /// Dot mode of a line layer: dwell time (ms) and spacing (mm).
    #[serde(default)]
    pub dot: Option<(f64, f64)>,
}

/// One machine operation. Powers are percent, speeds mm/s, times ms.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Op {
    /// Start of a layer; `layer` indexes [`Toolpath::layers`].
    LayerStart {
        layer: usize,
    },
    /// Start of pass `pass` (1-based) of the current layer.
    PassStart {
        pass: u32,
    },
    /// Laser off move at rapid speed.
    Rapid {
        to: Point,
    },
    /// Straight move with the laser at `power` (0 = laser off at cut speed).
    Cut {
        to: Point,
        speed: f64,
        power: f64,
    },
    /// Laser off move at scan speed past the end of a scan line.
    Overscan {
        to: Point,
        speed: f64,
    },
    /// Circular arc from the current position.
    Arc {
        to: Point,
        center: Point,
        clockwise: bool,
        speed: f64,
        power: f64,
    },
    /// Stand still for `ms`, firing at `power` (0 = off).
    Dwell {
        ms: f64,
        power: f64,
    },
    Air {
        on: bool,
    },
    /// Select laser 1 or 2.
    Laser {
        index: u8,
    },
    /// Absolute Z position, mm.
    ZMove {
        z: f64,
    },
    Comment {
        text: String,
    },
}

impl Op {
    /// End point of a motion op.
    pub fn target(&self) -> Option<Point> {
        match self {
            Op::Rapid { to } | Op::Cut { to, .. } | Op::Overscan { to, .. } | Op::Arc { to, .. } => Some(*to),
            _ => None,
        }
    }

    /// True for operations that move the head in X/Y.
    pub fn is_motion(&self) -> bool {
        self.target().is_some()
    }
}

/// The generated job.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Toolpath {
    pub space: CoordSpace,
    pub layers: Vec<IrLayer>,
    pub ops: Vec<Op>,
    /// Where the head is assumed to be before the first op.
    pub start: Point,
}

/// Modal state reached after a run of operations.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModalState {
    pub pos: Point,
    pub layer: Option<usize>,
    pub pass: Option<u32>,
    pub air: bool,
    pub laser: Option<u8>,
    pub z: Option<f64>,
}

impl Toolpath {
    /// Modal state just before `ops[index]`.
    pub fn state_before(&self, index: usize) -> ModalState {
        let mut s = ModalState { pos: self.start, ..Default::default() };
        for op in self.ops.iter().take(index) {
            match op {
                Op::LayerStart { layer } => {
                    s.layer = Some(*layer);
                    s.pass = None;
                }
                Op::PassStart { pass } => s.pass = Some(*pass),
                Op::Air { on } => s.air = *on,
                Op::Laser { index } => s.laser = Some(*index),
                Op::ZMove { z } => s.z = Some(*z),
                _ => {}
            }
            if let Some(p) = op.target() {
                s.pos = p;
            }
        }
        s
    }

    /// "Start here": a toolpath that begins at `ops[index]`, with the layer, pass, laser, air
    /// and Z state restored and a rapid move to where `ops[index]` starts.
    pub fn slice_from(&self, index: usize) -> Toolpath {
        let index = index.min(self.ops.len());
        let s = self.state_before(index);
        let mut ops = Vec::new();
        if let Some(layer) = s.layer {
            ops.push(Op::LayerStart { layer });
        }
        if let Some(pass) = s.pass {
            ops.push(Op::PassStart { pass });
        }
        if let Some(index) = s.laser {
            ops.push(Op::Laser { index });
        }
        if s.air {
            ops.push(Op::Air { on: true });
        }
        if let Some(z) = s.z {
            ops.push(Op::ZMove { z });
        }
        ops.push(Op::Rapid { to: s.pos });
        ops.extend(self.ops[index..].iter().cloned());
        Toolpath { space: self.space, layers: self.layers.clone(), ops, start: self.start }
    }

    /// Bounding box of all laser moves (cuts, arcs, dwells), if any.
    pub fn burn_bounds(&self) -> Option<kurbo::Rect> {
        let mut pos = self.start;
        let mut r: Option<kurbo::Rect> = None;
        let mut add = |p: Point| {
            r = Some(match r {
                Some(r) => r.union_pt(p),
                None => kurbo::Rect::from_points(p, p),
            });
        };
        for op in &self.ops {
            match op {
                Op::Cut { to, power, .. } | Op::Arc { to, power, .. } if *power > 0.0 => {
                    add(pos);
                    add(*to);
                }
                Op::Dwell { power, .. } if *power > 0.0 => add(pos),
                _ => {}
            }
            if let Some(p) = op.target() {
                pos = p;
            }
        }
        r
    }
}
