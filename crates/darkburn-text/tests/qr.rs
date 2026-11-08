use chrono::NaiveDate;
use darkburn_text::*;
use kurbo::Shape;

fn text_qr(t: &str) -> QrSpec {
    QrSpec { content: QrContent::Text(t.into()), ..Default::default() }
}

#[test]
fn wifi_payload_is_escaped() {
    let c = QrContent::Wifi { ssid: "my;net".into(), password: "pa:ss\"w,d".into(), security: WifiSecurity::Wpa, hidden: false };
    assert_eq!(c.payload(), "WIFI:T:WPA;S:my\\;net;P:pa\\:ss\\\"w\\,d;;");
    let open = QrContent::Wifi { ssid: "cafe".into(), password: "ignored".into(), security: WifiSecurity::Open, hidden: true };
    assert_eq!(open.payload(), "WIFI:T:nopass;S:cafe;H:true;;");
    let wep = QrContent::Wifi { ssid: "a".into(), password: "b".into(), security: WifiSecurity::Wep, hidden: false };
    assert_eq!(wep.payload(), "WIFI:T:WEP;S:a;P:b;;");
}

#[test]
fn mecard_payload() {
    let c = QrContent::Contact(ContactCard {
        name: "Doe,John".into(),
        phone: "+4712345678".into(),
        email: "j@x.no".into(),
        note: "a;b".into(),
        ..Default::default()
    });
    assert_eq!(c.payload(), "MECARD:N:Doe\\,John;TEL:+4712345678;EMAIL:j@x.no;NOTE:a\\;b;;");
}

#[test]
fn url_gets_a_scheme() {
    assert_eq!(QrContent::Url("example.com/a".into()).payload(), "https://example.com/a");
    assert_eq!(QrContent::Url("http://x.y".into()).payload(), "http://x.y");
    assert_eq!(QrContent::Url("mailto:a@b.c".into()).payload(), "mailto:a@b.c");
}

#[test]
fn symbol_geometry() {
    let mut s = text_qr("A");
    s.ecc = EccLevel::L;
    s.module_mm = 0.5;
    s.merged = false;
    let r = s.generate().unwrap();
    assert_eq!(r.modules, 21, "version 1");
    assert!((r.size_mm - 10.5).abs() < 1e-9);
    assert!((r.total_size_mm - (10.5 + 4.0)).abs() < 1e-9);
    // finder pattern: top-left 7x7 ring dark, inner ring light, centre dark
    assert!(r.matrix[0][..7].iter().all(|d| *d));
    assert!(!r.matrix[1][1] && !r.matrix[1][5]);
    assert!(r.matrix[3][3]);
    let dark = r.matrix.iter().flatten().filter(|d| **d).count();
    assert_eq!(r.paths.len(), dark, "one square per module");
    // Y is up: the top matrix row sits at the top of the symbol.
    let top = r.paths.iter().map(|p| p.bounding_box().y1).fold(0.0, f64::max);
    assert!((top - r.size_mm).abs() < 1e-9);
}

#[test]
fn merged_outline_covers_the_same_area() {
    let mut s = text_qr("https://darkburn.example/12345");
    s.module_mm = 0.4;
    let merged = s.generate().unwrap();
    s.merged = false;
    let squares = s.generate().unwrap();
    let dark = squares.paths.len();
    assert!(merged.paths.len() < dark / 2);
    let area: f64 = merged.paths.iter().map(|p| p.area().abs()).sum();
    assert!((area - dark as f64 * 0.16).abs() < 1e-6, "{area}");
}

#[test]
fn byte_limit_is_enforced() {
    let mut s = text_qr(&"A".repeat(2000));
    s.ecc = EccLevel::L;
    assert!(s.generate().is_ok());
    s.content = QrContent::Text("A".repeat(2001));
    let e = s.generate().unwrap_err();
    assert!(e.to_string().contains("2000"), "{e}");
    // Highest error correction cannot hold 2000 bytes: reported, not a panic.
    s.content = QrContent::Text("A".repeat(2000));
    s.ecc = EccLevel::H;
    assert!(s.generate().is_err());
    assert!(text_qr("").generate().is_err());
    let mut bad = text_qr("x");
    bad.module_mm = 0.0;
    assert!(bad.generate().is_err());
}

#[test]
fn ecc_level_changes_the_symbol() {
    let mut s = text_qr("The quick brown fox");
    s.ecc = EccLevel::L;
    let l = s.generate().unwrap();
    s.ecc = EccLevel::H;
    let h = s.generate().unwrap();
    assert!(h.modules > l.modules);
}

#[test]
fn model_is_editable_and_serialises() {
    let spec = QrSpec {
        content: QrContent::Wifi { ssid: "home".into(), password: "secret".into(), security: WifiSecurity::Wpa, hidden: false },
        ecc: EccLevel::Q,
        module_mm: 0.8,
        quiet_zone: 2,
        merged: true,
    };
    let json = serde_json::to_string(&spec).unwrap();
    let back: QrSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(back, spec);
    let a = spec.generate().unwrap();
    let b = back.generate().unwrap();
    assert_eq!(a.matrix, b.matrix);
    // edit: change the password, symbol regenerates differently
    let mut edited = back;
    if let QrContent::Wifi { password, .. } = &mut edited.content {
        *password = "other".into();
    }
    assert_ne!(edited.generate().unwrap().matrix, a.matrix);
}

#[test]
fn qr_follows_the_variable_text_index() {
    let spec = text_qr("ID-{serial:4}");
    let state = VariableState { serial: SerialConfig { start: 1, end: 100, current: 7, advance_by: 1, padding: 0 }, ..Default::default() };
    let ctx = EvalContext::new(NaiveDate::from_ymd_opt(2026, 1, 2).unwrap().and_hms_opt(3, 4, 5).unwrap());
    let a = spec.generate_with(&|s| state.evaluate(s, &ctx)).unwrap();
    assert_eq!(a.payload, "ID-0007");
    let n = state.next();
    let b = spec.generate_with(&|s| n.evaluate(s, &ctx)).unwrap();
    assert_eq!(b.payload, "ID-0008");
    assert_ne!(a.matrix, b.matrix);
    // Wi-Fi fields are evaluated too, before escaping.
    let w = QrSpec {
        content: QrContent::Wifi { ssid: "net-{serial}".into(), password: "x".into(), security: WifiSecurity::Wpa, hidden: false },
        ..Default::default()
    };
    assert_eq!(w.generate_with(&|s| n.evaluate(s, &ctx)).unwrap().payload, "WIFI:T:WPA;S:net-8;P:x;;");
}
