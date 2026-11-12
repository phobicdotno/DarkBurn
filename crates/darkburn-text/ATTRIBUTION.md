# Attribution for darkburn-text

> The root [`ATTRIBUTION.md`](../../ATTRIBUTION.md) is newer than this file. It lists where the two
> differ, and where they differ the root file holds.

All code in this crate is original DarkBurn code (MIT OR Apache-2.0). Formats were implemented from
public specifications; no third-party source was copied and nothing from any commercial laser software
was consulted.

## Specifications used (documentation only)

- AutoCAD shape / SHX file format: publicly documented header, shape table and byte codes.
- Code 128 (ISO/IEC 15417), Code 39 (ISO/IEC 16388), EAN-13 / UPC-A (ISO/IEC 15420), Interleaved 2 of 5
  (ISO/IEC 16390): symbol tables written out from the public symbology descriptions and checked by
  tests (pattern sums, uniqueness, round-trip decoding).
- QR payload conventions: `WIFI:` (ZXing barcode contents wiki, public) and `MECARD:` (NTT DoCoMo, public).
- Unicode Bidirectional Algorithm (UAX #9) through `unicode-bidi`.

## Dependencies (all permissive)

| Crate | Licence | Use |
|---|---|---|
| kurbo | Apache-2.0 OR MIT | path geometry |
| ttf-parser | MIT OR Apache-2.0 | glyph outlines |
| rustybuzz | MIT | text shaping (HarfBuzz port) |
| fontdb | MIT | system and file font discovery |
| unicode-bidi | MIT OR Apache-2.0 | RTL reordering |
| i_overlay | MIT OR Apache-2.0 | union for weld and merged QR outlines |
| qrcode | MIT OR Apache-2.0 | QR encoding |
| datamatrix | Apache-2.0 OR MIT | Data Matrix encoding |
| chrono | MIT OR Apache-2.0 | strftime date/time |
| csv | Unlicense OR MIT | merge files |
| serde, serde_json | MIT OR Apache-2.0 | serialisation |

## Test fonts (`tests/fonts/`)

- IBM Plex Mono Regular and IBM Plex Sans Variable, Copyright IBM Corp., SIL Open Font License 1.1
  (`OFL.txt`, `OFL-sans.txt`). Copied from the repository's `assets/fonts`. Used only by tests.
