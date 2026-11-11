# Attribution for darkburn-vision

> The root [`ATTRIBUTION.md`](../../ATTRIBUTION.md) is newer than this file. It lists where the two
> differ, and where they differ the root file holds.

All code in this crate is original DarkBurn code (MIT OR Apache-2.0), written from published mathematics.
No code was copied from any other project, and no OpenCV code or data is used.

## Methods (public literature)

- Camera calibration from a planar target: Z. Zhang, "A flexible new technique for camera calibration", IEEE TPAMI 2000
  (closed-form intrinsics from homographies, then non-linear refinement of intrinsics, distortion and poses).
- Radial distortion polynomial (Brown-Conrady, k1 to k3) and the equidistant fisheye model `θd = θ(1 + k1θ² + k2θ⁴ + ...)`
  as described in the standard computer-vision literature.
- Normalised direct linear transform for homographies: R. Hartley and A. Zisserman, "Multiple View Geometry in Computer Vision",
  and R. Hartley, "In defense of the eight-point algorithm", 1997.
- Levenberg-Marquardt least squares: K. Levenberg 1944, D. Marquardt 1963.
- Jacobi eigenvalue iteration for symmetric matrices (textbook method).
- Otsu thresholding: N. Otsu, "A threshold selection method from gray-level histograms", 1979.
- Saddle-point (Hessian determinant) corner detection for checkerboards, connected-component blob detection: textbook image processing.
- Similarity transform fit between point sets: S. Umeyama, "Least-squares estimation of transformation parameters between two point
  patterns", IEEE TPAMI 1991.
- Circle fit: I. Kåsa, "A circle fitting procedure and its error analysis", 1976, refined by geometric least squares.
- Monotone-chain convex hull: A. M. Andrew, 1979.

## Dependencies

- `image` (MIT OR Apache-2.0), `kurbo` (MIT OR Apache-2.0), `serde` and `serde_json` (MIT OR Apache-2.0).
- `nokhwa` (Apache-2.0), optional, only with the off-by-default `capture` feature, for live webcam frames.

## Behaviour references

Feature behaviour (what the camera, rotary, print and cut and test tools do) comes from `docs/PARITY-CHECKLIST.md` and
`docs/research/lightburn-feature-inventory.md`, which describe features, not implementation. The seven-segment stroke font
in `src/strokefont.rs` is an original design.
