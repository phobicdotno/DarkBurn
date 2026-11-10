//! Whatever a controller sends, the driver must not panic.

use darkburn_grbl::protocol::*;
use darkburn_grbl::*;

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
fn parsers_take_any_text() {
    let mut r = Rng(0xD1B5_4A32_D192_ED03);
    let alphabet = [
        "<",
        ">",
        "|",
        ",",
        ":",
        "Idle",
        "Run",
        "Hold",
        "Alarm",
        "Door",
        "MPos",
        "WPos",
        "WCO",
        "FS",
        "F",
        "Bf",
        "Ov",
        "Pn",
        "A",
        "Ln",
        "0",
        "1",
        "9",
        "-",
        "+",
        ".",
        " ",
        "$",
        "=",
        "[",
        "]",
        "MSG",
        "VER",
        "OPT",
        "ok",
        "error",
        "ALARM",
        "Grbl ",
        "GrblHAL ",
        "FluidNC",
        "é",
        "→",
        "G",
        "X",
        "(",
        ")",
        ";",
        "nan",
        "inf",
        "1e400",
        "$J=",
        "65536",
        "4294967296",
        "99999999999999999999",
    ];
    for _ in 0..40_000 {
        let t = r.text(&alphabet, 24);
        let _ = parse_response(&t);
        let _ = parse_status(&t);
        let _ = parse_banner(&t);
        let _ = parse_version(&t);
        let _ = clean_line(&t);
        let _ = is_motion_line(&t);
        let _ = words(&t);
        let _ = SettingsBackup::parse(&t);
        let mut fw = parse_banner("Grbl 1.1h ['$' for help]").unwrap();
        apply_build_info(&mut fw, "VER", &t);
        apply_build_info(&mut fw, "OPT", &t);
    }
}

/// A link that says whatever the test queued, whatever it is sent.
struct Babble {
    lines: Vec<u8>,
    at: usize,
}

impl Transport for Babble {
    fn write_all(&mut self, _data: &[u8]) -> std::io::Result<()> {
        Ok(())
    }
    fn read(&mut self, buf: &mut [u8], _timeout: std::time::Duration) -> std::io::Result<usize> {
        if self.at >= self.lines.len() {
            std::thread::sleep(std::time::Duration::from_millis(1));
            return Ok(0);
        }
        let n = buf.len().min(self.lines.len() - self.at).min(97);
        buf[..n].copy_from_slice(&self.lines[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
    fn name(&self) -> String {
        "babble".into()
    }
}

#[test]
fn the_driver_survives_a_controller_that_talks_nonsense() {
    let mut r = Rng(0x2545_F491_4F6C_DD1D);
    let alphabet = [
        "<",
        ">",
        "|",
        ",",
        ":",
        "Idle",
        "Run",
        "Hold:0",
        "Alarm",
        "MPos:",
        "WPos:",
        "WCO:",
        "FS:",
        "Bf:",
        "Ov:",
        "0",
        "1",
        "9",
        "-",
        ".",
        " ",
        "$",
        "=",
        "[MSG:",
        "]",
        "[VER:",
        "[OPT:",
        "ok\n",
        "ok\n",
        "ok\n",
        "error:",
        "ALARM:",
        "\n",
        "\n",
        "\r\n",
        "é",
        "nan",
        "$13=1\n",
        "$130=",
        "$30=0\n",
        "Grbl 1.1h ['$' for help]\n",
        "Grbl 0.9j ['$' for help]\n",
        "<Idle|MPos:0,0,0|FS:0,0>\n",
        "<Hold,MPos:1,2,3,WPos:1,2,3>\n",
        "99999999999999999999",
    ];
    for round in 0..2 {
        let mut text = String::from("Grbl 1.1h ['$' for help]\nok\nok\n");
        text.push_str(&r.text(&alphabet, 4000));
        let mut bytes = text.into_bytes();
        // Some raw bytes that are not UTF-8.
        for _ in 0..50 {
            let i = r.below(bytes.len());
            bytes[i] = r.next() as u8;
        }
        let m = GrblMachine::connect(
            Box::new(Babble { lines: bytes, at: 0 }),
            ConnectOptions { banner_timeout: std::time::Duration::from_millis(200), ..Default::default() },
        );
        let end = std::time::Instant::now() + std::time::Duration::from_millis(700);
        while std::time::Instant::now() < end {
            let _ = m.poll_events();
            let _ = m.jog(JogStep { dx: 1.0, feed: 600.0, ..Default::default() });
            let _ = m.start_job(Job::from_text("G1 X1 F100\nG1 X2"));
            let _ = m.stop();
            let _ = m.home();
            let _ = m.fire(1.0, std::time::Duration::from_millis(50));
            let _ = m.read_settings();
            let _ = m.pause();
            let _ = m.resume();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // A panic in the worker would poison nothing visible here, so ask the thread itself:
        // the snapshot keeps being served, and disconnect joins the worker.
        let _ = m.snapshot();
        m.disconnect();
        let _ = round;
    }
}
