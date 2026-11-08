# darkburn-raster attribution

No third-party source code is copied or ported into this crate. Everything is written from
scratch, from published descriptions of the algorithms:

| Part | Source of the method |
|---|---|
| Error diffusion weights (`dither.rs`) | Floyd & Steinberg (1976); Jarvis, Judice & Ninke (1976); Stucki (1981); Bill Atkinson's MacPaint dither. The weight tables are published facts. |
| Bayer ordered dither | B. E. Bayer, "An optimum method for two-level rendition of continuous-tone pictures" (1973). |
| Distance transform (`edt.rs`) | P. Felzenszwalb & D. Huttenlocher, "Distance Transforms of Sampled Functions", Theory of Computing (2012). |
| Thinning (`trace/contour.rs`) | T. Y. Zhang & C. Y. Suen, "A fast parallel algorithm for thinning digital patterns", CACM (1984). |
| Curve fitting (`trace/fit.rs`) | P. J. Schneider, "An Algorithm for Automatically Fitting Digitized Curves", Graphics Gems (1990). |
| Polyline simplification | Ramer (1972) / Douglas & Peucker (1973). |
| sRGB transfer functions | IEC 61966-2-1. |

The image tracer is our own (pixel-edge boundary loops, corner detection, least-squares cubic
fitting). It contains no potrace code; potrace is GPL and is not used or read.

## Dependencies

| Crate | Licence | Use |
|---|---|---|
| `image` | MIT OR Apache-2.0 | Decoding and encoding images |
| `kurbo` | MIT OR Apache-2.0 | Geometry types at the crate boundary |
| `tiny-skia` | BSD-3-Clause | Rasterising vectors (Convert to bitmap, image masks) |
| `serde`, `serde_json` | MIT OR Apache-2.0 | Presets |
