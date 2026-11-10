//! Whatever a controller (or a damaged file) sends, the driver must not panic.

use std::time::Duration;

use darkburn_ruida::command::{Command, PanelCommand, parse_commands, split_commands};
use darkburn_ruida::*;

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
        let b = r.bytes(40);
        let _ = Command::decode(&b);
        let _ = parse_commands(&b);
        let _ = split_commands(&b);
        let _ = PanelCommand::decode(&b);
        let _ = RdJob::from_logical(&b);
        let _ = RdJob::from_rd_file(&b, None);
        let _ = RdJob::from_rd_file(&b, Some(Magic::DEFAULT));
        let _ = darkburn_ruida::swizzle::detect_magic_histogram(&b);
        let _ = darkburn_ruida::encoding::decode_cstring(&b);
        let _ = darkburn_ruida::packet::unframe(&b);
    }
}

/// A link that answers every datagram with an acknowledge and then with noise.
struct Noise {
    rng: Rng,
    queue: Vec<Vec<u8>>,
    framing: Framing,
}

impl Transport for Noise {
    fn framing(&self) -> Framing {
        self.framing
    }
    fn send(&mut self, _channel: Channel, _data: &[u8]) -> Result<()> {
        if self.framing == Framing::Udp {
            self.queue.push(vec![darkburn_ruida::swizzle::swizzle_byte(0xCC, Magic::DEFAULT)]);
        }
        for _ in 0..self.rng.below(3) {
            let logical = self.rng.bytes(30);
            self.queue.push(darkburn_ruida::swizzle::swizzle(&logical, Magic::DEFAULT));
        }
        self.queue.reverse();
        Ok(())
    }
    fn recv(&mut self, _channel: Channel, _timeout: Duration) -> Result<Option<Vec<u8>>> {
        Ok(self.queue.pop())
    }
    fn describe(&self) -> String {
        "noise".into()
    }
}

#[test]
fn the_driver_survives_a_controller_that_talks_nonsense() {
    for (seed, framing) in [(1u64, Framing::Udp), (2, Framing::Stream), (3, Framing::Udp), (4, Framing::Stream)] {
        let t = Noise { rng: Rng(seed * 0x2545_F491_4F6C_DD1D), queue: Vec::new(), framing };
        let mut m = RuidaMachine::new(t, MachineConfig { retries: 1, ..Default::default() });
        for _ in 0..300 {
            let _ = m.connect();
            let _ = m.status();
            let _ = m.list_files();
            let _ = m.read_controller_info();
            let _ = m.calc_time(3);
            let _ = m.calc_time(u16::MAX);
            let _ = m.jog_step(Axis::X, 1.0e12);
            let _ = m.jog_step(Axis::Z, f64::NAN);
            let _ = m.stop();
        }
    }
}
