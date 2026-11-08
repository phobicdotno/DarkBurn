//! Small helpers: random hex, constant-time compare, base64 decode, JSON Schema checks.

use serde_json::Value;

/// `n` random bytes as lower-case hex (2n digits), from the OS random source.
pub fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    if getrandom::fill(&mut buf).is_err() {
        // The OS refused randomness: fall back to time and address entropy mixed through FNV so a
        // token is at least never constant. This path is not expected on any supported system.
        let seed = crate::types::now_ms() ^ (std::ptr::addr_of!(buf) as usize as u64);
        let mut h = seed | 1;
        for b in &mut buf {
            h ^= h << 13;
            h ^= h >> 7;
            h ^= h << 17;
            *b = (h & 0xff) as u8;
        }
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compare two byte strings in time that depends only on the longer length, not on where they
/// differ.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= usize::from(x ^ y);
    }
    diff == 0
}

/// Standard base64 decode (padding optional). `None` on a bad character.
/// Standard base64 with padding.
pub fn base64_encode(data: &[u8]) -> String {
    solidcraft_mcp_server::base64(data)
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\r' | b'\n' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// `Ok` when `schema` is a valid JSON Schema document (compiles under its draft's meta-schema).
pub fn check_schema_document(schema: &Value) -> Result<(), String> {
    jsonschema::validator_for(schema).map(|_| ()).map_err(|e| e.to_string())
}

/// Validate `instance` against `schema`; the error lists up to five problems with their JSON
/// paths.
pub fn validate_instance(schema: &Value, instance: &Value) -> Result<(), String> {
    let v = jsonschema::validator_for(schema).map_err(|e| format!("the command's own params schema is invalid: {e}"))?;
    let errs: Vec<String> = v
        .iter_errors(instance)
        .take(5)
        .map(|e| {
            let p = e.instance_path.to_string();
            if p.is_empty() { e.to_string() } else { format!("at `{p}`: {e}") }
        })
        .collect();
    if errs.is_empty() { Ok(()) } else { Err(errs.join("; ")) }
}
