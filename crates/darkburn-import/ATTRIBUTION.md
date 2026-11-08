# darkburn-import attribution

`darkburn-import` is part of DarkBurn and licensed MIT OR Apache-2.0. Nothing in it is taken from
LightBurn: no code, sample files, icons or text. All test fixtures in `tests/data/` were written
for this crate by `tests/gen_fixtures.py`.

## Formats written from public descriptions and permissive readers

| Area | Source | Licence | What was used |
|---|---|---|---|
| LightBurn `.lbrn` / `.lbrn2` reader and `.lbrn2` writer (`src/lbrn.rs`) | MeerK40t, `meerk40t/extra/lbrn.py` (https://github.com/meerk40t/meerk40t) | MIT | How `VertList` / `PrimList` are tokenised (`V`, `S`, `c0x c0y c1x c1y`, the `c0x1` "no handle" placeholder), the `LineClosed` and empty `PrimList` forms, XForm composition, `Polygon` vertex placement, `Bitmap` placement around the shape centre. Re-implemented in Rust, not copied. |
| | lbrn2-to-svg (https://github.com/jlucaso1/lbrn2-to-svg), Copyright (c) 2025 João Lucas de Oliveira Lopes | MIT | Cross-check of shape semantics (centred `Rect`/`Ellipse`/`Bitmap`, group transform composition). |
| | DarkBurn research notes, `docs/research/laser-protocols-and-devices.md` part 2e | - | Element and attribute names, CutSetting fields, v0 vs v1 layout. |
| DXF, PDF, HPGL, SVG, G-code | Public format specifications | - | Interpreters written from the specifications. |
| ACI colour palette (`dxf_import::aci_to_rgb`) | Generated from the published hue/shade layout of the AutoCAD Colour Index | - | Formula, not a copied table. |

No GPL, LGPL or AGPL code was read for porting (for example Boxes.py's lbrn2 writer was not used).

## Crates used as dependencies (unchanged, from crates.io)

| Crate | Licence | Used for |
|---|---|---|
| `dxf` (IxMilia) | MIT | DXF reading and writing |
| `lopdf` | MIT | PDF object model, stream filters, PDF writing for AI export |
| `fax` (pdf-rs) | MIT | CCITT G3/G4 image decoding in PDFs |
| `quick-xml` | MIT | XML reading for LightBurn projects |
| `kurbo` | Apache-2.0 OR MIT | Paths, arcs, transforms |
| `image` | MIT OR Apache-2.0 | PNG/JPEG decoding and PNG encoding |
| `flate2` | MIT OR Apache-2.0 | zlib compression for PDF/AI export |
| `base64` | MIT OR Apache-2.0 | Embedded images and thumbnails |
| `serde` | MIT OR Apache-2.0 | Serialisable options and layer settings |
