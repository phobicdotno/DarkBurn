//! Bug hunt 10: sizes and positions far beyond any machine must be refused or carried through,
//! never run over the end of a number.

use darkburn_raster::{Adjustments, GrayF, ImageMode, ImageSettings, PaintStyle, RasterError, placement, process_image, rasterize_paths};
use kurbo::{Point, Rect, Shape, Size};

/// A shape or a pen far too large for a bitmap is "too large", whatever the numbers.
#[test]
fn a_bitmap_too_large_to_count_is_refused_as_too_large() {
    let small = Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.01);
    let huge = Rect::new(0.0, 0.0, 1e30, 1e30).to_path(0.01);
    let fill = PaintStyle::Fill { even_odd: false };
    let too_large = |r: Result<_, RasterError>| matches!(r, Err(RasterError::TooLarge { .. }));
    assert!(too_large(rasterize_paths(std::slice::from_ref(&huge), fill, 254.0, None, 1 << 26)));
    assert!(too_large(rasterize_paths(std::slice::from_ref(&small), PaintStyle::Stroke { width_mm: 1e30 }, 254.0, None, 1 << 26)));
    assert!(too_large(rasterize_paths(std::slice::from_ref(&small), fill, f64::INFINITY, None, 1 << 26)));
    assert!(too_large(rasterize_paths(std::slice::from_ref(&small), fill, 254.0, Some(Rect::new(-1e300, -1e300, 1e300, 1e300)), 1 << 26)));
    // Nothing changes for a size that fits.
    let ok = rasterize_paths(&[Rect::new(0.0, 0.0, 25.4, 25.4).to_path(0.01)], fill, 100.0, None, 1 << 26).unwrap();
    assert_eq!((ok.image.width, ok.image.height), (100, 100));
}

/// A picture placed absurdly far away (the monkey does that) is still processed in every
/// mode, above and below the origin.
#[test]
fn a_picture_absurdly_far_away_is_processed_in_every_mode() {
    let src = GrayF::new(32, 16, 0.5);
    let modes = [
        ImageMode::Threshold { threshold: 0.5, ramp_length_mm: 0.0 },
        ImageMode::Ordered,
        ImageMode::Newsprint,
        ImageMode::Atkinson,
        ImageMode::FloydSteinberg,
        ImageMode::Halftone { cells_per_inch: 60.0, angle_deg: 30.0 },
        ImageMode::Grayscale,
        ImageMode::PassThrough,
    ];
    for far in [5e299, -5e299] {
        for mode in &modes {
            let t = placement(32, 16, Point::new(far, far), Size::new(3.2, 1.6));
            let settings = ImageSettings { mode: mode.clone(), ..ImageSettings::default() };
            let out = process_image(&src, t, &Adjustments::default(), &settings);
            assert_eq!(out.map.data.len(), out.map.width as usize * out.map.height as usize, "{mode:?} at {far}");
        }
    }
}
