//! Whatever a card (or a damaged file) sends, nothing may panic.

use darkburn_mcc::*;

/// Small deterministic generator, so a failure can be repeated.
struct Rng(u64);
#[allow(dead_code)]
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn bytes(&mut self, max: usize) -> Vec<u8> {
        let n = self.below(max + 1);
        let narrow = self.below(3) == 0;
        (0..n)
            .map(|_| if narrow { [0x00, 0x7F, 0x80, 0xFF, 0xDA, 0x01, 0xE8, 0xCC, 0xD7, 0x88, 0xC6][self.below(11)] } else { self.next() as u8 })
            .collect()
    }
    fn text(&mut self, alphabet: &[&str], max: usize) -> String {
        let n = self.below(max + 1);
        (0..n).map(|_| alphabet[self.below(alphabet.len())]).collect()
    }
}

#[test]
fn decoders_take_any_bytes() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..20_000 {
        let b = r.bytes(80);
        let f = RawFrame::parse(&b);
        let _ = (f.crc_ok(), f.length_ok(), f.is_exception(), f.exception_code());
        let _ = Request::decode(&b);
        let _ = Reply::decode(&b, None);
        let _ = Reply::decode(&b, Some(r.next() as u16));
        let _ = read_chf(&b, false);
        let _ = read_chf(&b, true);
        let words: Vec<u32> = b.chunks(4).map(|c| c.iter().fold(0u32, |a, x| (a << 8) | u32::from(*x))).collect();
        let _ = darkburn_mcc::fifo::parse_fifo_words(&words);
        if let Some(s) = darkburn_mcc::registers::StatusBlock::from_words(&words) {
            let _ = darkburn_mcc::registers::derive_machine_state(&s, &words);
            let _ = darkburn_mcc::registers::alarm_codes(s.alarm1(), s.alarm2(), Some(&words));
            for port in [0u32, 1, 31, 32, 63, 64, 1000, u32::MAX] {
                let _ = (s.do_is_on(port), s.di_level(port));
            }
        }
        let _ = darkburn_mcc::registers::ZfStatus::from_words(&words);
        let _ = darkburn_mcc::command::SafeWrite::try_from_raw(words.first().copied().unwrap_or(0), &words);
    }
}

#[test]
fn text_parsers_take_any_text() {
    let mut r = Rng(0xD1B5_4A32_D192_ED03);
    let alphabet = [
        "0", "1", "9", "-", "+", ".", ",", " ", "\t", "\n", "\r", "x", "0x", "FIFO", "frame", ":", "=", "[", "]", "é", "→", "e", "E", "#", ";",
        "nan", "inf",
    ];
    for _ in 0..20_000 {
        let t = r.text(&alphabet, 30);
        let _ = darkburn_mcc::fifo::parse_frame_line(&t);
        let _ = darkburn_mcc::chf::ChfText::from_text(&t);
        let _ = read_chf(t.as_bytes(), false);
    }
}
