//! Whatever a board (or a damaged file) sends, the driver must not panic.

use darkburn_galvo::*;

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
        let b = r.bytes(64);
        let _ = Command::from_bytes(&b);
        let _ = Reply::from_bytes(&b);
        let _ = ListPacket::from_bytes(&b);
        let _ = CorTable::parse(&b);
        let _ = darkburn_galvo::ezcad::import(&b);
    }
    // Sizes around the real ones.
    for n in [0usize, 1, 7, 8, 11, 12, 13, 3071, 3072, 3073, 4096, 20_000] {
        for _ in 0..20 {
            let b: Vec<u8> = (0..n).map(|_| r.next() as u8).collect();
            let _ = Command::from_bytes(&b);
            let _ = Reply::from_bytes(&b);
            if let Ok(p) = ListPacket::from_bytes(&b) {
                let _ = p.commands();
                let _ = p.payload();
            }
            let _ = CorTable::parse(&b);
            let _ = darkburn_galvo::ezcad::import(&b);
        }
    }
}
