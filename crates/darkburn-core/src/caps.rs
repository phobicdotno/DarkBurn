//! What a machine can do, so the cut-settings panel only shows the fields that machine uses.
//!
//! A [`MachineCaps`] describes one device kind: its laser sources and the cutting features it
//! has (height follower, assist-gas control, piercing, source frequency, micro-joints, film
//! removal). [`MachineCaps::layer_fields`] turns that into the sections to show for one
//! sub-layer. On a dual-source Gweike M-series machine the fibre sections show only for layers
//! that use the fibre source; CO2 layers keep speed, power, passes and air.
//!
//! The caps only steer what is shown. Hidden data stays in the layer and is saved as it is.

use serde::{Deserialize, Serialize};

use crate::layer::{CutSetting, GweikeSource};

/// Laser sources a machine has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sources {
    pub co2: bool,
    pub fibre: bool,
    pub diode: bool,
    /// Scanning (galvo) head rather than a gantry.
    pub galvo: bool,
}

/// Gweike M-series models (M LASER). All of them carry a fibre source and a CO2 tube.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GweikeModel {
    /// 620 x 620 mm, 800 or 1200 W fibre plus 80 W CO2.
    M3,
    /// 620 x 620 mm, 800 or 1200 W fibre plus 80 W CO2.
    M3Pro,
    /// 1390 frame, 1200 W fibre plus CO2.
    M3Max,
    /// 1390 frame, 1200 W fibre plus 130 W CO2 (manual focus on the CO2 head).
    #[default]
    M3Ultra,
}

impl GweikeModel {
    pub const ALL: [GweikeModel; 4] = [GweikeModel::M3, GweikeModel::M3Pro, GweikeModel::M3Max, GweikeModel::M3Ultra];
    pub fn label(self) -> &'static str {
        match self {
            GweikeModel::M3 => "Gweike M3",
            GweikeModel::M3Pro => "Gweike M3 Pro",
            GweikeModel::M3Max => "Gweike M3 Max",
            GweikeModel::M3Ultra => "Gweike M3 Ultra",
        }
    }
    /// Work area, mm.
    pub fn work_area(self) -> (f64, f64) {
        match self {
            GweikeModel::M3 | GweikeModel::M3Pro => (620.0, 620.0),
            GweikeModel::M3Max | GweikeModel::M3Ultra => (1371.0, 950.0),
        }
    }
}

/// The features one device kind has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineCaps {
    pub sources: Sources,
    /// Two tubes picked per layer (Laser 1 / Laser 2), as on Ruida DSP controllers.
    pub dual_laser: bool,
    /// The layer picks its source (CO2 or fibre) on a dual-source machine.
    pub source_select: bool,
    /// Capacitive height follower: cut height, fixed-height position and frog-jump lift.
    pub follower: bool,
    /// Gas type, pressure and gas delays per layer.
    pub gas_control: bool,
    /// Pre-drill, pierce stages and slow start.
    pub piercing: bool,
    /// Pulse frequency, peak current and duty of the source.
    pub source_freq: bool,
    /// Micro-joints (micro-links) left on the contour.
    pub micro_joints: bool,
    /// Film-removal contour before the cut.
    pub film_removal: bool,
    /// Gweike machine (Mlaser recipe and layer files).
    pub gweike: Option<GweikeModel>,
}

/// The sections of the cut-settings panel to show for one sub-layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerFields {
    pub source_select: bool,
    pub follower: bool,
    pub gas_control: bool,
    pub piercing: bool,
    pub source_freq: bool,
    pub micro_joints: bool,
    pub film_removal: bool,
}

impl LayerFields {
    /// Any machine-specific section at all.
    pub fn any(&self) -> bool {
        self.source_select || self.follower || self.gas_control || self.piercing || self.source_freq || self.micro_joints || self.film_removal
    }
}

impl MachineCaps {
    /// GRBL, grblHAL, FluidNC, Marlin, Smoothie and the like: a diode or CO2 laser on a gantry.
    pub fn gcode() -> MachineCaps {
        MachineCaps { sources: Sources { co2: true, diode: true, ..Sources::default() }, ..MachineCaps::default() }
    }
    /// Ruida DSP: CO2, one or two tubes.
    pub fn ruida() -> MachineCaps {
        MachineCaps { sources: Sources { co2: true, ..Sources::default() }, dual_laser: true, ..MachineCaps::default() }
    }
    /// Lihuiyu / K40 boards: one CO2 tube.
    pub fn lihuiyu() -> MachineCaps {
        MachineCaps { sources: Sources { co2: true, ..Sources::default() }, ..MachineCaps::default() }
    }
    /// EzCad-style galvo boards (fibre, MOPA, UV or CO2 galvo). Their own fields live in the
    /// galvo section; none of the fibre-cutting sections apply.
    pub fn galvo() -> MachineCaps {
        MachineCaps { sources: Sources { fibre: true, co2: true, galvo: true, ..Sources::default() }, ..MachineCaps::default() }
    }
    /// No laser: design only.
    pub fn design_only() -> MachineCaps {
        MachineCaps::default()
    }
    /// A Gweike M-series (M LASER) dual-source cutter: fibre plus CO2.
    pub fn gweike_m(model: GweikeModel) -> MachineCaps {
        MachineCaps {
            sources: Sources { co2: true, fibre: true, ..Sources::default() },
            dual_laser: false,
            source_select: true,
            follower: true,
            gas_control: true,
            piercing: true,
            source_freq: true,
            micro_joints: true,
            film_removal: true,
            gweike: Some(model),
        }
    }

    /// The sections to show for `c` on this machine. On a source-select machine a CO2 layer gets
    /// only the source picker; a fibre layer gets everything the machine has.
    pub fn layer_fields(&self, c: &CutSetting) -> LayerFields {
        let fibre = !self.source_select || c.gweike.as_ref().map_or(GweikeSource::default(), |g| g.source) == GweikeSource::Fibre;
        let on = |cap: bool| cap && fibre;
        LayerFields {
            source_select: self.source_select,
            follower: on(self.follower),
            gas_control: on(self.gas_control),
            piercing: on(self.piercing),
            source_freq: on(self.source_freq),
            micro_joints: on(self.micro_joints),
            film_removal: on(self.film_removal),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::GweikeLayer;

    #[test]
    fn plain_machines_show_no_fibre_sections() {
        let c = CutSetting::default();
        for caps in [MachineCaps::gcode(), MachineCaps::ruida(), MachineCaps::lihuiyu(), MachineCaps::galvo(), MachineCaps::design_only()] {
            assert!(!caps.layer_fields(&c).any(), "{caps:?}");
            assert!(caps.gweike.is_none());
        }
        assert!(MachineCaps::ruida().dual_laser);
        assert!(MachineCaps::galvo().sources.galvo);
        assert!(MachineCaps::gcode().sources.diode);
    }

    #[test]
    fn gweike_fibre_layers_show_everything_co2_layers_only_the_source() {
        for model in GweikeModel::ALL {
            let caps = MachineCaps::gweike_m(model);
            assert!(caps.sources.co2 && caps.sources.fibre);
            let fibre = CutSetting::default();
            let f = caps.layer_fields(&fibre);
            assert!(f.source_select && f.follower && f.gas_control && f.piercing && f.source_freq && f.micro_joints && f.film_removal);
            let co2 = CutSetting { gweike: Some(GweikeLayer { source: GweikeSource::Co2, ..Default::default() }), ..Default::default() };
            assert_eq!(caps.layer_fields(&co2), LayerFields { source_select: true, ..Default::default() });
        }
        assert_eq!(GweikeModel::M3Ultra.work_area(), (1371.0, 950.0));
    }
}
