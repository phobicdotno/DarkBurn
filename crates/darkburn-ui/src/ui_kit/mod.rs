//! Tokenised dense card library, copied from SolidCraft's `crates/ui-egui/src/ui_kit`
//! (MIT OR Apache-2.0, see ATTRIBUTION.md). CAD-only modules were left out.
//! Geometry, typography and colours come from `solidcraft_theme::Tokens`.
mod actions;
mod attention;
mod checks;
mod choice;
mod color;
mod compat;
mod confirm;
pub mod container;
mod dash;
mod details;
mod disclosure;
mod drag;
mod fields;
mod layout;
mod menu;
mod metrics;
pub mod panel;
mod preferences_layout;
mod preferences_small;
pub mod recent_colors;
mod search;
pub mod settings_search;
mod shortcuts;
mod stages;
mod status;
mod table;
mod text;
mod text_menu;
mod tree;
mod value_focus;
mod wheel;

pub use actions::*;
pub use attention::*;
pub use checks::*;
pub use choice::*;
pub use color::*;
pub use compat::*;
pub use confirm::{Confirm, Tone};
pub use container::{BackingTooltipExt, Container, DockEdges, Edge, LayoutMode, SHELL_GAP, Surface, input_blocked, set_input_blocked};
pub use dash::dashed_line;
pub use details::*;
pub use disclosure::*;
pub use drag::*;
pub use fields::*;
pub use layout::*;
pub use menu::*;
pub use metrics::{ContentWidth, header_height, row_height, table_row_height};
pub use panel::*;
pub use preferences_small::*;
pub use search::{CommandSearch, SearchBadge, SearchFieldGeometry};
pub use shortcuts::*;
pub use stages::{Cards, StageAction, StageBar};
pub use status::*;
pub use table::*;
pub use text_menu::menu_open as text_menu_open;
pub use tree::*;
pub use value_focus::*;

pub use layout::label as row_label;

/// Whether `need` points fit in `have`, answered so that a scrollbar coming and going does not
/// flip it. Keyed by `key` within `ui`.
pub fn fits_steadily(ui: &egui::Ui, key: impl std::hash::Hash + std::fmt::Debug, need: f32, have: f32) -> bool {
    let slack = ui.spacing().scroll.allocated_width();
    let id = ui.make_persistent_id(("fits_steadily", key));
    let was = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(true);
    let fits = need <= have + if was { slack } else { 0.0 };
    ui.data_mut(|data| data.insert_temp(id, fits));
    fits
}
