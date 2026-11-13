//! Arithmetic typed into number fields: `+ - * /`, parentheses, unary minus, a decimal point
//! or comma, and length units (`mm`, `cm`, `m`, `in` or `"`, `ft` or `'`, `pt`).
//!
//! A number with a unit is a length and is worked out in millimetres; a bare number is plain.
//! `5' + 2"`, `10in/4` and `25.4mm*2` are lengths, `3*4` is plain. Feet and inches may also be
//! written side by side (`5' 2"`). When a bare number is added to a length it is taken in the
//! field's display unit (see [`eval_with`]), so `1in + 1` in an inch field is two inches.

use std::fmt;

/// The result of an expression: a plain number, or a length in millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quantity {
    pub value: f64,
    /// `value` is a length in millimetres.
    pub length: bool,
}

impl Quantity {
    fn plain(value: f64) -> Quantity {
        Quantity { value, length: false }
    }
}

/// Why an expression could not be worked out.
#[derive(Clone, Debug, PartialEq)]
pub enum ExprError {
    Empty,
    /// Something unexpected at this character offset.
    Unexpected(usize),
    UnknownUnit(String),
    DivideByZero,
    /// A length times a length, or a number divided by a length.
    BadUnits,
    NotFinite,
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExprError::Empty => write!(f, "nothing typed"),
            ExprError::Unexpected(at) => write!(f, "unexpected text at {at}"),
            ExprError::UnknownUnit(u) => write!(f, "unknown unit \"{u}\""),
            ExprError::DivideByZero => write!(f, "division by zero"),
            ExprError::BadUnits => write!(f, "units do not fit together"),
            ExprError::NotFinite => write!(f, "number too large"),
        }
    }
}

impl std::error::Error for ExprError {}

/// Millimetres in one `unit` (case does not matter), or None for an unknown unit.
pub fn unit_mm(unit: &str) -> Option<f64> {
    Some(match unit.to_ascii_lowercase().as_str() {
        "mm" => 1.0,
        "cm" => 10.0,
        "m" => 1000.0,
        "in" | "inch" | "inches" | "\"" | "\u{2033}" => 25.4,
        "ft" | "foot" | "feet" | "'" | "\u{2032}" => 304.8,
        "pt" => 25.4 / 72.0,
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Unit(String),
    Plus,
    Minus,
    Star,
    Slash,
    Open,
    Close,
}

fn lex(text: &str) -> Result<Vec<(usize, Tok)>, ExprError> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let tok = match c {
            '+' => Tok::Plus,
            '-' | '\u{2212}' => Tok::Minus,
            '*' | '\u{d7}' => Tok::Star,
            '/' | '\u{f7}' => Tok::Slash,
            '(' => Tok::Open,
            ')' => Tok::Close,
            '"' | '\'' | '\u{2032}' | '\u{2033}' => Tok::Unit(c.to_string()),
            _ if c.is_ascii_digit() || c == '.' || c == ',' => {
                let mut s = String::new();
                let mut separators = 0;
                while i < chars.len() {
                    let ch = chars[i].1;
                    if ch.is_ascii_digit() {
                        s.push(ch);
                    } else if ch == '.' || ch == ',' {
                        separators += 1;
                        if separators > 1 {
                            return Err(ExprError::Unexpected(chars[i].0));
                        }
                        s.push('.');
                    } else {
                        break;
                    }
                    i += 1;
                }
                // An exponent: e or E, an optional sign, then digits.
                if i < chars.len() && matches!(chars[i].1, 'e' | 'E') {
                    let mut j = i + 1;
                    let mut exp = String::from("e");
                    if j < chars.len() && matches!(chars[j].1, '+' | '-') {
                        exp.push(chars[j].1);
                        j += 1;
                    }
                    let digits_from = j;
                    while j < chars.len() && chars[j].1.is_ascii_digit() {
                        exp.push(chars[j].1);
                        j += 1;
                    }
                    if j > digits_from {
                        s.push_str(&exp);
                        i = j;
                    }
                }
                if s == "." {
                    return Err(ExprError::Unexpected(at));
                }
                let v: f64 = s.parse().map_err(|_| ExprError::Unexpected(at))?;
                out.push((at, Tok::Num(v)));
                continue;
            }
            _ if c.is_alphabetic() => {
                let mut s = String::new();
                while i < chars.len() && chars[i].1.is_alphabetic() {
                    s.push(chars[i].1);
                    i += 1;
                }
                out.push((at, Tok::Unit(s)));
                continue;
            }
            _ => return Err(ExprError::Unexpected(at)),
        };
        out.push((at, tok));
        i += 1;
    }
    Ok(out)
}

/// How deep brackets and signs may nest. Far more than anybody writes, and far less than runs a
/// thread out of stack (each level is a few calls deep).
const MAX_DEPTH: usize = 500;

struct Parser {
    toks: Vec<(usize, Tok)>,
    pos: usize,
    end: usize,
    bare_mm: f64,
    /// Nesting of brackets and signs so far.
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|t| &t.1)
    }

    fn at(&self) -> usize {
        self.toks.get(self.pos).map(|t| t.0).unwrap_or(self.end)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).map(|t| t.1.clone());
        self.pos += 1;
        t
    }

    fn add(&self, a: Quantity, b: Quantity, sign: f64) -> Quantity {
        let lift = |q: Quantity| if q.length { q.value } else { q.value * self.bare_mm };
        if a.length == b.length {
            Quantity { value: a.value + sign * b.value, length: a.length }
        } else {
            Quantity { value: lift(a) + sign * lift(b), length: true }
        }
    }

    fn expr(&mut self) -> Result<Quantity, ExprError> {
        let mut left = self.term()?;
        loop {
            match self.peek() {
                Some(Tok::Plus) => {
                    self.pos += 1;
                    let right = self.term()?;
                    left = self.add(left, right, 1.0);
                }
                Some(Tok::Minus) => {
                    self.pos += 1;
                    let right = self.term()?;
                    left = self.add(left, right, -1.0);
                }
                // Lengths side by side add up: 5' 2".
                Some(Tok::Num(_)) if left.length => {
                    let at = self.at();
                    let right = self.term()?;
                    if !right.length {
                        return Err(ExprError::Unexpected(at));
                    }
                    left = self.add(left, right, 1.0);
                }
                _ => return Ok(left),
            }
        }
    }

    fn term(&mut self) -> Result<Quantity, ExprError> {
        let mut left = self.unary()?;
        loop {
            match self.peek() {
                Some(Tok::Star) => {
                    self.pos += 1;
                    let right = self.unary()?;
                    if left.length && right.length {
                        return Err(ExprError::BadUnits);
                    }
                    left = Quantity { value: left.value * right.value, length: left.length || right.length };
                }
                Some(Tok::Slash) => {
                    self.pos += 1;
                    let right = self.unary()?;
                    if right.value == 0.0 {
                        return Err(ExprError::DivideByZero);
                    }
                    left = match (left.length, right.length) {
                        (true, true) => Quantity::plain(left.value / right.value),
                        (false, true) => return Err(ExprError::BadUnits),
                        (l, false) => Quantity { value: left.value / right.value, length: l },
                    };
                }
                _ => return Ok(left),
            }
        }
    }

    fn unary(&mut self) -> Result<Quantity, ExprError> {
        // Every bracket and every sign comes through here one level deeper.
        if self.depth >= MAX_DEPTH {
            return Err(ExprError::Unexpected(self.at()));
        }
        self.depth += 1;
        let q = self.unary_inner();
        self.depth -= 1;
        q
    }

    fn unary_inner(&mut self) -> Result<Quantity, ExprError> {
        match self.peek() {
            Some(Tok::Minus) => {
                self.pos += 1;
                let q = self.unary()?;
                Ok(Quantity { value: -q.value, ..q })
            }
            Some(Tok::Plus) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.postfix(),
        }
    }

    fn postfix(&mut self) -> Result<Quantity, ExprError> {
        let q = self.primary()?;
        if let Some(Tok::Unit(u)) = self.peek() {
            let u = u.clone();
            let at = self.at();
            self.pos += 1;
            if q.length {
                return Err(ExprError::Unexpected(at));
            }
            let mm = unit_mm(&u).ok_or(ExprError::UnknownUnit(u))?;
            return Ok(Quantity { value: q.value * mm, length: true });
        }
        Ok(q)
    }

    fn primary(&mut self) -> Result<Quantity, ExprError> {
        let at = self.at();
        match self.next() {
            Some(Tok::Num(v)) => Ok(Quantity::plain(v)),
            Some(Tok::Open) => {
                let q = self.expr()?;
                match self.next() {
                    Some(Tok::Close) => Ok(q),
                    _ => Err(ExprError::Unexpected(self.toks.get(self.pos.saturating_sub(1)).map(|t| t.0).unwrap_or(self.end))),
                }
            }
            Some(Tok::Unit(u)) if unit_mm(&u).is_none() => Err(ExprError::UnknownUnit(u)),
            _ => Err(ExprError::Unexpected(at)),
        }
    }
}

/// Work out `text`. Bare numbers added to lengths count as millimetres.
pub fn eval(text: &str) -> Result<Quantity, ExprError> {
    eval_with(text, 1.0)
}

/// Work out `text`, taking bare numbers added to lengths as `bare_mm` millimetres each (the
/// field's display unit: 25.4 in an inch field).
pub fn eval_with(text: &str, bare_mm: f64) -> Result<Quantity, ExprError> {
    let toks = lex(text)?;
    if toks.is_empty() {
        return Err(ExprError::Empty);
    }
    let mut p = Parser { toks, pos: 0, end: text.len(), bare_mm, depth: 0 };
    let q = p.expr()?;
    if p.pos < p.toks.len() {
        return Err(ExprError::Unexpected(p.at()));
    }
    if !q.value.is_finite() {
        return Err(ExprError::NotFinite);
    }
    Ok(q)
}

/// `text` as a number in a field shown in a unit `unit_mm` millimetres long (1 for mm, 25.4
/// for inches): a length is converted to that unit, a bare number is taken as it is.
pub fn eval_in(text: &str, unit_mm: f64) -> Option<f64> {
    let unit_mm = if unit_mm.is_finite() && unit_mm > 0.0 { unit_mm } else { 1.0 };
    let q = eval_with(text, unit_mm).ok()?;
    Some(if q.length { q.value / unit_mm } else { q.value })
}

/// `text` as millimetres (a bare number is taken as millimetres).
pub fn eval_mm(text: &str) -> Option<f64> {
    eval_in(text, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mm(text: &str) -> f64 {
        eval_mm(text).unwrap_or_else(|| panic!("{text} did not evaluate"))
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn plain_arithmetic_and_precedence() {
        assert_eq!(mm("1 + 2 * 3"), 7.0);
        assert_eq!(mm("(1 + 2) * 3"), 9.0);
        assert_eq!(mm("10 / 4"), 2.5);
        assert_eq!(mm("10 - 4 - 3"), 3.0, "left to right");
        assert_eq!(mm("8 / 4 / 2"), 1.0, "left to right");
        assert_eq!(mm("  42 "), 42.0);
        assert_eq!(mm("((2))"), 2.0);
        assert_eq!(mm("1e3"), 1000.0);
        assert_eq!(mm("2.5E-1"), 0.25);
        assert_eq!(mm(".5"), 0.5);
        assert_eq!(mm("5."), 5.0);
        assert!(!eval("3*4").unwrap().length);
    }

    #[test]
    fn unary_minus_and_plus() {
        assert_eq!(mm("-5"), -5.0);
        assert_eq!(mm("--5"), 5.0);
        assert_eq!(mm("+5"), 5.0);
        assert_eq!(mm("3 - -2"), 5.0);
        assert_eq!(mm("-(2 + 3) * 2"), -10.0);
        assert_eq!(mm("\u{2212}4"), -4.0, "the minus sign character");
        assert_eq!(mm("2 \u{d7} 3 \u{f7} 4"), 1.5);
    }

    #[test]
    fn decimal_comma() {
        assert_eq!(mm("2,5"), 2.5);
        assert_eq!(mm("2,5 + 0,5"), 3.0);
        assert!(close(mm("1,5in"), 38.1));
        assert!(eval("1,2.3").is_err(), "a comma beside a point is not guessed at");
        assert!(eval("1.2.3").is_err());
    }

    #[test]
    fn units_convert_to_millimetres() {
        assert_eq!(mm("5mm"), 5.0);
        assert_eq!(mm("2 cm"), 20.0);
        assert_eq!(mm("0.5m"), 500.0);
        assert!(close(mm("1in"), 25.4));
        assert!(close(mm("1\""), 25.4));
        assert!(close(mm("1ft"), 304.8));
        assert!(close(mm("1'"), 304.8));
        assert!(close(mm("72pt"), 25.4));
        assert!(close(mm("2 IN"), 50.8), "case does not matter");
        assert!(close(mm("1 inch"), 25.4));
        assert!(close(mm("2 feet"), 609.6));
        assert!(eval("2in").unwrap().length);
    }

    #[test]
    fn the_examples() {
        assert!(close(mm("5' + 2\""), 5.0 * 304.8 + 2.0 * 25.4));
        assert!(close(mm("10in/4"), 63.5));
        assert!(close(mm("25.4mm*2"), 50.8));
        assert!(close(mm("5' 2\""), 5.0 * 304.8 + 2.0 * 25.4), "feet and inches side by side");
        assert!(close(mm("(2 + 3)cm"), 50.0), "a unit after brackets");
        assert!(close(mm("2 * 3in"), 152.4));
        assert!(close(mm("1in - 4mm"), 21.4));
    }

    #[test]
    fn display_units() {
        assert!(close(eval_in("25.4mm", 25.4).unwrap(), 1.0));
        assert!(close(eval_in("2", 25.4).unwrap(), 2.0), "bare numbers stay in the field's unit");
        assert!(close(eval_in("1in + 1", 25.4).unwrap(), 2.0), "a bare number added to a length is in the field's unit");
        assert!(close(eval_in("1in + 1", 1.0).unwrap(), 26.4));
        assert!(close(eval_in("10in / 2in", 1.0).unwrap(), 5.0), "a length over a length is a ratio");
        assert!(close(eval_in("50 %", 1.0).unwrap_or(-1.0), -1.0), "per cent is not a unit");
    }

    #[test]
    fn errors() {
        assert_eq!(eval(""), Err(ExprError::Empty));
        assert_eq!(eval("   "), Err(ExprError::Empty));
        assert_eq!(eval("1 / 0"), Err(ExprError::DivideByZero));
        assert_eq!(eval("1 / (2 - 2)"), Err(ExprError::DivideByZero));
        assert_eq!(eval("3 parsecs"), Err(ExprError::UnknownUnit("parsecs".into())));
        assert_eq!(eval("2in * 3in"), Err(ExprError::BadUnits));
        assert_eq!(eval("2 / 3in"), Err(ExprError::BadUnits));
        assert!(matches!(eval("(1 + 2"), Err(ExprError::Unexpected(_))));
        assert!(matches!(eval("1 + 2)"), Err(ExprError::Unexpected(_))));
        assert!(matches!(eval("1 +"), Err(ExprError::Unexpected(_))));
        assert!(matches!(eval("* 2"), Err(ExprError::Unexpected(_))));
        assert!(matches!(eval("2mm mm"), Err(ExprError::Unexpected(_))));
        assert!(matches!(eval("2 3"), Err(ExprError::Unexpected(_))), "bare numbers side by side");
        assert!(matches!(eval("5' 2"), Err(ExprError::Unexpected(_))), "only lengths add side by side");
        assert!(matches!(eval("."), Err(ExprError::Unexpected(_))));
        assert!(matches!(eval("abc"), Err(ExprError::UnknownUnit(_))));
        assert_eq!(eval("1e308 * 10"), Err(ExprError::NotFinite));
        assert!(eval_mm("1 / 0").is_none());
        assert!(!ExprError::DivideByZero.to_string().is_empty());
    }
}
