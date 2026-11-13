//! Speed codes against 3064 codes written by the original board software, with the same
//! comparison rules as MeerK40t's `test_laser_speeds.py` (MIT; the code list comes from that
//! file, see `ATTRIBUTION.md`).

use darkburn_lihuiyu::speed::{Board, RasterStep, SpeedOptions, decode_16bit, speed_code, speed_from_code};

const CODES: &str = include_str!("data/speedcodes.txt");

struct Case<'a> {
    code: &'a str,
    board_name: &'a str,
    board: Board,
    speed: f64,
    created: String,
}

fn cases() -> Vec<Case<'static>> {
    let mut out = Vec::new();
    for line in CODES.lines().filter(|l| !l.trim().is_empty()) {
        let v: Vec<&str> = line.split(' ').collect();
        assert_eq!(v.len(), 4, "{line}");
        let board = Board::from_name(v[1]).unwrap_or_else(|| panic!("board {}", v[1]));
        let speed: f64 = v[2].parse().unwrap();
        let step: u32 = v[3].parse().unwrap();
        let opts = SpeedOptions { board, raster_step: (step != 0).then_some(RasterStep::Bidirectional(step)), ..Default::default() };
        out.push(Case { code: v[0], board_name: v[1], board, speed, created: speed_code(speed, &opts) });
    }
    out
}

fn part(s: &str, a: usize, b: usize) -> &str {
    s.get(a..b).unwrap_or_else(|| panic!("{s} has no [{a}..{b}]"))
}

#[test]
fn the_list_is_complete() {
    let c = cases();
    assert_eq!(c.len(), 3064);
    assert_eq!(c.iter().filter(|c| c.code.contains('G')).count(), 947);
    assert_eq!(c.iter().filter(|c| c.code.contains("V167")).count(), 54);
}

#[test]
fn values_match_within_one() {
    for c in cases() {
        if c.code.contains('G') {
            let (a, b) = (decode_16bit(part(&c.created, 1, 7)), decode_16bit(part(c.code, 1, 7)));
            assert!((a.unwrap() - b.unwrap()).abs() <= 1, "{} {}: made {} for {}", c.board_name, c.speed, c.created, c.code);
        } else if !c.code.contains("V167") {
            let (a, b) = (decode_16bit(part(&c.created, 2, 8)), decode_16bit(part(c.code, 2, 8)));
            assert!((a.unwrap() - b.unwrap()).abs() <= 1, "{} {}: made {} for {}", c.board_name, c.speed, c.created, c.code);
        }
    }
}

#[test]
fn acceleration_digits_match() {
    for c in cases() {
        let at = if c.code.contains('G') {
            7
        } else if !c.code.contains("V167") {
            8
        } else {
            continue;
        };
        assert_eq!(part(&c.created, at, at + 1), part(c.code, at, at + 1), "{} {}: made {} for {}", c.board_name, c.speed, c.created, c.code);
    }
}

#[test]
fn steps_and_diagonals_match() {
    for c in cases() {
        if matches!(c.board_name, "A" | "B" | "M") || c.code.contains('G') || c.code.contains("V167") {
            continue;
        }
        assert_eq!(part(&c.created, 9, 12), part(c.code, 9, 12), "{} {}: made {} for {}", c.board_name, c.speed, c.created, c.code);
        let v0 = decode_16bit(part(&c.created, 12, 18)).unwrap() as f64;
        let v1 = decode_16bit(part(c.code, 12, 18)).unwrap() as f64;
        assert!((v0 - v1).abs() <= (v1 / 3000.0).max(1.0), "{} {}: made {} for {}", c.board_name, c.speed, c.created, c.code);
    }
}

#[test]
fn codes_decode_to_their_speed() {
    for c in cases() {
        let g = c.code.contains('G');
        // Above 240 mm/s the original software wrote a 19.05 mm/s code; raster codes below
        // 7 mm/s are ambiguous.
        if (!g && c.speed > 240.0) || (g && c.speed < 7.0) {
            continue;
        }
        let s = speed_from_code(c.code, c.board, false).unwrap_or_else(|e| panic!("{}: {e}", c.code));
        assert!((s - c.speed).abs() <= c.speed / 100.0, "{} {}: {} reads as {s}", c.board_name, c.speed, c.code);
    }
}
