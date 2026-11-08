//! Robustness: random, truncated and mutated input must never panic (errors are fine).

use darkburn_import::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

const FIXTURES: &[&str] = &[
    "entities.dxf",
    "minimal.dxf",
    "vector.pdf",
    "images.pdf",
    "pdf_compatible.ai",
    "legacy_postscript.ai",
    "project.lbrn2",
    "legacy.lbrn",
    "sample.plt",
    "design.sc",
];

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// Runs every importer on the bytes, and every exporter on whatever imported.
fn run_all(bytes: &[u8]) {
    let opts = ImportOptions::default();
    let all_pages = pdf_import::PdfOptions { pages: pdf_import::PageSelection::All, ..Default::default() };
    let results = [
        dxf_import::import(bytes, &opts.dxf),
        dxf_import::import(bytes, &dxf_import::DxfOptions { auto_join_tolerance: Some(0.5), ..Default::default() }),
        pdf_import::import(bytes, &all_pages),
        pdf_import::import_ai(bytes, &opts.pdf),
        hpgl::import(bytes, &opts.hpgl),
        lbrn::import(bytes),
        import_auto(None, bytes, &opts),
    ];
    let _ = detect_format(Some("x.dxf"), bytes);
    for doc in results.into_iter().flatten() {
        let _ = svg_export::export(&doc.items, &Default::default());
        let _ = dxf_export::export(&doc.items, &Default::default());
        let _ = hpgl::export(&doc.items, &Default::default());
        let _ = lbrn::write_lbrn2(&doc, &Default::default());
        let _ = gweike::export_nc(&doc.items, &Default::default());
        let _ = ai_export::export(&doc.items, &Default::default());
    }
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let prefixes: &[&[u8]] = &[
        b"",
        b"%PDF-1.4\n",
        b"0\nSECTION\n2\nENTITIES\n0\n",
        b"<LightBurnProject FormatVersion=\"1\">",
        b"IN;SP1;PD",
        b"AutoCAD Binary DXF\r\n\x1a\x00",
        b"%!PS-Adobe-3.0\n",
    ];
    for prefix in prefixes {
        for _ in 0..150 {
            let n = rng.below(600);
            let mut b = prefix.to_vec();
            b.extend(rng.bytes(n));
            run_all(&b);
        }
    }
}

#[test]
fn truncated_fixtures_never_panic() {
    for name in FIXTURES {
        let full = data(name);
        let step = (full.len() / 200).max(1);
        let mut len = 0;
        while len <= full.len() {
            run_all(&full[..len]);
            len += step;
        }
    }
}

#[test]
fn mutated_fixtures_never_panic() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for name in FIXTURES {
        let full = data(name);
        for _ in 0..150 {
            let mut b = full.clone();
            for _ in 0..1 + rng.below(8) {
                if b.is_empty() {
                    break;
                }
                let i = rng.below(b.len());
                match rng.below(4) {
                    0 => b[i] = rng.next() as u8,
                    1 => b[i] = b"0123456789-.e;,<>/\" \n"[rng.below(21)],
                    2 => {
                        b.remove(i);
                    }
                    _ => {
                        let j = rng.below(b.len());
                        b.insert(i, b[j]);
                    }
                }
            }
            run_all(&b);
        }
    }
}

#[test]
fn hostile_numbers_never_panic() {
    let cases: &[&[u8]] = &[
        b"IN;SP99999999999;PD1e308,1e308,-1e308,-1e308;CI0.0000001;AA0,0,1e300;CI1e30,0.00001;",
        b"IN;PU;PD;PR;PA;CI;AA;EA;ER;",
        b"<LightBurnProject><Shape Type=\"Polygon\" N=\"1e12\" Rx=\"1\"/><Shape Type=\"Rect\" W=\"NaN\" H=\"inf\" Cr=\"-5\"/>\
          <Shape Type=\"Path\"><VertList>V1e309 5c0x</VertList><PrimList>B0 99999999999999999999L5</PrimList></Shape>\
          <Shape Type=\"Bitmap\" W=\"1\" H=\"1\" Data=\"!!!\"/><CutSetting><index Value=\"-7\"/></CutSetting></LightBurnProject>",
        b"<LightBurnProject><Shape Type=\"Group\"><Children><Shape Type=\"Group\"><Children></Children></Shape></Children></Shape></LightBurnProject>",
    ];
    for c in cases {
        run_all(c);
    }
    // Deeply nested groups.
    let mut s = String::from("<LightBurnProject>");
    for _ in 0..300 {
        s.push_str("<Shape Type=\"Group\"><XForm>1 0 0 1 1 1</XForm><Children>");
    }
    for _ in 0..300 {
        s.push_str("</Children></Shape>");
    }
    s.push_str("</LightBurnProject>");
    run_all(s.as_bytes());
}
