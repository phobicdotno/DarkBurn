//! Command templates for custom devices: `{name}` placeholders in a line of G-code.

use darkburn_grbl::protocol::fmt_num;

/// Replaces `{key}` in `template` with the matching value. Unknown placeholders are left as
/// they are so a typo shows up in the console instead of silently vanishing.
pub fn render(template: &str, vars: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let key = &after[..end];
                match vars.iter().find(|(k, _)| *k == key) {
                    Some((_, v)) => out.push_str(v),
                    None => {
                        out.push('{');
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Renders a multi-line template into non-empty lines.
pub fn render_lines(template: &str, vars: &[(&str, String)]) -> Vec<String> {
    template.lines().map(|l| render(l, vars)).map(|l| l.trim().to_owned()).filter(|l| !l.is_empty()).collect()
}

/// Number as used in G-code (at most three decimals, no trailing zeros).
pub fn num(v: f64) -> String {
    fmt_num(v)
}

/// Placeholders of a jog template.
pub fn jog_vars(dx: f64, dy: f64, dz: f64, feed: f64) -> Vec<(&'static str, String)> {
    vec![("dx", num(dx)), ("dy", num(dy)), ("dz", num(dz)), ("feed", num(feed))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_known_and_keeps_unknown() {
        let v = [("feed", "1500".to_owned())];
        assert_eq!(render("G1 F{feed} {oops}", &v), "G1 F1500 {oops}");
        assert_eq!(render("no braces", &v), "no braces");
        assert_eq!(render("open {brace", &v), "open {brace");
    }

    #[test]
    fn multi_line_skips_blank_lines() {
        let v = jog_vars(1.0, -2.5, 0.0, 600.0);
        assert_eq!(render_lines("G91\n\nG0 X{dx} Y{dy} F{feed}\nG90\n", &v), vec!["G91", "G0 X1 Y-2.5 F600", "G90"]);
    }
}
