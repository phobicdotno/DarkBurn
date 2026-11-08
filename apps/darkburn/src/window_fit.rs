// Copied from SolidCraft apps/solidcraft/src/window_fit.rs (MIT OR Apache-2.0, see ATTRIBUTION.md).
//! Keeping the restored window reachable.
//!
//! The window comes back where it was left. After a monitor was unplugged, the scale changed or
//! the settings came from a larger screen, that can put its title bar above the top edge or
//! under the taskbar, out of reach of the mouse. [`fit`] pulls a window back into the monitor's
//! work area (the screen without the taskbar), in whatever unit the caller measures both, as
//! long as it is the same one.

/// A rectangle: left, top, width, height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Area {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Area { x, y, w, h }
    }
    fn right(self) -> f32 {
        self.x + self.w
    }
    fn bottom(self) -> f32 {
        self.y + self.h
    }
    fn sane(self) -> bool {
        [self.x, self.y, self.w, self.h].iter().all(|v| v.is_finite()) && self.w > 0.0 && self.h > 0.0
    }
}

/// How much of the title bar must stay over the work area at least, to grab it.
pub const REACHABLE: f32 = 100.0;
/// The size a window gets when its saved geometry is unusable, before it is fitted to the screen.
pub const DEFAULT_SIZE: (f32, f32) = (1600.0, 1000.0);

/// `window` moved and shrunk so that it can be reached on `work`: no larger than the work area
/// (but not below `min`), the title bar not above the work area's top, the window not below its
/// bottom, and at least [`REACHABLE`] of it over the work area horizontally. A window that is
/// fine is returned unchanged. Geometry that makes no sense (not finite, empty), or a work
/// area that does not, gives the default size centred on the work area.
pub fn fit(window: Area, work: Area, min: (f32, f32)) -> Area {
    if !work.sane() {
        return window;
    }
    let min = (min.0.min(work.w), min.1.min(work.h));
    if !window.sane() {
        let w = DEFAULT_SIZE.0.min(work.w * 0.9).max(min.0);
        let h = DEFAULT_SIZE.1.min(work.h * 0.9).max(min.1);
        return Area::new(work.x + (work.w - w) / 2.0, work.y + (work.h - h) / 2.0, w, h);
    }
    let w = window.w.clamp(min.0, work.w);
    let h = window.h.clamp(min.1, work.h);
    let reach = REACHABLE.min(w);
    // A window that had to shrink goes fully onto the screen; one that fits may keep straddling two.
    let x = if w < window.w { window.x.max(work.x).min(work.right() - w) } else { window.x.max(work.x + reach - w).min(work.right() - reach) };
    let y = window.y.min(work.bottom() - h).max(work.y);
    Area::new(x, y, w, h)
}
