//! The centre card: its title row, and whichever view is current.
//!
//! The card is the product's main surface, so its title row carries the three
//! things a user reaches for without leaving it: what this task is, which view
//! of it is showing, and the panel toggle. The views themselves are unchanged —
//! this file only frames them and hands each one the rectangle it owns.

use eframe::egui::{self, Align, Layout, Rect, RichText, Sense, Ui, Vec2};

use crate::icons::{self, Icon};
use crate::panes;
use crate::state::{Dialog, HarnessState, Pane};
use crate::theme;

/// How tall the card's title row is.
const TITLE_ROW: f32 = 46.0;

/// Draws the centre card's chrome and the current view.
///
/// @param state the application state
/// @param ui the interface to draw into
pub fn show(state: &mut HarnessState, ui: &mut Ui) {
    let card = ui.max_rect();
    let title_rect = Rect::from_min_size(card.min, Vec2::new(card.width(), TITLE_ROW));
    theme::inside(ui, title_rect.shrink2(Vec2::new(20.0, 0.0)), |ui| {
        title(state, ui)
    });
    ui.painter().hline(
        card.x_range(),
        title_rect.bottom(),
        egui::Stroke::new(1.0, theme::BORDER),
    );

    let body = Rect::from_min_max(egui::pos2(card.left(), title_rect.bottom()), card.max);
    match state.pane {
        Pane::Conversation => panes::conversation::show(state, ui, body),
        _ => theme::inside(ui, body.shrink2(Vec2::new(16.0, 10.0)), |ui| {
            ui.set_min_height((body.height() - 20.0).max(0.0));
            match state.pane {
                Pane::Conversation => {}
                Pane::Terminal => panes::terminal::show(state, ui),
                Pane::Editor => panes::editor::show(state, ui),
                Pane::Git => panes::git::show(state, ui),
                Pane::Workflows => panes::workflows::show(state, ui),
                Pane::Observability => panes::observability::show(state, ui),
            }
        }),
    }
}

/// The title row: the task's name, its actions, and the view switch.
///
/// The switch is measured first and the title is drawn into whatever is left,
/// because a long task name next to six views is a row that runs out of room,
/// and the one thing that must not happen is the name and the switch sharing
/// pixels.
///
/// @param state the application state
/// @param ui the interface to draw into
fn title(state: &mut HarnessState, ui: &mut Ui) {
    let mut menu_action: Option<MenuAction> = None;
    let mut switch: Option<Pane> = None;
    let title = state.session_title();

    let rect = ui.max_rect();
    let height = theme::ROW;
    let top = rect.top() + (rect.height() - height) / 2.0;
    let switch_width = view_switch_width();
    let switch_rect = Rect::from_min_max(
        egui::pos2((rect.right() - switch_width).max(rect.left()), top),
        egui::pos2(rect.right(), top + height),
    );
    let title_rect = Rect::from_min_max(
        rect.min,
        egui::pos2((switch_rect.left() - 12.0).max(rect.left()), rect.bottom()),
    );

    theme::inside(ui, title_rect, |ui| {
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let more_width = theme::ROW;
            let text_width = (ui.available_width() - more_width).max(24.0);
            ui.scope(|ui| {
                ui.set_max_width(text_width);
                ui.add(
                    egui::Label::new(RichText::new(&title).size(15.0).color(theme::TEXT).strong())
                        .truncate(),
                )
                .on_hover_text(&title);
            });
            let more = icons::button(ui, Icon::More, theme::DIM, more_width, theme::HOVER_SOFT);
            egui::Popup::menu(&more).show(|ui| {
                ui.set_min_width(230.0);
                if ui.button("Export the transcript").clicked() {
                    menu_action = Some(MenuAction::Export);
                    ui.close();
                }
                if ui.button("Copy the transcript").clicked() {
                    menu_action = Some(MenuAction::Copy);
                    ui.close();
                }
                if ui.button("Restart the engine").clicked() {
                    menu_action = Some(MenuAction::Restart);
                    ui.close();
                }
                if ui.button("New task").clicked() {
                    menu_action = Some(MenuAction::NewTask);
                    ui.close();
                }
                ui.separator();
                if ui.button("Open folder…  ⌘O").clicked() {
                    menu_action = Some(MenuAction::Folder);
                    ui.close();
                }
                if ui.button("Models and routes…  ⌘,").clicked() {
                    menu_action = Some(MenuAction::Models);
                    ui.close();
                }
                if ui.button("About this harness").clicked() {
                    menu_action = Some(MenuAction::About);
                    ui.close();
                }
            });
        });
    });

    theme::inside(ui, switch_rect, |ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            let toggle = icons::button(ui, Icon::Panel, theme::DIM, height, theme::HOVER_SOFT);
            if toggle
                .on_hover_text("show or hide the session panel ⌘J")
                .clicked()
            {
                state.agent_open = !state.agent_open;
                state.save_config();
            }
            ui.add_space(3.0);
            for pane in Pane::ALL.iter().rev() {
                let selected = state.pane == *pane;
                if view_tab(ui, pane, selected).clicked() {
                    switch = Some(*pane);
                }
            }
        });
    });

    if let Some(pane) = switch {
        state.pane = pane;
    }
    match menu_action {
        Some(MenuAction::Export) => match state.export_transcript() {
            Ok(path) => state.toast(format!("wrote {}", path.display())),
            Err(err) => state.toast(err),
        },
        Some(MenuAction::Copy) => {
            let text = state.agent.conversation.transcript_text();
            ui.ctx().copy_text(text);
            state.toast("transcript copied");
        }
        Some(MenuAction::Restart) => state.restart_engine(),
        Some(MenuAction::NewTask) => state.new_task(),
        Some(MenuAction::Folder) => state.open_folder_dialog(),
        Some(MenuAction::Models) => state.open_model_dialog(),
        Some(MenuAction::About) => state.dialog = Some(Dialog::About),
        None => {}
    }
}

/// How wide the view switch and the panel toggle need.
///
/// @returns the width in points
fn view_switch_width() -> f32 {
    let tabs = Pane::ALL.len() as f32 * theme::ROW + (Pane::ALL.len() as f32 - 1.0) * 3.0;
    tabs + 3.0 + theme::ROW
}

/// One view of the switch: the pane's glyph, its name on hover.
///
/// @param ui the interface to draw into
/// @param pane the view it opens
/// @param selected whether it is the current view
/// @returns the response of the tab
fn view_tab(ui: &mut Ui, pane: &Pane, selected: bool) -> egui::Response {
    let size = theme::ROW;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let painter = ui.painter();
    let radius = theme::static_radius(size / 2.0);
    if selected {
        painter.rect_filled(rect, egui::CornerRadius::same(radius), theme::ELEVATED);
        theme::outline(painter, rect, radius, theme::BORDER);
    } else if response.hovered() {
        painter.rect_filled(rect, egui::CornerRadius::same(radius), theme::HOVER_SOFT);
    }
    let colour = if selected { theme::TEXT } else { theme::FAINT };
    icons::paint(ui, pane_icon(pane), rect.shrink(size * 0.24), colour);
    response.on_hover_text(pane.title())
}

/// The glyph that stands for a view.
///
/// @param pane the view
/// @returns its mark
fn pane_icon(pane: &Pane) -> Icon {
    match pane {
        Pane::Conversation => Icon::Sparkle,
        Pane::Terminal => Icon::Terminal,
        Pane::Editor => Icon::Doc,
        Pane::Git => Icon::Branch,
        Pane::Workflows => Icon::Book,
        Pane::Observability => Icon::Gauge,
    }
}

/// What the title row's menu asked for.
enum MenuAction {
    /// Write the transcript out.
    Export,
    /// Put the transcript on the clipboard.
    Copy,
    /// Start the engine again.
    Restart,
    /// Begin a new session.
    NewTask,
    /// Work in a different folder.
    Folder,
    /// Open the model-route editor.
    Models,
    /// Open the about panel.
    About,
}
