//! Rotary numbers from a Ruida controller [ROT-07]: the Rotary Setup window asks the controller
//! for its U axis settings and takes the pulses per rotation and the rotary diameter from them.

use darkburn_ruida::settings::UAxisSettings;

/// `(steps per rotation, rotary diameter in mm)` as the controller has them; `None` when it
/// reports no rotary numbers (both zero).
pub fn from_controller(u: &UAxisSettings) -> Option<(f64, f64)> {
    let steps = f64::from(u.rotary_pulses_per_rotation);
    let diameter = f64::from(u.rotary_diameter_um) / 1000.0;
    (steps > 0.0 || diameter > 0.0).then_some((steps, diameter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::execute_headless;
    use crate::devices::DeviceType;
    use crate::machine_api::LaserState;
    use crate::prefs::Prefs;
    use crate::shell::DarkBurn;
    use serde_json::json;
    use std::time::{Duration, Instant};

    #[test]
    fn the_numbers_convert() {
        let u = UAxisSettings { rotary_pulses_per_rotation: 8000, rotary_diameter_um: 62_500, ..Default::default() };
        assert_eq!(from_controller(&u), Some((8000.0, 62.5)));
        assert_eq!(from_controller(&UAxisSettings::default()), None);
    }

    /// Against the Ruida simulator: the read command brings the U axis settings back.
    #[test]
    fn the_ruida_simulator_answers_a_u_axis_read() {
        let mut a = DarkBurn::new(Prefs::default());
        let id = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorRuida).unwrap().id;
        execute_headless(&mut a, "device.select", &json!({ "id": id })).unwrap();
        execute_headless(&mut a, "machine.connect", &json!({})).unwrap();
        let end = Instant::now() + Duration::from_secs(20);
        while a.machine.handle.snapshot().state != LaserState::Ready {
            a.machine.tick();
            assert!(Instant::now() < end, "never ready");
            std::thread::sleep(Duration::from_millis(5));
        }
        execute_headless(&mut a, "machine.dsp_read", &json!({ "what": "u_axis" })).unwrap();
        while a.machine.handle.snapshot().ruida.u_axis.is_none() {
            a.machine.tick();
            assert!(Instant::now() < end, "no U axis answer");
            std::thread::sleep(Duration::from_millis(5));
        }
        let u = a.machine.handle.snapshot().ruida.u_axis.unwrap();
        // Whatever the simulator holds converts without panicking.
        let _ = from_controller(&u);
    }
}
