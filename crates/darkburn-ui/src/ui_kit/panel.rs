//! Panel elements for dense cards; see `docs/ui-kit.md` for tokens and usage.
use super::{actions::icon_button_at, metrics::*, text::paint_text};
use crate::ui_kit::BackingTooltipExt as _;
use egui::{Ui, vec2};
use solidcraft_theme::Tokens;

const WINDOW_PAGES_NARROW_WIDTH: f32 = 150.0;
const WINDOW_PAGES_NARROW_AT: f32 = 700.0;

/// `v` kept within `lo..=hi`, or `lo` when that range is empty: `f32::clamp` panics on
/// `lo > hi`, which a rect squeezed to nothing between cards gives.
pub fn within(v: f32, lo: f32, hi: f32) -> f32 {
    if lo <= hi { v.clamp(lo, hi) } else { lo }
}

/// Shared active-tab hover tint for the browser and collapsible panel chrome.
pub fn panel_header_hover(painter: &egui::Painter, rect: egui::Rect) {
    let t = Tokens::get();
    painter.rect_filled(rect, t.radius_lg.saturating_sub(t.space_4 as u8), solidcraft_theme::TabAccent::current(painter.ctx()).soft);
}

/// Feature title/icon/close band. Tokens: `form_header_height, form_icon_size, form_close_size, form_inset, font_label, text, dialog_rule, space_8/10; icon_button_at tokens`.
pub fn header(ui: &mut Ui, icon: &str, title: &str, on_close: impl FnOnce()) {
    header_impl(ui, (icon, None), title, None, true, on_close);
}

/// Optional fold action for managers which historically hide their inputs without losing edits.
/// Tokens: header tokens and space_4; retains the dialog:collapse scenario handle.
pub fn collapsible_header(ui: &mut Ui, icon: &str, title: &str, collapsed: &mut bool, on_close: impl FnOnce()) {
    header_impl(ui, (icon, None), title, Some(collapsed), true, on_close);
}

fn header_impl(
    ui: &mut Ui,
    icon: (&str, Option<egui::Color32>),
    title: &str,
    collapsed: Option<&mut bool>,
    closeable: bool,
    on_close: impl FnOnce(),
) {
    header_actions(ui, icon, title, collapsed, closeable, on_close, None, None);
}

/// The title band with the place button (float, own window, dock as tab) left of the close.
fn header_with_place(
    ui: &mut Ui,
    icon: (&str, Option<egui::Color32>),
    title: &str,
    closeable: bool,
    on_close: impl FnOnce(),
    place: &mut dyn FnMut(&mut Ui, egui::Rect),
) {
    header_actions(ui, icon, title, None, closeable, on_close, None, Some(place));
}

fn header_actions(
    ui: &mut Ui,
    icon: (&str, Option<egui::Color32>),
    title: &str,
    collapsed: Option<&mut bool>,
    closeable: bool,
    on_close: impl FnOnce(),
    actions: Option<(f32, &mut dyn FnMut(&mut Ui))>,
    place: Option<&mut dyn FnMut(&mut Ui, egui::Rect)>,
) {
    let t = Tokens::get();
    let (rect, _) = ui.allocate_exact_size(vec2(dimension(ui.available_width()), header_height()), egui::Sense::hover());
    // Shared card chrome: 12 pt left inset, 19 pt icon, 8 pt gap and 26 pt close target.
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + t.space_12 + t.form_icon_size / 2.0, rect.center().y),
        vec2(t.form_icon_size, t.form_icon_size),
    );
    if icon.0.is_empty() {
        ui.painter().circle_filled(icon_rect.center(), t.space_3, solidcraft_theme::TabAccent::current(ui.ctx()).ink);
    } else if let Some(ink) = icon.1 {
        crate::icons::paint_ink(ui.painter(), icon_rect, icon.0, ink);
    } else {
        crate::icons::paint_icon(ui.painter(), icon_rect, icon.0, 1.0);
    }
    let close_rect = egui::Rect::from_center_size(
        egui::pos2((rect.right() - t.space_8 - t.form_close_size / 2.0).max(rect.left()), rect.center().y),
        vec2(t.form_close_size, t.form_close_size),
    );
    if closeable {
        let close = icon_button_at(ui, close_rect, "close", "Close");
        solidcraft_ui_registry::publish_handle("dialog:close", close.rect.center());
        if close.clicked() {
            on_close();
        }
    }
    let right = if let Some((width, actions)) = actions {
        let end = if closeable { close_rect.left() - t.space_8 } else { rect.right() - t.space_8 };
        let width = if closeable { width.min((end - icon_rect.right() - t.space_24 * 4.0).max(0.0)) } else { width };
        let action_rect = egui::Rect::from_center_size(
            egui::pos2(end - width / 2.0, rect.center().y),
            vec2(width, if closeable { t.form_close_size } else { t.control_height }),
        );
        let builder = egui::UiBuilder::new().max_rect(action_rect);
        let builder = if closeable { builder.layout(egui::Layout::left_to_right(egui::Align::Center)) } else { builder };
        let mut action_ui = ui.new_child(builder);
        if closeable {
            action_ui.set_clip_rect(ui.clip_rect().intersect(action_rect));
        }
        actions(&mut action_ui);
        solidcraft_ui_registry::publish_rect("dialog:header:accessory", action_rect);
        action_rect.left() - t.space_8
    } else if let Some(place) = place {
        let end = if closeable { close_rect.left() - t.space_4 } else { rect.right() - t.space_8 };
        let button = egui::Rect::from_min_size(egui::pos2(end - t.form_close_size, close_rect.top()), close_rect.size());
        place(ui, button);
        button.left()
    } else if let Some(collapsed) = collapsed {
        let fold = egui::Rect::from_min_size(egui::pos2(close_rect.left() - t.form_close_size - t.space_4, close_rect.top()), close_rect.size());
        let response =
            super::disclosure_button_at(ui, fold, !*collapsed, solidcraft_ui_text::tr_label(if *collapsed { "Expand" } else { "Collapse" }));
        solidcraft_ui_registry::publish_handle("dialog:collapse", response.rect.center());
        if response.clicked() {
            *collapsed = !*collapsed;
        }
        fold.left()
    } else if closeable {
        close_rect.left()
    } else {
        rect.right() - t.space_8
    };
    let left = (icon_rect.right() + t.space_8).min(right);
    let title_rect = egui::Rect::from_min_max(egui::pos2(left, rect.top()), egui::pos2(right.max(left), rect.bottom()));
    paint_text(ui, title_rect, title, crate::fonts::semibold(if icon.0.is_empty() { t.font_label } else { t.font_heading }), t.form_ink, false);
    let title_response = ui.interact(title_rect, ui.make_persistent_id("title_text"), egui::Sense::hover());
    title_response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), title));
    solidcraft_ui_registry::publish_rect("dialog:header", rect);
    t.secondary_hline(ui.painter(), rect.x_range(), rect.bottom(), t.dialog_rule);
}

/// Where a feature panel's OK, Cancel and Apply sit (Preferences › Interface › Dialog buttons,
/// #6708). The app publishes the preference each frame, so every `Panel::feature` follows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogButtons {
    /// In a footer row under the inputs (how it always was). Also what a value this version
    /// doesn't know reads as (`prefs::lenient`).
    #[default]
    Bottom,
    /// In the header row, right of the title, where they stay while sections open and close.
    Top,
}

impl DialogButtons {
    pub fn publish(self, ctx: &egui::Context) {
        ctx.data_mut(|data| data.insert_temp(egui::Id::unique("sc_dialog_buttons"), self));
    }

    pub fn current(ctx: &egui::Context) -> Self {
        ctx.data(|data| data.get_temp(egui::Id::unique("sc_dialog_buttons"))).unwrap_or_default()
    }
}

/// The header row of a feature panel whose actions sit at the top. It is laid out before the
/// body, and its icon, title and buttons are drawn after it, once the compute status and whether
/// OK is enabled are known. Same tokens as `header`, plus `HeaderActions`'.
struct TopHeader<'a> {
    icon: egui::Rect,
    title: egui::Rect,
    actions: super::HeaderActions<'a>,
}

impl<'a> TopHeader<'a> {
    fn new(ui: &mut Ui, ok: &'a str, apply: Option<&'a str>, cancel: &'a str, collapsed: Option<&mut bool>, close: &mut bool) -> Self {
        let t = Tokens::get();
        let (band, _) = ui.allocate_exact_size(vec2(dimension(ui.available_width()), header_height()), egui::Sense::hover());
        let icon = egui::Rect::from_center_size(
            egui::pos2(band.left() + t.space_12 + t.form_icon_size / 2.0, band.center().y),
            vec2(t.form_icon_size, t.form_icon_size),
        );
        // The buttons end where the fields below do.
        let end = (band.right() - t.space_12).max(band.left());
        let actions = super::HeaderActions::new(ui, band, end, ok, apply, cancel);
        let buttons = actions.left(end);
        // Nothing placed yet beside the buttons: the next control keeps clear of them.
        let beside_buttons = |right: f32| buttons < end && right >= buttons;
        // An icon target to the left of what is already placed, `space_4` clear of the buttons.
        let target = |right: f32| {
            let right = if beside_buttons(right) { right - t.space_4 } else { right };
            egui::Rect::from_center_size(
                egui::pos2((right - t.form_close_size / 2.0).max(band.left()), band.center().y),
                vec2(t.form_close_size, t.form_close_size),
            )
        };
        let mut right = buttons;
        // The header's close does what Cancel does in every panel, so with Cancel up here it is
        // left out: one control for one action, and its width goes to the title. Its handle
        // moves to Cancel, so automation that closes a panel keeps working.
        if let Some(cancel_rect) = actions.rect(super::Action::Cancel) {
            solidcraft_ui_registry::publish_handle("dialog:close", cancel_rect.center());
        } else {
            let close_rect = target(right);
            let response = icon_button_at(ui, close_rect, "close", "Close");
            solidcraft_ui_registry::publish_handle("dialog:close", response.rect.center());
            if response.clicked() {
                *close = true;
            }
            right = close_rect.left();
        }
        if let Some(collapsed) = collapsed {
            let fold = target(right);
            let response =
                super::disclosure_button_at(ui, fold, !*collapsed, solidcraft_ui_text::tr_label(if *collapsed { "Expand" } else { "Collapse" }));
            solidcraft_ui_registry::publish_handle("dialog:collapse", response.rect.center());
            if response.clicked() {
                *collapsed = !*collapsed;
            }
            right = fold.left();
        }
        // Text next to a text button keeps a gap; an icon target carries its own padding.
        if beside_buttons(right) {
            right = (right - t.space_6).max(band.left());
        }
        let left = (icon.right() + t.space_8).min(right);
        let title = egui::Rect::from_min_max(egui::pos2(left, band.top()), egui::pos2(right.max(left), band.bottom()));
        // Dragging the panel by its title must not start on a button.
        solidcraft_ui_registry::publish_handle("dialog:title", title.center());
        solidcraft_ui_registry::publish_rect("dialog:header", band);
        t.secondary_hline(ui.painter(), band.x_range(), band.bottom(), t.dialog_rule);
        Self { icon, title, actions }
    }

    /// The icon (the compute spinner while busy), the title (the compute status while there is
    /// one: the footer that showed it is gone) and the buttons. Returns the action asked for.
    fn finish(&self, ui: &mut Ui, icon: &str, title: &str, state: &ComputeStatus, ok_enabled: bool) -> super::Action {
        let t = Tokens::get();
        if state.busy {
            egui::Spinner::new().size(t.space_14).paint_at(ui, self.icon);
            let status = state.text.as_deref().unwrap_or_else(|| solidcraft_ui_text::tr_label("Updating the preview"));
            ui.interact(self.icon, ui.scope_id().with("compute_status"), egui::Sense::hover()).backing_on_hover_text(status);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            crate::icons::paint_icon(ui.painter(), self.icon, icon, 1.0);
        }
        let heading = crate::fonts::semibold(t.font_heading);
        match state.text.as_deref() {
            Some(status) => paint_text(ui, self.title, status, crate::fonts::medium(t.font_label), t.form_dim, false),
            None => paint_text(ui, self.title, title, heading.clone(), t.form_ink, false),
        }
        let title_response = ui.interact(self.title, ui.make_persistent_id("title_text"), egui::Sense::hover());
        title_response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), title));
        // The buttons take the row's width first; a title that no longer fits says itself on hover.
        if ui.painter().layout_no_wrap(title.to_owned(), heading, t.form_ink).size().x > self.title.width() {
            title_response.backing_on_hover_text(title);
        }
        self.actions.show(ui, ok_enabled)
    }
}

/// Selection editor chrome, shared by floating Inspectors. Returns an action for registry dispatch.
#[derive(Clone, Copy)]
pub enum InspectorAction {
    Close,
    Remove,
}

pub fn inspector_header(ui: &mut Ui, icon: &str, title: &str, kind: &str) -> Option<InspectorAction> {
    let t = Tokens::get();
    let mut action = None;
    let (rect, _) = ui.allocate_exact_size(vec2(dimension(ui.available_width()), header_height()), egui::Sense::hover());
    let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + (t.space_16 + t.space_4), rect.center().y), vec2(t.space_14, t.space_14));
    crate::icons::paint_ink(ui.painter(), icon_rect, icon, solidcraft_theme::TabAccent::current(ui.ctx()).ink);
    let close_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - (t.space_16 + t.space_4), rect.center().y),
        vec2(t.form_close_size, t.form_close_size),
    );
    let close = super::icon_button_at(ui, close_rect, "close", solidcraft_ui_text::tr_label("Close"));
    solidcraft_ui_registry::publish_target("render:appearance:close", close.rect);
    if close.clicked() {
        action = Some(InspectorAction::Close);
    }
    let more_rect = close_rect.translate(vec2(-t.control_height - t.space_4, 0.0));
    {
        let mut more_ui = ui.new_child(egui::UiBuilder::new().id_salt("inspector_more").max_rect(more_rect));
        let ui = &mut more_ui;
        super::Menu::text(ui, "…", |ui| {
            if super::MenuItem::new("render:appearance:remove", solidcraft_ui_text::tr("render.inspector_remove"), "").show(ui).clicked() {
                action = Some(InspectorAction::Remove);
                ui.close();
            }
        });
    }
    let badge_width = ui.painter().layout_no_wrap(kind.to_owned(), crate::fonts::semibold(t.font_small), t.form_ink).size().x + t.space_16;
    let badge = egui::Rect::from_center_size(
        egui::pos2(more_rect.left() - t.space_8 - badge_width / 2.0, rect.center().y),
        vec2(badge_width, t.space_16 + t.space_4),
    );
    {
        let mut badge_ui = ui.new_child(egui::UiBuilder::new().id_salt("inspector_kind").max_rect(badge));
        super::workspace_chip(&mut badge_ui, kind);
    }
    let name = egui::Rect::from_min_max(egui::pos2(icon_rect.right() + t.space_8, rect.top()), egui::pos2(badge.left() - t.space_8, rect.bottom()));
    paint_text(ui, name, title, crate::fonts::semibold(t.font_label), t.form_ink, false);
    ui.interact(name, ui.make_persistent_id("inspector_title"), egui::Sense::hover())
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), title));
    t.secondary_hline(ui.painter(), rect.x_range(), rect.bottom(), t.dialog_rule);
    solidcraft_ui_registry::publish_rect("dialog:header", rect);
    action
}

/// A bounded docked or floating card. Uses form_card_width and the caller's tokenised frame.
/// Window IDs and content scopes are retained so moving a card does not reset field focus.
pub struct Panel<'a> {
    title: &'a str,
    id: egui::Id,
    anchor: egui::Pos2,
    viewport: egui::Rect,
    side: crate::frame::Side,
    frame: egui::Frame,
    width: f32,
    body_rows: Option<f32>,
    body_height: Option<f32>,
    ok: &'a str,
    cancel: &'a str,
    reset_scroll: bool,
    joined: Option<super::DockEdges>,
    grow_key: Option<egui::Id>,
}

pub struct WorkspaceOptions<'a> {
    pub id: &'static str,
    pub title: &'a str,
    pub icon: &'a str,
    pub bounds: egui::Rect,
    pub fill_height: bool,
    /// Use the minimum dock width without overwriting its remembered width.
    pub compact: bool,
    pub editor: bool,
    pub enabled: bool,
    pub header_actions: Option<&'a mut dyn FnMut(&mut Ui)>,
}

pub struct FeatureOptions<'a> {
    pub icon: &'a str,
    pub title: &'a str,
    /// The dock offset the title band drags; empty for a card that stays in its place.
    pub drag_key: &'a str,
    pub moved: egui::Vec2,
    pub docked: egui::Pos2,
    pub apply_label: Option<&'a str>,
}

/// State read after rendering the fields, just as in the original feature card.
pub struct ComputeStatus {
    pub text: Option<String>,
    pub busy: bool,
    pub applying: bool,
}

pub enum Part {
    Header,
    Body,
    Footer,
}

impl<'a> Panel<'a> {
    /// Shared vertical dock policy: reserve room for a second card, then place it
    /// below the measured first card with the standard container gap.
    pub fn stack_first_bounds(mut bounds: egui::Rect) -> egui::Rect {
        bounds.max.y = bounds.top()
            + bounds
                .height()
                .min(Tokens::get().form_header_height + Tokens::get().control_height * 10.0 + Tokens::get().space_8 * 2.0)
                .min((bounds.height() * 0.5).floor());
        bounds
    }

    pub fn stack_next_bounds(mut bounds: egui::Rect, previous: egui::Rect) -> egui::Rect {
        bounds.min.y = (previous.bottom() + Tokens::get().space_8).min(bounds.bottom());
        bounds
    }
    /// Pinned inline actions inside an existing feature/card surface. This is not
    /// a second window or painted card: the parent owns the chrome. Tokens:
    /// control_height, space_24/8; bounded row count preserves finite geometry.
    pub fn pinned_footer<R>(ui: &mut Ui, key: &'static str, rows: usize, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
        let t = Tokens::get();
        // This band is content inside the enclosing card, not a second adaptive card.
        let host = super::Container::new(ui.ctx(), super::Surface::Viewport);
        host.edge(key, super::Edge::Bottom)
            .exact_size(t.control_height * rows.clamp(1, 8) as f32 + t.space_24 + t.space_8)
            .resizable(false)
            .frame(host.edge_frame(egui::Color32::TRANSPARENT))
            .show(ui, add)
    }

    /// One-row editor footer, pinned inside the shared workspace card.
    pub fn editor_footer<R>(ui: &mut Ui, key: &'static str, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
        let t = Tokens::get();
        let host = super::Container::new(ui.ctx(), super::Surface::Viewport);
        host.edge(key, super::Edge::Bottom)
            .exact_size(t.form_footer_height)
            .resizable(false)
            .frame(
                host.edge_frame(t.dialog_footer)
                    .outer_margin(egui::Margin::symmetric(-(t.space_12 as i8), 0))
                    .inner_margin(egui::Margin::symmetric(t.space_12 as i8, t.space_8 as i8)),
            )
            .show(ui, |ui| {
                let band = ui.max_rect().expand2(vec2(t.space_12, t.space_8));
                ui.painter().rect_filled(band, egui::CornerRadius { nw: 0, ne: 0, sw: t.radius_xl, se: t.radius_xl }, t.dialog_footer);
                t.secondary_hline(ui.painter(), band.x_range(), band.top(), t.dialog_rule);
                solidcraft_ui_registry::publish_rect("dialog:editor_footer", band);
                add(ui)
            })
    }

    /// Shared strip composition reserves actions at the right and scrolls choices at the left.
    pub fn strip_contents(ui: &mut egui::Ui, right: impl FnOnce(&mut egui::Ui), choices: impl FnOnce(&mut egui::Ui)) {
        ui.spacing_mut().item_spacing.x = Tokens::get().space_8;
        ui.horizontal_centered(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                right(ui);
                egui::ScrollArea::horizontal()
                    .id_salt("strip_choices")
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), choices);
                    });
            });
        });
    }

    /// Floating workspace strip using the same bottom rectangle and chrome as History.
    pub fn bottom_strip<R>(
        ui: &mut Ui,
        id: &'static str,
        available: egui::Rect,
        height: f32,
        add: impl FnOnce(&mut Ui) -> R,
    ) -> Option<egui::InnerResponse<R>> {
        let container = super::Container::floating_card(super::Surface::Panel);
        let rect = container.bottom_strip_rect(available, height);
        if !rect.is_finite() || rect.width() < Tokens::get().control_height || rect.height() < Tokens::get().control_height {
            return None;
        }
        let ctx = ui.ctx().clone();
        Some(container.area(egui::Id::unique(id)).fixed_pos(rect.min).movable(false).show(&ctx, |ui| {
            let frame = container.frame(&ctx, Tokens::get().panel, crate::frame::Side::Left).inner_margin(Tokens::get().space_4);
            container
                .show_frame(ui, frame, rect, |ui| {
                    ui.set_min_size((rect.size() - frame.total_margin().sum()).max(egui::Vec2::ZERO));
                    ui.set_max_size((rect.size() - frame.total_margin().sum()).max(egui::Vec2::ZERO));
                    super::scope(ui, add)
                })
                .inner
        }))
    }

    /// Bounded floating workspace card. The shared grip chooses a side; the inner resize
    /// rail changes its remembered width. The inner result pairs the body with a context-menu
    /// side request, which the caller dispatches through its command. Neither gesture edits the document.
    pub fn workspace<R>(
        ui: &mut Ui,
        mut options: WorkspaceOptions<'_>,
        side: &mut String,
        width: &mut f32,
        add: impl FnOnce(&mut Ui) -> R,
    ) -> Option<egui::InnerResponse<(R, Option<&'static str>)>> {
        let actions = options.header_actions.take();
        Self::workspace_with_header(
            ui,
            options,
            side,
            width,
            false,
            |ui, options| {
                header_actions(
                    ui,
                    (options.icon, None),
                    options.title,
                    None,
                    false,
                    || {},
                    actions.map(|actions| (Tokens::get().control_height * 2.0, actions)),
                    None,
                );
            },
            add,
        )
    }

    /// Shared tree cards can place navigation in the title band and let rows own their insets.
    pub fn workspace_with_header<R>(
        ui: &mut Ui,
        options: WorkspaceOptions<'_>,
        side: &mut String,
        width: &mut f32,
        tree: bool,
        header: impl FnOnce(&mut Ui, &WorkspaceOptions<'_>),
        add: impl FnOnce(&mut Ui) -> R,
    ) -> Option<egui::InnerResponse<(R, Option<&'static str>)>> {
        let container = super::Container::floating_card(super::Surface::Panel);
        // The ribbon has already reserved the top gap, as for the Design Browser.
        let gap = Tokens::get().space_8;
        let bounds = egui::Rect::from_min_max(options.bounds.min + vec2(gap, 0.0), options.bounds.max - vec2(gap, gap));
        let t = Tokens::get();
        if !bounds.is_finite() || bounds.width() < t.control_height || bounds.height() < t.form_header_height {
            return None;
        }
        let maximum = (t.form_card_width * 1.8).min((bounds.width() - gap).max(0.0) / 2.0);
        if maximum < t.control_height * 4.0 {
            return None;
        }
        let minimum = crate::frame::DIALOG_MIN_W.min(maximum);
        let display_width = if options.compact {
            minimum
        } else {
            *width = if width.is_finite() { width.clamp(minimum, maximum) } else { t.form_card_width.min(maximum) };
            *width
        };
        let left = side == "left";
        let rect = egui::Rect::from_min_size(
            egui::pos2(if left { bounds.left() } else { bounds.right() - display_width }, bounds.top()),
            vec2(display_width, bounds.height()),
        );
        let ctx = ui.ctx().clone();
        let mut move_side = None;
        let response = container.area(egui::Id::unique(options.id)).fixed_pos(rect.min).movable(false).show(&ctx, |ui| {
            let frame = container
                .frame(
                    &ctx,
                    if options.editor { t.dialog_body } else { t.panel },
                    if left { crate::frame::Side::Left } else { crate::frame::Side::Right },
                )
                .inner_margin(0.0);
            let content = container.show_frame(ui, frame, bounds, |ui| {
                let size = (rect.size() - frame.total_margin().sum()).max(egui::Vec2::ZERO);
                // Allocate the bounded host once; overflowing controls scroll/clip inside a child,
                // so a short window cannot grow a floating dock over the top bar or bottom strip.
                let (body, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt("workspace_body"));
                let ui = &mut child;
                if !options.enabled {
                    ui.disable();
                }
                ui.set_clip_rect(ui.clip_rect().intersect(body));
                ui.set_width(size.x);
                ui.set_max_height(size.y);
                if options.fill_height {
                    ui.set_min_height(size.y);
                }
                // Use the same fixed-height header as Panel/Window. Workspace cards are
                // persistent; their navigation lives in the separately padded body.
                ui.spacing_mut().item_spacing.y = 0.0;
                let band = egui::Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), header_height()));
                let grip_rect = if tree { egui::Rect::from_min_size(band.min, vec2(t.space_24 * 2.0 + t.space_8, band.height())) } else { band };
                let grip = ui
                    .interact(grip_rect, ui.scope_id().with("workspace_grip"), egui::Sense::click_and_drag())
                    .on_hover_cursor(egui::CursorIcon::Grab);
                solidcraft_ui_registry::publish_target(&format!("{}:grip", options.id), grip.rect);
                super::Menu::context(&grip, |ui| {
                    let (target, label) = if left { ("right", "Move to right side") } else { ("left", "Move to left side") };
                    let item = super::MenuItem::new("view.render_dock", label, "").show(ui);
                    solidcraft_ui_registry::publish_target(&format!("{}:move_side", options.id), item.rect);
                    if item.clicked() {
                        move_side = Some(target);
                        ui.close();
                    }
                });
                header(ui, &options);
                solidcraft_ui_registry::publish_rect(&format!("{}:header", options.id), band);
                if grip.dragged() {
                    let frame_nr = ui.ctx().cumulative_frame_nr();
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("sc_dock_drag_frame"), frame_nr));
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
                if grip.drag_stopped()
                    && let Some(at) = ui.ctx().pointer_latest_pos()
                {
                    *side = if at.x < options.bounds.center().x { "left" } else { "right" }.into();
                    ui.ctx().request_repaint();
                }
                egui::Frame::NONE
                    .inner_margin(if tree {
                        egui::Margin::symmetric(0, t.space_4 as i8)
                    } else if options.editor {
                        egui::Margin { left: t.space_12 as i8, right: t.space_12 as i8, top: t.space_10 as i8, bottom: 0 }
                    } else {
                        egui::Margin::same(if options.icon.is_empty() { t.space_10 } else { t.space_12 } as i8)
                    })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = t.form_gap_y;
                        if tree {
                            ui.spacing_mut().item_spacing.y = 0.0;
                        }
                        super::scope(ui, |ui| {
                            if options.editor {
                                ui.spacing_mut().item_spacing.y = t.space_8;
                            }
                            add(ui)
                        })
                    })
                    .inner
            });
            let card = content.response.rect;
            let rail = egui::Rect::from_x_y_ranges(
                if left { card.right() - t.space_6..=card.right() } else { card.left()..=card.left() + t.space_6 },
                card.y_range(),
            );
            let resize =
                ui.interact(rail, ui.scope_id().with("workspace_resize"), egui::Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            solidcraft_ui_registry::publish_target(&format!("{}:resize", options.id), rail);
            if resize.dragged() && !options.compact {
                let delta = ui.input(|i| i.pointer.delta().x) * if left { 1.0 } else { -1.0 };
                if delta.is_finite() {
                    *width = (*width + delta).clamp(minimum, maximum);
                }
            }
            (content.inner, move_side)
        });
        Some(response)
    }

    /// Shared floating card surface for panels, windows and modal confirmations.
    pub fn card_frame(ctx: &egui::Context) -> egui::Frame {
        let t = Tokens::get();
        super::Container::new(ctx, super::Surface::Window).frame(ctx, t.dialog_body, crate::frame::Side::Right).inner_margin(0.0)
    }

    /// Feature surface; shared container policy supplies the border and shadow.
    pub fn feature_frame(ctx: &egui::Context, layout: super::LayoutMode, side: crate::frame::Side) -> egui::Frame {
        super::Container::for_layout(layout, super::Surface::Feature).frame(ctx, Tokens::get().dialog_body, side).inner_margin(0.0)
    }

    /// Sketch palette uses the same layout policy as every other workspace panel.
    pub fn palette_frame(ctx: &egui::Context, layout: super::LayoutMode) -> egui::Frame {
        let t = Tokens::get();
        super::Container::for_layout(layout, super::Surface::Palette)
            .frame(ctx, t.panel.gamma_multiply(t.sketch_palette_alpha), crate::frame::Side::Right)
            .inner_margin(0.0)
    }

    pub fn new(title: &'a str, id: egui::Id, anchor: egui::Pos2, viewport: egui::Rect, side: crate::frame::Side, frame: egui::Frame) -> Self {
        Self {
            title,
            id,
            anchor,
            viewport,
            side,
            frame,
            width: Tokens::get().form_card_width,
            body_rows: None,
            body_height: None,
            ok: "OK",
            cancel: "Cancel",
            reset_scroll: false,
            joined: None,
            grow_key: None,
        }
    }

    /// The edges the card meets the window or a docked surface at (a dialog in the left column):
    /// in Compact, only its other corners round.
    pub fn joined(mut self, edges: super::DockEdges) -> Self {
        self.joined = Some(edges);
        self
    }

    /// A newly opened manager starts at its first section. Later frames retain user scrolling.
    pub fn reset_scroll(mut self, reset: bool) -> Self {
        self.reset_scroll = reset;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// The feature card's form measures its labels and publishes the width it needs under
    /// this key, so the card can widen for the current language (#10620).
    pub fn grow_label(mut self, key: egui::Id) -> Self {
        self.grow_key = Some(key);
        self
    }

    /// The footer's primary and closing actions when they are not OK and Cancel. An empty `ok`
    /// leaves the primary action out (a card that only closes, e.g. Measure).
    pub fn actions(mut self, ok: &'a str, cancel: &'a str) -> Self {
        (self.ok, self.cancel) = (ok, cancel);
        self
    }

    /// Table surface width derives from the same card-width token and remains viewport bounded.
    pub fn wide(self) -> Self {
        self.width(Tokens::get().form_card_width * 3.0)
    }

    /// Cap a manager's scrolling body independently of its position, in control rows.
    /// Short viewports still take precedence so the header and actions remain reachable.
    pub fn body_rows(mut self, rows: f32) -> Self {
        if rows.is_finite() {
            self.body_rows = Some(rows.clamp(1.0, 32.0));
        }
        self
    }

    /// Hold the scrolling body at a height the user chose by resizing the card, in points.
    /// It replaces `body_rows`; short viewports still take precedence.
    pub fn body_height(mut self, height: Option<f32>) -> Self {
        self.body_height = height.filter(|h| h.is_finite() && *h > 0.0);
        self
    }

    /// The scrolling body's height on the last frame, for a card the user resizes.
    pub fn last_body_height(&self, ctx: &egui::Context) -> Option<f32> {
        ctx.data(|d| d.get_temp(self.id.with("body_height")))
    }

    /// The painted anchor also drives dragging, including the shell's edge clearance.
    pub fn bounded_anchor(&self, ctx: &egui::Context) -> egui::Pos2 {
        let viewport = self.viewport;
        let gap = super::Container::new(ctx, super::Surface::Feature).gap();
        let width = self.width.min(viewport.width().max(0.0));
        let x = match self.side {
            crate::frame::Side::Right => self.anchor.x.clamp(
                viewport.left() + width,
                (viewport.right() - if gap > 0.0 { Tokens::get().space_8 } else { 0.0 }).max(viewport.left() + width),
            ),
            crate::frame::Side::Left => within(self.anchor.x, viewport.left(), viewport.right() - width),
        };
        let t = Tokens::get();
        let chrome = t.form_header_height + t.form_footer_height + t.form_gap_y * 2.0 + self.frame.total_margin().sum().y + gap;
        egui::pos2(x, self.anchor.y.clamp(viewport.top(), (viewport.bottom() - chrome).max(viewport.top())))
    }

    /// One positioned frame supplies both the outer surface and its footer corners.
    fn positioned_frame(&self, ctx: &egui::Context) -> egui::Frame {
        let anchor = self.bounded_anchor(ctx);
        let right = match self.side {
            crate::frame::Side::Right => anchor.x,
            crate::frame::Side::Left => anchor.x + self.width.min(self.viewport.width().max(0.0)),
        };
        let container = super::Container::new(ctx, super::Surface::Feature);
        self.joined.map_or(container, |edges| container.joined(edges)).tool_frame(self.frame, right >= self.viewport.right() - 0.5)
    }

    pub fn window(&self, ctx: &egui::Context) -> super::container::FeatureWindow<'a> {
        self.window_placed(ctx, None)
    }

    /// The same window for contents the caller holds to `size` (egui's `fixed_size`); the card
    /// is that plus its frame.
    pub fn fixed_window(&self, ctx: &egui::Context, size: egui::Vec2) -> super::container::FeatureWindow<'a> {
        let outer = size.x + self.positioned_frame(ctx).total_margin().sum().x;
        self.window_placed(ctx, Some(outer)).fixed_size(size)
    }

    fn window_placed(&self, ctx: &egui::Context, outer_width: Option<f32>) -> super::container::FeatureWindow<'a> {
        let gap = super::Container::new(ctx, super::Surface::Feature).gap();
        let viewport = self.viewport;
        let anchor = self.bounded_anchor(ctx);
        let side = self.side;
        let frame = self.positioned_frame(ctx);
        let margin = frame.total_margin();
        let width = self.width.min(viewport.width().max(0.0));
        let inner_width = (width - margin.sum().x).max(0.0);
        let height = (viewport.bottom() - anchor.y - gap).max(0.0);
        // Place the card by its left edge, worked out from the outer width the window is held to.
        // Pivoting on the right edge subtracts the size egui measured last frame, and at a
        // fractional display scale rounding the left edge to a physical pixel changes the next
        // measured width by one, so the card hopped a pixel back and forth on every repaint (#9457).
        let left_top = match side {
            crate::frame::Side::Right => egui::pos2(anchor.x - outer_width.unwrap_or(inner_width), anchor.y),
            crate::frame::Side::Left => anchor,
        };
        // Keep drawing safe when even the header/footer cannot fit. egui clips the contents to
        // the remaining viewport; the normal small-height case still keeps both actions visible.
        let window = super::Container::new(ctx, super::Surface::Feature)
            .window(self.title)
            .id(self.id)
            .title_bar(false)
            .pivot(egui::Align2::LEFT_TOP)
            .fixed_pos(left_top)
            .constrain(false)
            .collapsible(false)
            .resizable(false)
            .min_width(inner_width)
            .max_width(inner_width)
            .default_width(inner_width)
            .min_height(0.0)
            .max_height((height - margin.sum().y).max(0.0))
            .scroll(false)
            .frame(frame)
            .default_height(0.0)
            .fade_in(false)
            .fade_out(false)
            .order(egui::Order::Middle);
        super::container::FeatureWindow::new(window, self.id, frame)
    }

    /// Feature header, scrolling padded body, pinned actions, drag target and compute status.
    /// Tokens: form_header_height, form_footer_height, space_6/8/10/14/16, dialog_body.
    pub fn feature(
        &self,
        ui: &mut Ui,
        heading: FeatureOptions<'_>,
        disabled: bool,
        valid: bool,
        body: impl FnOnce(&mut Ui) -> ComputeStatus,
    ) -> (bool, super::Action) {
        self.feature_impl(ui, heading, disabled, valid, None, body)
    }

    /// Same card with a persistent fold state owned by its caller.
    pub fn feature_collapsible(
        &self,
        ui: &mut Ui,
        heading: FeatureOptions<'_>,
        disabled: bool,
        valid: bool,
        collapsed: &mut bool,
        body: impl FnOnce(&mut Ui) -> ComputeStatus,
    ) -> (bool, super::Action) {
        self.feature_impl(ui, heading, disabled, valid, Some(collapsed), body)
    }

    /// The feature card behind `feature` and `feature_collapsible`: title band (and drag target),
    /// a scrolling body held to the viewport, `body_rows` or `body_height`, then the footer or top
    /// actions; returns whether the card closed and the action taken.
    fn feature_impl(
        &self,
        ui: &mut Ui,
        heading: FeatureOptions<'_>,
        disabled: bool,
        valid: bool,
        mut collapsed: Option<&mut bool>,
        body: impl FnOnce(&mut Ui) -> ComputeStatus,
    ) -> (bool, super::Action) {
        let card = |ui: &mut Ui| {
            let t = Tokens::get();
            let min = ui.cursor().min;
            let band = egui::Rect::from_min_size(min, vec2(ui.available_width(), super::header_height()));
            if heading.drag_key.is_empty() {
                solidcraft_ui_registry::publish_handle("dialog:title", band.center());
            } else {
                crate::frame::drag_dock(ui, band, heading.drag_key, heading.moved, self.bounded_anchor(ui.ctx()) - heading.docked);
            }
            let mut close = false;
            let top = if DialogButtons::current(ui.ctx()) == DialogButtons::Top {
                Some(TopHeader::new(ui, self.ok, heading.apply_label, self.cancel, collapsed.as_deref_mut(), &mut close))
            } else {
                if let Some(collapsed) = collapsed.as_deref_mut() {
                    collapsible_header(ui, heading.icon, heading.title, collapsed, || close = true);
                } else {
                    header(ui, heading.icon, heading.title, || close = true);
                }
                None
            };
            let bottom = self.viewport.bottom()
                - super::Container::new(ui.ctx(), super::Surface::Feature).gap()
                - self.frame.inner_margin.bottomf()
                - self.frame.stroke.width;
            let footer_height = if top.is_some() { 0.0 } else { t.form_footer_height };
            let fields_height = (bottom - ui.cursor().top() - footer_height - ui.spacing().item_spacing.y).max(0.0);
            let fields_height = match self.body_height {
                Some(height) => fields_height.min(height),
                None => self.body_rows.map_or(fields_height, |rows| fields_height.min(rows * t.control_height + t.space_12 * 2.0)),
            };
            let fields_rect = egui::Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), fields_height));
            let state = if collapsed.as_deref().copied().unwrap_or(false) {
                solidcraft_ui_registry::publish_count("dialog:body:content_height", 0.0);
                ComputeStatus { text: None, busy: disabled, applying: disabled }
            } else {
                ui.scope_builder(egui::UiBuilder::new().max_rect(fields_rect), |ui| {
                    if self.reset_scroll {
                        // Clear persisted offset, animation and momentum once per opening.
                        egui::scroll_area::State::default().store(ui.ctx(), ui.make_persistent_id(egui::IdSalt::new("feature_fields")));
                    }
                    let scroll = egui::ScrollArea::vertical()
                        .id_salt("feature_fields")
                        .min_scrolled_height(0.0)
                        .max_height(fields_height)
                        .auto_shrink([true, self.body_height.is_none()])
                        .show(ui, |ui| {
                            if disabled {
                                ui.disable();
                            }
                            egui::Frame::NONE
                                .inner_margin(egui::Margin {
                                    left: t.space_12 as i8,
                                    right: t.space_12 as i8,
                                    top: t.space_12 as i8,
                                    bottom: t.space_12 as i8,
                                })
                                .show(ui, body)
                                .inner
                        });
                    ui.ctx().data_mut(|d| d.insert_temp(self.id.with("body_height"), scroll.inner_rect.height()));
                    solidcraft_ui_registry::publish_count("dialog:body:content_height", f64::from(scroll.content_size.y));
                    solidcraft_ui_registry::publish_count("dialog:body:scroll_y", f64::from(scroll.state.clamped_offset().y));
                    solidcraft_ui_registry::publish_rect("dialog:body:viewport", scroll.inner_rect);
                    scroll.inner
                })
                .inner
            };
            if let Some(top) = &top {
                let action = top.finish(ui, heading.icon, heading.title, &state, valid && !state.applying);
                super::Container::finish_content(ui, self.positioned_frame(ui.ctx()));
                return (close, action);
            }
            let corners = self.positioned_frame(ui.ctx()).corner_radius;
            // The spinner sits in the footer's own row, at its side inset and on the buttons' centre
            // line, in the slot the status text leaves for it.
            let action = super::actions::footer_with_left(
                ui,
                self.ok,
                self.cancel,
                valid && !state.applying,
                super::Footer { apply: heading.apply_label, status: state.text.as_deref(), corners: Some(corners), ..Default::default() },
                |ui| {
                    if state.busy {
                        let (rect, _) = ui.allocate_exact_size(vec2(t.space_16, ui.available_height()), egui::Sense::hover());
                        egui::Spinner::new().size(t.space_14).paint_at(ui, rect);
                        solidcraft_ui_registry::publish_rect("dialog:footer:spinner", rect);
                        let status = state.text.as_deref().unwrap_or_else(|| solidcraft_ui_text::tr_label("Updating the preview"));
                        ui.interact(rect, ui.scope_id().with("compute_status"), egui::Sense::hover()).backing_on_hover_text(status);
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                    }
                },
            );
            super::Container::finish_content(ui, self.positioned_frame(ui.ctx()));
            (close, action)
        };
        match self.grow_key {
            Some(key) => super::scope_grow(ui, key, card),
            None => super::scope(ui, card),
        }
    }

    /// Palette composition retains its compact padding and whole-card scrolling on short views.
    /// Tokens: space_24 and form_footer_height; header/body/footer own their historical padding.
    pub fn palette(&self, ui: &mut Ui, bottom: f32, top: f32, mut add: impl FnMut(&mut Ui, Part)) {
        let t = Tokens::get();
        // Dialog buttons at top (#9860): the footer row moves under the header, above the options.
        let at_top = DialogButtons::current(ui.ctx()) == DialogButtons::Top;
        ui.set_max_height((bottom - ui.cursor().top()).max(0.0));
        super::scope(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let height = (bottom - top - self.frame.total_margin().sum().y).max(0.0);
            let short = height < t.space_24 * 6.0;
            let mut contents = |ui: &mut Ui| {
                add(ui, Part::Header);
                if at_top {
                    add(ui, Part::Footer);
                }
                let footer_height = if at_top { 0.0 } else { t.form_footer_height };
                let body_height = (bottom - ui.cursor().top() - footer_height).max(0.0);
                let mut body = |ui: &mut Ui| {
                    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(t.space_10 as i8, t.space_4 as i8)).show(ui, |ui| add(ui, Part::Body));
                };
                if short {
                    body(ui);
                } else {
                    let body_rect = egui::Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), body_height));
                    let scroll = ui
                        .scope_builder(egui::UiBuilder::new().max_rect(body_rect), |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("sc_palette_options")
                                .max_height(body_height)
                                .min_scrolled_height(0.0)
                                .auto_shrink([false, true])
                                .show(ui, body)
                        })
                        .inner;
                    solidcraft_ui_registry::publish_rect("sketch_palette:options", scroll.inner_rect);
                }
                if !at_top {
                    add(ui, Part::Footer);
                }
            };
            if short {
                let scroll = egui::ScrollArea::vertical()
                    .id_salt("sc_palette_content")
                    .max_height(height)
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, true])
                    .show(ui, contents);
                solidcraft_ui_registry::publish_rect("sketch_palette:options", scroll.inner_rect);
            } else {
                contents(ui);
            }
            super::Container::finish_content(ui, self.positioned_frame(ui.ctx()));
        });
    }
}

/// System window built on Panel's shared floating card: 40 pt header, bounded content,
/// optional page list, and 48 pt actions with a left slot. Existing IDs and editor policies
/// survive the migration. All geometry and colours derive from Tokens.
pub struct Window<'a> {
    id: egui::Id,
    icon: &'a str,
    title: &'a str,
    anchor: Option<(egui::Align2, egui::Vec2)>,
    placed: Option<(egui::Align2, egui::Pos2)>,
    ok: &'a str,
    cancel: &'a str,
    apply: Option<&'a str>,
    nothing_to_apply: bool,
    enter_submits: bool,
    escape_closes: bool,
    closeable: bool,
    width: f32,
    body_padding: bool,
    page_rows: f32,
    page_padding: egui::Margin,
    page_chrome: bool,
    left_slot: bool,
    page_header_in_titlebar: bool,
    dock_key: Option<&'a str>,
    /// Esc may close a footerless window floating in the main window this frame.
    escape_here: bool,
    answer_keys: super::AnswerKeys,
    unsaved: bool,
}

/// How [`Window::content`] lays itself out for where the window is shown.
#[derive(Clone, Copy, Debug)]
struct Chrome<'k> {
    /// Draw the title band (a docked tab has its title on the tab).
    header: bool,
    /// Enter and Esc act on the window (not for a tab: the keys belong to the main window).
    keys: bool,
    /// The body's height when the window has a size of its own (resized, docked, native).
    body_height: Option<f32>,
    /// The window's key, for the header's place menu.
    place: Option<&'k str>,
    /// Esc closes a window without a footer (one that has a footer closes by its Cancel).
    escape: bool,
    /// The window was closed with edits not applied and waits for the answer to its question:
    /// nothing in it can be used, and the keys go to the question.
    waiting: bool,
}

impl Chrome<'_> {
    const CARD: Chrome<'static> = Chrome { header: true, keys: true, body_height: None, place: None, escape: false, waiting: false };
}

impl<'a> Window<'a> {
    /// A card with OK and Cancel, centred and movable.
    pub fn new(id: egui::Id, icon: &'a str, title: &'a str) -> Self {
        Self {
            id,
            icon,
            title,
            anchor: None,
            placed: None,
            ok: "OK",
            cancel: "Cancel",
            apply: None,
            nothing_to_apply: false,
            enter_submits: true,
            escape_closes: true,
            closeable: true,
            width: Tokens::get().form_card_width * 1.5,
            body_padding: true,
            page_rows: 16.0,
            page_padding: egui::Margin::same(Tokens::get().space_16 as i8),
            page_chrome: false,
            left_slot: false,
            page_header_in_titlebar: false,
            dock_key: None,
            escape_here: true,
            answer_keys: super::AnswerKeys::Off,
            unsaved: false,
        }
    }

    /// An outer width of its own, in points, instead of one and a half cards.
    pub fn width(mut self, width: f32) -> Self {
        if width.is_finite() && width > 0.0 {
            self.width = width.min(Tokens::get().form_card_width * 16.0);
        }
        self
    }

    /// A body `width` points wide, with the window's padding around it.
    pub fn body_width(self, width: f32) -> Self {
        let t = Tokens::get();
        self.width(width + 2.0 * t.space_12 + 2.0)
    }

    /// The body has no padding and no scroll area of its own: it fills the window (a picture
    /// that takes all the room there is).
    pub fn fill_body(mut self) -> Self {
        self.body_padding = false;
        self
    }

    /// For a window without a footer floating in the main window: whether Esc is this window's
    /// to take this frame (no other window, menu or list is open beside it). In a window of its
    /// own Esc is always its own; a tab leaves Esc to the main window.
    pub fn escape_here(mut self, here: bool) -> Self {
        self.escape_here = here;
        self
    }

    /// A settings window the user can resize, pop out into a window of its own, or dock as a
    /// tab in a panel area of the app; `key` names it in the stored layout. See [`super::window_host`].
    pub fn dockable(mut self, key: &'a str) -> Self {
        if !key.is_empty() {
            self.dock_key = Some(key);
        }
        self
    }

    /// Whether the window holds edits that OK would apply and Cancel would throw away. A
    /// dockable window that does and is closed from its tab, its header or its native window
    /// asks first: Apply (as OK), Discard (as Cancel) or Keep open. Without edits it closes at
    /// once. Esc and the footer's buttons keep their meaning.
    pub fn unsaved(mut self, edited: bool) -> Self {
        self.unsaved = edited;
        self
    }

    /// Pin the card at `align` of the screen, `offset` away (it then no longer moves).
    pub fn anchor(mut self, align: egui::Align2, offset: egui::Vec2) -> Self {
        self.anchor = Some((align, offset));
        self
    }

    /// Start with `pivot` of the card at `pos`; the card can then be moved.
    pub fn placed(mut self, pivot: egui::Align2, pos: egui::Pos2) -> Self {
        self.placed = Some((pivot, pos));
        self
    }

    /// The primary and closing actions, and an optional third one between them. An empty `ok`
    /// leaves the primary action out.
    pub fn actions(mut self, ok: &'a str, cancel: &'a str, apply: Option<&'a str>) -> Self {
        (self.ok, self.cancel, self.apply) = (ok, cancel, apply);
        self
    }

    /// Whether the third action has anything to do: for an Apply that keeps the window open,
    /// whether anything changed since it opened or since the last Apply (#8561). Without it
    /// the action is disabled for the pointer, the keyboard and accessibility.
    pub fn apply_enabled(mut self, enabled: bool) -> Self {
        self.nothing_to_apply = !enabled;
        self
    }

    /// A card of editable fields: Enter belongs to the fields and never submits (confirmations
    /// let Enter give the primary action when nothing has focus).
    pub fn fields(mut self) -> Self {
        self.enter_submits = false;
        self
    }

    /// Esc does not close the card (the header's close and the closing action still do): for a
    /// card holding a long text the user is editing, where Esc must not discard the work.
    pub fn keep_open_on_escape(mut self) -> Self {
        self.escape_closes = false;
        self
    }

    /// `cards` times the card width instead of one and a half, for wide footers.
    pub fn cards(mut self, cards: f32) -> Self {
        if cards.is_finite() && cards > 0.0 {
            self.width = Tokens::get().form_card_width * cards.min(16.0);
        }
        self
    }

    /// Maximum page-body height in control rows, still bounded by the viewport.
    pub fn page_rows(mut self, rows: f32) -> Self {
        if rows.is_finite() {
            self.page_rows = rows.clamp(1.0, 32.0);
        }
        self
    }

    /// Content inset for a page-based window, independent of its navigation rail.
    pub fn page_padding(mut self, padding: egui::Margin) -> Self {
        self.page_padding =
            egui::Margin { left: padding.left.max(0), right: padding.right.max(0), top: padding.top.max(0), bottom: padding.bottom.max(0) };
        self
    }

    /// Place the existing page-header callback beside the title; other windows retain their body header.
    pub fn page_header_in_titlebar(mut self) -> Self {
        self.page_header_in_titlebar = true;
        self
    }

    /// Let Y and N answer the box (see [`super::AnswerKeys`]): only for a yes or no question with
    /// exactly two answers.
    pub fn answer_keys(mut self, keys: super::AnswerKeys) -> Self {
        self.answer_keys = keys;
        self
    }

    /// A required choice, such as web recovery, has no dismiss action. Existing data remains
    /// pending until the body explicitly handles a choice.
    pub fn required(mut self) -> Self {
        self.closeable = false;
        self.escape_closes = false;
        self
    }

    /// A standard closable window for platform content and reports. Uses the same header,
    /// bounded scrolling body and fixed outer chrome as every other system window.
    /// Returns whether the shared header's close action was clicked.
    pub fn show_body(self, ctx: &egui::Context, body: impl FnOnce(&mut Ui)) -> bool {
        self.fields().actions("", "", None).show(ctx, true, body).0
    }

    /// Show a temporary status window with no header dismissal or dialog shortcuts.
    pub fn show_passive_body(mut self, ctx: &egui::Context, body: impl FnOnce(&mut Ui)) {
        self.closeable = false;
        // No footer handles either key, so leave both in the shared input queue.
        self.enter_submits = true;
        self.escape_closes = true;
        self.actions("", "", None).show(ctx, true, body);
    }

    /// Draw shared header, content and optional footer, retaining the caller's IDs.
    pub fn show(self, ctx: &egui::Context, valid: bool, body: impl FnOnce(&mut Ui)) -> (bool, super::Action) {
        if let Some(key) = self.dock_key {
            return self.hosted(ctx, key, valid, body, |_| {});
        }
        self.surface(ctx, false, |ui| self.content(ui, valid, body, |_| {}))
    }

    /// A dockable window that floats as a window of the caller's own making (one with behaviour
    /// the card has not, such as a picture the user sizes freely): docked or in a window of its
    /// own it is drawn here, without a footer, and the result says whether it was closed. `None`:
    /// it floats, and the caller draws it this frame (with [`super::window_host::place_menu`] and
    /// [`super::window_host::dock_on_drop`] on its title bar, so it can be moved from there).
    pub fn show_unless_floating(self, ctx: &egui::Context, body: impl FnOnce(&mut Ui)) -> Option<bool> {
        use super::window_host as host;
        let key = self.dock_key?;
        let place = host::place(ctx, key);
        let docked = matches!(&place.placement, host::Placement::Docked(area) if host::area_on_screen(ctx, area));
        if !docked && place.placement != host::Placement::Detached {
            host::shown(ctx, key, self.title);
            return None;
        }
        Some(self.actions("", "", None).hosted(ctx, key, true, body, |_| {}).0)
    }

    /// A pinned left action slot, independent of the right-aligned footer buttons.
    pub fn show_with_left(
        mut self,
        ctx: &egui::Context,
        valid: bool,
        body: impl FnOnce(&mut Ui),
        left: impl FnOnce(&mut Ui),
    ) -> (bool, super::Action) {
        self.left_slot = true;
        if let Some(key) = self.dock_key.filter(|_| !self.page_chrome) {
            return self.hosted(ctx, key, valid, body, left);
        }
        self.surface(ctx, false, |ui| self.content(ui, valid, body, left))
    }

    pub fn content(&self, ui: &mut Ui, valid: bool, body: impl FnOnce(&mut Ui), left: impl FnOnce(&mut Ui)) -> (bool, super::Action) {
        self.content_in(ui, valid, body, left, Chrome::CARD)
    }

    fn has_footer(&self) -> bool {
        !(self.ok.is_empty() && self.cancel.is_empty() && self.apply.is_none() && !self.left_slot)
    }

    /// The height of everything but the body: header, body padding and footer.
    fn chrome_height(&self, header: bool) -> f32 {
        let t = Tokens::get();
        (if header { super::header_height() } else { 0.0 })
            + if self.body_padding { t.space_12 * 2.0 } else { 0.0 }
            + if self.has_footer() { t.form_footer_height } else { 0.0 }
    }

    fn content_in(
        &self,
        ui: &mut Ui,
        valid: bool,
        body: impl FnOnce(&mut Ui),
        left: impl FnOnce(&mut Ui),
        chrome: Chrome<'_>,
    ) -> (bool, super::Action) {
        super::scope(ui, |ui| {
            let t = Tokens::get();
            if chrome.waiting {
                ui.disable();
            }
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut close = false;
            if !self.page_header_in_titlebar && chrome.header {
                match chrome.place {
                    Some(key) => {
                        let mut menu = |ui: &mut Ui, rect: egui::Rect| super::window_host::place_button(ui, rect, key);
                        header_with_place(
                            ui,
                            (self.icon, self.page_chrome.then_some(t.form_primary)),
                            self.title,
                            self.closeable,
                            || close = true,
                            &mut menu,
                        );
                    }
                    None => {
                        header_impl(ui, (self.icon, self.page_chrome.then_some(t.form_primary)), self.title, None, self.closeable, || close = true)
                    }
                }
            }
            egui::Frame::NONE.inner_margin(if self.body_padding { t.space_12 } else { 0.0 }).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = t.form_gap_y;
                if let (true, Some(height)) = (self.body_padding, chrome.body_height) {
                    // A window with a size of its own (a tab too): the body fills it and scrolls,
                    // also sideways when it is wider. A body that only scrolled up and down would
                    // make the whole window as wide as itself and push the footer's buttons out of
                    // a tab, past the edge of its panel area.
                    let height = height.max(0.0);
                    egui::ScrollArea::both()
                        .id_salt("window_body")
                        .max_height(height)
                        .min_scrolled_height(height)
                        .auto_shrink([false, false])
                        .show(ui, body);
                } else if self.body_padding {
                    let height = (dimension(room_height(ui.ctx())) - t.form_header_height - t.form_footer_height - t.space_24 * 2.0).max(0.0);
                    // Offer the body all the room the screen leaves, not just the height the card
                    // had last frame: a card first laid out on a tiny page (a browser canvas not
                    // sized yet) would otherwise keep a body of no height, its buttons hidden (#9472).
                    egui::ScrollArea::vertical()
                        .id_salt("window_body")
                        .max_height(height)
                        .min_scrolled_height(height)
                        .auto_shrink([false, true])
                        .show(ui, body);
                } else {
                    body(ui);
                }
            });
            if !chrome.keys || chrome.waiting {
                // A tab: Enter and Esc belong to the main window, not to this window's footer.
                // Waiting: they answer the question.
            } else if !self.enter_submits && ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                // The key is kept from OK (Enter ends the typing in a field), but a footer
                // button that holds the keyboard is still pressed by it.
                super::actions::note_enter_kept_from_ok(ui.ctx());
            }
            if chrome.keys && !chrome.waiting && !self.escape_closes {
                ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            }
            // A window without a footer closes on Esc itself, as its close button does, unless a
            // menu or list of its own is open (Esc closes that first). The key is used up.
            if chrome.keys
                && chrome.escape
                && self.escape_closes
                && self.closeable
                && !self.has_footer()
                && ui.is_enabled()
                && !egui::Popup::is_any_open(ui.ctx())
                && ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            {
                close = true;
            }
            let action = if !self.has_footer() {
                super::Action::None
            } else {
                super::actions::footer_with_left(
                    ui,
                    self.ok,
                    self.cancel,
                    valid,
                    super::Footer { apply: self.apply, nothing_to_apply: self.nothing_to_apply, keys_off: !chrome.keys, ..Default::default() },
                    left,
                )
            };
            let action = match action {
                super::Action::None if ui.is_enabled() && self.apply.is_none() && !self.cancel.is_empty() => {
                    super::answer_key(ui.ctx(), self.answer_keys, valid && !self.ok.is_empty())
                }
                other => other,
            };
            (close, action)
        })
    }

    /// Declarative page navigation: the kit owns geometry, selection and page scrolling.
    /// Preferences uses [`Self::show_pages_with_header`]; this entry is what the page-list tests call.

    /// Page navigation with a strip above the page body (Preferences search).
    /// `on_select` runs when a page tab is clicked, before that page is drawn.
    pub fn show_pages_with_header<S>(
        self,
        ctx: &egui::Context,
        selected: &mut usize,
        pages: &[WindowPage<S>],
        state: &mut S,
        header: fn(&mut Ui, &mut S),
        on_select: fn(&mut S),
        sidebar_footer: Option<(f32, fn(&mut Ui, &mut S))>,
        left: impl FnOnce(&mut Ui),
    ) -> (bool, super::Action) {
        self.show_pages_inner(ctx, selected, pages, state, Some(header), Some(on_select), sidebar_footer, left)
    }

    fn show_pages_inner<S>(
        mut self,
        ctx: &egui::Context,
        selected: &mut usize,
        pages: &[WindowPage<S>],
        state: &mut S,
        header: Option<fn(&mut Ui, &mut S)>,
        on_select: Option<fn(&mut S)>,
        sidebar_footer: Option<(f32, fn(&mut Ui, &mut S))>,
        left: impl FnOnce(&mut Ui),
    ) -> (bool, super::Action) {
        self.body_padding = false;
        self.page_chrome = true;
        let rows = self.page_rows;
        let padding = self.page_padding;
        let in_titlebar = self.page_header_in_titlebar;
        let (icon, title, closeable) = (self.icon, self.title, self.closeable);
        let header_closed = std::cell::Cell::new(false);
        let result = self.show_with_left(
            ctx,
            true,
            |ui| {
                if in_titlebar {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let mut accessory = |ui: &mut Ui| {
                        if let Some(header) = header {
                            header(ui, state);
                        }
                    };
                    header_actions(
                        ui,
                        (icon, Some(Tokens::get().form_primary)),
                        title,
                        None,
                        closeable,
                        || header_closed.set(true),
                        Some((Tokens::get().space_24 * 9.0 + Tokens::get().space_14, &mut accessory)),
                        None,
                    );
                }
                page_list_sized(
                    ui,
                    selected,
                    pages,
                    state,
                    rows,
                    padding,
                    if in_titlebar { None } else { header },
                    on_select,
                    sidebar_footer,
                    header.is_some(),
                );
            },
            left,
        );
        (result.0 || header_closed.get(), result.1)
    }

    /// The same bounded card frame, either movable or an input-blocking modal.
    pub fn surface(&self, ctx: &egui::Context, modal: bool, add: impl FnOnce(&mut Ui) -> (bool, super::Action)) -> (bool, super::Action) {
        let frame = if self.page_chrome {
            // A system window covers what is behind it, like every other window (#9078).
            super::navigation_popup_frame().inner_margin(0).fill(Tokens::get().dialog_body)
        } else {
            Panel::card_frame(ctx)
        };
        let inner_width = (self.width.min(dimension(ctx.content_rect().width())) - frame.total_margin().sum().x).max(0.0);
        if modal {
            let shown = super::Container::new(ctx, super::Surface::Window)
                .modal(self.id)
                .backdrop_color(Tokens::get().tab_on_fill_dark.gamma_multiply(0.35))
                .frame(frame)
                .show(ctx, |ui| {
                    ui.set_width(inner_width);
                    let result = add(ui);
                    super::Container::finish_content(ui, frame);
                    result
                });
            solidcraft_ui_registry::publish_rect("dialog:window", shown.response.rect);
            return shown.inner;
        }
        let window = super::Container::new(ctx, super::Surface::Window)
            .window(self.title)
            .id(self.id)
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .min_width(inner_width)
            .max_width(inner_width)
            .default_width(inner_width)
            .scroll(false)
            .frame(frame)
            .fade_in(false)
            .fade_out(false);
        let window = match (self.anchor, self.placed) {
            (Some((align, offset)), _) => window.anchor(align, offset),
            (None, Some((pivot, pos))) => window.pivot(pivot).default_pos(pos),
            (None, None) => window.pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center() - vec2(0.0, row_height() * 2.5)),
        };
        let shown = super::container::FeatureWindow::new(window, self.id, frame).show(ctx, |ui| {
            let result = add(ui);
            super::Container::finish_content(ui, frame);
            result
        });
        if let Some(shown) = &shown {
            solidcraft_ui_registry::publish_rect("dialog:window", shown.response.rect);
        }
        shown.and_then(|r| r.inner).unwrap_or((false, super::Action::None))
    }
}

impl Window<'_> {
    /// A dockable window, where the user put it: a floating card, a native window or a tab.
    fn hosted(&self, ctx: &egui::Context, key: &str, valid: bool, body: impl FnOnce(&mut Ui), left: impl FnOnce(&mut Ui)) -> (bool, super::Action) {
        use super::window_host as host;
        host::shown(ctx, key, self.title);
        let mut asking = host::asking(ctx, key);
        if asking && !self.unsaved {
            // Nothing left to ask about.
            host::set_asking(ctx, key, false);
            return (true, super::Action::None);
        }
        if host::take_close(ctx, key) {
            if !self.unsaved {
                return (true, super::Action::None);
            }
            host::set_asking(ctx, key, true);
            asking = true;
        }
        let place = host::place(ctx, key);
        // Asked to come into view (its tab is brought to the front by `raise` itself).
        let raise = host::take_raise(ctx, key);
        // The answer, when a window of its own asked the question itself.
        let answered = std::cell::Cell::new(None);
        let (closed, action) = match &place.placement {
            host::Placement::Docked(area) if host::area_on_screen(ctx, area) => {
                if raise {
                    host::focus_main_window(ctx);
                }
                self.docked(ctx, key, asking, valid, body, left)
            }
            host::Placement::Detached => self.detached(ctx, key, raise, (asking, &answered), valid, body, left),
            _ => self.floating(ctx, key, &place, raise, asking, valid, body, left),
        };
        if asking {
            let answer = answered.take().unwrap_or_else(|| self.ask_before_closing(ctx, key, valid));
            return answer.map_or((false, super::Action::None), |a| (false, a));
        }
        if closed && self.unsaved {
            // Closed with edits not applied: ask first, from the next frame on.
            host::set_asking(ctx, key, true);
            return (false, super::Action::None);
        }
        (closed, action)
    }

    /// The question a window with edits not applied asks when it is closed. `Some(Ok)` applies
    /// them (Apply), `Some(Cancel)` throws them away (Discard); either closes the window. Keep
    /// open, Esc and the question's own close button end the question and leave all as it is.
    fn ask_before_closing(&self, ctx: &egui::Context, key: &str, valid: bool) -> Option<super::Action> {
        use super::window_host as host;
        let texts = host::texts(ctx);
        let (closed, action) = super::Confirm::new(egui::Id::unique(("db_close_question", key)), self.icon, self.title)
            .tone(super::Tone::Warning)
            .actions(&texts.close_apply, &texts.close_keep_open, Some(&texts.close_discard))
            .secondary_left()
            .show(ctx, valid, |ui| {
                ui.add(egui::Label::new(texts.close_question.as_str()).wrap());
            });
        let answer = match action {
            super::Action::Ok => Some(super::Action::Ok),
            super::Action::Apply => Some(super::Action::Cancel),
            super::Action::Cancel => None,
            super::Action::None if closed => None,
            super::Action::None => return None,
        };
        host::set_asking(ctx, key, false);
        answer
    }

    /// A tab in a panel area: the contents fill the body its tab group gave it, under the tabs.
    fn docked(
        &self,
        ctx: &egui::Context,
        key: &str,
        waiting: bool,
        valid: bool,
        body: impl FnOnce(&mut Ui),
        left: impl FnOnce(&mut Ui),
    ) -> (bool, super::Action) {
        super::Container::record_host_window(ctx, self.id, None);
        let Some((rect, layer)) = super::window_host::slot(ctx, key) else {
            // Behind another tab, or its tab group is drawn next frame.
            return (false, super::Action::None);
        };
        let mut ui = Ui::new(ctx.clone(), self.id.with("docked"), egui::UiBuilder::new().layer_id(layer).max_rect(rect));
        ui.set_clip_rect(rect);
        let chrome = Chrome { header: false, keys: false, body_height: Some(rect.height() - self.chrome_height(false)), place: None, escape: false, waiting };
        let shown = self.content_in(&mut ui, valid, body, left, chrome);
        solidcraft_ui_registry::publish_rect("dialog:window", rect);
        shown
    }

    /// A native window of its own. Where the system cannot open one (a single-window backend,
    /// the test harness) egui shows it as a plain window inside the main one.
    fn detached(
        &self,
        ctx: &egui::Context,
        key: &str,
        raise: bool,
        (waiting, answered): (bool, &std::cell::Cell<Option<Option<super::Action>>>),
        valid: bool,
        body: impl FnOnce(&mut Ui),
        left: impl FnOnce(&mut Ui),
    ) -> (bool, super::Action) {
        use super::window_host as host;
        super::Container::record_host_window(ctx, self.id, None);
        let (size, pos) = host::native_opening(ctx, key, self.first_native_size(ctx, key));
        let size = egui::Vec2::from(size);
        let mut builder = egui::ViewportBuilder::default().with_title(self.title).with_inner_size(size).with_min_inner_size(host::MIN_SIZE);
        if let Some(pos) = pos {
            builder = builder.with_position(egui::Pos2::from(pos));
        }
        let viewport = egui::ViewportId::from_hash_of(("db_window", key));
        // A question in the main window waits for its answer. Its modal cannot reach a window of
        // its own, so that window waits too: nothing in it changes what the question is about.
        let question = super::Container::modal_input_blocked(ctx);
        let mut parts = Some((body, left));
        let mut closed_by_system = false;
        let shown = ctx.show_viewport_immediate(viewport, builder, |ui, class| {
            let Some((body, left)) = parts.take() else {
                return (false, super::Action::None);
            };
            let native = class != egui::ViewportClass::EmbeddedWindow;
            if native && question {
                ui.disable();
            }
            // Shown inside the main window instead (no native windows here): its own size, kept
            // clear of the screen edges so the footer stays in reach.
            let screen = dimension(ui.ctx().content_rect().height()) - Tokens::get().space_24 * 3.0;
            let mut height = Some(size.y.min(screen) - self.chrome_height(true));
            if native {
                let (inner, outer, close, focused, minimized) = ui.input(|i| {
                    let v = i.viewport();
                    (v.inner_rect, v.outer_rect, v.close_requested(), v.focused == Some(true), v.minimized == Some(true))
                });
                host::native_moved(ui.ctx(), key, inner.map(|r| r.size()), outer.map(|r| r.min));
                host::native_focus(ui.ctx(), key, focused);
                host::native_input(ui.ctx());
                if raise && host::app_focused(ui.ctx()) {
                    if minimized {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    }
                    if minimized || !focused {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                }
                closed_by_system = close;
                if close && self.unsaved {
                    // It asks first; the system leaves it open meanwhile.
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
                let rect = ui.max_rect();
                ui.painter().rect_filled(rect, 0.0, Tokens::get().dialog_body);
                height = Some(rect.height() - self.chrome_height(true));
            }
            let chrome = Chrome { header: true, keys: true, body_height: height, place: Some(key), escape: true, waiting };
            let shown = self.content_in(ui, valid, body, left, chrome);
            if native && waiting {
                // In the window that was closed, where the user is looking.
                answered.set(Some(self.ask_before_closing(ui.ctx(), key, valid)));
            }
            if native {
                // The keys go to this window while it has the focus; the app's shortcuts get
                // the ones nothing here used (embedded, the main window has them already).
                host::pass_on_keys(ui.ctx());
            }
            shown
        });
        (shown.0 || closed_by_system, shown.1)
    }

    /// The size a window first opens with as a window of its own: its floating card's (the size
    /// the user gave it, else the one it was last drawn at), held to most of the screen the main
    /// window is on so its footer is in reach.
    fn first_native_size(&self, ctx: &egui::Context, key: &str) -> egui::Vec2 {
        use super::window_host as host;
        let card = host::place(ctx, key)
            .size
            .map(egui::Vec2::from)
            .or_else(|| egui::AreaState::load(ctx, self.id).and_then(|a| a.size))
            .filter(|s| s.is_finite())
            .unwrap_or(vec2(self.width, self.width * 0.9));
        let screen = ctx.input_for(egui::ViewportId::ROOT, |i| i.viewport().monitor_size).filter(|m| m.is_finite() && m.x > 0.0 && m.y > 0.0);
        // Room for the system's title bar and task bar.
        let most = screen.map_or(host::MAX_SIZE, |m| m * 0.85).max(host::MIN_SIZE);
        card.max(host::MIN_SIZE).min(most)
    }

    /// A floating card in the main window, resizable from its right and bottom edges, that
    /// docks as a tab when dropped on a panel area.
    fn floating(
        &self,
        ctx: &egui::Context,
        key: &str,
        place: &super::window_host::WindowPlace,
        raise: bool,
        waiting: bool,
        valid: bool,
        body: impl FnOnce(&mut Ui),
        left: impl FnOnce(&mut Ui),
    ) -> (bool, super::Action) {
        use super::window_host as host;
        let frame = Panel::card_frame(ctx);
        let margin = frame.total_margin().sum();
        // The card keeps clear of the main window's resize border, which takes the pointer
        // before the card does: at the bottom of the screen the card's own corner was under it.
        let keep = crate::titlebar::keep_clear_of_border(ctx);
        let room = ctx.content_rect().shrink(keep);
        let screen = room.size();
        let (outer_width, body_height) = match place.size {
            Some([w, h]) => {
                let h = h.min(dimension(ctx.content_rect().height()) - Tokens::get().space_24.max(2.0 * keep));
                (w.min(dimension(screen.x)), Some(h - margin.y - self.chrome_height(true)))
            }
            None => (self.width.min(dimension(screen.x)), None),
        };
        let inner_width = (outer_width - margin.x).max(0.0);
        let window = super::Container::new(ctx, super::Surface::Window)
            .window(self.title)
            .id(self.id)
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .min_width(inner_width)
            .max_width(inner_width)
            .default_width(inner_width)
            .scroll(false)
            .frame(frame)
            .fade_in(false)
            .fade_out(false)
            .constrain_to(room);
        let stored = place.pos.map(egui::Pos2::from);
        let window = match (self.anchor, self.placed, stored) {
            (Some((align, offset)), _, _) => window.anchor(align, offset),
            (None, _, Some(pos)) => {
                // Put where it was dropped (a tab dragged out) or first left (when it still holds
                // the pivot it opened with), unless it is already there.
                let off =
                    egui::AreaState::load(ctx, self.id).is_some_and(|a| a.pivot != egui::Align2::LEFT_TOP || a.left_top_pos().distance(pos) > 0.5);
                let window = window.pivot(egui::Align2::LEFT_TOP).default_pos(pos);
                if off && !ctx.is_being_dragged(self.id) { window.current_pos(pos) } else { window }
            }
            (None, Some((pivot, pos)), None) => window.pivot(pivot).default_pos(pos),
            (None, None, None) => window.pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center() - vec2(0.0, row_height() * 2.5)),
        };
        let resize_id = self.id.with("db_resize");
        let shown = super::container::FeatureWindow::new(window, self.id, frame).show(ctx, |ui| {
            let chrome = Chrome { header: true, keys: true, body_height, place: Some(key), escape: self.escape_here, waiting };
            let result = self.content_in(ui, valid, body, left, chrome);
            super::Container::finish_content(ui, frame);
            resize_grips(ui, resize_id, key, margin);
            result
        });
        if let Some(shown) = &shown {
            solidcraft_ui_registry::publish_rect("dialog:window", shown.response.rect);
            drop_on_dock(ctx, self.id, key, place.pos.is_some(), &shown.response);
            if raise {
                ctx.move_to_top(shown.response.layer_id);
                host::focus_main_window(ctx);
            }
        }
        shown.and_then(|r| r.inner).unwrap_or((false, super::Action::None))
    }
}

/// The height a card in the main window can have: the screen's, less the main window's resize
/// border at the top and the bottom, which a floating card keeps clear of.
fn room_height(ctx: &egui::Context) -> f32 {
    ctx.content_rect().height() - 2.0 * crate::titlebar::keep_clear_of_border(ctx)
}

/// The floating card's right edge, bottom edge and corner: dragging one gives the card a size of
/// its own, remembered under `key`.
fn resize_grips(ui: &mut Ui, id: egui::Id, key: &str, margin: egui::Vec2) {
    let t = Tokens::get();
    let r = ui.min_rect();
    let grip = t.space_12;
    let edge = t.space_4;
    let corner = egui::Rect::from_min_max(r.max - vec2(grip, grip), r.max);
    let zones = [
        (corner, id.with("corner"), egui::CursorIcon::ResizeNwSe, vec2(1.0, 1.0)),
        (
            egui::Rect::from_min_max(egui::pos2(r.right() - edge, r.top() + header_height()), egui::pos2(r.right(), r.bottom() - grip)),
            id.with("right"),
            egui::CursorIcon::ResizeHorizontal,
            vec2(1.0, 0.0),
        ),
        (
            egui::Rect::from_min_max(egui::pos2(r.left() + grip, r.bottom() - edge), egui::pos2(r.right() - grip, r.bottom())),
            id.with("bottom"),
            egui::CursorIcon::ResizeVertical,
            vec2(0.0, 1.0),
        ),
    ];
    let outer = r.size() + margin;
    for (rect, zone, cursor, axes) in zones {
        let response = ui.interact(rect, zone, egui::Sense::drag());
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(cursor);
        }
        if response.drag_started() {
            ui.data_mut(|d| d.insert_temp(zone, outer));
        }
        if response.dragged()
            && let Some(start) = ui.data(|d| d.get_temp::<egui::Vec2>(zone))
        {
            let delta = response.total_drag_delta().unwrap_or_default() * axes;
            super::window_host::update(ui.ctx(), key, |p| p.resize(start + delta));
        }
        solidcraft_ui_registry::publish_rect(
            &format!(
                "dialog:resize:{}",
                if axes == vec2(1.0, 1.0) {
                    "corner"
                } else if axes.x > 0.0 {
                    "right"
                } else {
                    "bottom"
                }
            ),
            rect,
        );
    }
    // Three short diagonal strokes in the corner, the usual sign that a window can be resized.
    let ink = t.text_dim.gamma_multiply(0.7);
    for i in 1..=3 {
        let d = i as f32 * t.space_3;
        ui.painter().line_segment([corner.max - vec2(d, 1.0), corner.max - vec2(1.0, d)], egui::Stroke::new(1.0, ink));
    }
}

/// While the floating card is dragged over a panel area, show that it will dock there; dropped
/// there, it becomes a tab.
///
/// egui drags the card on any press that no widget on it takes as a drag, a click on one of its
/// buttons included. Only a press that has really moved the card counts: a click on Close over a
/// panel area must not dock it.
fn drop_on_dock(ctx: &egui::Context, id: egui::Id, key: &str, stored: bool, response: &egui::Response) {
    use super::window_host as host;
    let moving_id = id.with("db_moving");
    let click_dist = ctx.options(|o| o.input_options.max_click_dist);
    if response.dragged() && ctx.input(|i| i.pointer.total_drag_delta()).is_some_and(|d| d.length() > click_dist) {
        ctx.data_mut(|d| d.insert_temp(moving_id, true));
    }
    let moved = ctx.data(|d| d.get_temp::<bool>(moving_id)).unwrap_or(false);
    if !response.dragged() {
        ctx.data_mut(|d| d.remove::<bool>(moving_id));
    }
    let dropped = response.drag_stopped() && moved;
    // Where the card is now (dragged, or kept on screen by egui) is where it opens next time. A
    // card never moved keeps opening where it asks to: its first frames are still finding their size.
    if (stored || dropped)
        && let Some(state) = egui::AreaState::load(ctx, id)
    {
        host::moved(ctx, key, state.left_top_pos());
    }
    // Docking and the drop hint follow a press that has moved further than a click, kept by the
    // host from where the press began.
    host::dock_on_drop(ctx, key, response);
}

/// A page's existing icon, translated label key, stable handle and draw callback.
pub struct WindowPage<S> {
    pub icon: &'static str,
    pub label: &'static str,
    pub group: Option<&'static str>,
    pub handle: &'static str,
    pub scroll: bool,
    pub draw: fn(&mut Ui, &mut S),
}

fn page_list_sized<S>(
    ui: &mut Ui,
    selected: &mut usize,
    pages: &[WindowPage<S>],
    state: &mut S,
    rows: f32,
    padding: egui::Margin,
    header: Option<fn(&mut Ui, &mut S)>,
    on_select: Option<fn(&mut S)>,
    sidebar_footer: Option<(f32, fn(&mut Ui, &mut S))>,
    search_scroll: bool,
) {
    let t = Tokens::get();
    // The search strip, the gap under it, and the following item spacing sit above the page.
    // They must not eat the rows the page already had, or the graphics checkboxes fall out of view.
    let header_band = if header.is_some() { t.control_height + t.form_gap_y + t.space_8 } else { 0.0 };
    let height = (dimension(room_height(ui.ctx())) - t.form_header_height - t.form_footer_height - t.space_24 * 3.0)
        .clamp(0.0, t.control_height * rows + header_band);
    let (body, _) = ui.allocate_exact_size(vec2(dimension(ui.available_width()), height), egui::Sense::hover());
    let width = if body.width() + 2.0 * super::NAV_BORDER < WINDOW_PAGES_NARROW_AT {
        WINDOW_PAGES_NARROW_WIDTH
    } else {
        t.form_label_width * 2.0 + t.space_24 * 2.0
    }
    .min(body.width());
    let nav = egui::Rect::from_min_size(body.min, vec2(width, body.height()));
    ui.painter().rect_filled(nav, 0.0, t.dialog_footer);
    t.secondary_vline(ui.painter(), nav.right(), nav.y_range(), t.dialog_rule);
    solidcraft_ui_registry::publish_rect("dialog:pages", nav);
    *selected = (*selected).min(pages.len().saturating_sub(1));
    let nav_inner = if sidebar_footer.is_some() {
        egui::Rect::from_min_max(nav.min + vec2(t.space_8, t.space_6), nav.max - vec2(t.space_8, t.space_10))
    } else {
        nav.shrink2(vec2(t.space_8, t.space_6))
    };
    let footer_height = sidebar_footer.map_or(0.0, |(height, _)| dimension(height).min(nav_inner.height()));
    let list = egui::Rect::from_min_max(nav_inner.min, egui::pos2(nav_inner.right(), nav_inner.bottom() - footer_height));
    ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
        ui.set_clip_rect(ui.clip_rect().intersect(nav));
        ui.spacing_mut().item_spacing.y = t.space_2;
        let selection_id = ui.scope_id().with("page_selection");
        let changed = ui.data_mut(|data| {
            let previous = data.get_temp::<usize>(selection_id);
            data.insert_temp(selection_id, *selected);
            previous != Some(*selected)
        });
        egui::ScrollArea::vertical().id_salt("window_pages").max_height(list.height()).show(ui, |ui| {
            for (index, page) in pages.iter().enumerate() {
                if let Some(group) = page.group {
                    ui.add_space(t.space_8);
                    let (rect, _) = ui.allocate_exact_size(vec2(dimension(ui.available_width()), t.space_24), egui::Sense::hover());
                    let rect = rect.shrink2(vec2(t.space_10, 0.0));
                    paint_text(
                        ui,
                        rect,
                        &crate::casing::upper(solidcraft_ui_text::tr(group)),
                        crate::fonts::semibold(t.font_small),
                        t.form_dim,
                        true,
                    );
                }
                let label = solidcraft_ui_text::tr(page.label);
                let (rect, response) =
                    ui.allocate_exact_size(vec2(dimension(ui.available_width()), t.control_height + t.space_4), egui::Sense::click());
                if response.clicked() {
                    *selected = index;
                    if let Some(on_select) = on_select {
                        on_select(state);
                    }
                }
                let on = *selected == index;
                if on && changed && sidebar_footer.is_some() && !list.contains_rect(rect) {
                    response.scroll_to_me(Some(egui::Align::Center));
                }
                if on || response.hovered() || response.has_focus() {
                    ui.painter().rect_filled(rect, t.control_radius, if on { t.selected_bg } else { t.hover });
                }
                let ink = if on { t.form_primary } else { t.form_ink };
                let icon =
                    egui::Rect::from_center_size(egui::pos2(rect.left() + t.space_10 + t.space_8, rect.center().y), vec2(t.space_16, t.space_16));
                crate::icons::paint_ink(ui.painter(), icon, page.icon, if on { ink } else { t.form_dim });
                let text_rect = egui::Rect::from_min_max(egui::pos2(icon.right() + super::NAV_ROW_GAP, rect.top()), rect.right_bottom());
                paint_text(
                    ui,
                    text_rect,
                    label,
                    if on { crate::fonts::semibold(t.font_label) } else { egui::FontId::proportional(t.font_label) },
                    ink,
                    false,
                );
                response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), on, label));
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                solidcraft_ui_registry::publish_target(page.handle, rect);
                response.backing_on_hover_text(label);
            }
        });
    });
    if let Some((_, footer)) = sidebar_footer {
        let rect = egui::Rect::from_min_max(egui::pos2(nav_inner.left(), list.bottom()), nav_inner.max);
        ui.scope_builder(egui::UiBuilder::new().id_salt("pages_footer").max_rect(rect), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(rect));
            footer(ui, state);
        });
        solidcraft_ui_registry::publish_rect("dialog:pages_footer", rect);
    }
    let content = egui::Rect::from_min_max(egui::pos2(nav.right(), body.top()), body.max);
    let padding = egui::epaint::MarginF32 {
        left: padding.leftf().min(content.width() / 2.0),
        right: padding.rightf().min(content.width() / 2.0),
        top: padding.topf().min(content.height() / 2.0),
        bottom: padding.bottomf().min(content.height() / 2.0),
    };
    let content = content - padding;
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(content)
            .ui_stack_info(egui::UiStackInfo::default().with_tag_value(super::preferences::PAGE_SCROLL_GUTTER, padding.right)),
        |ui| {
            let clip = egui::Rect::from_min_max(content.min, egui::pos2(body.right(), content.bottom()));
            ui.set_clip_rect(ui.clip_rect().intersect(clip));
            ui.spacing_mut().item_spacing.y = t.form_gap_y;
            if let Some(header) = header {
                header(ui, state);
            }
            let force_scroll = search_scroll && super::settings_search::force_scroll(ui.ctx());
            if let Some(page) = pages.get(*selected) {
                if page.scroll || force_scroll {
                    let scroll = super::settings_scroll(ui, ("sc_prefs_page", *selected), |ui| (page.draw)(ui, state));
                    solidcraft_ui_registry::publish_rect("dialog:page_body", scroll.inner_rect);
                } else {
                    (page.draw)(ui, state);
                    solidcraft_ui_registry::publish_rect("dialog:page_body", content);
                }
            }
        },
    );
}
