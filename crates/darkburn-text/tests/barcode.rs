use darkburn_text::barcode::{CODE39_PATTERNS, CODE128_PATTERNS, code128_values};
use darkburn_text::*;
use std::collections::HashSet;

fn spec(kind: BarcodeKind, data: &str) -> BarcodeSpec {
    BarcodeSpec { kind, data: data.into(), module_mm: 0.5, height_mm: 12.0, ..Default::default() }
}

fn total_modules(r: &BarcodeResult) -> f64 {
    r.runs.iter().map(|x| x.1).sum()
}

#[test]
fn code128_table_is_sane() {
    let mut seen = HashSet::new();
    for (i, p) in CODE128_PATTERNS.iter().enumerate() {
        let sum: u32 = p.bytes().map(|b| (b - b'0') as u32).sum();
        assert_eq!(sum, if i == 106 { 13 } else { 11 }, "pattern {i}");
        assert_eq!(p.len(), if i == 106 { 7 } else { 6 });
        assert!(seen.insert(*p), "duplicate pattern {p}");
    }
}

#[test]
fn code128_values_and_checksum() {
    assert_eq!(code128_values("A").unwrap(), vec![104, 33, 34, 106]);
    // all digits: set C, pairs
    assert_eq!(code128_values("12345678").unwrap(), vec![105, 12, 34, 56, 78, 47, 106]);
    // letter then odd digit run: B, switch to C after one digit
    assert_eq!(code128_values("a12345").unwrap(), vec![104, 65, 17, 99, 23, 45, 96, 106]);
    // back from C to B
    let v = code128_values("1234x").unwrap();
    assert_eq!(&v[..5], &[105, 12, 34, 100, 88]);
    assert!(code128_values("tab\there").is_err());
}

fn decode128(r: &BarcodeResult) -> Vec<u32> {
    let widths: Vec<u32> = r.runs.iter().map(|x| x.1 as u32).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < widths.len() {
        let n = if widths.len() - i == 7 { 7 } else { 6 };
        let s: String = widths[i..i + n].iter().map(|w| char::from(b'0' + *w as u8)).collect();
        out.push(CODE128_PATTERNS.iter().position(|p| *p == s).expect("known pattern") as u32);
        i += n;
    }
    out
}

#[test]
fn code128_round_trips_through_its_own_bars() {
    for data in ["Hello, World 42!", "0123456789", "ab1234567c", "~ }"] {
        let r = spec(BarcodeKind::Code128, data).generate().unwrap();
        assert_eq!(decode128(&r), code128_values(data).unwrap(), "{data}");
        assert!(r.runs[0].0, "starts with a bar");
        assert!(!r.paths.is_empty());
    }
}

#[test]
fn code128_geometry() {
    let r = spec(BarcodeKind::Code128, "A").generate().unwrap();
    // start(11) + 1 data(11) + check(11) + stop(13) modules
    assert_eq!(total_modules(&r), 46.0);
    assert!((r.width_mm - 23.0).abs() < 1e-9);
    assert!((r.height_mm - 12.0).abs() < 1e-9);
    assert!((r.total_width_mm - (23.0 + 10.0)).abs() < 1e-9);
    let b = r.paths.iter().map(kurbo::Shape::bounding_box).reduce(|a, b| a.union(b)).unwrap();
    assert!((b.y1 - 12.0).abs() < 1e-9 && b.y0.abs() < 1e-9);
}

#[test]
fn code39() {
    let mut seen = HashSet::new();
    for p in CODE39_PATTERNS {
        assert_eq!(p.len(), 9);
        assert_eq!(p.bytes().filter(|b| *b == b'w').count(), 3, "{p}");
        assert!(seen.insert(p));
    }
    let r = spec(BarcodeKind::Code39, "abc").generate().unwrap();
    assert_eq!(r.payload, "ABC");
    assert_eq!(r.runs.len(), 5 * 9 + 4);
    // ratio 3: each char = 6 narrow + 3 wide = 15 modules ... 5 chars + 4 gaps
    assert_eq!(total_modules(&r), 5.0 * 15.0 + 4.0);
    let mut s = spec(BarcodeKind::Code39, "CODE39");
    s.code39_check = true;
    assert_eq!(s.generate().unwrap().payload, "CODE39W");
    assert!(spec(BarcodeKind::Code39, "a*b").generate().is_err());
    assert!(spec(BarcodeKind::Code39, "a_b").generate().is_err());
    s.wide_ratio = 2.0;
    s.code39_check = false;
    assert_eq!(total_modules(&s.generate().unwrap()), 8.0 * 12.0 + 7.0);
}

#[test]
fn ean13_and_upc() {
    let r = spec(BarcodeKind::Ean13, "4006381333931").generate().unwrap();
    assert_eq!(total_modules(&r), 95.0);
    assert_eq!(&r.runs[..3], &[(true, 1.0), (false, 1.0), (true, 1.0)]);
    let n = r.runs.len();
    assert_eq!(&r.runs[n - 3..], &[(true, 1.0), (false, 1.0), (true, 1.0)]);
    // check digit is computed when missing
    assert_eq!(spec(BarcodeKind::Ean13, "400638133393").generate().unwrap().payload, "4006381333931");
    assert!(spec(BarcodeKind::Ean13, "4006381333932").generate().is_err());
    assert!(spec(BarcodeKind::Ean13, "12345").generate().is_err());
    assert!(spec(BarcodeKind::Ean13, "40063813339a1").generate().is_err());
    let u = spec(BarcodeKind::UpcA, "03600029145").generate().unwrap();
    assert_eq!(u.payload, "036000291452");
    assert_eq!(total_modules(&u), 95.0);
    // UPC-A is the same bars as EAN-13 with a leading zero
    let e = spec(BarcodeKind::Ean13, "0036000291452").generate().unwrap();
    assert_eq!(u.runs, e.runs);
    // centre guard: 01010 in the middle (modules 45..50)
    let mut bits = String::new();
    for (bar, w) in &r.runs {
        for _ in 0..*w as usize {
            bits.push(if *bar { '1' } else { '0' });
        }
    }
    assert_eq!(&bits[45..50], "01010");
}

#[test]
fn itf() {
    let r = spec(BarcodeKind::Itf, "1234").generate().unwrap();
    assert_eq!(total_modules(&r), 4.0 + 36.0 + 5.0);
    assert_eq!(r.payload, "1234");
    let odd = spec(BarcodeKind::Itf, "123").generate().unwrap();
    assert_eq!(odd.payload, "0123");
    assert!(spec(BarcodeKind::Itf, "12a4").generate().is_err());
}

#[test]
fn data_matrix() {
    let r = spec(BarcodeKind::DataMatrix, "Hello DarkBurn").generate().unwrap();
    assert!(r.runs.is_empty());
    assert!((r.width_mm - r.height_mm).abs() < 1e-9, "square symbol");
    assert!(r.width_mm > 3.0);
    assert!(!r.paths.is_empty());
    // different data, different symbol
    let r2 = spec(BarcodeKind::DataMatrix, "Hello DarkBurn 2").generate().unwrap();
    assert!(r2.width_mm >= r.width_mm);
}

#[test]
fn variable_text_feeds_barcodes() {
    let mut st = VariableState::default();
    st.serial.current = 42;
    let ctx = EvalContext::local_now();
    let s = spec(BarcodeKind::Code128, "SN{serial:6}");
    let r = s.generate_with(&|t| st.evaluate(t, &ctx)).unwrap();
    assert_eq!(r.payload, "SN000042");
    assert!(spec(BarcodeKind::Code128, "").generate().is_err());
}

#[test]
fn spec_serialises() {
    let s = spec(BarcodeKind::Ean13, "400638133393");
    let j = serde_json::to_string(&s).unwrap();
    assert_eq!(serde_json::from_str::<BarcodeSpec>(&j).unwrap(), s);
}
