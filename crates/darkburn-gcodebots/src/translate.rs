//! Rewrites a GRBL-style job stream (`M3`/`M4`/`M5`, `S` 0..`in_s_max`, `M7`/`M8`/`M9`) into
//! the dialect of the target controller, applying the G-code Device Settings of DS-09..DS-15.
//!
//! The translator is a pure text-to-text function; the machine drivers do not call it by
//! themselves, so the application decides when a job is translated.

use darkburn_grbl::protocol::fmt_num;

use crate::profile::{DeviceKind, DeviceProfile, MarlinPowerMode, Units};

/// How laser power is written.
#[derive(Clone, Debug, PartialEq)]
pub enum PowerStyle {
    /// `M3`/`M4`/`M5` and `S` words stay (scaled to `out_s_max`): GRBL, Smoothieware, Marlin
    /// M3/M4.
    Passthrough,
    /// Marlin inline mode: `M3 I`/`M4 I`/`M5 I`, `S` on every `G1`.
    MarlinInline,
    /// Power is a separate command written before a move whenever it changes
    /// (`M106 P0 S128`, `M67 E0 Q50`). `set` has a `{value}` placeholder.
    Command { set: String, off: String },
}

/// Translation options.
#[derive(Clone, Debug)]
pub struct TranslateOptions {
    pub style: PowerStyle,
    /// S value that means 100 % in the input.
    pub in_s_max: f64,
    /// S value that means 100 % in the output.
    pub out_s_max: f64,
    /// Replaces `M7` and `M8`. Empty drops them.
    pub air_on: String,
    /// Replaces `M9`. Empty drops it.
    pub air_off: String,
    pub swap_xy: bool,
    /// Write `S` on every `G1` (DS-15).
    pub s_with_every_g1: bool,
    /// A powered-off `G1` becomes `G0` (DS-15, overscan and travel).
    pub g0_for_overscan: bool,
    /// Blank (S0) `G1` moves run at `whitespace_speed` (DS-10), mm/min.
    pub fast_whitespace: Option<f64>,
    /// Merge collinear same-power moves (GCODE-01).
    pub cluster: bool,
    /// Output in inches (`G20`, coordinates and feed divided by 25.4).
    pub inches: bool,
    pub start_script: String,
    pub end_script: String,
    /// Written first when "auto-home on start" is set (DS-09).
    pub home_command: Option<String>,
    /// Write `S0` with every travel move (`G0`) that follows a powered cut, and the power again
    /// with the next cut. For controllers that are not known to switch the laser off for `G0`
    /// by themselves (a custom G-code device).
    pub travel_s0: bool,
    /// Write the time of a dwell (`G4 P`) in milliseconds. The input gives seconds, as GRBL
    /// reads it; Marlin and its relatives read `P` as milliseconds.
    pub dwell_p_ms: bool,
}

impl Default for TranslateOptions {
    fn default() -> Self {
        Self {
            style: PowerStyle::Passthrough,
            in_s_max: 1000.0,
            out_s_max: 1000.0,
            air_on: "M8".into(),
            air_off: "M9".into(),
            swap_xy: false,
            s_with_every_g1: false,
            g0_for_overscan: false,
            fast_whitespace: None,
            cluster: false,
            inches: false,
            start_script: String::new(),
            end_script: String::new(),
            home_command: None,
            travel_s0: false,
            dwell_p_ms: false,
        }
    }
}

impl TranslateOptions {
    /// Options for a profile (the job is assumed to use `in_s_max` = 1000).
    pub fn for_profile(p: &DeviceProfile) -> Self {
        let g = &p.gcode;
        let style = match p.kind {
            DeviceKind::Marlin | DeviceKind::Snapmaker => match g.marlin_power {
                // Outside inline mode Marlin keeps the laser at the last power through travel
                // moves, and reads no `S` from a `G1`: the power is its own command before a
                // cut, and `M5` before the head travels.
                MarlinPowerMode::M3 => PowerStyle::Command { set: "M3 S{value}".into(), off: "M5".into() },
                MarlinPowerMode::M4 => PowerStyle::Command { set: "M4 S{value}".into(), off: "M5".into() },
                MarlinPowerMode::Inline => PowerStyle::MarlinInline,
                MarlinPowerMode::Fan => {
                    PowerStyle::Command { set: format!("M106 P{} S{{value}}", g.tool_index), off: format!("M107 P{}", g.tool_index) }
                }
                MarlinPowerMode::Percent => PowerStyle::Command { set: "M3 P{value}".into(), off: "M5".into() },
            },
            DeviceKind::LinuxCnc => PowerStyle::Command { set: "M67 E0 Q{value}".into(), off: "M67 E0 Q0".into() },
            _ => PowerStyle::Passthrough,
        };
        Self {
            style,
            in_s_max: 1000.0,
            out_s_max: power_scale(p),
            air_on: g.air_assist.on.clone(),
            air_off: g.air_assist.off.clone(),
            swap_xy: g.swap_xy,
            s_with_every_g1: g.s_with_every_g1 || matches!((&p.kind, g.marlin_power), (DeviceKind::Marlin, MarlinPowerMode::Inline)),
            g0_for_overscan: g.g0_for_overscan,
            fast_whitespace: g.fast_whitespace_scan.then_some(g.whitespace_speed),
            cluster: g.clustering,
            inches: g.units == Units::Inches,
            start_script: g.start_script.clone(),
            end_script: g.end_script.clone(),
            home_command: g.auto_home_on_start.then(|| home_command(p)),
            travel_s0: p.kind == DeviceKind::Custom,
            dwell_p_ms: matches!(p.kind, DeviceKind::Marlin | DeviceKind::Snapmaker),
        }
    }
}

/// The number that means full power in the power words: 100 for the percent mode of a
/// Marlin-style controller (`M3 P<percent>`), the profile's S range otherwise.
pub fn power_scale(p: &DeviceProfile) -> f64 {
    if matches!(p.kind, DeviceKind::Marlin | DeviceKind::Snapmaker) && p.gcode.marlin_power == MarlinPowerMode::Percent {
        100.0
    } else {
        p.gcode.s_max
    }
}

/// The home command of a profile's controller.
pub fn home_command(p: &DeviceProfile) -> String {
    match p.kind {
        DeviceKind::Smoothie => "$H".to_owned(),
        DeviceKind::Marlin | DeviceKind::Snapmaker => "G28 X Y".to_owned(),
        DeviceKind::Custom if !p.gcode.custom.home.trim().is_empty() => p.gcode.custom.home.lines().next().unwrap_or("G28").trim().to_owned(),
        DeviceKind::LinuxCnc | DeviceKind::Custom => "G28".to_owned(),
        _ => "$H".to_owned(),
    }
}

type Word = (char, String);

/// Splits a G-code line into words. `None` for lines that are not plain G-code (`$H`, console).
pub(crate) fn parse_words(line: &str) -> Option<Vec<Word>> {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c.is_ascii_alphabetic() {
            chars.next();
            let mut v = String::new();
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() || matches!(d, '.' | '-' | '+') {
                    v.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            words.push((c.to_ascii_uppercase(), v));
        } else {
            return None;
        }
    }
    Some(words)
}

fn strip_comment(line: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in line.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            ';' if depth == 0 => break,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().to_owned()
}

fn join(words: &[Word]) -> String {
    words.iter().map(|(l, v)| format!("{l}{v}")).collect::<Vec<_>>().join(" ")
}

struct State {
    motion: u8,
    s_in: f64,
    laser_on: bool,
    cmd_state: Option<String>,
    feed_src: Option<f64>,
    feed_out: Option<f64>,
    air_is_on: bool,
    /// A travel move was given `S0` (`travel_s0`): the next cut states its power again.
    travel_zeroed: bool,
}

/// Translates a job. Comments are dropped; lines the translator does not understand pass
/// through untouched.
pub fn translate<I, S>(lines: I, opts: &TranslateOptions) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out: Vec<String> = Vec::new();
    if let Some(h) = &opts.home_command {
        out.push(h.clone());
    }
    out.extend(script_lines(&opts.start_script));
    let mut st =
        State { motion: 0, s_in: 0.0, laser_on: false, cmd_state: None, feed_src: None, feed_out: None, air_is_on: false, travel_zeroed: false };
    for raw in lines {
        let line = strip_comment(raw.as_ref());
        if line.is_empty() {
            continue;
        }
        translate_line(&line, opts, &mut st, &mut out);
    }
    // Never leave power on at the end.
    if let PowerStyle::Command { off, .. } = &opts.style
        && st.cmd_state.is_some()
    {
        out.push(off.clone());
    }
    if st.air_is_on && !opts.air_off.trim().is_empty() {
        out.push(opts.air_off.trim().to_owned());
    }
    out.extend(script_lines(&opts.end_script));
    if opts.cluster { cluster(&out) } else { out }
}

fn script_lines(s: &str) -> Vec<String> {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect()
}

fn scale_s(s_in: f64, opts: &TranslateOptions) -> f64 {
    if opts.in_s_max <= 0.0 { 0.0 } else { (s_in / opts.in_s_max * opts.out_s_max).clamp(0.0, opts.out_s_max.max(0.0)) }
}

fn fmt_s(v: f64, out_max: f64) -> String {
    if out_max >= 50.0 { format!("{}", v.round() as i64) } else { fmt_num(v) }
}

fn translate_line(line: &str, opts: &TranslateOptions, st: &mut State, out: &mut Vec<String>) {
    let Some(mut words) = parse_words(line) else {
        out.push(line.to_owned());
        return;
    };
    // Mn codes in this line.
    let mut m_code: Option<u32> = None;
    let mut g_motion: Option<u8> = None;
    for (l, v) in &words {
        match l {
            'M' => m_code = v.parse::<f64>().ok().map(|x| x as u32),
            'G' => {
                if let Ok(g) = v.parse::<f64>() {
                    let g = g as u8;
                    if g <= 3 {
                        g_motion = Some(g);
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(s) = words.iter().find(|(l, _)| *l == 'S').and_then(|(_, v)| v.parse::<f64>().ok()) {
        st.s_in = s;
    }

    // Air assist.
    match m_code {
        Some(7) | Some(8) => {
            if !opts.air_on.trim().is_empty() && !st.air_is_on {
                out.push(opts.air_on.trim().to_owned());
            }
            st.air_is_on = true;
            return;
        }
        Some(9) => {
            if !opts.air_off.trim().is_empty() {
                out.push(opts.air_off.trim().to_owned());
            }
            st.air_is_on = false;
            return;
        }
        _ => {}
    }

    // Laser on/off words.
    if let Some(m @ (3..=5)) = m_code {
        st.laser_on = m != 5;
        match &opts.style {
            PowerStyle::Command { off, .. } => {
                if m == 5 {
                    out.push(off.clone());
                    st.cmd_state = None;
                }
                return;
            }
            PowerStyle::MarlinInline => {
                // `M3 I` has no power of its own; S comes with the moves.
                let o = vec![('M', m.to_string()), ('I', String::new())];
                if m == 5 {
                    st.s_in = 0.0;
                }
                out.push(join(&o));
                return;
            }
            PowerStyle::Passthrough => {
                if m == 5 {
                    st.s_in = 0.0;
                }
                for w in &mut words {
                    if w.0 == 'S' {
                        w.1 = fmt_s(scale_s(st.s_in, opts), opts.out_s_max);
                    }
                }
                out.push(join(&words));
                return;
            }
        }
    }
    if m_code.is_some() {
        out.push(join(&words));
        return;
    }

    let motion = g_motion.unwrap_or(st.motion);
    let is_motion_word = words.iter().any(|(l, _)| matches!(l, 'X' | 'Y' | 'Z' | 'I' | 'J'));
    if let Some(g) = g_motion {
        st.motion = g;
    }

    // Units.
    for w in &mut words {
        match (w.0, w.1.parse::<f64>()) {
            ('G', Ok(g)) if opts.inches && (g - 21.0).abs() < 1e-9 => w.1 = "20".into(),
            ('G', Ok(g)) if !opts.inches && (g - 20.0).abs() < 1e-9 => w.1 = "21".into(),
            ('X' | 'Y' | 'Z' | 'I' | 'J' | 'F', Ok(v)) if opts.inches => {
                w.1 = format!("{:.4}", v / 25.4).trim_end_matches('0').trim_end_matches('.').to_owned()
            }
            _ => {}
        }
    }
    if opts.swap_xy {
        for w in &mut words {
            w.0 = match w.0 {
                'X' => 'Y',
                'Y' => 'X',
                'I' => 'J',
                'J' => 'I',
                c => c,
            };
        }
    }

    let src_feed = words.iter().find(|(l, _)| *l == 'F').and_then(|(_, v)| v.parse::<f64>().ok());
    if let Some(f) = src_feed {
        // Source feed, in source units (inches were converted above, so undo for tracking).
        st.feed_src = Some(if opts.inches { f * 25.4 } else { f });
    }

    if !is_motion_word && !words.iter().any(|(l, _)| *l == 'S') {
        let dwell = words.iter().any(|(l, v)| *l == 'G' && v.parse::<f64>().is_ok_and(|g| (g - 4.0).abs() < 1e-9));
        if dwell && opts.dwell_p_ms {
            for w in &mut words {
                if w.0 == 'P'
                    && let Ok(seconds) = w.1.parse::<f64>()
                {
                    w.1 = format!("{}", (seconds * 1000.0).round().max(0.0) as i64);
                }
            }
        }
        out.push(join(&words));
        return;
    }

    let burning = motion != 0 && st.laser_on && st.s_in > 0.0;
    let s_out = scale_s(st.s_in, opts);
    let is_cut_move = matches!(motion, 1..=3) && is_motion_word;

    // G0 for blank moves.
    if opts.g0_for_overscan && motion == 1 && is_motion_word && !burning {
        for w in &mut words {
            if w.0 == 'G' && w.1.parse::<f64>().ok() == Some(1.0) {
                w.1 = "0".into();
            }
        }
        if !words.iter().any(|(l, _)| *l == 'G') {
            words.insert(0, ('G', "0".into()));
        }
        st.motion = 0;
    }

    // Fast whitespace scan: blank G1 moves go fast, powered moves get their feed back.
    if let Some(fast) = opts.fast_whitespace
        && is_cut_move
        && motion == 1
        && st.laser_on
    {
        let fast_out = if opts.inches { fast / 25.4 } else { fast };
        let want = if burning { st.feed_src.map(|f| if opts.inches { f / 25.4 } else { f }) } else { Some(fast_out) };
        if let Some(w) = want
            && (st.feed_out.is_none_or(|cur| (cur - w).abs() > 1e-9) || words.iter().any(|(l, _)| *l == 'F'))
        {
            words.retain(|(l, _)| *l != 'F');
            words.push(('F', fmt_num(w)));
            st.feed_out = Some(w);
        }
    }

    match &opts.style {
        PowerStyle::Command { set, off } => {
            words.retain(|(l, _)| *l != 'S');
            if is_cut_move && st.laser_on {
                let want = burning.then(|| fmt_s(s_out, opts.out_s_max));
                if want != st.cmd_state {
                    match &want {
                        Some(v) => out.push(set.replace("{value}", v)),
                        None => out.push(off.clone()),
                    }
                    st.cmd_state = want;
                }
            } else if matches!(motion, 0) && st.cmd_state.is_some() && is_motion_word {
                out.push(off.clone());
                st.cmd_state = None;
            }
            if words.is_empty() {
                return;
            }
        }
        PowerStyle::MarlinInline | PowerStyle::Passthrough => {
            let has_s = words.iter().any(|(l, _)| *l == 'S');
            if has_s {
                for w in &mut words {
                    if w.0 == 'S' {
                        w.1 = fmt_s(scale_s(w.1.parse().unwrap_or(0.0), opts), opts.out_s_max);
                    }
                }
                st.travel_zeroed = false;
            } else if opts.travel_s0 && motion == 0 && is_motion_word && st.laser_on && st.s_in > 0.0 && !st.travel_zeroed {
                words.push(('S', "0".into()));
                st.travel_zeroed = true;
            } else if opts.travel_s0 && st.travel_zeroed && is_cut_move {
                words.push(('S', fmt_s(if st.laser_on { s_out } else { 0.0 }, opts.out_s_max)));
                st.travel_zeroed = false;
            } else if (opts.s_with_every_g1 || opts.style == PowerStyle::MarlinInline) && motion == 1 && is_motion_word {
                words.push(('S', fmt_s(if st.laser_on { s_out } else { 0.0 }, opts.out_s_max)));
            }
        }
    }
    out.push(join(&words));
}

/// Merges consecutive collinear `G1` moves that have the same power and feed into one move.
/// The path is identical; only the line count shrinks. S and F are written explicitly on a
/// merged move so the modal state stays the same. This is the lossless form of "G-code
/// clustering" for fast rasters.
pub fn cluster<S: AsRef<str>>(lines: &[S]) -> Vec<String> {
    struct Pending {
        start: (f64, f64),
        end: (f64, f64),
        s: Option<String>,
        f: Option<String>,
    }
    /// `G1` with only X/Y/S/F words: (x, y, s, f).
    type Mv = (Option<f64>, Option<f64>, Option<String>, Option<String>);
    fn parse_mv(l: &str) -> Option<Mv> {
        let words = parse_words(l)?;
        let (mut x, mut y, mut s, mut f, mut g1) = (None, None, None, None, false);
        for (c, v) in &words {
            match c {
                'G' if v.parse::<f64>().ok() == Some(1.0) => g1 = true,
                'X' => x = Some(v.parse().ok()?),
                'Y' => y = Some(v.parse().ok()?),
                'S' => s = Some(v.clone()),
                'F' => f = Some(v.clone()),
                _ => return None,
            }
        }
        (g1 && (x.is_some() || y.is_some())).then_some((x, y, s, f))
    }
    fn flush(p: &mut Option<Pending>, out: &mut Vec<String>) {
        if let Some(p) = p.take() {
            let mut s = format!("G1 X{} Y{}", fmt_num(p.end.0), fmt_num(p.end.1));
            if let Some(v) = &p.s {
                s.push_str(&format!(" S{v}"));
            }
            if let Some(v) = &p.f {
                s.push_str(&format!(" F{v}"));
            }
            out.push(s);
        }
    }

    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut pend: Option<Pending> = None;
    let (mut px, mut py): (Option<f64>, Option<f64>) = (None, None);
    let (mut cur_s, mut cur_f): (Option<String>, Option<String>) = (None, None);
    let mut abs = true;
    for l in lines {
        let l = l.as_ref();
        let words = parse_words(l);
        if let Some(w) = &words {
            for (c, v) in w {
                if *c == 'G' {
                    match v.parse::<f64>().ok() {
                        Some(g) if (g - 91.0).abs() < 1e-9 => abs = false,
                        Some(g) if (g - 90.0).abs() < 1e-9 => abs = true,
                        _ => {}
                    }
                }
            }
        }
        let mv = if abs && px.is_some() && py.is_some() { parse_mv(l) } else { None };
        if let (Some((mx, my, ms, mf)), Some(cx), Some(cy)) = (mv, px, py) {
            let target = (mx.unwrap_or(cx), my.unwrap_or(cy));
            let s = ms.or(cur_s.clone());
            let f = mf.or(cur_f.clone());
            let merge = pend.as_ref().is_some_and(|p| {
                let (dx0, dy0) = (p.end.0 - p.start.0, p.end.1 - p.start.1);
                let (dx1, dy1) = (target.0 - p.end.0, target.1 - p.end.1);
                (dx0 * dy1 - dy0 * dx1).abs() < 1e-9 && dx0 * dx1 + dy0 * dy1 > 0.0 && p.s == s && p.f == f
            });
            if merge {
                if let Some(p) = pend.as_mut() {
                    p.end = target;
                }
            } else {
                flush(&mut pend, &mut out);
                pend = Some(Pending { start: (cx, cy), end: target, s: s.clone(), f: f.clone() });
            }
            (px, py, cur_s, cur_f) = (Some(target.0), Some(target.1), s, f);
        } else {
            flush(&mut pend, &mut out);
            out.push(l.to_owned());
            if let Some(w) = words {
                for (c, v) in &w {
                    match c {
                        'X' if abs => px = v.parse().ok().or(px),
                        'Y' if abs => py = v.parse().ok().or(py),
                        'S' => cur_s = Some(v.clone()),
                        'F' => cur_f = Some(v.clone()),
                        _ => {}
                    }
                }
            } else {
                px = None;
                py = None;
            }
            if !abs {
                px = None;
                py = None;
            }
        }
    }
    flush(&mut pend, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lines: &[&str], o: &TranslateOptions) -> Vec<String> {
        translate(lines.iter().copied(), o)
    }

    /// The job gives a dwell in seconds, as GRBL reads `G4 P`. Marlin and Snapmaker read `P`
    /// as milliseconds: unchanged, a quarter of a second would be a quarter of a millisecond
    /// (dot mode and the pauses at the start and end of a cut would do nothing).
    #[test]
    fn a_dwell_is_written_in_the_unit_the_controller_reads() {
        let src = ["M3", "G1 X10 Y10 F600 S500", "G4 P0.25", "S0", "G4 P1.5", "G0 X0 Y0", "M5"];
        for kind in [DeviceKind::Marlin, DeviceKind::Snapmaker] {
            let out = run(&src, &TranslateOptions::for_profile(&DeviceProfile::for_kind(kind)));
            assert!(out.iter().any(|l| l == "G4 P250") && out.iter().any(|l| l == "G4 P1500"), "{kind:?}: {out:?}");
            assert!(!out.iter().any(|l| l.contains("P0.25")), "{kind:?}: {out:?}");
        }
        // Seconds stay for a controller that reads them (and where the unit is not known).
        for kind in [DeviceKind::Smoothie, DeviceKind::Custom] {
            let out = run(&src, &TranslateOptions::for_profile(&DeviceProfile::for_kind(kind)));
            assert!(out.iter().any(|l| l == "G4 P0.25") && out.iter().any(|l| l == "G4 P1.5"), "{kind:?}: {out:?}");
        }
    }

    /// A custom controller is not known to switch the laser off for `G0` by itself.
    #[test]
    fn a_custom_device_gets_the_laser_switched_off_for_travel() {
        let mut p = DeviceProfile::for_kind(DeviceKind::Custom);
        (p.gcode.start_script, p.gcode.end_script) = (String::new(), String::new());
        let src = ["M4 S0", "G0 X1 Y1", "G1 X5 S500 F600", "G1 Y5", "G0 X20 Y20", "G0 X21", "G1 X25", "G1 Y25 S250", "G0 X0 Y0", "M5"];
        let out = translate(src, &TranslateOptions::for_profile(&p));
        let s = |v: f64| fmt_s(v / 1000.0 * p.gcode.s_max, p.gcode.s_max);
        let want = [
            "M4 S0".to_owned(),
            "G0 X1 Y1".to_owned(),
            format!("G1 X5 S{} F600", s(500.0)),
            "G1 Y5".to_owned(),
            "G0 X20 Y20 S0".to_owned(),
            "G0 X21".to_owned(),
            format!("G1 X25 S{}", s(500.0)),
            format!("G1 Y25 S{}", s(250.0)),
            "G0 X0 Y0 S0".to_owned(),
            "M5".to_owned(),
        ];
        assert_eq!(out, want);
        // GRBL-style controllers switch off for G0 themselves: nothing is added.
        let plain = translate(src, &TranslateOptions::default());
        assert!(plain.contains(&"G0 X20 Y20".to_owned()), "{plain:?}");
    }

    /// Outside inline mode Marlin keeps the laser on through G0: power is its own command.
    #[test]
    fn marlin_m3_mode_switches_the_laser_off_for_travel() {
        let mut p = DeviceProfile::for_kind(DeviceKind::Marlin);
        p.gcode.marlin_power = MarlinPowerMode::M3;
        (p.gcode.start_script, p.gcode.end_script) = (String::new(), String::new());
        let src = ["M4 S0", "G0 X1 Y1", "G1 X5 S500 F600", "G1 Y5", "G0 X20 Y20", "G1 X25", "M5"];
        let out = translate(src, &TranslateOptions::for_profile(&p));
        let on = format!("M3 S{}", fmt_s(0.5 * p.gcode.s_max, p.gcode.s_max));
        assert_eq!(out, ["G0 X1 Y1", on.as_str(), "G1 X5 F600", "G1 Y5", "M5", "G0 X20 Y20", on.as_str(), "G1 X25", "M5"]);
    }

    #[test]
    fn smoothie_scales_s_to_unit_range() {
        let o = TranslateOptions { out_s_max: 1.0, ..Default::default() };
        let r = run(&["G21", "G90", "M4 S0", "G0 X10 Y10", "G1 X20 S500 F3000", "M5"], &o);
        assert_eq!(r, vec!["G21", "G90", "M4 S0", "G0 X10 Y10", "G1 X20 S0.5 F3000", "M5"]);
    }

    #[test]
    fn marlin_fan_mode_writes_power_commands() {
        let o = TranslateOptions {
            style: PowerStyle::Command { set: "M106 P1 S{value}".into(), off: "M107 P1".into() },
            out_s_max: 255.0,
            ..Default::default()
        };
        let r = run(&["M4 S0", "G0 X5", "G1 X10 S1000 F900", "G1 X20 S500", "G1 X30 S500", "G0 X40", "M5"], &o);
        assert_eq!(r, vec!["G0 X5", "M106 P1 S255", "G1 X10 F900", "M106 P1 S128", "G1 X20", "G1 X30", "M107 P1", "G0 X40", "M107 P1"]);
    }

    #[test]
    fn linuxcnc_uses_m67_analog() {
        let o = TranslateOptions {
            style: PowerStyle::Command { set: "M67 E0 Q{value}".into(), off: "M67 E0 Q0".into() },
            out_s_max: 100.0,
            ..Default::default()
        };
        let r = run(&["M3 S0", "G1 X10 S250 F600", "M5"], &o);
        assert_eq!(r, vec!["M67 E0 Q25", "G1 X10 F600", "M67 E0 Q0"]);
    }

    #[test]
    fn marlin_inline_puts_s_on_every_g1() {
        let o = TranslateOptions { style: PowerStyle::MarlinInline, out_s_max: 255.0, ..Default::default() };
        let r = run(&["M4 S0", "G1 X10 S1000 F900", "G1 X20", "G1 X30 S0", "M5"], &o);
        assert_eq!(r[0], "M4 I");
        assert_eq!(r[1], "G1 X10 S255 F900");
        assert_eq!(r[2], "G1 X20 S255");
        assert_eq!(r[3], "G1 X30 S0");
        assert_eq!(r[4], "M5 I");
    }

    #[test]
    fn air_assist_commands_are_replaced() {
        let o = TranslateOptions { air_on: "M106".into(), air_off: "M107".into(), ..Default::default() };
        let r = run(&["M8", "G1 X1", "M9"], &o);
        assert_eq!(r, vec!["M106", "G1 X1", "M107"]);
        let none = TranslateOptions { air_on: String::new(), air_off: String::new(), ..Default::default() };
        assert_eq!(run(&["M8", "G1 X1", "M9"], &none), vec!["G1 X1"]);
    }

    #[test]
    fn swap_xy_and_scripts_and_home() {
        let o = TranslateOptions {
            swap_xy: true,
            start_script: "G21\nG90".into(),
            end_script: "M5".into(),
            home_command: Some("$H".into()),
            ..Default::default()
        };
        let r = run(&["G0 X1 Y2"], &o);
        assert_eq!(r, vec!["$H", "G21", "G90", "G0 Y1 X2", "M5"]);
    }

    #[test]
    fn s_with_every_g1_and_g0_for_blank_moves() {
        let o = TranslateOptions { s_with_every_g1: true, g0_for_overscan: true, ..Default::default() };
        let r = run(&["M4 S0", "G1 X5 S0", "G1 X10 S800 F1000", "G1 X20", "M5"], &o);
        assert_eq!(r[1], "G0 X5 S0");
        assert_eq!(r[2], "G1 X10 S800 F1000");
        assert_eq!(r[3], "G1 X20 S800");
    }

    #[test]
    fn fast_whitespace_restores_cut_feed() {
        let o = TranslateOptions { fast_whitespace: Some(9000.0), ..Default::default() };
        let r = run(&["M4 S0", "G1 X1 S500 F2000", "G1 X5 S0", "G1 X6 S500", "M5"], &o);
        assert_eq!(r[1], "G1 X1 S500 F2000");
        assert_eq!(r[2], "G1 X5 S0 F9000");
        assert_eq!(r[3], "G1 X6 S500 F2000");
    }

    #[test]
    fn inches_scale_coordinates_and_feed() {
        let o = TranslateOptions { inches: true, ..Default::default() };
        let r = run(&["G21", "G1 X25.4 F508"], &o);
        assert_eq!(r, vec!["G20", "G1 X1 F20"]);
    }

    #[test]
    fn clustering_merges_collinear_same_power_runs() {
        let lines = ["G0 X0 Y0", "G1 X1 Y0 S500 F3000", "G1 X2 Y0 S500", "G1 X3 Y0 S500", "G1 X4 Y0 S0", "G1 X5 Y0 S0", "G1 X5 Y1 S0"];
        let r = cluster(&lines);
        assert_eq!(r, vec!["G0 X0 Y0", "G1 X3 Y0 S500 F3000", "G1 X5 Y0 S0 F3000", "G1 X5 Y1 S0 F3000"]);
    }

    #[test]
    fn clustering_keeps_the_toolpath() {
        // Every vertex of the original that survives is on the merged path and S changes stay put.
        let lines = ["G0 X0 Y0", "G1 X1 Y0 S100 F1000", "G1 X2 Y0 S200", "G1 X3 Y0 S200", "G1 X3 Y1 S200"];
        let r = cluster(&lines);
        assert_eq!(r.len(), 4);
        assert_eq!(r[2], "G1 X3 Y0 S200 F1000");
    }
}
