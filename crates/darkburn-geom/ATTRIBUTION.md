# darkburn-geom attribution

No third-party source code has been copied or ported into this crate. All algorithms here
are written from scratch from public textbook descriptions (shoelace area, Andrew's monotone
chain convex hull, bilinear/projective maps, tensor-product Bézier surfaces, shelf packing).

## Libraries used as dependencies (unmodified, from crates.io)

| Crate | Version | Licence | Used for |
|---|---|---|---|
| [kurbo](https://github.com/linebender/kurbo) | 0.13.1 | MIT OR Apache-2.0 | Paths, curves, arcs, affine maps, flattening, nearest point, arc length |
| [i_overlay](https://github.com/iShape-Rust/iOverlay) | 9.1.0 | MIT OR Apache-2.0 | Polygon booleans, shape outline (offset) and stroke construction |
| [cavalier_contours](https://github.com/jbuckmccready/cavalier_contours) | 0.9.0 | MIT OR Apache-2.0 | Line-and-arc polyline parallel offset for kerf offsets |

No GPL, LGPL or AGPL code (Clipper ports, potrace and similar) is used.
