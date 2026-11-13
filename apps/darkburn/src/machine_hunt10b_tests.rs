//! Bug hunt 10, the app's machine side (Laser panel, device reads, galvo settings). Every test
//! drives the app against the in-memory emulators; no port, USB device or network connection is
//! opened.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::devices::DeviceType;
use crate::machine_api::LaserState;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

const T: Duration = Duration::from_secs(30);

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

#[track_caller]
fn wait_ready(app: &mut DarkBurn) {
    let end = Instant::now() + T;
    while app.machine.handle.snapshot().state != LaserState::Ready {
        app.machine.tick();
        assert!(Instant::now() < end, "never ready: {:?}", app.machine.handle.snapshot().state);
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Draw one of the Laser panel's panels for a frame.
fn draw_panel(app: &mut DarkBurn, panel: &str) {
    let ctx = egui::Context::default();
    for _ in 0..2 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert!(crate::laser_panel::show_panel(ui, panel, app));
        });
        out.textures_delta.clear();
    }
}

/// Files ticked in the File List of one controller stay ticked after the user switches to
/// another controller, whose list has not even been read: its Start and Delete buttons then run
/// or delete files of the new controller by the old one's numbers.
#[test]
fn the_file_list_forgets_ticks_from_another_controller() {
    let mut a = DarkBurn::new(Prefs::default());
    let first = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorRuida).unwrap().id;
    run(&mut a, "device.select", json!({ "id": first })).unwrap();
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait_ready(&mut a);
    // Ticked on the first controller.
    a.machine.file_sel.insert(3);
    let second = run(&mut a, "device.duplicate", json!({ "id": first })).unwrap()["id"].as_u64().unwrap();
    run(&mut a, "device.select", json!({ "id": second })).unwrap();
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait_ready(&mut a);
    assert!(a.machine.handle.snapshot().ruida.files.is_none(), "the new controller's list is not read yet");
    draw_panel(&mut a, "file_list");
    assert!(
        a.machine.file_sel.is_empty(),
        "Start and Delete would act on file(s) {:?} of a controller whose list was never shown",
        a.machine.file_sel
    );
}
