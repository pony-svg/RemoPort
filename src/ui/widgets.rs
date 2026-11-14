//! Shared UI components used by the shell and individual pages.

use crate::ui::app::RemoportApp;
use crate::ui::fonts;
use crate::ui::i18n::{t, tf};
use crate::ui::icons::{self, Icon};
use crate::ui::theme::Theme;
use egui::popup::{popup_below_widget, PopupCloseBehavior};
use egui::{
    pos2, vec2, Align, Align2, Color32, Context, CursorIcon, FontId, Layout, Painter, Rect,
    Response, RichText, Rounding, Sense, Stroke, Ui,
};
use remoport_core::domain::{DeviceBookmark, RemoteAppItem};
use remoport_core::logging;

const PC_CARD: egui::Vec2 = vec2(280.0, 204.0);
const APP_CARD: egui::Vec2 = vec2(190.0, 150.0);
const FAVORITE: Color32 = Color32::from_rgb(255, 184, 0);
const DANGER: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);

/// What the button of an empty view does.
#[derive(Clone, Copy)]
pub(super) enum EmptyAction {
    AddPc,
    AddWorkspace,
}

pub(super) fn icon_button(theme: &Theme, ui: &mut Ui, icon: Icon, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.painter()
            .rect_filled(rect, Rounding::same(6.0), theme.nav_hover);
    }
    let color = if hovered {
        theme.text_primary
    } else {
        theme.text_secondary
    };
    icons::paint(ui.painter(), rect.shrink(8.0), icon, color);
    response
        .on_hover_text(tooltip)
        .on_hover_cursor(CursorIcon::PointingHand)
}

/// A rounded filter pill; filled when selected, with a chevron when it
/// opens a list.
pub(super) fn chip(
    theme: &Theme,
    ui: &mut Ui,
    text: &str,
    selected: bool,
    dropdown: bool,
) -> Response {
    let text_color = if selected {
        theme.bg_dark
    } else {
        theme.text_primary
    };
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), FontId::proportional(13.0), text_color);
    let width = galley.size().x + 26.0 + if dropdown { 18.0 } else { 0.0 };
    let (rect, response) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
    let painter = ui.painter();
    let (fill, stroke) = if selected {
        (theme.text_primary, Stroke::NONE)
    } else if response.hovered() {
        (theme.card_hover, theme.card_stroke())
    } else {
        (theme.card_bg, theme.card_stroke())
    };
    painter.rect(rect, Rounding::same(15.0), fill, stroke);
    let text_pos = pos2(rect.left() + 13.0, rect.center().y - galley.size().y / 2.0);
    painter.galley(text_pos, galley, text_color);
    if dropdown {
        icons::paint(
            painter,
            Rect::from_center_size(pos2(rect.right() - 17.0, rect.center().y), vec2(14.0, 14.0)),
            Icon::ChevronDown,
            text_color,
        );
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// "All" plus a drop-down chip choosing one of `options`.
pub(super) fn filter_chips(
    theme: &Theme,
    ui: &mut Ui,
    id: &str,
    label: &str,
    options: &[String],
    filter: &mut Option<String>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if chip(theme, ui, t("All"), filter.is_none(), false).clicked() && filter.is_some() {
            logging::event(format_args!("Filter: all ({label})"));
            *filter = None;
        }
        let text = filter.clone().unwrap_or_else(|| label.to_string());
        let chip_resp = chip(theme, ui, &text, filter.is_some(), true);
        let menu = ui.make_persistent_id(id);
        if chip_resp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(menu));
        }
        popup_below_widget(
            ui,
            menu,
            &chip_resp,
            PopupCloseBehavior::CloseOnClick,
            |ui| {
                ui.set_min_width(180.0);
                if options.is_empty() {
                    ui.label(
                        RichText::new(t("Nothing to filter by yet")).color(theme.text_secondary),
                    );
                }
                for option in options {
                    if ui
                        .selectable_label(filter.as_deref() == Some(option.as_str()), option)
                        .clicked()
                    {
                        logging::event(format_args!("Filter: {label} \"{option}\""));
                        *filter = Some(option.clone());
                    }
                }
            },
        );
    });
}

/// A foldable section title; returns whether the section is open.
pub(super) fn section(
    app: &mut RemoportApp,
    ui: &mut Ui,
    key: &str,
    title: &str,
    count: usize,
) -> bool {
    let open = !app.state.collapsed_groups.iter().any(|g| g == key);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    let painter = ui.painter();
    let chevron = if open {
        Icon::ChevronDown
    } else {
        Icon::ChevronRight
    };
    icons::paint(
        painter,
        Rect::from_center_size(pos2(rect.left() + 8.0, rect.center().y), vec2(16.0, 16.0)),
        chevron,
        app.theme.text_secondary,
    );
    let title = painter.layout_no_wrap(
        title.to_owned(),
        fonts::medium(14.0),
        app.theme.text_primary,
    );
    let title_width = title.size().x;
    painter.galley(
        pos2(rect.left() + 24.0, rect.center().y - title.size().y / 2.0),
        title,
        app.theme.text_primary,
    );
    painter.text(
        pos2(rect.left() + 32.0 + title_width, rect.center().y),
        Align2::LEFT_CENTER,
        count.to_string(),
        FontId::proportional(12.0),
        app.theme.text_secondary,
    );
    if response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
        if open {
            logging::event(format_args!("Folded {key}"));
            app.state.collapsed_groups.push(key.to_string());
        } else {
            logging::event(format_args!("Unfolded {key}"));
            app.state.collapsed_groups.retain(|g| g != key);
        }
    }
    ui.add_space(6.0);
    open
}

/// Lays cards out left to right, wrapping, with their tops aligned.
pub(super) fn card_row(ui: &mut Ui, add_cards: impl FnOnce(&mut Ui)) {
    ui.with_layout(
        Layout::left_to_right(Align::TOP).with_main_wrap(true),
        |ui| {
            ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
            add_cards(ui);
        },
    );
}

pub(super) fn show_empty_state(
    app: &mut RemoportApp,
    ui: &mut Ui,
    icon: Icon,
    title: &str,
    description: &str,
    action: Option<(&str, EmptyAction)>,
) {
    ui.vertical_centered(|ui| {
        ui.add_space(70.0);
        let (rect, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
        icons::paint(ui.painter(), rect, icon, app.theme.text_secondary);
        ui.add_space(12.0);
        ui.label(
            RichText::new(title)
                .font(fonts::medium(18.0))
                .color(app.theme.text_primary),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(description)
                .size(13.0)
                .color(app.theme.text_secondary),
        );
        if let Some((text, action)) = action {
            ui.add_space(16.0);
            if ui
                .add_sized([220.0, 34.0], app.theme.primary_button(text))
                .clicked()
            {
                match action {
                    EmptyAction::AddPc => {
                        app.add_edit_dialog.open_add(app.state.default_resolution);
                    }
                    EmptyAction::AddWorkspace => app.add_workspace_dialog.open(),
                }
            }
        }
    });
}

/// A one-off connection by name or address, without saving the PC.
pub(super) fn quick_connect(app: &mut RemoportApp, ui: &mut Ui, width: f32) {
    let field = ui.add(
        egui::TextEdit::singleline(&mut app.quick_connect_host)
            .hint_text(t("Name or address of the PC"))
            .desired_width(width),
    );
    let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let clicked = ui.add(app.theme.primary_button(t("Connect"))).clicked();
    let host = app.quick_connect_host.trim().to_string();
    if (entered || clicked) && !host.is_empty() {
        app.start_quick_connect(host);
    }
}

fn paint_thumbnail(theme: &Theme, painter: &Painter, thumb: Rect) {
    painter.add(icons::gradient_rect(
        thumb,
        theme.thumb_from,
        theme.thumb_to,
    ));
    let clipped = painter.with_clip_rect(thumb);
    clipped.circle_filled(
        pos2(thumb.right() - 40.0, thumb.top() + 26.0),
        72.0,
        // Premultiplied on purpose: from_rgba_unmultiplied premultiplies in
        // linear space, which turns a faint white into a bright one.
        Color32::from_rgba_premultiplied(26, 26, 26, 26),
    );
    clipped.circle_filled(
        pos2(thumb.left() + 54.0, thumb.bottom() + 6.0),
        86.0,
        Color32::from_rgba_premultiplied(16, 16, 16, 16),
    );
    icons::paint(
        painter,
        Rect::from_center_size(thumb.center(), vec2(44.0, 44.0)),
        Icon::Monitor,
        Color32::WHITE,
    );
    // Round the shaded corners off with the card's own color.
    painter.rect_stroke(
        thumb.expand(3.0),
        Rounding::same(11.0),
        Stroke::new(6.0_f32, theme.card_bg),
    );
}

pub(super) fn show_pc_card(app: &mut RemoportApp, ui: &mut Ui, pc: &DeviceBookmark) {
    let (rect, response) = ui.allocate_exact_size(PC_CARD, Sense::click());
    let painter = ui.painter_at(rect.expand(1.0));
    let border = if response.hovered() {
        app.theme.accent
    } else {
        app.theme.card_border
    };
    painter.rect(
        rect,
        Rounding::same(10.0),
        app.theme.card_bg,
        Stroke::new(1.0_f32, border),
    );

    let thumb = Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(rect.width() - 12.0, 130.0));
    paint_thumbnail(&app.theme, &painter, thumb);
    if pc.is_favorite {
        let badge = Rect::from_center_size(thumb.right_top() + vec2(-18.0, 18.0), vec2(18.0, 18.0));
        painter.circle_filled(badge.center(), 13.0, app.theme.card_bg);
        icons::paint(&painter, badge, Icon::StarFilled, FAVORITE);
    }

    painter.text(
        pos2(rect.left() + 14.0, thumb.bottom() + 22.0),
        Align2::LEFT_CENTER,
        ellipsize(pc.display_title(), 26),
        fonts::medium(14.0),
        app.theme.text_primary,
    );
    painter.text(
        pos2(rect.left() + 14.0, thumb.bottom() + 44.0),
        Align2::LEFT_CENTER,
        ellipsize(&pc.pc_name, 32),
        FontId::proportional(12.0),
        app.theme.text_secondary,
    );

    let more_rect = Rect::from_center_size(
        pos2(rect.right() - 24.0, thumb.bottom() + 33.0),
        vec2(30.0, 30.0),
    );
    let more = ui
        .interact(more_rect, response.id.with("more"), Sense::click())
        .on_hover_text(t("More actions"))
        .on_hover_cursor(CursorIcon::PointingHand);
    if more.hovered() {
        painter.rect_filled(more_rect, Rounding::same(6.0), app.theme.nav_hover);
    }
    icons::paint(
        &painter,
        more_rect.shrink(7.0),
        Icon::More,
        app.theme.text_secondary,
    );

    let menu = response.id.with("menu");
    if more.clicked() {
        logging::event(format_args!("Menu of \"{}\"", pc.display_title()));
        ui.memory_mut(|m| m.toggle_popup(menu));
    } else if response.clicked() {
        app.connect_to(pc);
    }
    popup_below_widget(ui, menu, &more, PopupCloseBehavior::CloseOnClick, |ui| {
        pc_menu_items(app, ui, pc);
    });
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .context_menu(|ui| pc_menu_items(app, ui, pc));
}

fn pc_menu_items(app: &mut RemoportApp, ui: &mut Ui, pc: &DeviceBookmark) {
    ui.set_min_width(200.0);
    if ui.button(t("Connect")).clicked() {
        app.connect_to(pc);
        ui.close_menu();
    }
    if ui.button(t("Edit…")).clicked() {
        app.add_edit_dialog.open_edit(pc);
        ui.close_menu();
    }
    let favorite = if pc.is_favorite {
        t("Remove from favorites")
    } else {
        t("Add to favorites")
    };
    if ui.button(favorite).clicked() {
        app.toggle_favorite(pc.id);
        ui.close_menu();
    }
    ui.separator();
    if ui
        .button(RichText::new(t("Delete…")).color(DANGER))
        .clicked()
    {
        logging::event(format_args!(
            "Delete requested for \"{}\" ({})",
            pc.display_title(),
            pc.pc_name
        ));
        app.pending_delete = Some(pc.clone());
        ui.close_menu();
    }
}

pub(super) fn show_delete_confirmation(app: &mut RemoportApp, ctx: &Context) {
    let Some(pc) = app.pending_delete.clone() else {
        return;
    };
    let mut decision = None;
    egui::Window::new(t("Delete this PC?"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(tf(
                "\"{}\" ({}) will be removed from Remoport, with its password if it is \
                 remembered in the keyring. The PC itself is not affected.",
                &[&pc.display_title(), &pc.pc_name],
            ));
            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let delete = egui::Button::new(RichText::new(t("Delete")).color(Color32::WHITE))
                    .fill(DANGER);
                if ui.add(delete).clicked() {
                    decision = Some(true);
                }
                if ui.button(t("Cancel")).clicked() {
                    decision = Some(false);
                }
            });
        });
    match decision {
        Some(true) => {
            logging::event(format_args!(
                "Deleted \"{}\" ({})",
                pc.display_title(),
                pc.pc_name
            ));
            app.state.bookmarks.retain(|b| b.id != pc.id);
            // Its remembered password, if any, goes with it.
            let _ = app.ports.secrets.forget(pc.id);
            app.pending_delete = None;
        }
        Some(false) => {
            logging::event(format_args!(
                "Kept \"{}\": delete cancelled",
                pc.display_title()
            ));
            app.pending_delete = None;
        }
        None => {}
    }
}

pub(super) fn show_app_card(app: &mut RemoportApp, ui: &mut Ui, item: &RemoteAppItem) {
    let (rect, response) = ui.allocate_exact_size(APP_CARD, Sense::click());
    let painter = ui.painter_at(rect.expand(1.0));
    let border = if response.hovered() {
        app.theme.accent
    } else {
        app.theme.card_border
    };
    painter.rect(
        rect,
        Rounding::same(10.0),
        app.theme.card_bg,
        Stroke::new(1.0_f32, border),
    );

    let tile = Rect::from_center_size(pos2(rect.center().x, rect.top() + 44.0), vec2(52.0, 52.0));
    painter.rect_filled(tile, Rounding::same(12.0), app.theme.accent);
    match item.icon_name.as_str() {
        "terminal" => {
            painter.text(
                tile.center(),
                Align2::CENTER_CENTER,
                ">_",
                FontId::monospace(18.0),
                Color32::WHITE,
            );
        }
        _ => icons::paint(&painter, tile.shrink(12.0), Icon::Window, Color32::WHITE),
    }
    if item.is_favorite {
        icons::paint(
            &painter,
            Rect::from_center_size(rect.right_top() + vec2(-18.0, 18.0), vec2(16.0, 16.0)),
            Icon::StarFilled,
            FAVORITE,
        );
    }

    painter.text(
        pos2(rect.center().x, rect.top() + 94.0),
        Align2::CENTER_CENTER,
        ellipsize(&item.name, 22),
        fonts::medium(13.0),
        app.theme.text_primary,
    );
    painter.text(
        pos2(rect.center().x, rect.top() + 116.0),
        Align2::CENTER_CENTER,
        ellipsize(&item.workspace_name, 26),
        FontId::proportional(11.5),
        app.theme.text_secondary,
    );

    let id = item.id;
    let favorite = if item.is_favorite {
        t("Remove from favorites")
    } else {
        t("Add to favorites")
    };
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if response.clicked() {
        app.open_app(item);
    }
    response.context_menu(|ui| {
        if ui.button(t("Open")).clicked() {
            app.open_app(item);
            ui.close_menu();
        }
        if ui.button(favorite).clicked() {
            if let Some(app_item) = app.state.remote_apps.iter_mut().find(|a| a.id == id) {
                app_item.is_favorite = !app_item.is_favorite;
                logging::event(format_args!("{favorite}: app \"{}\"", app_item.name));
            }
            ui.close_menu();
        }
    });
}

/// Shortens `text` to `max` characters, ending with "…" when cut.
pub(super) fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}
