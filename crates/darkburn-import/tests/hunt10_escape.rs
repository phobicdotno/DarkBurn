//! Bug hunt 10: device escapes in HPGL files are skipped by their own length, not up to the
//! next `:`, `;` or newline, so the instruction after them is kept.

use darkburn_import::hpgl::{self, HpglOptions};

/// The bounds of every path the file gives.
fn bounds(bytes: &[u8]) -> Vec<kurbo::Rect> {
    hpgl::import(bytes, &HpglOptions::default()).unwrap().items.iter().filter_map(|i| i.bounds()).collect()
}

/// Whether the file draws exactly the line from (2.5, 2.5) to (10, 2.5) mm.
fn draws_the_line(prefix: &[u8]) -> Result<(), String> {
    let mut bytes = prefix.to_vec();
    bytes.extend_from_slice(b"PU100,100;PD400,100;");
    let b = bounds(&bytes);
    let ok = b.len() == 1 && {
        let r = b[0];
        (r.x0 - 2.5).abs() < 1e-6 && (r.y0 - 2.5).abs() < 1e-6 && (r.x1 - 10.0).abs() < 1e-6 && (r.y1 - 2.5).abs() < 1e-6
    };
    if ok { Ok(()) } else { Err(format!("{:?} gave {b:?}", String::from_utf8_lossy(prefix))) }
}

#[test]
fn escapes_without_a_terminator_keep_the_next_instruction() {
    // Plotter on/off and the output requests take no parameters and no terminator.
    for prefix in [&b"\x1b.("[..], b"\x1b.Y", b"\x1b.)", b"\x1b.Z", b"\x1b.R", b"\x1b.A"] {
        draws_the_line(prefix).unwrap();
    }
    // PCL: enter HP-GL/2, the universal exit, a reset and a combined command, all ended by
    // their capital letter.
    for prefix in [&b"\x1b%0B"[..], b"\x1b%1B", b"\x1b%-12345X", b"\x1bE", b"\x1b&l1o0E", b"\x1b*c100X"] {
        draws_the_line(prefix).unwrap();
    }
}

#[test]
fn escapes_with_a_terminator_still_skip_their_parameters() {
    for prefix in [&b"\x1b.@1024;0:"[..], b"\x1b.I81;;17:", b"\x1b.N;19:", b"\x1b.M:", b"\x1b.(\x1b.I81;;17:\x1b.N;19:"] {
        draws_the_line(prefix).unwrap();
    }
}
