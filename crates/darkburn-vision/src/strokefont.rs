//! A tiny single-stroke font (digits, a few signs and letters) so test generators can label rows with plain paths.
//! Original design: seven-segment digits on a 0.6 x 1.0 cell.

use kurbo::{BezPath, Point};

type Stroke = Vec<(f64, f64)>;

const W: f64 = 0.6;

fn seg(name: char) -> Stroke {
    match name {
        'a' => vec![(0.0, 1.0), (W, 1.0)],
        'b' => vec![(W, 1.0), (W, 0.5)],
        'c' => vec![(W, 0.5), (W, 0.0)],
        'd' => vec![(0.0, 0.0), (W, 0.0)],
        'e' => vec![(0.0, 0.0), (0.0, 0.5)],
        'f' => vec![(0.0, 0.5), (0.0, 1.0)],
        _ => vec![(0.0, 0.5), (W, 0.5)], // g
    }
}

fn segs(names: &str) -> Vec<Stroke> {
    names.chars().map(seg).collect()
}

/// Strokes of a glyph in a 0.6 wide, 1.0 high cell; `None` for characters the font does not have.
pub fn glyph(c: char) -> Option<Vec<Stroke>> {
    Some(match c {
        '0' => segs("abcdef"),
        '1' => segs("bc"),
        '2' => segs("abged"),
        '3' => segs("abgcd"),
        '4' => segs("fgbc"),
        '5' => segs("afgcd"),
        '6' => segs("afgedc"),
        '7' => segs("abc"),
        '8' => segs("abcdefg"),
        '9' => segs("abcdfg"),
        '-' => segs("g"),
        '+' => vec![seg('g'), vec![(W / 2.0, 0.25), (W / 2.0, 0.75)]],
        '.' => vec![vec![(0.0, 0.0), (0.0, 0.08), (0.08, 0.08), (0.08, 0.0), (0.0, 0.0)]],
        'm' => vec![vec![(0.0, 0.0), (0.0, 0.5), (0.3, 0.5), (0.3, 0.0)], vec![(0.3, 0.5), (W, 0.5), (W, 0.0)]],
        'n' => vec![vec![(0.0, 0.0), (0.0, 0.5), (W, 0.5), (W, 0.0)]],
        'i' => vec![vec![(0.3, 0.0), (0.3, 0.5)], vec![(0.3, 0.7), (0.3, 0.75)]],
        'x' => vec![vec![(0.0, 0.0), (W, 0.5)], vec![(0.0, 0.5), (W, 0.0)]],
        'Z' | 'z' => vec![vec![(0.0, 1.0), (W, 1.0), (0.0, 0.0), (W, 0.0)]],
        'v' => vec![vec![(0.0, 0.5), (0.3, 0.0), (W, 0.5)]],
        ' ' => Vec::new(),
        _ => return None,
    })
}

fn advance(c: char) -> f64 {
    match c {
        '.' => 0.3,
        ' ' => 0.5,
        'i' => 0.5,
        _ => W + 0.25,
    }
}

/// Width of `text` at the given cap height.
pub fn text_width(text: &str, height: f64) -> f64 {
    let n = text.chars().count();
    if n == 0 {
        return 0.0;
    }
    let sum: f64 = text.chars().map(advance).sum();
    // the last gap does not count
    (sum - 0.25) * height
}

/// Lay out `text` as open strokes with its baseline-left corner at `origin`. Characters the font lacks become gaps.
pub fn text_path(text: &str, height: f64, origin: Point) -> BezPath {
    let mut path = BezPath::new();
    let mut x = 0.0;
    for c in text.chars() {
        if let Some(strokes) = glyph(c) {
            for s in strokes {
                let mut it = s.iter();
                if let Some(&(px, py)) = it.next() {
                    path.move_to(Point::new(origin.x + (x + px) * height, origin.y + py * height));
                    for &(qx, qy) in it {
                        path.line_to(Point::new(origin.x + (x + qx) * height, origin.y + qy * height));
                    }
                }
            }
        }
        x += advance(c);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    #[test]
    fn digits_have_strokes_and_size() {
        for c in "0123456789-+.mnixZv".chars() {
            assert!(glyph(c).is_some_and(|g| !g.is_empty()), "{c}");
        }
        assert!(glyph('~').is_none());
        let p = text_path("8", 10.0, Point::new(5.0, 5.0));
        let b = p.bounding_box();
        assert!((b.x0 - 5.0).abs() < 1e-9 && (b.y0 - 5.0).abs() < 1e-9 && (b.width() - 6.0).abs() < 1e-9 && (b.height() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn width_matches_layout() {
        let p = text_path("12.5", 4.0, Point::ZERO);
        let w = text_width("12.5", 4.0);
        assert!((p.bounding_box().x1 - w).abs() < 1e-9, "{} vs {w}", p.bounding_box().x1);
        assert_eq!(text_width("", 4.0), 0.0);
        assert!(text_path("   ", 3.0, Point::ZERO).elements().is_empty());
    }
}
