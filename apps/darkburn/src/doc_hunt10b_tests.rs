//! Bug hunt 10, second pass over the document-side tools and windows: each test failed before
//! its fix.
use darkburn_core::kurbo::Rect as KRect;
use darkburn_core::{Shape, ShapeId};
use egui::{Event, Modifiers, Pos2, Rect, vec2};

use crate::prefs::Prefs;
use crate::shell::DarkBurn;

struct Rig {
    ctx: egui::Context,
    app: DarkBurn,
    time: f64,
}

impl Rig {
    fn new() -> Rig {
        let mut rig = Rig { ctx: egui::Context::default(), app: DarkBurn::new(Prefs::default()), time: 0.0 };
        rig.frame(vec![]);
        rig
    }

    fn frame(&mut self, mut events: Vec<Event>) {
        self.time += 0.016;
        events.insert(0, Event::ModifiersChanged(Modifiers::NONE));
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 900.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(input, |ui| {
            app.shortcuts(ui.ctx());
            let cmd = app.view_cmd.take();
            crate::canvas::show(ui, app, cmd);
        });
        out.textures_delta.clear();
    }

    fn add_rect(&mut self, r: KRect) -> ShapeId {
        let mut id = ShapeId(0);
        self.app.editor.edit("Add", |d, sel| {
            id = d.add(Shape::rect(r, 0.0, 0));
            sel.set([id]);
            true
        });
        id
    }

    fn select(&mut self, ids: &[ShapeId]) {
        let ids: Vec<u64> = ids.iter().map(|i| i.0).collect();
        crate::api::execute_headless(&mut self.app, "selection.set", &serde_json::json!({ "ids": ids })).unwrap();
    }
}

/// The measure tool's selection readout follows the selection: picking other shapes whose ids
/// happen to add up to the same number still shows the new selection's size.
#[test]
fn the_measure_readout_follows_a_selection_with_the_same_id_sum() {
    let mut rig = Rig::new();
    let a = rig.add_rect(KRect::new(0.0, 0.0, 10.0, 10.0));
    let b = rig.add_rect(KRect::new(100.0, 0.0, 110.0, 10.0));
    let c = rig.add_rect(KRect::new(200.0, 0.0, 210.0, 10.0));
    let d = rig.add_rect(KRect::new(20.0, 0.0, 30.0, 10.0));
    assert_eq!(a.0 + d.0, b.0 + c.0, "the test needs two pairs with the same id sum");
    rig.app.run(&rig.ctx.clone(), "tools.measure");
    rig.select(&[a, d]);
    rig.frame(vec![]);
    rig.frame(vec![]);
    let first = crate::node_tool::measure_selection_lines(&rig.ctx);
    assert!(first.first().is_some_and(|l| l.contains("30")), "a and d span 30 mm: {first:?}");
    rig.select(&[b, c]);
    rig.frame(vec![]);
    rig.frame(vec![]);
    let second = crate::node_tool::measure_selection_lines(&rig.ctx);
    assert!(second.first().is_some_and(|l| l.contains("110")), "b and c span 110 mm: {second:?}");
}

/// Leaving the text tool for another tool (a ribbon button, not Escape) ends the typing: a new
/// text with nothing typed is taken back, as it is with Escape, and keys pressed later are not
/// typed into it.
#[test]
fn switching_tools_while_typing_ends_the_typing() {
    use crate::tools::Tool;
    let mut g = crate::ui_harness::Gui::standard();
    g.app().set_tool(Tool::Text);
    g.frames(2);
    g.click_mm(100.0, 100.0);
    assert!(g.app().text.typing.is_some());
    assert_eq!(g.app().editor.doc.len(), 1, "the new text is placed");
    g.app().set_tool(Tool::Rect);
    g.frames(3);
    assert!(g.app().text.typing.is_none(), "still typing after another tool was picked");
    assert_eq!(g.app().editor.doc.len(), 0, "an empty text was left in the design");
    assert!(!g.app().editor.history.can_undo(), "an undo step for a text that was never typed");
}
