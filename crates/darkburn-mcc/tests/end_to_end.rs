//! A whole path through the crate, with no hardware: geometry in a `.chf`, read back, planned,
//! streamed through the gate and the codec into the simulator, and checked against the plan.

use darkburn_mcc::chf::{ChfDocument, Contour, ContourElement, Glyph, Graph, LwVertex, read_chf, write_chf};
use darkburn_mcc::command::{self, MachineParams};
use darkburn_mcc::craft::{CraftTable, ProcessType, Severity};
use darkburn_mcc::gate::ArmState;
use darkburn_mcc::planner::{PathSegment, PlanConfig, dry_run};
use darkburn_mcc::sim::{CardSimulator, SimConfig, SimLink};
use kurbo::Point;
use std::time::Duration;

fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

/// A 40 x 20 mm rectangle with a hole, on layer 1 and layer 2.
fn document() -> ChfDocument {
    let rect = |layer, x0, y0, x1, y1| {
        let vertices = vec![
            LwVertex { pt: p(x0, y0), bulge: 0.0 },
            LwVertex { pt: p(x1, y0), bulge: 0.0 },
            LwVertex { pt: p(x1, y1), bulge: 0.0 },
            LwVertex { pt: p(x0, y1), bulge: 0.0 },
        ];
        Contour::from_elements(0.01, layer, vec![ContourElement::forward(Glyph::LwPolyline { closed: 1, vertices })]).unwrap()
    };
    let hole = Contour::from_elements(0.01, 2, vec![ContourElement::forward(Glyph::Circle { center: p(40.0, 30.0), radius: 5.0 })]).unwrap();
    ChfDocument { graphs: vec![Graph::Contour(rect(1, 20.0, 20.0, 60.0, 40.0)), Graph::Contour(hole)], ..ChfDocument::default() }
}

/// Neutral toolpath from the rectangle contours: travel to the start, then cut the edges.
fn toolpath(doc: &ChfDocument, speed: f64) -> Vec<PathSegment> {
    let mut path = Vec::new();
    for g in &doc.graphs {
        for c in g.contours() {
            let Glyph::LwPolyline { vertices, .. } = &c.elements[0].glyph else { continue };
            path.push(PathSegment::rapid(vertices[0].pt.x, vertices[0].pt.y));
            for v in vertices.iter().skip(1).chain(std::iter::once(&vertices[0])) {
                path.push(PathSegment::cut(v.pt.x, v.pt.y, speed));
            }
        }
    }
    path
}

#[test]
fn chf_to_plan_to_simulator() {
    let doc = document();
    let bytes = write_chf(&doc).unwrap();
    let back = read_chf(&bytes, true).unwrap();
    // the file stores six decimals, so a value like pi * 10 comes back rounded; the bytes are stable
    assert_eq!(write_chf(&back).unwrap(), bytes);
    assert_eq!(back.graphs.len(), doc.graphs.len());

    let cfg = PlanConfig::default();
    let plan = dry_run(&toolpath(&back, 100.0), &cfg).unwrap();
    assert!((plan.cut_length_mm - 120.0).abs() < 1e-9);
    assert!(plan.estimated_seconds > 1.2, "{}", plan.estimated_seconds);

    let sim = CardSimulator::new(SimConfig { fifo_starvation_alarm: false, ..SimConfig::default() });
    let mut link = SimLink::new(sim);
    link.write(&command::connect_prologue().unwrap()).unwrap();
    link.gate.arming.arm_motion().unwrap();
    link.poll_status().unwrap();
    link.write(&command::fifo_clear().unwrap()).unwrap();

    // stream the whole plan, topping up from the status poll like the vendor driver does
    let mut feeder = plan.feeder();
    let mut started = false;
    let mut guard = 0;
    while feeder.pending() > 0 {
        let s = link.poll_status().unwrap();
        let r = feeder.fill(s.fifo_frame_id(), s.fifo_space_margin(), 50);
        for (id, frame) in r.frames {
            link.write(&command::fifo_packed_frame(id, &frame).unwrap()).unwrap();
        }
        if !started {
            link.write(&command::fifo_start().unwrap()).unwrap();
            started = true;
        }
        // let the card eat part of the queue
        link.advance(Duration::from_millis(50));
        guard += 1;
        assert!(guard < 10_000, "streaming never finished");
    }
    // drain the rest
    link.advance(Duration::from_secs_f64(plan.estimated_seconds + 1.0));
    assert_eq!(link.sim.ticks_consumed(), plan.tick_count);
    assert_eq!(link.sim.pulse_position(), (plan.total_pulses_x, plan.total_pulses_y));
    assert_eq!(link.sim.laser_refusals(), 0);
    assert!(link.sim.forbidden_attempts().is_empty());
    assert!(link.sim.consumed_items().is_empty(), "log is off by default");
    assert_eq!(link.gate.arming.state(), ArmState::MotionArmed);
    // the only 0x65 traffic was the prologue
    assert_eq!(link.sim.commands(), &[vec![9999, 5, 0, 0]]);
}

#[test]
fn a_stop_in_the_middle_leaves_the_card_quiet() {
    let plan = dry_run(&[PathSegment::cut(300.0, 0.0, 200.0)], &PlanConfig::default()).unwrap();
    let mut link = SimLink::new(CardSimulator::new(SimConfig::default()));
    link.gate.arming.arm_motion().unwrap();
    link.poll_status().unwrap();
    let mut feeder = plan.feeder();
    let s = link.poll_status().unwrap();
    for (id, frame) in feeder.fill(s.fifo_frame_id(), s.fifo_space_margin(), 50).frames {
        link.write(&command::fifo_packed_frame(id, &frame).unwrap()).unwrap();
    }
    link.write(&command::fifo_start().unwrap()).unwrap();
    link.advance(Duration::from_millis(100));
    link.poll_status().unwrap();
    assert!(link.sim.fifo_running());
    let steps = link.stop_sequence(false);
    assert!(steps >= 5);
    assert!(!link.sim.fifo_running());
    let before = link.sim.ticks_consumed();
    link.advance(Duration::from_millis(500));
    assert_eq!(link.sim.ticks_consumed(), before, "nothing runs after the stop");
    assert_eq!(link.sim.outputs(), 0);
    assert_eq!(link.gate.arming.state(), ArmState::MotionArmed);
}

#[test]
fn craft_table_validates_alongside_the_document() {
    let doc = document();
    let table = CraftTable::default();
    // every layer index in the document has a craft record
    for g in &doc.graphs {
        for c in g.contours() {
            assert!(table.fibre_layer(c.layer).is_some());
            assert!(table.co2_layer(c.layer).is_some());
        }
    }
    let mut t = table.clone();
    t.fibre[1].process = ProcessType::Pierce2;
    let issues = t.validate();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].severity, Severity::Error);
    assert_eq!(issues[0].path, "fibre[1].pierce");
}

#[test]
fn machine_params_default_to_the_documented_machine() {
    let p = MachineParams::default();
    assert_eq!((p.k, p.fcp_max_acc), (1000, 20000));
    let cfg = PlanConfig::default();
    assert_eq!(cfg.work_area_mm, (1371.0, 950.0));
    assert!((cfg.pulses_per_mm_x - 258.04).abs() < 0.01);
}
