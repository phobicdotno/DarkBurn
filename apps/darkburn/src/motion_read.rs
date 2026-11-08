//! Reads the controller's own speed and acceleration numbers into the device's time-estimate
//! settings, so the estimate and the preview match the machine [PREV-11, DS-19].
//!
//! GRBL: `$110`/`$111` max rate (mm/min), `$120`/`$121` acceleration (mm/s²), `$11` junction
//! deviation, `$112` Z rate. Ruida: the X/Y maximum speed and acceleration registers (µm/s and
//! µm/s²), the engraving acceleration and the idle (G0) speed. It works with the simulators the
//! same as with a real controller: it only looks at settings already read.

use std::collections::BTreeMap;

use darkburn_ruida::settings::{MachineSettings, addr};

use crate::job_build::AdditionalSettings;
use crate::machine_api::{Family, Snapshot};

fn grbl(values: &BTreeMap<u16, String>, n: u16) -> Option<f64> {
    values.get(&n)?.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

fn lower(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Fold the GRBL settings (`$$`) into `base`. `None` when the controller reported nothing useful.
pub fn from_grbl(values: &BTreeMap<u16, String>, base: AdditionalSettings) -> Option<AdditionalSettings> {
    let mut out = base;
    let mut found = false;
    if let Some(rate) = lower(grbl(values, 110), grbl(values, 111)) {
        out.max_speed = rate / 60.0;
        out.rapid_speed = rate / 60.0;
        found = true;
    }
    if let Some(acc) = lower(grbl(values, 120), grbl(values, 121)) {
        out.accel = acc;
        out.engrave_accel = acc;
        found = true;
    }
    if let Some(jd) = grbl(values, 11) {
        out.junction_deviation = jd;
        found = true;
    }
    if let Some(z) = grbl(values, 112) {
        out.z_speed = z / 60.0;
        found = true;
    }
    found.then_some(out)
}

/// Fold the Ruida registers into `base`.
pub fn from_ruida(s: &MachineSettings, base: AdditionalSettings) -> Option<AdditionalSettings> {
    let mm = |a: u16| s.get(a).map(|v| f64::from(v) / 1000.0).filter(|v| *v > 0.0);
    let mut out = base;
    let mut found = false;
    if let Some(v) = lower(mm(addr::X_MAX_VELOCITY), mm(addr::Y_MAX_VELOCITY)) {
        out.max_speed = v;
        found = true;
    }
    if let Some(v) = mm(addr::G0_VELOCITY) {
        out.rapid_speed = v;
        found = true;
    }
    if let Some(v) = lower(mm(addr::X_MAX_ACC), mm(addr::Y_MAX_ACC)) {
        out.accel = v;
        out.engrave_accel = v;
        found = true;
    }
    if let Some(v) = lower(mm(addr::X_ENG_ACC), mm(addr::Y_ENG_ACC)) {
        out.engrave_accel = v;
        found = true;
    }
    if let Some(v) = mm(addr::Z_WORK_VELOCITY) {
        out.z_speed = v;
        found = true;
    }
    found.then_some(out)
}

/// What the connected controller says, folded into `base`; `None` before its settings are read.
pub fn from_snapshot(snap: &Snapshot, base: AdditionalSettings) -> Option<AdditionalSettings> {
    match snap.family {
        Family::Grbl => from_grbl(&snap.grbl_settings, base),
        Family::Ruida => snap.ruida.settings.as_ref().and_then(|s| from_ruida(s, base)),
        Family::DesignOnly => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grbl_numbers_become_the_estimate_settings() {
        let v: BTreeMap<u16, String> = [(110, "6000"), (111, "3000"), (120, "800"), (121, "600"), (11, "0.02"), (112, "300")]
            .into_iter()
            .map(|(k, v)| (k, v.to_string()))
            .collect();
        let a = from_grbl(&v, AdditionalSettings::default()).unwrap();
        assert_eq!((a.max_speed, a.rapid_speed, a.accel, a.engrave_accel), (50.0, 50.0, 600.0, 600.0));
        assert_eq!((a.junction_deviation, a.z_speed), (0.02, 5.0));
        assert!(from_grbl(&BTreeMap::new(), AdditionalSettings::default()).is_none());
    }

    #[test]
    fn ruida_registers_are_micrometres() {
        let mut s = MachineSettings::default();
        s.set(addr::X_MAX_VELOCITY, 500_000);
        s.set(addr::Y_MAX_VELOCITY, 400_000);
        s.set(addr::X_MAX_ACC, 3_000_000);
        s.set(addr::Y_MAX_ACC, 2_000_000);
        s.set(addr::G0_VELOCITY, 200_000);
        s.set(addr::X_ENG_ACC, 1_000_000);
        let a = from_ruida(&s, AdditionalSettings::default()).unwrap();
        assert_eq!((a.max_speed, a.rapid_speed, a.accel, a.engrave_accel), (400.0, 200.0, 2000.0, 1000.0));
    }

    #[test]
    fn slower_controller_means_a_longer_estimate() {
        use crate::job_build::*;
        use darkburn_core::kurbo::Rect;
        let mut d = darkburn_core::Document::new();
        d.layer_mut(0).main_mut().speed = 200.0;
        d.add(darkburn_core::Shape::rect(Rect::new(10.0, 10.0, 60.0, 60.0), 0.0, 0));
        let input = BuildInput {
            doc: d,
            selection: Vec::new(),
            settings: JobProjectSettings::default(),
            device: JobDevice::new(200.0, 200.0),
            user_origin: None,
            head: None,
            motion: AdditionalSettings::default(),
            tuning: None,
        };
        let job = build_job_simple(&input).unwrap();
        let fast = job.simulate(&AdditionalSettings::default()).total_time;
        let v: BTreeMap<u16, String> = [(120, "50"), (121, "50")].into_iter().map(|(k, v)| (k, v.to_string())).collect();
        let slow_settings = from_grbl(&v, AdditionalSettings::default()).unwrap();
        let slow = job.simulate(&slow_settings).total_time;
        assert!(slow > fast * 1.2, "{slow} vs {fast}");
    }
}
