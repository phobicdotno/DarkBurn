//! How big the panel docks are: the size the user chose, kept inside what the window has room for.
//!
//! A dock has the size the user dragged it to (or its default). The window can be too small for
//! that: then the dock gives way so the drawing area, and the docks beside a bottom dock, stay on
//! screen. The chosen size is not forgotten: when the window grows again the dock goes back to it.

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

/// The three docks. The right dock holds two tab groups, one above the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dock {
    Left,
    Right,
    Bottom,
}

/// Width the drawing area keeps beside the side docks.
pub const CANVAS_MIN_WIDTH: f32 = 240.0;
/// Height the side docks and the drawing area keep above the bottom dock.
pub const SIDES_MIN_HEIGHT: f32 = 260.0;
/// A tab group is never squeezed below its tab row and a few lines, while the dock has the room.
pub const GROUP_MIN_HEIGHT: f32 = 110.0;

impl Dock {
    /// The name the size is saved under.
    pub fn key(self) -> &'static str {
        match self {
            Dock::Left => "left",
            Dock::Right => "right",
            Dock::Bottom => "bottom",
        }
    }

    pub fn default_size(self) -> f32 {
        match self {
            Dock::Left => 260.0,
            Dock::Right => 380.0,
            Dock::Bottom => 180.0,
        }
    }

    /// The sizes the user can drag the dock to when the window has the room.
    pub fn range(self) -> RangeInclusive<f32> {
        match self {
            Dock::Left => 180.0..=520.0,
            Dock::Right => 300.0..=640.0,
            Dock::Bottom => 100.0..=480.0,
        }
    }

    /// What the dock holds on to when the window is too small for everything: below this the
    /// drawing area has already given way and the dock takes what is left.
    fn keep(self) -> f32 {
        match self {
            Dock::Left | Dock::Right => 200.0,
            Dock::Bottom => 64.0,
        }
    }

    /// The room that must stay free beside this dock. `other_side` is another side dock that is
    /// still to be placed in the same room (the right dock, when the left one is placed first).
    pub fn reserve(self, other_side: bool) -> f32 {
        match self {
            Dock::Bottom => SIDES_MIN_HEIGHT,
            Dock::Left | Dock::Right => CANVAS_MIN_WIDTH + if other_side { *Dock::Right.range().start() } else { 0.0 },
        }
    }
}

/// The smallest and largest size the dock may have with `available` points of room along its
/// axis, `reserve` of which belong to what sits beside it.
pub fn limits(dock: Dock, available: f32, reserve: f32) -> (f32, f32) {
    let available = if available.is_finite() { available.max(0.0) } else { 0.0 };
    let range = dock.range();
    let hi = (available - reserve).max(dock.keep()).min(*range.end()).min(available);
    let lo = range.start().min(hi);
    (lo, hi)
}

/// The size to show: the wanted size inside the limits.
pub fn fit(wanted: f32, (lo, hi): (f32, f32)) -> f32 {
    if wanted.is_finite() { wanted.clamp(lo, hi) } else { lo }
}

/// The size the user chose for the dock, or its default.
pub fn wanted(sizes: &BTreeMap<String, f32>, dock: Dock) -> f32 {
    let range = dock.range();
    sizes.get(dock.key()).copied().filter(|s| s.is_finite()).map_or(dock.default_size(), |s| s.clamp(*range.start(), *range.end()))
}

/// Height of the upper tab group of the right dock, `height` points tall: 58 %, but neither
/// group is squeezed below [`GROUP_MIN_HEIGHT`] while there is room for both.
pub fn split(height: f32) -> f32 {
    let height = height.max(0.0);
    let least = GROUP_MIN_HEIGHT.min(height / 2.0);
    (height * 0.58).round().clamp(least, height - least)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dock_has_its_wanted_size_when_the_window_has_room() {
        for dock in [Dock::Left, Dock::Right, Dock::Bottom] {
            let lim = limits(dock, 2000.0, dock.reserve(true));
            assert_eq!(lim, (*dock.range().start(), *dock.range().end()));
            assert_eq!(fit(dock.default_size(), lim), dock.default_size());
            assert_eq!(fit(10_000.0, lim), *dock.range().end());
            assert_eq!(fit(1.0, lim), *dock.range().start());
            assert_eq!(fit(f32::NAN, lim), *dock.range().start());
        }
    }

    #[test]
    fn a_dock_gives_way_in_a_small_window_and_the_rest_keeps_its_room() {
        // 395 points between the ribbon and the status bar (a 600 point window): the bottom dock
        // leaves 260 to the side docks.
        let lim = limits(Dock::Bottom, 395.0, Dock::Bottom.reserve(false));
        assert_eq!(fit(180.0, lim), 135.0);
        assert_eq!(fit(480.0, lim), 135.0);
        // 920 points beside the tool column: a left dock dragged to its widest still leaves the
        // right dock its narrowest and the drawing area its minimum.
        let left = fit(520.0, limits(Dock::Left, 920.0, Dock::Left.reserve(true)));
        assert_eq!(left, 380.0);
        let right = fit(640.0, limits(Dock::Right, 920.0 - left, Dock::Right.reserve(false)));
        assert_eq!(right, 300.0);
        assert_eq!(920.0 - left - right, CANVAS_MIN_WIDTH);
    }

    #[test]
    fn a_dock_never_gets_more_than_there_is() {
        for dock in [Dock::Left, Dock::Right, Dock::Bottom] {
            for available in [-5.0, 0.0, 1.0, 30.0, 150.0, 250.0, 500.0, f32::NAN, f32::INFINITY] {
                let (lo, hi) = limits(dock, available, dock.reserve(true));
                assert!(lo <= hi && lo >= 0.0, "{dock:?} {available}: {lo}..{hi}");
                assert!(!available.is_finite() || hi <= available.max(0.0), "{dock:?} {available}: {hi}");
                let size = fit(dock.default_size(), (lo, hi));
                assert!((lo..=hi).contains(&size));
            }
        }
        // Too narrow for the drawing area's minimum: the dock still keeps 200 points.
        assert_eq!(limits(Dock::Right, 300.0, Dock::Right.reserve(false)), (200.0, 200.0));
    }

    #[test]
    fn the_wanted_size_is_the_saved_one_inside_the_docks_range() {
        let mut sizes = BTreeMap::new();
        assert_eq!(wanted(&sizes, Dock::Right), 380.0);
        sizes.insert("right".to_string(), 450.0);
        assert_eq!(wanted(&sizes, Dock::Right), 450.0);
        sizes.insert("right".to_string(), 5.0);
        assert_eq!(wanted(&sizes, Dock::Right), 300.0);
        sizes.insert("right".to_string(), f32::NAN);
        assert_eq!(wanted(&sizes, Dock::Right), 380.0);
    }

    #[test]
    fn the_right_dock_split_keeps_both_groups_usable() {
        assert_eq!(split(800.0), 464.0);
        assert_eq!(split(250.0), 140.0, "the lower group keeps 110 points");
        assert_eq!(split(100.0), 50.0, "too short for two minimums: half each");
        assert_eq!(split(0.0), 0.0);
        for h in [0.0, 10.0, 119.0, 220.0, 221.0, 400.0, 2000.0] {
            let s = split(h);
            assert!(s >= 0.0 && s <= h, "{h}: {s}");
        }
    }
}
