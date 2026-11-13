//! EGV distances.
//!
//! A distance in EGV is a count of steps (mils, 1/1000 inch) written after a direction or
//! laser letter. Whole blocks of 255 are written as `z`, the rest as one of three forms:
//!
//! * `1..=25`: one letter, `a` (1) to `y` (25);
//! * `26..=51`: a pipe and a letter, `|a` (26) to `|z` (51) (the pipe counts 25 and `z` after a
//!   pipe counts 26);
//! * `52..=254`: three digits, `052` to `254`.
//!
//! Zero is written as nothing.

/// Appends the EGV form of `v` steps to `out`.
pub fn push_distance(out: &mut String, v: u32) {
    let mut v = v;
    for _ in 0..v / 255 {
        out.push('z');
    }
    v %= 255;
    match v {
        0 => {}
        1..=25 => out.push(char::from(b'a' + (v as u8 - 1))),
        26..=51 => {
            out.push('|');
            out.push(char::from(b'a' + (v as u8 - 26)));
        }
        _ => {
            out.push_str(&format!("{v:03}"));
        }
    }
}

/// The EGV form of `v` steps.
pub fn encode_distance(v: u32) -> String {
    let mut s = String::new();
    push_distance(&mut s, v);
    s
}

/// Reads a whole string of distance characters back into steps. `None` if it holds anything
/// else, or digits that are not in groups of three.
pub fn decode_distance(text: &str) -> Option<u32> {
    let mut total: u32 = 0;
    let mut pipe = false;
    let mut digits = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            if digits.len() == 3 {
                total = total.checked_add(digits.parse::<u32>().ok()?)?;
                digits.clear();
            }
            continue;
        }
        if !digits.is_empty() {
            return None;
        }
        match c {
            '|' => {
                total = total.checked_add(25)?;
                pipe = true;
                continue;
            }
            'a'..='y' => total = total.checked_add(c as u32 - 'a' as u32 + 1)?,
            'z' => total = total.checked_add(if pipe { 26 } else { 255 })?,
            _ => return None,
        }
        pipe = false;
    }
    if !digits.is_empty() {
        return None;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_values_use_letters() {
        assert_eq!(encode_distance(0), "");
        assert_eq!(encode_distance(1), "a");
        assert_eq!(encode_distance(22), "v");
        assert_eq!(encode_distance(25), "y");
    }

    #[test]
    fn middle_values_use_a_pipe() {
        assert_eq!(encode_distance(26), "|a");
        assert_eq!(encode_distance(27), "|b");
        assert_eq!(encode_distance(51), "|z");
    }

    #[test]
    fn larger_values_use_three_digits() {
        assert_eq!(encode_distance(52), "052");
        assert_eq!(encode_distance(139), "139");
        assert_eq!(encode_distance(254), "254");
    }

    #[test]
    fn blocks_of_255_are_z() {
        assert_eq!(encode_distance(255), "z");
        assert_eq!(encode_distance(256), "za");
        assert_eq!(encode_distance(394), "z139");
        assert_eq!(encode_distance(787), "zzzv");
        assert_eq!(encode_distance(510 + 30), "zz|e");
    }

    #[test]
    fn every_value_round_trips() {
        for v in 0..5000 {
            assert_eq!(decode_distance(&encode_distance(v)), Some(v), "{v}");
        }
    }

    #[test]
    fn decode_rejects_other_text() {
        assert_eq!(decode_distance("B"), None);
        assert_eq!(decode_distance("12"), None);
        assert_eq!(decode_distance("1a"), None);
    }
}
