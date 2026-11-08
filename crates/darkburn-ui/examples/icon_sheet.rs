//! Renders icons into a raw RGBA contact sheet for review:
//! `cargo run -p darkburn-ui --example icon_sheet -- <out.rgba> <px> <look> <id>...`, where look is
//! 0 (light), 1 (dark) or t (dark artwork on a transparent sheet without padding).
//! Prints the sheet's width and height. Used to check new artwork and to make the app icon.
use darkburn_ui::icon_svg::{Look, doc_for, render, resolve};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (out, px, dark, clear) = (&args[0], args[1].parse::<u32>().unwrap(), args[2] != "0", args[2] == "t");
    let ids = &args[3..];
    let cols = ids.len().clamp(1, 10) as u32;
    let rows = (ids.len() as u32).div_ceil(cols);
    let pad = if clear { 0 } else { 4 };
    let cell = px + 2 * pad;
    let (w, h) = (cols * cell, rows * cell);
    let bg = if clear {
        [0, 0, 0, 0]
    } else if dark {
        [0x27, 0x2b, 0x34, 255]
    } else {
        [0xf6, 0xf7, 0xf9, 255]
    };
    let mut sheet: Vec<u8> = bg.iter().copied().cycle().take((w * h * 4) as usize).collect();
    for (i, id) in ids.iter().enumerate() {
        let r = resolve(id);
        let doc = doc_for(r.master, px).expect("icon");
        let look = Look { dark, accent: r.accent, vision: solidcraft_theme::ColorVision::Off, palette: None };
        let pm = render(doc, look, px).expect("render");
        let (cx, cy) = ((i as u32 % cols) * cell + pad, (i as u32 / cols) * cell + pad);
        for y in 0..px {
            for x in 0..px {
                let p = pm.pixel(x, y).unwrap().demultiply();
                let a = p.alpha() as u32;
                let o = (((cy + y) * w + cx + x) * 4) as usize;
                if clear {
                    sheet[o..o + 4].copy_from_slice(&[p.red(), p.green(), p.blue(), p.alpha()]);
                    continue;
                }
                for (k, c) in [p.red(), p.green(), p.blue()].into_iter().enumerate() {
                    sheet[o + k] = ((c as u32 * a + sheet[o + k] as u32 * (255 - a)) / 255) as u8;
                }
            }
        }
    }
    std::fs::write(out, sheet).unwrap();
    println!("{w} {h}");
}
