//! Tolerant reader and byte-faithful writer for Mlaser's parameter XML (recipes, layer tables,
//! machine backups).
//!
//! The vendor program writes a very small XML dialect: a root element (`<ParameterRoot>`), section
//! elements under it (`<PLayerParam11>`, `<PManuParam>`, ...) and self-closing entry elements in
//! each section (`<GP .../>`, `<MC .../>`), one element per line, CRLF line ends, a trailing CRLF,
//! UTF-8 without a byte-order mark and no XML declaration. Numbers are written like C's `%.17g`
//! (`0.59999999999999998`), integers plain. The format is described in
//! `docs/research/mlaser-files.md`.
//!
//! [`ParamDoc`] keeps every element and attribute in file order with the attribute text exactly as
//! stored ([`Attr::raw`]), so writing back a document that was read is byte-identical, and a
//! document where a few attributes were changed differs only in those attributes. The parser is
//! hand-written and has no dependency: it accepts an XML declaration, comments and both
//! self-closing and open/close entries, refuses deeper nesting, text content and unbalanced tags
//! with an error, and never panics on hostile input. Comments are dropped.

use std::fmt;

/// Largest input accepted, bytes. The vendor's largest file (a full machine backup) is about
/// 60 KB.
pub const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
/// Most elements accepted in one document.
pub const MAX_ELEMENTS: usize = 200_000;
/// Most attributes accepted on one element.
pub const MAX_ATTRS: usize = 4096;

/// An error while reading an Mlaser file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MlaserError {
    /// 1-based line, when known.
    pub line: Option<usize>,
    /// What is wrong.
    pub message: String,
}

impl MlaserError {
    /// An error without a line.
    pub fn new(message: impl Into<String>) -> MlaserError {
        MlaserError { line: None, message: message.into() }
    }

    /// An error at a line.
    pub fn at(line: usize, message: impl Into<String>) -> MlaserError {
        MlaserError { line: Some(line), message: message.into() }
    }
}

impl fmt::Display for MlaserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(l) => write!(f, "line {l}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for MlaserError {}

/// How the text of a file was encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextEncoding {
    /// UTF-8 without a byte-order mark (what the vendor writes).
    #[default]
    Utf8,
    /// UTF-8 with a byte-order mark.
    Utf8Bom,
    /// Code page 936 (GBK), decoded with the `cp936` feature.
    Gbk,
    /// Neither UTF-8 nor decodable: bytes that are not UTF-8 became U+FFFD. Writing such a
    /// document produces UTF-8.
    Lossy,
}

/// Decode file bytes: a UTF-8 byte-order mark is stripped, valid UTF-8 is taken as is, anything
/// else is code page 936 with the `cp936` feature and a lossy UTF-8 conversion without it.
pub fn decode_text(bytes: &[u8]) -> (String, TextEncoding) {
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return (String::from_utf8_lossy(rest).into_owned(), TextEncoding::Utf8Bom);
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return (s.to_string(), TextEncoding::Utf8);
    }
    #[cfg(feature = "cp936")]
    {
        (encoding_rs::GBK.decode_without_bom_handling(bytes).0.into_owned(), TextEncoding::Gbk)
    }
    #[cfg(not(feature = "cp936"))]
    {
        (String::from_utf8_lossy(bytes).into_owned(), TextEncoding::Lossy)
    }
}

/// Encode text back into the encoding it was read with. [`TextEncoding::Lossy`] (and
/// [`TextEncoding::Gbk`] without the `cp936` feature) writes UTF-8.
pub fn encode_text(s: &str, enc: TextEncoding) -> Vec<u8> {
    match enc {
        TextEncoding::Utf8 | TextEncoding::Lossy => s.as_bytes().to_vec(),
        TextEncoding::Utf8Bom => {
            let mut v = b"\xEF\xBB\xBF".to_vec();
            v.extend_from_slice(s.as_bytes());
            v
        }
        TextEncoding::Gbk => {
            #[cfg(feature = "cp936")]
            {
                encoding_rs::GBK.encode(s).0.into_owned()
            }
            #[cfg(not(feature = "cp936"))]
            {
                s.as_bytes().to_vec()
            }
        }
    }
}

/// Line ending of a text file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LineEnding {
    /// `\r\n` (what the vendor writes).
    #[default]
    CrLf,
    /// `\n`.
    Lf,
}

impl LineEnding {
    /// The characters.
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::CrLf => "\r\n",
            LineEnding::Lf => "\n",
        }
    }

    /// The ending of the first line break in `s`, CRLF when there is none.
    pub fn detect(s: &str) -> LineEnding {
        match s.find('\n') {
            Some(i) if i > 0 && s.as_bytes().get(i - 1) == Some(&b'\r') => LineEnding::CrLf,
            Some(_) => LineEnding::Lf,
            None => LineEnding::CrLf,
        }
    }
}

/// Format a number the way the vendor program does (C's `%.17g`): 17 significant digits, trailing
/// zeros removed, exponent form below 1e-4 and from 1e17 up. `0.6` becomes `0.59999999999999998`,
/// `100.0` becomes `100`. Non-finite values have no vendor form and give `None`.
pub fn format_g17(x: f64) -> Option<String> {
    if !x.is_finite() {
        return None;
    }
    if x == 0.0 {
        return Some(if x.is_sign_negative() { "-0".to_string() } else { "0".to_string() });
    }
    const P: i32 = 17;
    let e_form = format!("{:.*e}", (P - 1) as usize, x);
    let (mantissa, exp) = e_form.split_once('e')?;
    let exp: i32 = exp.parse().ok()?;
    if !(-4..P).contains(&exp) {
        let m = strip_zeros(mantissa);
        let sign = if exp < 0 { '-' } else { '+' };
        Some(format!("{m}e{sign}{:02}", exp.abs()))
    } else {
        let decimals = (P - 1 - exp).max(0) as usize;
        Some(strip_zeros(&format!("{x:.decimals$}")).to_string())
    }
}

fn strip_zeros(s: &str) -> &str {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { s }
}

/// Parse a vendor number. Leading and trailing blanks are allowed; anything that is not a finite
/// number (including `nan`, `inf` and values that overflow) is `None`.
pub fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() || t.len() > 64 {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    if lower.contains("inf") || lower.contains("nan") {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Escape text for an attribute value.
pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\r' => out.push_str("&#13;"),
            '\n' => out.push_str("&#10;"),
            '\t' => out.push_str("&#9;"),
            c => out.push(c),
        }
    }
    out
}

/// Decode the entities of an attribute value. An unknown or broken entity is kept as written.
pub fn unescape_attr(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let decoded = tail.get(1..).and_then(|t| t.find(';').filter(|&j| j <= 10).map(|j| (&t[..j], j))).and_then(|(name, j)| {
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
                n if n.starts_with('#') => n[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            c.map(|c| (c, j + 2))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// One attribute, with its value exactly as stored (entities not decoded).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attr {
    /// Name.
    pub name: String,
    /// Stored text, entities not decoded.
    pub raw: String,
}

impl Attr {
    /// The value with entities decoded.
    pub fn value(&self) -> String {
        unescape_attr(&self.raw)
    }
}

/// A list of attributes in file order, with typed getters and setters. A setter changes an
/// attribute in place, or appends it when absent.
pub trait AttrList {
    /// The attributes.
    fn attrs(&self) -> &[Attr];
    /// The attributes, mutable.
    fn attrs_mut(&mut self) -> &mut Vec<Attr>;

    /// The decoded value of the first attribute called `name`.
    fn get(&self, name: &str) -> Option<String> {
        self.attrs().iter().find(|a| a.name == name).map(Attr::value)
    }
    /// True if the attribute is present.
    fn has(&self, name: &str) -> bool {
        self.attrs().iter().any(|a| a.name == name)
    }
    /// A finite number, or `None` when absent or not a number.
    fn get_f64(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(|v| parse_number(&v))
    }
    /// An integer (a number with no fraction that fits), or `None`.
    fn get_i64(&self, name: &str) -> Option<i64> {
        self.get_f64(name).filter(|v| v.fract() == 0.0 && v.abs() < 9.0e15).map(|v| v as i64)
    }
    /// `0` is false, any other integer true.
    fn get_bool(&self, name: &str) -> Option<bool> {
        self.get_i64(name).map(|v| v != 0)
    }
    /// Set the stored text, escaping it.
    fn set(&mut self, name: &str, value: &str) {
        let raw = escape_attr(value);
        let attrs = self.attrs_mut();
        match attrs.iter_mut().find(|a| a.name == name) {
            Some(a) => a.raw = raw,
            None => attrs.push(Attr { name: name.to_string(), raw }),
        }
    }
    /// Set a number in the vendor's `%.17g` form. Non-finite numbers are refused.
    fn set_f64(&mut self, name: &str, value: f64) -> Result<(), MlaserError> {
        let s = format_g17(value).ok_or_else(|| MlaserError::new(format!("{name}: {value} is not a finite number")))?;
        self.set(name, &s);
        Ok(())
    }
    /// Set `1` or `0`.
    fn set_bool(&mut self, name: &str, value: bool) {
        self.set(name, if value { "1" } else { "0" });
    }
    /// Remove an attribute; true if it was present.
    fn remove(&mut self, name: &str) -> bool {
        let attrs = self.attrs_mut();
        let before = attrs.len();
        attrs.retain(|a| a.name != name);
        attrs.len() != before
    }
}

/// An entry element (`<GP .../>`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Entry {
    /// Element name.
    pub name: String,
    /// Attributes in file order.
    pub attrs: Vec<Attr>,
}

impl AttrList for Entry {
    fn attrs(&self) -> &[Attr] {
        &self.attrs
    }
    fn attrs_mut(&mut self) -> &mut Vec<Attr> {
        &mut self.attrs
    }
}

/// A section element (`<PLayerParam11>`) with its entries.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Section {
    /// Element name.
    pub name: String,
    /// Attributes of the section element itself (the vendor writes none).
    pub attrs: Vec<Attr>,
    /// Entries in file order.
    pub entries: Vec<Entry>,
}

impl Section {
    /// The first entry called `name`.
    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// The first entry called `name`, mutable.
    pub fn entry_mut(&mut self, name: &str) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.name == name)
    }
}

/// A parsed parameter document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamDoc {
    /// The XML declaration, without `<?` and `?>`, when the file had one.
    pub declaration: Option<String>,
    /// Root element name (`ParameterRoot`).
    pub root: String,
    /// Attributes of the root element (the vendor writes none).
    pub root_attrs: Vec<Attr>,
    /// Sections in file order.
    pub sections: Vec<Section>,
    /// Line ending used when writing.
    pub line_ending: LineEnding,
    /// Whether the file ends with a line ending.
    pub trailing_newline: bool,
    /// Text encoding used when writing.
    pub encoding: TextEncoding,
}

impl Default for ParamDoc {
    fn default() -> ParamDoc {
        ParamDoc {
            declaration: None,
            root: "ParameterRoot".to_string(),
            root_attrs: Vec::new(),
            sections: Vec::new(),
            line_ending: LineEnding::CrLf,
            trailing_newline: true,
            encoding: TextEncoding::Utf8,
        }
    }
}

impl ParamDoc {
    /// The first section called `name`.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    /// The first section called `name`, mutable.
    pub fn section_mut(&mut self, name: &str) -> Option<&mut Section> {
        self.sections.iter_mut().find(|s| s.name == name)
    }

    /// Parse file bytes.
    pub fn parse(bytes: &[u8]) -> Result<ParamDoc, MlaserError> {
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(MlaserError::new(format!("file is {} bytes, more than the {MAX_INPUT_BYTES} accepted", bytes.len())));
        }
        let (text, encoding) = decode_text(bytes);
        let mut doc = parse_str(&text)?;
        doc.encoding = encoding;
        Ok(doc)
    }

    /// Write the document as text, in the vendor layout.
    pub fn to_text(&self) -> String {
        let nl = self.line_ending.as_str();
        let mut lines: Vec<String> = Vec::new();
        if let Some(d) = &self.declaration {
            lines.push(format!("<?{d}?>"));
        }
        lines.push(open_tag(&self.root, &self.root_attrs, false));
        for s in &self.sections {
            lines.push(open_tag(&s.name, &s.attrs, false));
            for e in &s.entries {
                lines.push(open_tag(&e.name, &e.attrs, true));
            }
            lines.push(format!("</{}>", s.name));
        }
        lines.push(format!("</{}>", self.root));
        let mut out = lines.join(nl);
        if self.trailing_newline {
            out.push_str(nl);
        }
        out
    }

    /// Write the document as bytes in its encoding.
    pub fn to_bytes(&self) -> Vec<u8> {
        encode_text(&self.to_text(), self.encoding)
    }
}

fn open_tag(name: &str, attrs: &[Attr], self_closing: bool) -> String {
    let mut s = format!("<{name}");
    for a in attrs {
        s.push(' ');
        s.push_str(&a.name);
        s.push_str("=\"");
        s.push_str(&a.raw);
        s.push('"');
    }
    s.push_str(if self_closing { "/>" } else { ">" });
    s
}

enum Token {
    Open { name: String, attrs: Vec<Attr>, self_closing: bool, line: usize },
    Close { name: String, line: usize },
}

struct Lexer<'a> {
    s: &'a str,
    pos: usize,
    line: usize,
}

impl<'a> Lexer<'a> {
    fn rest(&self) -> &'a str {
        self.s.get(self.pos..).unwrap_or("")
    }

    fn advance(&mut self, n: usize) {
        let end = (self.pos + n).min(self.s.len());
        let skipped = self.s.get(self.pos..end).unwrap_or("");
        self.line += skipped.matches('\n').count();
        self.pos = end;
    }

    fn skip_ws(&mut self) {
        let n = self.rest().len() - self.rest().trim_start().len();
        self.advance(n);
    }

    fn name(&mut self) -> Result<String, MlaserError> {
        let r = self.rest();
        let n = r.find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))).unwrap_or(r.len());
        if n == 0 {
            return Err(MlaserError::at(self.line, "expected a name"));
        }
        let name = r[..n].to_string();
        self.advance(n);
        Ok(name)
    }

    /// Next token, or `None` at the end. Text outside tags must be blank.
    fn next_token(&mut self, decl: &mut Option<String>) -> Result<Option<Token>, MlaserError> {
        loop {
            self.skip_ws();
            let r = self.rest();
            if r.is_empty() {
                return Ok(None);
            }
            if !r.starts_with('<') {
                return Err(MlaserError::at(self.line, "text outside a tag"));
            }
            if let Some(body) = r.strip_prefix("<?") {
                let end = body.find("?>").ok_or_else(|| MlaserError::at(self.line, "unterminated declaration"))?;
                if decl.is_none() {
                    *decl = Some(body[..end].to_string());
                }
                self.advance(end + 4);
                continue;
            }
            if let Some(body) = r.strip_prefix("<!--") {
                let end = body.find("-->").ok_or_else(|| MlaserError::at(self.line, "unterminated comment"))?;
                self.advance(end + 7);
                continue;
            }
            if r.starts_with("<!") {
                return Err(MlaserError::at(self.line, "DOCTYPE and CDATA are not supported"));
            }
            let line = self.line;
            if r.starts_with("</") {
                self.advance(2);
                let name = self.name()?;
                self.skip_ws();
                if !self.rest().starts_with('>') {
                    return Err(MlaserError::at(self.line, format!("expected '>' after </{name}")));
                }
                self.advance(1);
                return Ok(Some(Token::Close { name, line }));
            }
            self.advance(1);
            let name = self.name()?;
            let mut attrs: Vec<Attr> = Vec::new();
            loop {
                self.skip_ws();
                let r = self.rest();
                if r.starts_with("/>") {
                    self.advance(2);
                    return Ok(Some(Token::Open { name, attrs, self_closing: true, line }));
                }
                if r.starts_with('>') {
                    self.advance(1);
                    return Ok(Some(Token::Open { name, attrs, self_closing: false, line }));
                }
                if r.is_empty() {
                    return Err(MlaserError::at(self.line, format!("unterminated tag <{name}")));
                }
                let aname = self.name()?;
                self.skip_ws();
                if !self.rest().starts_with('=') {
                    return Err(MlaserError::at(self.line, format!("attribute {aname} has no value")));
                }
                self.advance(1);
                self.skip_ws();
                let q = match self.rest().chars().next() {
                    Some(c @ ('"' | '\'')) => c,
                    _ => return Err(MlaserError::at(self.line, format!("attribute {aname} is not quoted"))),
                };
                self.advance(1);
                let r = self.rest();
                let end = r.find(q).ok_or_else(|| MlaserError::at(self.line, format!("attribute {aname} is not closed")))?;
                let raw = &r[..end];
                if raw.contains('<') {
                    return Err(MlaserError::at(self.line, format!("attribute {aname} contains '<'")));
                }
                // Single-quoted values are stored as if they had been written with double quotes.
                let raw = if q == '\'' { raw.replace('"', "&quot;") } else { raw.to_string() };
                if attrs.iter().any(|a| a.name == aname) {
                    return Err(MlaserError::at(self.line, format!("attribute {aname} given twice")));
                }
                if attrs.len() >= MAX_ATTRS {
                    return Err(MlaserError::at(self.line, format!("more than {MAX_ATTRS} attributes")));
                }
                attrs.push(Attr { name: aname, raw });
                self.advance(end + 1);
            }
        }
    }
}

/// Parse decoded text.
pub fn parse_str(text: &str) -> Result<ParamDoc, MlaserError> {
    let mut lx = Lexer { s: text, pos: 0, line: 1 };
    let mut doc =
        ParamDoc { line_ending: LineEnding::detect(text), trailing_newline: text.ends_with('\n'), root: String::new(), ..ParamDoc::default() };
    let mut decl = None;
    // 0 = before root, 1 = in root, 2 = in section, 3 = in an open/close entry, 4 = after root.
    let mut depth = 0u8;
    let mut count = 0usize;
    while let Some(tok) = lx.next_token(&mut decl)? {
        count += 1;
        if count > MAX_ELEMENTS {
            return Err(MlaserError::at(lx.line, format!("more than {MAX_ELEMENTS} elements")));
        }
        match tok {
            Token::Open { name, attrs, self_closing, line } => match depth {
                0 => {
                    doc.root = name;
                    doc.root_attrs = attrs;
                    depth = if self_closing { 4 } else { 1 };
                }
                1 => {
                    doc.sections.push(Section { name, attrs, entries: Vec::new() });
                    if !self_closing {
                        depth = 2;
                    }
                }
                2 => {
                    if let Some(s) = doc.sections.last_mut() {
                        s.entries.push(Entry { name, attrs });
                    }
                    if !self_closing {
                        depth = 3;
                    }
                }
                3 => return Err(MlaserError::at(line, format!("element <{name}> nested deeper than an entry"))),
                _ => return Err(MlaserError::at(line, format!("element <{name}> after the root element"))),
            },
            Token::Close { name, line } => {
                let expected = match depth {
                    1 => Some(doc.root.as_str()),
                    2 => doc.sections.last().map(|s| s.name.as_str()),
                    3 => doc.sections.last().and_then(|s| s.entries.last()).map(|e| e.name.as_str()),
                    _ => None,
                };
                if expected != Some(name.as_str()) {
                    return Err(MlaserError::at(line, format!("unexpected </{name}>")));
                }
                depth = if depth == 1 { 4 } else { depth - 1 };
            }
        }
    }
    doc.declaration = decl;
    match depth {
        0 => Err(MlaserError::new("no root element")),
        4 => Ok(doc),
        _ => Err(MlaserError::at(lx.line, "file ends inside an element")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str =
        "<ParameterRoot>\r\n<PDemo>\r\n<GP A=\"1\" B=\"0.59999999999999998\" C=\"L&quot;x&quot;\"/>\r\n<HP/>\r\n</PDemo>\r\n</ParameterRoot>\r\n";

    #[test]
    fn round_trip_is_byte_identical() {
        let doc = ParamDoc::parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(doc.root, "ParameterRoot");
        assert_eq!(doc.sections.len(), 1);
        let gp = doc.sections[0].entry("GP").unwrap();
        assert_eq!(gp.get("C").unwrap(), "L\"x\"");
        assert_eq!(gp.get_f64("B"), Some(0.6));
        assert_eq!(doc.to_bytes(), SAMPLE.as_bytes());
    }

    #[test]
    fn lf_and_no_trailing_newline_round_trip() {
        let s = "<R>\n<S>\n<E a=\"x\"/>\n</S>\n</R>";
        let doc = parse_str(s).unwrap();
        assert_eq!(doc.line_ending, LineEnding::Lf);
        assert!(!doc.trailing_newline);
        assert_eq!(doc.to_text(), s);
    }

    #[test]
    fn bom_and_declaration_are_kept() {
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend_from_slice(b"<?xml version=\"1.0\"?>\r\n<R>\r\n</R>\r\n");
        let doc = ParamDoc::parse(&bytes).unwrap();
        assert_eq!(doc.encoding, TextEncoding::Utf8Bom);
        assert_eq!(doc.declaration.as_deref(), Some("xml version=\"1.0\""));
        assert_eq!(doc.to_bytes(), bytes);
    }

    #[test]
    fn open_close_entries_comments_and_single_quotes_are_accepted() {
        let s = "<R><!-- c --><S><E a='say \"hi\"'></E></S></R>";
        let doc = parse_str(s).unwrap();
        let e = doc.sections[0].entry("E").unwrap();
        assert_eq!(e.get("a").unwrap(), "say \"hi\"");
    }

    #[test]
    fn g17_matches_c() {
        for (x, s) in [
            (0.6, "0.59999999999999998"),
            (2000.0 / 60.0, "33.333333333333336"),
            (3.45, "3.4500000000000002"),
            (100.0, "100"),
            (5000.0, "5000"),
            (0.0, "0"),
            (-0.0, "-0"),
            (1e17, "1e+17"),
            (1e-5, "1.0000000000000001e-05"),
            (-2.5, "-2.5"),
            (0.0001, "0.0001"),
            (123456789012345680.0, "1.2345678901234568e+17"),
        ] {
            assert_eq!(format_g17(x).unwrap(), s, "{x}");
        }
        assert_eq!(format_g17(f64::NAN), None);
        assert_eq!(format_g17(f64::INFINITY), None);
    }

    #[test]
    fn numbers_are_parsed_strictly() {
        assert_eq!(parse_number(" 12.5 "), Some(12.5));
        assert_eq!(parse_number("1e+17"), Some(1e17));
        assert_eq!(parse_number("nan"), None);
        assert_eq!(parse_number("-inf"), None);
        assert_eq!(parse_number("1e999"), None);
        assert_eq!(parse_number(""), None);
        assert_eq!(parse_number("12abc"), None);
        let mut e = Entry::default();
        e.set("I", "3.5");
        assert_eq!(e.get_i64("I"), None);
        e.set("I", "99999999999999999999999");
        assert_eq!(e.get_i64("I"), None);
    }

    #[test]
    fn setters_escape_and_append() {
        let mut e = Entry { name: "GP".into(), attrs: vec![Attr { name: "A".into(), raw: "1".into() }] };
        e.set("A", "a<b&\"c\"");
        e.set_bool("B", true);
        e.set_f64("C", 0.6).unwrap();
        assert!(e.set_f64("D", f64::NAN).is_err());
        assert_eq!(e.attrs[0].raw, "a&lt;b&amp;&quot;c&quot;");
        assert_eq!(e.get("A").unwrap(), "a<b&\"c\"");
        assert_eq!(e.attrs.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["A", "B", "C"]);
        assert!(e.remove("B"));
        assert!(!e.has("B"));
    }

    #[test]
    fn entities() {
        assert_eq!(unescape_attr("&#65;&#x42;&amp;&bogus;&"), "AB&&bogus;&");
        assert_eq!(unescape_attr("&#xFFFFFFFF;"), "&#xFFFFFFFF;");
        assert_eq!(unescape_attr(&escape_attr("a\r\nb\t\"<>&")), "a\r\nb\t\"<>&");
    }

    #[test]
    fn hostile_input_is_an_error_not_a_panic() {
        let cases: &[&[u8]] = &[
            b"",
            b"   ",
            b"<",
            b"<R",
            b"<R>",
            b"<R><S>",
            b"<R></S>",
            b"<R><S><E><F/></E></S></R>",
            b"<R>text</R>",
            b"<R><S a=1/></R>",
            b"<R><S a=\"1/></R>",
            b"<R><S a/></R>",
            b"<R><S a=\"1\" a=\"2\"/></R>",
            b"<R/><R/>",
            b"<!DOCTYPE x><R/>",
            b"<!-- open",
            b"<?xml",
            b"<R><S a=\"<\"/></R>",
            b"</R>",
            b"<\xFF\xFE>",
            b"<R>\xE2\x82</R>",
            b"<R><=\"\"/></R>",
        ];
        for c in cases {
            let _ = ParamDoc::parse(c);
        }
        assert!(ParamDoc::parse(b"<R><S><E><F/></E></S></R>").is_err());
        assert!(ParamDoc::parse(b"<R>text</R>").is_err());
        assert!(ParamDoc::parse(b"").is_err());
        assert!(ParamDoc::parse(b"<R/>").is_ok());
    }

    #[test]
    fn every_prefix_and_byte_flip_of_a_valid_file_is_handled() {
        let bytes = SAMPLE.as_bytes();
        for n in 0..bytes.len() {
            let _ = ParamDoc::parse(&bytes[..n]);
        }
        for i in 0..bytes.len() {
            for b in [b'<', b'>', b'"', b'/', b'&', 0u8, 0xFF] {
                let mut v = bytes.to_vec();
                v[i] = b;
                if let Ok(doc) = ParamDoc::parse(&v) {
                    let _ = doc.to_bytes();
                }
            }
        }
    }

    #[test]
    fn limits_are_enforced() {
        let mut s = String::from("<R><S>");
        for _ in 0..(MAX_ELEMENTS + 1) {
            s.push_str("<E/>");
        }
        s.push_str("</S></R>");
        assert!(parse_str(&s).is_err());
        let big = vec![b' '; MAX_INPUT_BYTES + 1];
        assert!(ParamDoc::parse(&big).is_err());
    }

    #[test]
    fn non_utf8_is_decoded_or_marked_lossy() {
        let bytes = b"<R>\r\n<S>\r\n<E n=\"\xB2\xBB\xD0\xE2\xB8\xD6\"/>\r\n</S>\r\n</R>\r\n";
        let doc = ParamDoc::parse(bytes).unwrap();
        #[cfg(feature = "cp936")]
        {
            assert_eq!(doc.encoding, TextEncoding::Gbk);
            assert_eq!(doc.sections[0].entries[0].get("n").unwrap(), "\u{4e0d}\u{9508}\u{94a2}");
            assert_eq!(doc.to_bytes(), bytes.to_vec());
        }
        #[cfg(not(feature = "cp936"))]
        {
            assert_eq!(doc.encoding, TextEncoding::Lossy);
            assert!(doc.sections[0].entries[0].get("n").unwrap().contains('\u{FFFD}'));
        }
    }
}
