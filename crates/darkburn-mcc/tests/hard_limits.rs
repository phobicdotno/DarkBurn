//! The hard limits of `docs/BUILD-RULES.md`, checked from outside the crate through its public API.

use darkburn_mcc::command::{self, MachineParams, Policy, SafeWrite};
use darkburn_mcc::crc::CrcOrder;
use darkburn_mcc::fifo::FifoItem;
use darkburn_mcc::frame::{Request, RequestError};
use darkburn_mcc::registers::{self, Forbidden};
use std::path::Path;

/// Small deterministic generator, so the property tests need no dependency.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 24
    }
}

fn read_sources() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") {
            out.push((path.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(&path).unwrap()));
        }
    }
    out
}

#[test]
fn the_crate_has_no_network_or_serial_code() {
    let banned = [
        "UdpSocket",
        "TcpStream",
        "TcpListener",
        "std::net",
        "tokio",
        "serialport",
        "mio::",
        "socket2",
        "ToSocketAddrs",
        "hidapi",
        "nusb",
        "rusb",
        "winhttp",
        "reqwest",
        "ureq",
    ];
    for (name, text) in read_sources() {
        for b in banned {
            assert!(!text.contains(b), "{name} mentions {b}");
        }
    }
}

#[test]
fn the_manifest_has_no_transport_dependency() {
    let manifest = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    for dep in ["serialport", "tokio", "mio", "socket2", "reqwest", "ureq", "nusb", "rusb", "hidapi", "async-std", "smol"] {
        assert!(!manifest.contains(dep), "Cargo.toml mentions {dep}");
    }
}

#[test]
fn no_remote_monitor_or_cloud_endpoint_exists() {
    for (name, text) in read_sources() {
        // comments may name what is absent; code may not
        let text: String = text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join(
            "
",
        );
        // the vendor monitor lives in Alibaba Cloud space on port 9001; none of it is here
        assert!(!text.contains("47.104"), "{name}");
        assert!(!text.contains("9001"), "{name}");
        assert!(!text.to_lowercase().contains("cloud"), "{name}");
        // and no source line builds a socket address of the card network either
        assert!(!text.contains("10.1.1."), "{name}");
    }
}

#[test]
fn licence_registers_and_the_two_command_vectors_are_unrepresentable() {
    // every register that is not exactly one of the three typed registers
    for reg in 0..70_000u32 {
        if matches!(reg, 0x65..=0x67) {
            continue;
        }
        assert!(SafeWrite::try_from_raw(reg, &[0]).is_err(), "register {reg}");
        assert!(SafeWrite::try_from_raw(reg, &[5555]).is_err(), "register {reg}");
        assert!(SafeWrite::try_from_raw(reg, &[9999]).is_err(), "register {reg}");
    }
    assert!(matches!(SafeWrite::try_from_raw(150, &[5555]), Err(Forbidden::Register150Or151(_))));
    assert!(matches!(SafeWrite::try_from_raw(151, &[0]), Err(Forbidden::Register150Or151(_))));
    assert!(matches!(SafeWrite::try_from_raw(59500, &[0]), Err(Forbidden::LicenceArea(_))));
    assert!(matches!(SafeWrite::try_from_raw(59511, &[0]), Err(Forbidden::LicenceArea(_))));
    // every single-word write to the command register, all 65536 values, is refused: this covers
    // 0x65 <- [0x65] and 0x65 <- [0x66]
    for v in 0..=u16::MAX as u32 {
        assert!(SafeWrite::try_from_raw(0x65, &[v]).is_err(), "[{v}]");
    }
    // reads of the same areas
    for addr in [150, 151, 59500, 59502, 59503, 59599] {
        assert!(Request::read(addr, 1).is_err(), "{addr}");
    }
}

#[test]
fn random_vectors_only_pass_through_the_allow_list() {
    let mut rng = Lcg(0x5eed);
    let mut accepted = 0;
    for _ in 0..200_000 {
        let n = (rng.next() % 10) as usize;
        let sub = [1u64, 2, 3, 5, 9999, 101, 102, 4, 7, 107, 118, 0, 0x65, 0x66][(rng.next() % 14) as usize] as u32;
        let mut words = vec![sub];
        for _ in 0..n {
            // favour small values and the values of real vectors
            let w = match rng.next() % 5 {
                0 => rng.next() % 4,
                1 => 0x8000_0000 | (rng.next() % 4) as u32 as u64,
                2 => rng.next() % 0x20000,
                3 => 0xFFFF,
                _ => rng.next() & 0xFFFF_FFFF,
            } as u32;
            words.push(w);
        }
        if let Ok(w) = SafeWrite::try_from_raw(0x65, &words) {
            accepted += 1;
            // anything accepted is one of the documented vectors, never a forbidden one
            assert!(matches!(words[0], 1 | 2 | 3 | 5 | 9999), "{words:?}");
            assert_ne!(w.policy(), Policy::Laser, "{words:?}");
            if words[0] == 9999 {
                assert!(matches!(words[1], 2..=4 | 5 | 0x11), "{words:?}");
                if words[1] == 4 {
                    assert_eq!(words[3], 0, "no non-zero analogue output: {words:?}");
                }
                if matches!(words[1], 3 | 0x11) {
                    assert!(words[3..].iter().all(|&x| x == 0), "{words:?}");
                }
                if words[1] == 2 {
                    // output writes switch nothing on that is a laser port
                    assert_eq!(words[3] & ((1 << 4) | (1 << 5) | (1 << 8)) & words[2], 0, "{words:?}");
                }
            }
        }
    }
    assert!(accepted > 0, "the generator never hit a valid vector, the test would prove nothing");
}

#[test]
fn random_fifo_words_never_yield_laser_data() {
    let mut rng = Lcg(77);
    let mut accepted = 0;
    for _ in 0..200_000 {
        let n = (rng.next() % 12) as usize;
        let mut words = vec![(rng.next() % 5) as u32];
        let mut i = 0;
        while i < n {
            // mostly well-formed headers so the parser gets past the first item
            let (op, payload) = match rng.next() % 8 {
                0 => (3000u32, 2u32),
                1 => (3001, 0),
                2 => (3002, 1),
                3 => (9999, 3),
                4 => (103, 2),
                5 => (2002, 1),
                6 => (3, 2),
                _ => ((rng.next() % 4000) as u32, (rng.next() % 4) as u32),
            };
            words.push((payload * 4) << 16 | op);
            for _ in 0..payload {
                words.push(match rng.next() % 4 {
                    0 => 0,
                    1 => 0x100,
                    2 => (5000 << 16) | (rng.next() % 3) as u32,
                    _ => rng.next() as u32 & 0xFFFF,
                });
            }
            i += 1;
        }
        if SafeWrite::try_from_raw(0x66, &words).is_ok() {
            accepted += 1;
            let frame = darkburn_mcc::fifo::parse_fifo_words(&words).unwrap();
            assert!(frame.items.iter().all(|i| !i.is_laser()), "{words:?}");
        }
    }
    assert!(accepted > 100);
}

#[test]
fn decoding_hand_built_frames_cannot_smuggle_a_forbidden_write() {
    // Build a datagram by hand (the crate gives no public builder for arbitrary frames).
    fn frame(func: u8, payload: &[u8]) -> Vec<u8> {
        let mut tail = vec![];
        tail.extend_from_slice(&((2 + payload.len()) as u16).to_be_bytes());
        tail.push(0);
        tail.push(func);
        tail.extend_from_slice(payload);
        let crc = darkburn_mcc::crc16_modbus(&tail);
        let mut out = vec![0, 1];
        out.extend_from_slice(&darkburn_mcc::crc::crc_bytes(crc, CrcOrder::HiFirst));
        out.extend_from_slice(&tail);
        out
    }
    fn write(addr: u32, words: &[u32]) -> Vec<u8> {
        let mut p = addr.to_le_bytes().to_vec();
        p.extend_from_slice(&(words.len() as u16).to_le_bytes());
        for w in words {
            p.extend_from_slice(&w.to_le_bytes());
        }
        frame(0x40, &p)
    }
    for (addr, words) in [
        (150u32, vec![9999u32]),
        (151, vec![1]),
        (59500, vec![1]),
        (59600, vec![0]),
        (0x65, vec![0x65]),
        (0x65, vec![0x66]),
        (11002, vec![1]),
        (100, vec![9999]),
    ] {
        assert!(matches!(Request::decode(&write(addr, &words)), Err(RequestError::Forbidden(_))), "{addr} {words:?}");
    }
    // a byte block and the two streams never become requests
    let mut bb = 0x1000u32.to_le_bytes().to_vec();
    bb.extend_from_slice(&1u16.to_le_bytes());
    bb.push(0xAA);
    for func in [0x26u8, 0x20, 0x21] {
        assert_eq!(Request::decode(&frame(func, &bb)), Err(RequestError::Forbidden(Forbidden::Function(func))));
    }
    // a real, allowed write decodes
    let stop = command::stop_all_no_decel().unwrap();
    assert_eq!(Request::decode(&write(0x65, stop.words())).unwrap().1, Request::write(stop));
}

#[test]
fn constructors_never_produce_laser_policy_or_forbidden_registers() {
    let p = MachineParams::default();
    let mut all = vec![
        command::connect_prologue().unwrap(),
        command::jog_continuous(0, true, &p, None).unwrap(),
        command::jog_step(1, -3.0, &p).unwrap(),
        command::move_axis_absolute(0, 10.0, &p).unwrap(),
        command::go_to(10.0, 10.0, &p).unwrap(),
        command::home_axis(0).unwrap(),
        command::home_axis(1).unwrap(),
        command::jog_release_stop(&[0, 1], 200_000, &p).unwrap(),
        command::stop_all(0, &p).unwrap(),
        command::stop_all_no_decel().unwrap(),
        command::fifo_clear().unwrap(),
        command::fifo_start().unwrap(),
        command::fifo_stop().unwrap(),
        command::da_zero(1).unwrap(),
        command::da_zero(2).unwrap(),
        command::pwm_off(1234, 3).unwrap(),
        command::pwm_off(1234, 0x11).unwrap(),
        command::do_bulk_off(0, &[5, 6, 9]).unwrap(),
        command::fifo_frame(1, &[FifoItem::boundary(), FifoItem::mode(4), FifoItem::tick(5, -5)]).unwrap(),
    ];
    for port in 1..=10 {
        all.push(command::do_off(port).unwrap());
    }
    all.extend(command::stop_sequence(&p, false, 0, 0, &[5, 6, 9]).unwrap());
    all.extend(command::stop_sequence(&p, true, 0, 0b100, &[5, 6, 9]).unwrap());
    for w in all {
        assert_ne!(w.policy(), Policy::Laser, "{}", w.name());
        assert!(matches!(w.register(), 0x65..=0x67), "{}", w.name());
        assert_ne!(w.words(), [0x65], "{}", w.name());
        assert_ne!(w.words(), [0x66], "{}", w.name());
        assert!(registers::BLOCKS.iter().any(|b| b.start == w.register()));
    }
}

#[test]
fn follower_and_pierce_records_cannot_ride_in_a_fifo_frame() {
    // The same records are refused as 0x65 commands ([103, ..] follower move, [104, ..] follow,
    // [118, ..] pierce). Inside a FIFO frame they are just as much the vendor interlocks' job:
    // they decode, so the simulator can model them, but no write can be built from them.
    let item = |op: u32, payload: &[u32]| {
        let mut w = vec![1u32, ((payload.len() as u32 * 4) << 16) | op];
        w.extend_from_slice(payload);
        w
    };
    let cases = [
        item(103, &[1000, 0]),
        item(104, &[1, 2]),
        item(104, &[1, 2, 0, 0, 0, 0, 0, 0]),
        item(105, &[0, 0, 0, 0]),
        item(106, &[0, 0, 0, 0, 0, 0]),
        item(108, &[0, 0, 0, 0]),
        item(109, &[0, 0]),
        item(118, &[5, 0, 0]),
        item(2001, &[0, 0]),
    ];
    for words in &cases {
        assert!(darkburn_mcc::fifo::parse_fifo_words(words).is_ok(), "{words:?} is in the grammar");
        assert!(SafeWrite::try_from_raw(0x66, words).is_err(), "{words:?} must not be a write this crate can build");
        // The same through a datagram decoded off the wire.
        let mut payload = Vec::new();
        payload.extend_from_slice(&0x66u32.to_le_bytes());
        payload.extend_from_slice(&(words.len() as u16).to_le_bytes());
        for w in words {
            payload.extend_from_slice(&w.to_le_bytes());
        }
        let mut tail = vec![];
        tail.extend_from_slice(&((2 + payload.len()) as u16).to_be_bytes());
        tail.push(0);
        tail.push(0x40);
        tail.extend_from_slice(&payload);
        let crc = darkburn_mcc::crc16_modbus(&tail);
        let mut frame = vec![0u8, 9];
        frame.extend_from_slice(&darkburn_mcc::crc::crc_bytes(crc, CrcOrder::HiFirst));
        frame.extend_from_slice(&tail);
        assert!(Request::decode(&frame).is_err(), "{words:?} decoded into a request");
    }
    // A dry frame next to them still builds.
    let mut dry = vec![1u32];
    for i in [FifoItem::boundary(), FifoItem::mode(4), FifoItem::tick(3, -3)] {
        dry.extend(i.words());
    }
    assert_eq!(SafeWrite::try_from_raw(0x66, &dry).unwrap().policy(), Policy::Motion);
}
