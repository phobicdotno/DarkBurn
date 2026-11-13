//! `.egv` files: EGV text behind a short header.
//!
//! The header written here is the one MeerK40t writes (MIT; see `ATTRIBUTION.md`):
//!
//! ```text
//! Document type : LHYMICRO-GL file
//! File version: 1.0.01
//! Copyright: Unknown
//! Creator-Software: DarkBurn v0.0.0
//!
//! %0%0%0%0%
//! ```
//!
//! Reading is tolerant, as in MeerK40t: the body starts after the third line end and then the
//! fifth `%`. A file without them is taken to be EGV from the first byte.

/// First line of the header.
pub const HEADER_FIRST_LINE: &str = "Document type : LHYMICRO-GL file";

/// The header for a file written by `creator` (a name and version, e.g. `DarkBurn v1.0`).
/// Line ends and `%` are dropped from the name because they mark where the header ends.
pub fn header(creator: &str) -> String {
    let creator: String = creator.chars().filter(|c| !matches!(c, '\n' | '\r' | '%')).collect();
    format!("{HEADER_FIRST_LINE}\nFile version: 1.0.01\nCopyright: Unknown\nCreator-Software: {creator}\n\n%0%0%0%0%\n")
}

/// A whole `.egv` file from EGV text, written by this version of DarkBurn.
pub fn write_egv(body: &str) -> String {
    let mut out = header(&format!("DarkBurn v{}", env!("CARGO_PKG_VERSION")));
    out.push_str(body);
    out
}

/// The EGV part of a `.egv` file.
pub fn egv_body(data: &[u8]) -> &[u8] {
    fn skip(data: &[u8], from: usize, byte: u8, count: usize) -> Option<usize> {
        let mut seen = 0;
        for (i, &b) in data.iter().enumerate().skip(from) {
            if b == byte {
                seen += 1;
                if seen == count {
                    return Some(i + 1);
                }
            }
        }
        None
    }
    match skip(data, 0, b'\n', 3).and_then(|at| skip(data, at, b'%', 5)) {
        Some(at) => &data[at..],
        None => data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECT: &str = "IBzzzvRzzzvS1P\nICV2490731016000027CNLBS1EDz139Rz139Tz139Lz139FNSE-\n";

    #[test]
    fn header_matches_the_reference_layout() {
        assert_eq!(
            header("DarkBurn v1.2.3"),
            "Document type : LHYMICRO-GL file\nFile version: 1.0.01\nCopyright: Unknown\nCreator-Software: DarkBurn v1.2.3\n\n%0%0%0%0%\n"
        );
    }

    #[test]
    fn written_files_read_back() {
        let file = write_egv(RECT);
        assert!(file.starts_with(HEADER_FIRST_LINE));
        assert!(file.contains(&format!("DarkBurn v{}", env!("CARGO_PKG_VERSION"))));
        assert_eq!(egv_body(file.as_bytes()), format!("\n{RECT}").as_bytes());
    }

    #[test]
    fn creator_names_cannot_break_the_header() {
        let file = format!("{}{RECT}", header("odd%\nname"));
        assert!(file.contains("Creator-Software: oddname\n"));
        assert_eq!(egv_body(file.as_bytes()), format!("\n{RECT}").as_bytes());
    }

    #[test]
    fn files_without_a_header_are_all_body() {
        assert_eq!(egv_body(RECT.as_bytes()), RECT.as_bytes());
        assert_eq!(egv_body(b""), b"");
        // Three line ends but not five percent signs.
        let odd = "a\nb\nc\n%%\nIPP\n";
        assert_eq!(egv_body(odd.as_bytes()), odd.as_bytes());
    }

    #[test]
    fn other_headers_are_skipped_too() {
        let file =
            "Document type : LHYMICRO-GL file\r\nFile version: 1.0.01\r\nCopyright: Unknown\r\nCreator-Software: Other\r\n\r\n%0%0%0%0%\r\nIPP\n";
        assert_eq!(egv_body(file.as_bytes()), b"\r\nIPP\n");
    }
}
