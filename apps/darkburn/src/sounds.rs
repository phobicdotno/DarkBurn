//! Sounds for errors and finished jobs [SET-22, LASER-25]. No audio library: a short tone is
//! generated as a WAV file in the temp folder and handed to the system's own player (PowerShell's
//! SoundPlayer on Windows, `afplay` on macOS, `aplay` elsewhere), on a background thread. Failure
//! is silent. Under test nothing is played: the request is only noted (see [`take_requests`]).

use crate::options::Options;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    JobDone,
    Error,
}

/// Whether the user wants `s` played.
pub fn wanted(opts: &Options, s: Sound) -> bool {
    match s {
        Sound::JobDone => opts.sound_done,
        Sound::Error => opts.sound_error,
    }
}

pub fn play_if(opts: &Options, s: Sound) {
    if wanted(opts, s) {
        play(s);
    }
}

/// Mono 16-bit 22.05 kHz WAV: a rising two-note chime for a finished job, a low double buzz for
/// an error.
pub fn wav(s: Sound) -> Vec<u8> {
    const RATE: u32 = 22_050;
    let notes: &[(f64, f64)] = match s {
        Sound::JobDone => &[(660.0, 0.12), (880.0, 0.2)],
        Sound::Error => &[(180.0, 0.15), (0.0, 0.05), (180.0, 0.15)],
    };
    let mut samples: Vec<i16> = Vec::new();
    for (hz, secs) in notes {
        let n = (secs * f64::from(RATE)) as usize;
        for i in 0..n {
            let t = i as f64 / f64::from(RATE);
            let fade = (1.0 - i as f64 / n as f64).min(i as f64 / 200.0).min(1.0);
            let v = if *hz == 0.0 { 0.0 } else { (t * hz * std::f64::consts::TAU).sin() * 0.4 * fade };
            samples.push((v * f64::from(i16::MAX)) as i16);
        }
    }
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for v in samples {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
thread_local! {
    static REQUESTS: std::cell::RefCell<Vec<Sound>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The sounds asked for on this thread since the last call (tests only: nothing is ever played
/// under test).
#[cfg(test)]
pub fn take_requests() -> Vec<Sound> {
    REQUESTS.with(|r| std::mem::take(&mut *r.borrow_mut()))
}

/// Play `s` now, whatever the settings say (the Settings window's try buttons).
pub fn play(s: Sound) {
    #[cfg(test)]
    {
        REQUESTS.with(|r| r.borrow_mut().push(s));
    }
    if cfg!(test) {
        return; // tests never make a noise
    }
    std::thread::spawn(move || {
        let path = std::env::temp_dir().join(match s {
            Sound::JobDone => "darkburn-done.wav",
            Sound::Error => "darkburn-error.wav",
        });
        if std::fs::write(&path, wav(s)).is_err() {
            return;
        }
        let mut cmd = if cfg!(windows) {
            let mut c = std::process::Command::new("powershell");
            c.args(["-NoProfile", "-NonInteractive", "-Command"]).arg(format!("(New-Object Media.SoundPlayer '{}').PlaySync()", path.display()));
            c
        } else if cfg!(target_os = "macos") {
            let mut c = std::process::Command::new("afplay");
            c.arg(&path);
            c
        } else {
            let mut c = std::process::Command::new("aplay");
            c.arg("-q").arg(&path);
            c
        };
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let _ = cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generated_files_are_valid_wavs() {
        for s in [Sound::JobDone, Sound::Error] {
            let w = wav(s);
            assert_eq!(&w[..4], b"RIFF");
            assert_eq!(&w[8..12], b"WAVE");
            let data_len = u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize;
            assert_eq!(w.len(), 44 + data_len);
            assert!(w[44..].iter().any(|b| *b != 0), "not silent");
        }
    }

    #[test]
    fn only_wanted_sounds_play() {
        let mut o = Options::default();
        assert!(!wanted(&o, Sound::JobDone) && !wanted(&o, Sound::Error));
        o.sound_done = true;
        assert!(wanted(&o, Sound::JobDone) && !wanted(&o, Sound::Error));
        o.sound_error = true;
        assert!(wanted(&o, Sound::Error));
    }

    #[test]
    fn a_wanted_sound_is_requested_and_an_unwanted_one_is_not() {
        let mut o = Options::default();
        take_requests();
        play_if(&o, Sound::JobDone);
        play_if(&o, Sound::Error);
        assert!(take_requests().is_empty(), "both are off by default");
        o.sound_done = true;
        play_if(&o, Sound::JobDone);
        play_if(&o, Sound::Error);
        assert_eq!(take_requests(), [Sound::JobDone]);
        o.sound_error = true;
        play_if(&o, Sound::Error);
        assert_eq!(take_requests(), [Sound::Error]);
        assert!(take_requests().is_empty(), "taking empties the list");
    }
}
