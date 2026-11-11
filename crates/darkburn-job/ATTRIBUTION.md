# darkburn-job attribution

> The root [`ATTRIBUTION.md`](../../ATTRIBUTION.md) is newer than this file. It lists where the two
> differ, and where they differ the root file holds.

No third-party source code is copied or ported into this crate. Everything here was written
from scratch for DarkBurn.

## Libraries

| Crate | Licence | Use |
|---|---|---|
| [kurbo](https://github.com/linebender/kurbo) | MIT OR Apache-2.0 | Points, affine maps, Bézier flattening |
| [serde](https://github.com/serde-rs/serde) | MIT OR Apache-2.0 | Serialising the toolpath and settings |

## Documentation consulted (interfaces only, no code)

- GRBL 1.1 wiki: laser mode (M3/M4, `$30`), G-code and dwell semantics.
  GRBL, grblHAL and FluidNC firmware are GPL-3.0; none of their code was read or used.
- Marlin, Smoothieware and LinuxCNC public G-code references (laser power words, `M67`/`M68`,
  `G4` units).
- The junction speed limit in `sim.rs` follows the junction deviation model as described in
  public motion-control write-ups; it is re-derived, not ported.
