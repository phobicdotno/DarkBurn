//! A tiny read-only XML tree on top of quick-xml.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

#[derive(Clone, Debug, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn attr_f64(&self, key: &str) -> Option<f64> {
        self.attr(key).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
    }

    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

const MAX_DEPTH: usize = 512;

fn start_element(e: &BytesStart) -> Element {
    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    let mut attrs = Vec::new();
    for a in e.attributes().with_checks(false).flatten() {
        let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let v = a
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map(|v| v.into_owned())
            .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned());
        attrs.push((k, v));
    }
    Element { name, attrs, children: Vec::new(), text: String::new() }
}

fn entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        _ => return None,
    })
}

/// Parses a document and returns its root element.
pub fn parse(text: &str) -> Result<Element, String> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().check_end_names = false;
    let mut stack: Vec<Element> = vec![Element::default()];
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if stack.len() > MAX_DEPTH {
                    return Err("XML nested too deeply".into());
                }
                stack.push(start_element(&e));
            }
            Ok(Event::Empty(e)) => {
                let el = start_element(&e);
                if let Some(top) = stack.last_mut() {
                    top.children.push(el);
                }
            }
            Ok(Event::End(_)) => {
                if stack.len() > 1 {
                    let el = stack.pop().unwrap_or_default();
                    if let Some(top) = stack.last_mut() {
                        top.children.push(el);
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if let (Some(top), Ok(s)) = (stack.last_mut(), t.decode()) {
                    top.text.push_str(&s);
                }
            }
            Ok(Event::CData(t)) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(&t));
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if let Some(top) = stack.last_mut() {
                    if let Ok(Some(c)) = r.resolve_char_ref() {
                        top.text.push(c);
                    } else if let Some(c) = r.decode().ok().and_then(|n| entity(&n)) {
                        top.text.push(c);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("XML error at byte {}: {e}", reader.buffer_position())),
        }
    }
    while stack.len() > 1 {
        let el = stack.pop().unwrap_or_default();
        if let Some(top) = stack.last_mut() {
            top.children.push(el);
        }
    }
    let doc = stack.pop().unwrap_or_default();
    doc.children.into_iter().next().ok_or_else(|| "empty XML document".to_string())
}

/// Escapes text for an XML attribute value.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}
