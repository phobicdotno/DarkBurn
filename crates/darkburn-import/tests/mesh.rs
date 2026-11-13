//! STL and OBJ readers and the depth picture made from a model.

use darkburn_import::ImportError;
use darkburn_import::depth_map::{DepthLimits, DepthSettings, View, depth_buffer, depth_map, extent, frame};
use darkburn_import::mesh::*;

/// A square pyramid: base 0..20 by 0..20 at z = 0, apex (10, 10, 10).
fn pyramid() -> Mesh {
    let (a, b, c, d, apex) = ([0.0, 0.0, 0.0], [20.0, 0.0, 0.0], [20.0, 20.0, 0.0], [0.0, 20.0, 0.0], [10.0, 10.0, 10.0]);
    Mesh { triangles: vec![[a, b, apex], [b, c, apex], [c, d, apex], [d, a, apex], [a, c, b], [a, d, c]] }
}

/// A latitude-longitude sphere around the origin.
fn sphere(r: f32, segments: usize, rings: usize) -> Mesh {
    use std::f64::consts::PI;
    let point = |ring: usize, seg: usize| {
        let theta = PI * ring as f64 / rings as f64;
        let phi = 2.0 * PI * seg as f64 / segments as f64;
        let r = f64::from(r);
        [(r * theta.sin() * phi.cos()) as f32, (r * theta.sin() * phi.sin()) as f32, (r * theta.cos()) as f32]
    };
    let mut triangles = Vec::new();
    for ring in 0..rings {
        for seg in 0..segments {
            let (p00, p01, p10, p11) = (point(ring, seg), point(ring, seg + 1), point(ring + 1, seg), point(ring + 1, seg + 1));
            if ring > 0 {
                triangles.push([p00, p10, p01]);
            }
            if ring + 1 < rings {
                triangles.push([p01, p10, p11]);
            }
        }
    }
    Mesh { triangles }
}

fn one_px_per_mm(width_mm: f64) -> DepthSettings {
    DepthSettings { width_mm, dpi: 25.4, ..DepthSettings::default() }
}

fn limits() -> MeshLimits {
    MeshLimits::default()
}

#[test]
fn pyramid_from_the_top_gives_its_heights() {
    let buf = depth_buffer(&pyramid(), &one_px_per_mm(20.0), &DepthLimits::default()).unwrap();
    assert_eq!((buf.frame.px_width, buf.frame.px_height), (20, 20));
    assert!((buf.frame.height_mm - 20.0).abs() < 1e-9);
    for j in 0..20 {
        for i in 0..20 {
            let (x, y) = (i as f64 + 0.5, 20.0 - (j as f64 + 0.5));
            let want = 10.0 - (x - 10.0).abs().max((y - 10.0).abs());
            let got = f64::from(buf.at(i, j).expect("covered"));
            assert!((got - want).abs() < 1e-3, "pixel {i},{j}: {got} want {want}");
        }
    }
    assert!((buf.visible_depth() - 9.0).abs() < 1e-3);
}

#[test]
fn pyramid_scales_with_the_width() {
    // 40 mm wide at 1 px/mm: twice the size, heights twice as tall.
    let buf = depth_buffer(&pyramid(), &one_px_per_mm(40.0), &DepthLimits::default()).unwrap();
    assert_eq!((buf.frame.px_width, buf.frame.px_height), (40, 40));
    assert!((buf.frame.scale - 2.0).abs() < 1e-12);
    assert!((f64::from(buf.at(19, 19).unwrap()) - 19.5).abs() < 1e-3);
    assert!((f64::from(buf.at(0, 0).unwrap()) - 0.5).abs() < 1e-3);
}

#[test]
fn pyramid_from_the_front() {
    // Looking along +Y the front face (y = z) is nearest; the picture is 20 wide and 10 tall.
    let s = DepthSettings { view: View::Front, ..one_px_per_mm(20.0) };
    let buf = depth_buffer(&pyramid(), &s, &DepthLimits::default()).unwrap();
    assert_eq!((buf.frame.px_width, buf.frame.px_height), (20, 10));
    // Row 5 is z = 4.5; nearness is measured from y = 20 (the back), so the face sits at 20 - z.
    for i in 0..20 {
        let x = i as f64 + 0.5;
        let got = buf.at(i, 5);
        if (x - 10.0).abs() < 5.5 - 1e-6 {
            assert!((f64::from(got.unwrap()) - 15.5).abs() < 1e-3, "pixel {i}: {got:?}");
        } else if (x - 10.0).abs() > 5.5 + 1e-6 {
            assert_eq!(got, None, "pixel {i}");
        }
    }
}

#[test]
fn views_map_axes_and_flip_mirrors() {
    let p = [1.0, 2.0, 3.0];
    assert_eq!(View::Top.project(false, p), [1.0, 2.0, 3.0]);
    assert_eq!(View::Front.project(false, p), [1.0, 3.0, -2.0]);
    assert_eq!(View::Side.project(false, p), [2.0, 3.0, 1.0]);
    assert_eq!(View::Top.project(true, p), [-1.0, 2.0, -3.0]);
    assert_eq!(View::from_name("FRONT"), Some(View::Front));
    assert_eq!(View::from_name("back"), None);
    assert_eq!(extent(&pyramid(), View::Front, false), [20.0, 10.0, 20.0]);
    assert_eq!(extent(&pyramid(), View::Side, true), [20.0, 10.0, 20.0]);
    assert_eq!(extent(&Mesh::default(), View::Top, false), [0.0; 3]);

    // A ramp rising to the right (+X): from the top the right edge is nearest; from below
    // (flipped) the flat underside is all one depth and the picture is mirrored.
    let ramp = Mesh {
        triangles: vec![
            [[0.0, 0.0, 0.0], [20.0, 0.0, 10.0], [20.0, 10.0, 10.0]],
            [[0.0, 0.0, 0.0], [20.0, 10.0, 10.0], [0.0, 10.0, 0.0]],
            [[0.0, 0.0, 0.0], [20.0, 10.0, 0.0], [20.0, 0.0, 0.0]],
            [[0.0, 0.0, 0.0], [0.0, 10.0, 0.0], [20.0, 10.0, 0.0]],
        ],
    };
    let top = depth_buffer(&ramp, &one_px_per_mm(20.0), &DepthLimits::default()).unwrap();
    assert!(top.at(19, 5).unwrap() > top.at(0, 5).unwrap());
    let below = depth_buffer(&ramp, &DepthSettings { flip: true, ..one_px_per_mm(20.0) }, &DepthLimits::default()).unwrap();
    assert!(below.visible_depth().abs() < 1e-4);
    // Side view from +X: the picture is 10 wide (Y) and 10 tall (Z), and the slope (x = 2z) is
    // nearest at the top row (z = 9.5, x = 19).
    let side = depth_buffer(&ramp, &DepthSettings { view: View::Side, ..one_px_per_mm(10.0) }, &DepthLimits::default()).unwrap();
    assert_eq!((side.frame.px_width, side.frame.px_height), (10, 10));
    assert!((f64::from(side.at(5, 0).unwrap()) - 19.0).abs() < 1e-3);
    assert!((f64::from(side.at(5, 9).unwrap()) - 1.0).abs() < 1e-3);
}

#[test]
fn sphere_from_the_top_gives_its_heights() {
    let buf = depth_buffer(&sphere(10.0, 96, 48), &one_px_per_mm(20.0), &DepthLimits::default()).unwrap();
    assert_eq!((buf.frame.px_width, buf.frame.px_height), (20, 20));
    let mut checked = 0;
    for j in 0..20 {
        for i in 0..20 {
            let (x, y) = (i as f64 + 0.5 - 10.0, 10.0 - (j as f64 + 0.5));
            let r2 = x * x + y * y;
            let got = buf.at(i, j);
            if r2 < 81.0 {
                // Nearness is measured from the bottom of the sphere (z = -10).
                let want = 10.0 + (100.0 - r2).sqrt();
                let got = f64::from(got.expect("covered"));
                assert!((got - want).abs() < 0.05, "pixel {i},{j}: {got} want {want}");
                checked += 1;
            } else if r2 > 101.0 {
                assert_eq!(got, None, "pixel {i},{j} is outside the sphere");
            }
        }
    }
    assert!(checked > 200);
}

#[test]
fn binary_and_ascii_stl_agree() {
    let model = sphere(12.5, 40, 20);
    let bin = write_binary_stl(&model);
    let text = write_ascii_stl(&model, "ball");
    let a = read_stl(&bin, &limits()).unwrap();
    let b = read_stl(text.as_bytes(), &limits()).unwrap();
    assert_eq!(a, model);
    assert_eq!(a, b);
    let s = DepthSettings { width_mm: 30.0, dpi: 100.0, ..DepthSettings::default() };
    let (ma, mb) = (depth_map(&a, &s, &DepthLimits::default()).unwrap(), depth_map(&b, &s, &DepthLimits::default()).unwrap());
    assert_eq!(ma, mb);
    assert_eq!(ma.to_png().unwrap(), mb.to_png().unwrap());
}

#[test]
fn binary_stl_with_solid_header_is_binary() {
    let mut bin = write_binary_stl(&pyramid());
    bin[..12].copy_from_slice(b"solid pyrami");
    assert_eq!(read_stl(&bin, &limits()).unwrap(), pyramid());
    // Padding after the last record is allowed.
    bin.extend_from_slice(&[0, 1, 2, 3, 200, 201]);
    assert_eq!(read_stl(&bin, &limits()).unwrap(), pyramid());
}

#[test]
fn ascii_stl_variants() {
    let text = "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 1 1 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n\
                SOLID second\r\n  FACET NORMAL 0 0 1\r\n OUTER LOOP\r\n VERTEX 0 0 1\r\n VERTEX 1 0 1\r\n VERTEX 1 1 1\r\n ENDLOOP\r\n ENDFACET\r\nENDSOLID\r\n";
    let m = read_stl(text.as_bytes(), &limits()).unwrap();
    assert_eq!(m.triangles.len(), 3, "a quad loop becomes two triangles");
    assert_eq!(m.triangles[2][0], [0.0, 0.0, 1.0]);
    assert_eq!(m.size(), [1.0, 1.0, 1.0]);
}

#[test]
fn truncated_files_are_errors() {
    let bin = write_binary_stl(&pyramid());
    for cut in [0, 10, 83, 84 + 49, bin.len() - 1] {
        assert!(matches!(read_stl(&bin[..cut], &limits()), Err(ImportError::Parse(_))), "binary cut at {cut}");
    }
    let text = write_ascii_stl(&pyramid(), "p");
    for cut in [text.len() / 2, text.find("endsolid").unwrap(), text.find("endloop").unwrap()] {
        let r = read_stl(&text.as_bytes()[..cut], &limits());
        assert!(matches!(r, Err(ImportError::Parse(_))), "ascii cut at {cut}: {r:?}");
    }
    let obj = b"v 0 0 0\nv 1 0 0\nv 1 1 0\nf 1 2";
    assert!(matches!(read_obj(obj, &limits()), Err(ImportError::Parse(_))));
}

#[test]
fn coordinates_that_are_not_numbers_are_refused() {
    let mut bin = write_binary_stl(&pyramid());
    bin[84 + 12..84 + 16].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(read_stl(&bin, &limits()).is_err());
    bin[84 + 12..84 + 16].copy_from_slice(&f32::INFINITY.to_le_bytes());
    assert!(read_stl(&bin, &limits()).is_err());
    for bad in ["nan", "inf", "-infinity", "1e39"] {
        let text = format!("solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex {bad} 1 0\nendloop\nendfacet\nendsolid t\n");
        assert!(read_stl(text.as_bytes(), &limits()).is_err(), "{bad}");
        let obj = format!("v 0 0 0\nv 1 0 0\nv {bad} 1 0\nf 1 2 3\n");
        assert!(read_obj(obj.as_bytes(), &limits()).is_err(), "{bad}");
    }
}

#[test]
fn triangle_budget_holds() {
    let small = MeshLimits { max_triangles: 5, max_vertices: 100 };
    let model = sphere(5.0, 8, 4);
    assert!(model.triangles.len() > 5);
    assert!(matches!(read_stl(&write_binary_stl(&model), &small), Err(ImportError::Unsupported(_))));
    assert!(matches!(read_stl(write_ascii_stl(&model, "s").as_bytes(), &small), Err(ImportError::Unsupported(_))));
    // Six triangles as one polygon.
    let obj = "v 0 0 0\nv 1 0 0\nv 2 1 0\nv 2 2 0\nv 1 3 0\nv 0 3 0\nv -1 2 0\nv -1 1 0\nf 1 2 3 4 5 6 7 8\n";
    assert!(matches!(read_obj(obj.as_bytes(), &small), Err(ImportError::Unsupported(_))));
    assert_eq!(read_obj(obj.as_bytes(), &limits()).unwrap().triangles.len(), 6);
    // Too many vertices.
    let many = "v 0 0 0\n".repeat(101);
    assert!(matches!(read_obj(many.as_bytes(), &small), Err(ImportError::Unsupported(_))));
}

#[test]
fn a_huge_count_in_the_header_allocates_nothing() {
    // The header claims four billion triangles but the file holds none: refused before any buffer
    // is sized from the count.
    let mut bin = vec![b' '; 84];
    bin[80..84].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(read_stl(&bin, &limits()), Err(ImportError::Unsupported(_))));
    // Within the budget but longer than the file: truncated.
    bin[80..84].copy_from_slice(&1_000_000u32.to_le_bytes());
    assert!(matches!(read_stl(&bin, &limits()), Err(ImportError::Parse(_))));
}

#[test]
fn pixel_and_work_budgets_hold() {
    let m = pyramid();
    let huge = DepthSettings { width_mm: 10_000.0, dpi: 600.0, ..DepthSettings::default() };
    assert!(matches!(frame(&m, &huge, &DepthLimits::default()), Err(ImportError::Unsupported(_))));
    let tight = DepthLimits { max_pixels: 100, ..DepthLimits::default() };
    assert!(depth_buffer(&m, &one_px_per_mm(20.0), &tight).is_err());
    assert!(depth_buffer(&m, &one_px_per_mm(10.0), &tight).is_ok());
    let little_work = DepthLimits { max_work: 50, ..DepthLimits::default() };
    assert!(matches!(depth_buffer(&m, &one_px_per_mm(20.0), &little_work), Err(ImportError::Unsupported(_))));
}

#[test]
fn settings_are_checked() {
    let m = pyramid();
    let lim = DepthLimits::default();
    for s in [
        DepthSettings { width_mm: 0.0, ..DepthSettings::default() },
        DepthSettings { width_mm: f64::NAN, ..DepthSettings::default() },
        DepthSettings { dpi: -1.0, ..DepthSettings::default() },
        DepthSettings { depth_from: Some(3.0), depth_to: Some(2.0), ..DepthSettings::default() },
        DepthSettings { depth_to: Some(f64::INFINITY), ..DepthSettings::default() },
    ] {
        assert!(frame(&m, &s, &lim).is_err(), "{s:?}");
    }
    // A model with no width seen from this side: a flat plate seen edge on.
    let flat = Mesh { triangles: vec![[[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 1.0]]] };
    assert!(frame(&flat, &DepthSettings::default(), &lim).is_err());
    assert!(frame(&Mesh::default(), &DepthSettings::default(), &lim).is_err());
}

#[test]
fn tones_follow_near_white_and_the_clamp() {
    let m = pyramid();
    let s = one_px_per_mm(20.0);
    let map = depth_map(&m, &s, &DepthLimits::default()).unwrap();
    let at = |map: &darkburn_import::depth_map::DepthMap, i: usize, j: usize| map.tones[j * 20 + i];
    // Nearest pixels white, the farthest covered pixel black.
    assert_eq!(at(&map, 9, 9), 65535);
    assert_eq!(at(&map, 0, 0), 0);
    assert!((map.visible_depth_mm - 9.0).abs() < 1e-3);

    let dark = depth_map(&m, &DepthSettings { near_white: false, ..s }, &DepthLimits::default()).unwrap();
    assert_eq!(at(&dark, 9, 9), 0);
    assert_eq!(at(&dark, 0, 0), 65535);

    // Clamp to 2..4 mm below the top (9.5): heights above 7.5 white, below 5.5 black.
    let clamped = depth_map(&m, &DepthSettings { depth_from: Some(2.0), depth_to: Some(4.0), ..s }, &DepthLimits::default()).unwrap();
    assert_eq!(at(&clamped, 8, 8), 65535); // height 8.5
    assert_eq!(at(&clamped, 3, 3), 0); // height 3.5
    // Height 6.5 (pixel 6,6) is 3 mm down: half way.
    let mid = at(&clamped, 6, 6);
    assert!((i32::from(mid) - 32768).abs() < 40, "{mid}");

    // Pixels the model does not cover take the far tone.
    let ball = depth_map(&sphere(10.0, 32, 16), &s, &DepthLimits::default()).unwrap();
    assert_eq!(ball.tones[0], 0);
    let ball = depth_map(&sphere(10.0, 32, 16), &DepthSettings { near_white: false, ..s }, &DepthLimits::default()).unwrap();
    assert_eq!(ball.tones[0], 65535);
}

#[test]
fn png_is_16_bit_grey() {
    let map = depth_map(&pyramid(), &one_px_per_mm(20.0), &DepthLimits::default()).unwrap();
    let png = map.to_png().unwrap();
    let img = image::load_from_memory(&png).unwrap();
    let grey = img.as_luma16().expect("16-bit grey");
    assert_eq!(grey.dimensions(), (20, 20));
    assert_eq!(grey.get_pixel(9, 9).0[0], 65535);
    assert_eq!(grey.as_raw(), &map.tones);
}

#[test]
fn obj_faces_and_indices() {
    let obj = "# a square\nmtllib x.mtl\no sq\nv 0 0 0\nv 10 0 0 1\nv 10 10 0\nv 0 10 0\nvt 0 0\nvn 0 0 1\nusemtl m\ns off\n\
               f 1/1/1 2/1/1 3/1/1 4/1/1\nf -4//1 -2//1 -1//1 # relative\n";
    let m = read_obj(obj.as_bytes(), &limits()).unwrap();
    assert_eq!(m.triangles.len(), 3);
    assert_eq!(m.triangles[0], [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]]);
    assert_eq!(m.triangles[2], [[0.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0]]);
    assert!(read_obj(b"v 0 0 0\nf 1 2 3\n", &limits()).is_err(), "index past the end");
    assert!(read_obj(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 1 2\n", &limits()).is_err(), "zero index");
    assert!(read_obj(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf -4 1 2\n", &limits()).is_err(), "relative index before the start");
    assert!(read_obj(b"v 0 0\n", &limits()).is_err(), "two coordinates");
    assert!(read_mesh(MeshFormat::Obj, b"v 0 0 0\n", &limits()).is_err(), "no faces");
    assert_eq!(format_of("STL"), Some(MeshFormat::Stl));
    assert_eq!(format_of("obj"), Some(MeshFormat::Obj));
    assert_eq!(format_of("png"), None);
}

#[test]
fn garbage_never_panics() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let base = write_binary_stl(&pyramid());
    let text = write_ascii_stl(&pyramid(), "p").into_bytes();
    for round in 0..400 {
        let mut b = if round % 2 == 0 { base.clone() } else { text.clone() };
        for _ in 0..1 + next() % 8 {
            let at = (next() as usize) % b.len();
            b[at] = next() as u8;
        }
        let cut = (next() as usize) % (b.len() + 1);
        let b = &b[..cut];
        for f in [MeshFormat::Stl, MeshFormat::Obj] {
            if let Ok(m) = read_mesh(f, b, &limits()) {
                let _ = depth_map(&m, &DepthSettings { width_mm: 20.0, dpi: 50.0, ..DepthSettings::default() }, &DepthLimits::default());
            }
        }
    }
}
