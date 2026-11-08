//! Writes a small sample project: `cargo run -p darkburn-core --example sample -- out.dbrn`.
use darkburn_core::kurbo::{BezPath, Rect};
use darkburn_core::{Document, Mode, Shape, project};

fn main() {
    let mut d = Document::new();
    d.add(Shape::rect(Rect::new(40.0, 40.0, 140.0, 110.0), 8.0, 1));
    d.add(Shape::rect(Rect::new(60.0, 60.0, 90.0, 90.0), -5.0, 2));
    d.add(Shape::ellipse(Rect::new(180.0, 50.0, 280.0, 120.0), 3));
    d.add(Shape::polygon(darkburn_core::kurbo::Point::new(100.0, 220.0), 6, 40.0, 4));
    let mut p = BezPath::new();
    p.move_to((200.0, 200.0));
    p.curve_to((230.0, 280.0), (270.0, 160.0), (320.0, 240.0));
    d.add(Shape::path(p, 5));
    d.layer_mut(3).main_mut().mode = Mode::Fill;
    d.layer_mut(4).main_mut().mode = Mode::Fill;
    let bytes = project::save(&d, "sample", None).expect("save");
    let out = std::env::args().nth(1).expect("output path");
    std::fs::write(out, bytes).expect("write");
}
