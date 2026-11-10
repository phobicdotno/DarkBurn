//! Bug hunt 6: the galvo job as the app builds it from a document.

use darkburn_core::kurbo::{Affine, Rect, Shape as _};
use darkburn_core::{Document, ShapeKind};
use darkburn_galvo as galvo;

use crate::devices::{Device, DeviceType};

/// Where the first picture of the galvo job is marked, in document millimetres: the box
/// around every pixel with any depth.
fn marked_box(doc: &Document) -> Rect {
    let dev = Device::new(DeviceType::SimulatorGalvo);
    let bed = crate::galvo_machine::bed_of(&dev.settings.galvo);
    let b = crate::galvo_job::build_job(doc, &dev).unwrap();
    let s =
        b.job.layers.iter().find_map(|l| if let galvo::LayerMode::Sliced(s) = &l.mode { Some(s.clone()) } else { None }).expect("a picture layer");
    let (pw, ph) = (s.size_mm.0 / f64::from(s.width_px), s.size_mm.1 / f64::from(s.height_px));
    let mut out: Option<Rect> = None;
    for row in 0..s.height_px {
        for col in 0..s.width_px {
            if s.depth[(row * s.width_px + col) as usize] > 0 {
                let x = s.origin.x + bed.0 / 2.0 + f64::from(col) * pw;
                let y = s.origin.y + bed.1 / 2.0 + s.size_mm.1 - f64::from(row + 1) * ph;
                let r = Rect::new(x, y, x + pw, y + ph);
                out = Some(out.map_or(r, |o| o.union(r)));
            }
        }
    }
    out.expect("something is marked")
}

fn near(a: Rect, b: Rect) -> bool {
    (a.x0 - b.x0).abs() < 0.01 && (a.y0 - b.y0).abs() < 0.01 && (a.x1 - b.x1).abs() < 0.01 && (a.y1 - b.y1).abs() < 0.01
}

/// A 10 mm picture at (5, 5) to (15, 15) whose left half is black and right half white.
fn half_black() -> Document {
    crate::job_field_tests::image_doc(|_| {}, [0u8, 0, 255, 255].repeat(4))
}

#[test]
fn a_galvo_marks_a_picture_the_way_it_lies_in_the_design() {
    // As drawn: the black half is the left one.
    let got = marked_box(&half_black());
    assert!(near(got, Rect::new(5.0, 5.0, 10.0, 15.0)), "as drawn: {got:?}");
    // Mirrored left to right: the black half is the right one.
    let mut d = half_black();
    std::sync::Arc::make_mut(&mut d.shapes[0]).transform = Affine::translate((10.0, 10.0)) * Affine::scale_non_uniform(-1.0, 1.0);
    let got = marked_box(&d);
    assert!(near(got, Rect::new(10.0, 5.0, 15.0, 15.0)), "mirrored: {got:?}");
    // Turned a quarter counter-clockwise: the black half is the lower one.
    let mut d = half_black();
    std::sync::Arc::make_mut(&mut d.shapes[0]).transform = Affine::translate((10.0, 10.0)) * Affine::rotate(std::f64::consts::FRAC_PI_2);
    let got = marked_box(&d);
    assert!(near(got, Rect::new(5.0, 5.0, 15.0, 10.0)), "turned: {got:?}");
    // Cropped to its lower left quarter (local millimetres, centred): only that is marked.
    let mut d = half_black();
    if let ShapeKind::Image(img) = &mut std::sync::Arc::make_mut(&mut d.shapes[0]).kind {
        img.crop = Some(Rect::new(-5.0, -5.0, 0.0, 0.0).to_path(0.01));
    }
    let got = marked_box(&d);
    assert!(near(got, Rect::new(5.0, 5.0, 10.0, 10.0)), "cropped: {got:?}");
}
