use chrono::NaiveDate;
use darkburn_text::*;

fn ctx() -> EvalContext {
    EvalContext::new(NaiveDate::from_ymd_opt(2026, 3, 9).unwrap().and_hms_opt(14, 5, 7).unwrap())
}

fn serial(start: i64, end: i64, current: i64, adv: i64, pad: usize) -> VariableState {
    VariableState { serial: SerialConfig { start, end, current, advance_by: adv, padding: pad }, ..Default::default() }
}

#[test]
fn serial_numbers_pad_advance_and_wrap() {
    let s = serial(1, 3, 1, 1, 4);
    assert_eq!(s.evaluate("N{serial}", &ctx()), "N0001");
    assert_eq!(s.evaluate("{serial:2}", &ctx()), "01");
    let s = s.next();
    assert_eq!(s.serial.current, 2);
    let s = s.next().next();
    assert_eq!(s.serial.current, 1, "wraps to start after end");
    let back = s.previous();
    assert_eq!(back.serial.current, 3, "previous wraps to the last value");
    assert_eq!(s.advanced(2).serial.current, 3);
    // step 5 from 10..30
    let s = serial(10, 30, 30, 5, 0);
    assert_eq!(s.next().serial.current, 10);
    let s = serial(10, 32, 10, 5, 0);
    assert_eq!(s.previous().serial.current, 30, "last reachable value");
    // counting down
    let d = serial(10, 7, 10, -1, 0);
    assert_eq!(d.next().serial.current, 9);
    assert_eq!(serial(7, 10, 7, -1, 0).next().serial.current, 10);
}

#[test]
fn reset_restores_start_and_first_row() {
    let mut s = serial(5, 9, 8, 1, 0);
    s.merge = MergeSource::from_csv_str("a\nb\nc\n", false).unwrap();
    s.merge.current_row = 2;
    let r = s.reset();
    assert_eq!(r.serial.current, 5);
    assert_eq!(r.merge.current_row, 0);
    assert_eq!(s.serial.current, 8, "input untouched");
}

#[test]
fn date_and_time_formats() {
    let s = VariableState::default();
    let c = ctx();
    assert_eq!(s.evaluate("{date}", &c), "2026-03-09");
    assert_eq!(s.evaluate("{date:%d.%m.%Y}", &c), "09.03.2026");
    assert_eq!(s.evaluate("{time:%H:%M}", &c), "14:05");
    assert_eq!(s.evaluate("{time}", &c), "14:05:07");
    assert_eq!(s.evaluate("{now:%a %b %e}", &c), "Mon Mar  9");
    assert_eq!(s.evaluate("{date:%y%m%d}-{time:%H%M%S}", &c), "260309-140507");
    // invalid strftime code does not panic and shows the format
    assert_eq!(s.evaluate("{date:%Q}", &c), "%Q");
    // whole-object date/time type
    assert_eq!(s.evaluate_kind(VarKind::DateTime, "%Y/%m/%d %H:%M", &c), "2026/03/09 14:05");
    assert_eq!(s.evaluate_kind(VarKind::Normal, "%Y", &c), "%Y");
}

const CSV: &str = "name,qty,note\r\n\"Smith, Anna\",3,\"line one\nline two\"\r\nBob,12,\"say \"\"hi\"\"\"\r\n\r\nCy,7,\r\n";

#[test]
fn csv_merge_quoted_and_multiline() {
    let m = MergeSource::from_csv_str(CSV, true).unwrap();
    assert_eq!(m.row_count(), 3);
    assert_eq!(m.column_names(), vec!["name", "qty", "note"]);
    assert_eq!(m.value(0, 0), Some("Smith, Anna"));
    assert_eq!(m.value(0, 2), Some("line one\nline two"));
    assert_eq!(m.value(1, 2), Some("say \"hi\""));
    assert_eq!(m.value(2, 2), Some(""));
    let mut s = VariableState { merge: m, ..Default::default() };
    assert_eq!(s.evaluate("{csv:name} x{csv:qty}", &ctx()), "Smith, Anna x3");
    assert_eq!(s.evaluate("{csv:1}|{csv}", &ctx()), "3|Smith, Anna");
    s = s.next();
    assert_eq!(s.evaluate("{csv:note}", &ctx()), "say \"hi\"");
    s = s.next().next();
    assert_eq!(s.merge.current_row, 0, "wraps");
    assert_eq!(s.previous().merge.current_row, 2);
    // unknown column stays visible, missing cell is empty
    assert_eq!(s.evaluate("{csv:nope}", &ctx()), "{csv:nope}");
    assert_eq!(s.evaluate("{csv:9}", &ctx()), "");
}

#[test]
fn csv_without_header_semicolons_and_bom() {
    let m = MergeSource::from_csv_str("\u{feff}a;b\n1;2\n", false).unwrap();
    assert_eq!(m.row_count(), 2);
    assert_eq!(m.value(1, 1), Some("2"));
    let t = MergeSource::from_csv_str("x\ty\n", false).unwrap();
    assert_eq!(t.rows[0], vec!["x", "y"]);
    let empty = VariableState::default();
    assert_eq!(empty.evaluate("[{csv:0}]", &ctx()), "[]");
}

#[test]
fn csv_file_load_relocate_clear() {
    let dir = std::env::temp_dir().join(format!("darkburn-text-vt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.csv");
    let b = dir.join("moved.csv");
    std::fs::write(&a, "h\n1\n2\n3\n").unwrap();
    let mut m = MergeSource::load(&a, true).unwrap();
    assert_eq!(m.row_count(), 3);
    m.current_row = 2;
    // the file moves; project keeps its embedded rows, relocation re-reads from the new place
    std::fs::rename(&a, &b).unwrap();
    assert!(m.reload().is_err());
    assert_eq!(m.value(2, 0), Some("3"), "rows are embedded");
    std::fs::write(&b, "h\n1\n2\n").unwrap();
    let r = m.relocate(&b).unwrap();
    assert_eq!(r.row_count(), 2);
    assert_eq!(r.current_row, 1, "row clamped");
    assert_eq!(r.path.as_deref(), Some(b.to_string_lossy().as_ref()));
    let c = r.cleared();
    assert!(c.rows.is_empty() && c.path.is_none() && c.header);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn cut_setting_values() {
    let cut = CutValues { speed: 300.0, power: 62.5, interval: 0.1, passes: 2, layer: 5, laser_name: "Gweike M3 Ultra".into() };
    let c = ctx().with_cut(cut);
    let s = VariableState::default();
    assert_eq!(s.evaluate("S{cut:S} P{cut:P} I{cut:I} N{cut:n} C{cut:C}", &c), "S300 P62.5 I0.1 N2 C5");
    assert_eq!(s.evaluate("{cut:L}", &c), "Gweike M3 Ultra");
    assert_eq!(s.evaluate("{cut:L7,2}", &c), "M3");
    assert_eq!(s.evaluate("{cut:L99,2}", &c), "");
    assert_eq!(s.evaluate("{cut:Z}", &c), "{cut:Z}");
    // without cut info the token is left alone
    assert_eq!(s.evaluate("{cut:S}", &ctx()), "{cut:S}");
}

#[test]
fn mixed_templates_braces_and_unknowns() {
    let mut s = serial(1, 99, 7, 1, 3);
    s.merge = MergeSource::from_csv_str("id,who\nA1,Ann\n", true).unwrap();
    let t = "Part {serial} / {csv:id} / {csv:who} / {date:%Y}";
    assert_eq!(s.evaluate(t, &ctx()), "Part 007 / A1 / Ann / 2026");
    assert_eq!(s.evaluate("{{literal}} {serial}", &ctx()), "{literal} 007");
    assert_eq!(s.evaluate("{unknown} and { open", &ctx()), "{unknown} and { open");
    assert_eq!(s.evaluate("ünï {serial:1} ✓", &ctx()), "ünï 7 ✓");
    assert_eq!(s.evaluate("", &ctx()), "");
}

#[test]
fn test_bake_and_auto_advance() {
    let s = serial(1, 3, 2, 1, 2);
    let c = ctx();
    let preview = s.test("#{serial}", &c, 4);
    assert_eq!(preview, vec!["#02", "#03", "#01", "#02"]);
    assert_eq!(s.serial.current, 2, "test does not change the state");
    assert_eq!(s.bake("#{serial}", &c), "#02");

    let mut a = s.clone();
    assert_eq!(a.after_job(true), a, "auto-advance is off");
    a.auto_advance = true;
    assert_eq!(a.after_job(true).serial.current, 3);
    assert_eq!(a.after_job(false).serial.current, 2, "no variable text in the job");
}

#[test]
fn array_copies_use_advanced_state() {
    let s = serial(100, 200, 100, 10, 0);
    let c = ctx();
    let copies: Vec<String> = (0..4).map(|i| s.advanced(i).evaluate("{serial}", &c)).collect();
    assert_eq!(copies, vec!["100", "110", "120", "130"]);
}

#[test]
fn state_serialises() {
    let mut s = serial(1, 10, 4, 2, 3);
    s.auto_advance = true;
    s.merge = MergeSource::from_csv_str("a,b\n1,2\n", true).unwrap();
    let j = serde_json::to_string(&s).unwrap();
    let back: VariableState = serde_json::from_str(&j).unwrap();
    assert_eq!(back, s);
    let d: VariableState = serde_json::from_str("{}").unwrap();
    assert_eq!(d.serial.current, 1);
}
