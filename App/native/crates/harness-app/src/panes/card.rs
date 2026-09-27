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

/// The title row: navigation when the sidebar is retracted, the task's name
/// and menu, and the toolbar — the view picker, quick actions, and the three
/// panel toggles.
///
/// The toolbar is measured first and the title gets whatever is left, because
/// a long task name next to the toolbar is a row that runs out of room, and
/// the one thing that must not happen is the name and the buttons sharing
/// pixels.
///
/// @param state the application state
/// @param ui the interface to draw into
fn title(state: &mut HarnessState, ui: &mut Ui) {
    let mut menu_action: Option<MenuAction> = None;
    let mut switch: Option<Pane> = None;
    let title = state.session_title();

    let rect = ui.max_rect();
    // The whole row is the window's title bar: drag it, double-click to zoom.
    crate::shell::drag_strip(ui, rect, "title-drag");
    let height = theme::ROW;
    let top = rect.top() + (rect.height() - height) / 2.0;
    let toolbar_width = TOOLBAR_WIDTH.min((rect.width() * 0.55).max(120.0));
    let toolbar_rect = Rect::from_min_max(
        egui::pos2((rect.right() - toolbar_width).max(rect.left()), top),
        egui::pos2(rect.right(), top + height),
    );
    // With the sidebar retracted, its toggle and the arrows move here — past
    // the traffic lights, which macOS draws over the card's corner.
    let lead = if state.sidebar_open {
        0.0
    } else {
        (crate::shell::TRAFFIC_LIGHTS - (rect.left() - 20.0)).max(0.0) + 3.0 * 28.0 + 8.0
    };
    if !state.sidebar_open {
        let nav = Rect::from_min_max(
            egui::pos2(rect.left() + lead - 3.0 * 28.0 - 8.0, top),
            egui::pos2(rect.left() + lead, top + height),
        );
        theme::inside(ui, nav, |ui| ui.horizontal_centered(|ui| crate::shell::nav_buttons(state, ui)));
    }
    let title_rect = Rect::from_min_max(
        egui::pos2(rect.left() + lead, rect.top()),
        egui::pos2((toolbar_rect.left() - 12.0).max(rect.left() + lead), rect.bottom()),
    );

    let renaming_current = state
        .renaming
        .as_ref()
        .is_some_and(|(id, _)| state.current_task.as_deref() == Some(id.as_str()));
    let mut rename_done: Option<(String, String)> = None;
    theme::inside(ui, title_rect, |ui| {
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            crate::icons::tile(ui, Icon::Sparkle, theme::TEXT, 22.0, theme::ELEVATED, 6);
            let more_width = theme::ROW;
            let text_width = (ui.available_width() - more_width - 8.0).max(24.0);
            if renaming_current {
                if let Some((id, text)) = state.renaming.as_mut() {
                    let edit = ui.add(
                        egui::TextEdit::singleline(text)
                            .desired_width(text_width)
                            .font(egui::FontId::proportional(15.0)),
                    );
                    if !edit.has_focus() && !edit.lost_focus() {
                        edit.request_focus();
                    }
                    if edit.lost_focus() {
                        let keep = ui.input(|input| input.key_pressed(egui::Key::Enter));
                        rename_done = Some((id.clone(), if keep { text.clone() } else { String::new() }));
                    }
                }
            } else {
                let name = ui
                    .scope(|ui| {
                        ui.set_max_width(text_width);
                        ui.add(
                            egui::Label::new(RichText::new(&title).size(15.0).color(theme::TEXT).strong())
                                .truncate()
                                .selectable(false)
                                .sense(Sense::click()),
                        )
                    })
                    .inner;
                if name
                    .on_hover_text(format!("{title}\ndouble-click to rename"))
                    .double_clicked()
                {
                    menu_action = Some(MenuAction::Rename);
                }
            }
            let more = icons::button(ui, Icon::More, theme::DIM, more_width, theme::HOVER_SOFT);
            egui::Popup::menu(&more).show(|ui| {
                ui.set_min_width(230.0);
                if ui.button("New task  ⌘N").clicked() {
                    menu_action = Some(MenuAction::NewTask);
                    ui.close();
                }
                if ui
                    .add_enabled(state.current_task.is_some(), egui::Button::new("Rename"))
                    .clicked()
                {
                    menu_action = Some(MenuAction::Rename);
                    ui.close();
                }
                if ui.button("Export the transcript  ⌘⇧E").clicked() {
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
                if state.current_task.is_some() {
                    ui.separator();
                    if ui.button(RichText::new("Delete this task").color(theme::RED)).clicked() {
                        menu_action = Some(MenuAction::Delete);
                        ui.close();
                    }
                }
            });
        });
    });

    theme::inside(ui, toolbar_rect, |ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let toggle = |ui: &mut Ui, icon: Icon, on: bool, hint: &str| {
                let size = height;
                let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
                let radius = theme::static_radius(6.0);
                if on {
                    ui.painter()
                        .rect_filled(rect, egui::CornerRadius::same(radius), theme::ACTIVE.gamma_multiply(0.6));
                } else if response.hovered() {
                    ui.painter()
                        .rect_filled(rect, egui::CornerRadius::same(radius), theme::HOVER_SOFT);
                }
                icons::paint(ui, icon, rect.shrink(size * 0.2), if on { theme::TEXT } else { theme::DIM });
                response.on_hover_text(hint)
            };
            if toggle(ui, Icon::PanelRight, state.agent_open, "session panel ⌘J").clicked() {
                state.agent_open = !state.agent_open;
                state.save_config();
            }
            if toggle(ui, Icon::PanelBottom, state.pane == Pane::Terminal, "terminal ⌘2").clicked() {
                switch = Some(if state.pane == Pane::Terminal {
                    Pane::Conversation
                } else {
                    Pane::Terminal
                });
            }
            if toggle(ui, Icon::List, state.tree_open, "file tree ⌘B").clicked() {
                state.tree_open = !state.tree_open;
                state.save_config();
            }
            if toggle(ui, Icon::Bolt, false, "quick actions ⌘K").clicked() {
                state.palette = Some(crate::state::Palette::new(crate::state::PaletteMode::Commands));
                state.focus = Some(crate::state::FocusRequest::Palette);
            }
            ui.add_space(4.0);
            // The view picker: which view the card shows, and every other one.
            let (pill, response) =
                ui.allocate_exact_size(Vec2::new(46.0, height), Sense::click());
            let radius = theme::static_radius(height / 2.0);
            ui.painter()
                .rect_filled(pill, egui::CornerRadius::same(radius), if response.hovered() {
                    theme::HOVER_SOFT
                } else {
                    theme::ELEVATED
                });
            theme::outline(ui.painter(), pill, radius, theme::BORDER);
            icons::paint(
                ui,
                pane_icon(&state.pane),
                Rect::from_center_size(egui::pos2(pill.left() + 15.0, pill.center().y), Vec2::splat(15.0)),
                theme::ACCENT,
            );
            icons::paint(
                ui,
                Icon::Chevron,
                Rect::from_center_size(egui::pos2(pill.right() - 13.0, pill.center().y), Vec2::splat(12.0)),
                theme::FAINT,
            );
            let response = response.on_hover_text(format!("{} — switch the view", state.pane.title()));
            egui::Popup::menu(&response).show(|ui| {
                ui.set_min_width(200.0);
                for (index, pane) in Pane::ALL.iter().enumerate() {
                    let label = format!("{}   ⌘{}", pane.title(), index + 1);
                    if ui.selectable_label(state.pane == *pane, label).clicked() {
                        switch = Some(*pane);
                        ui.close();
                    }
                }
            });
        });
    });

    if let Some(pane) = switch {
        state.go(pane, crate::state::SidebarSpot::Task);
    }
    if let Some((id, text)) = rename_done {
        state.renaming = None;
        if !text.trim().is_empty() {
            state.rename_task(&id, &text);
        }
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
        Some(MenuAction::Rename) => match state.current_task.clone() {
            Some(id) => state.renaming = Some((id, title)),
            None => state.toast("a task can be renamed once something has been asked in it"),
        },
        Some(MenuAction::Delete) => {
            if let Some(id) = state.current_task.clone() {
                state.delete_task(&id);
            }
        }
        None => {}
    }
}

/// How wide the toolbar is: the view picker and four toggles.
const TOOLBAR_WIDTH: f32 = 46.0 + 4.0 + 4.0 * (theme::ROW + 4.0);

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
    /// Rename the task on screen.
    Rename,
    /// Delete the task on screen.
    Delete,
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
