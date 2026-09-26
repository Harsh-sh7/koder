//! The window's chrome: the sidebar, the floating cards, and the footer.
//!
//! The layout is the reference client's: a gray chrome with a task sidebar on
//! the left, a white conversation card floating in the middle with its composer
//! and a footer underneath, and a second white card on the right for what the
//! engine is doing — delegations and commands — plus a help button in the corner.
//!
//! Nothing here decides anything. The sidebar's rows set the current view and
//! the panel's rows open the tool call they name; every number drawn is read
//! from the session in [`HarnessState`], so the chrome cannot disagree with the
//! transcript it frames.

use eframe::egui::{self, Align, Color32, CornerRadius, Layout, Rect, RichText, Sense, Ui, Vec2};

use harness_core::agent::RunState;

use crate::app::{compact, shorten};
use crate::icons::{self, Icon};
use crate::state::{
    Dialog, FocusRequest, HarnessState, Palette, PaletteMode, Pane, ProcessSource, SidebarSpot,
};
use crate::theme;

/// The sidebar: the mode switch, the task, the tools, and who is working.
///
/// @param state the application state
/// @param ui the interface to draw into
pub fn sidebar(state: &mut HarnessState, ui: &mut Ui) {
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        mode_switch(state, ui);
    });

    ui.add_space(14.0);
    let mut go_spot: Option<(Pane, SidebarSpot)> = None;
    let mut new_task = false;

    if row(ui, Icon::NewTask, "New Task", false, false).clicked() {
        new_task = true;
    }
    if row(ui, Icon::Search, "Search", false, false)
        .on_hover_text("search file contents ⌘⇧F")
        .clicked()
    {
        state.palette = Some(Palette::new(PaletteMode::Contents));
        state.focus = Some(FocusRequest::Palette);
    }

    ui.add_space(12.0);
    heading(ui, "Workspaces");
    ui.add_space(2.0);
    let workspace = state
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| state.root.display().to_string());
    if row(ui, Icon::Harness, &workspace, false, true)
        .on_hover_text(format!(
            "{} — click to open a different folder (⌘O)",
            state.root.display()
        ))
        .clicked()
    {
        state.open_folder_dialog();
    }
    let title = shorten(&state.session_title(), 26);
    if row(
        ui,
        Icon::Sparkle,
        &title,
        state.spot == SidebarSpot::Task,
        true,
    )
    .clicked()
    {
        go_spot = Some((Pane::Conversation, SidebarSpot::Task));
    }

    // Everything else is pinned to the bottom, the way the reference draws it.
    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
        ui.add_space(10.0);
        profile(state, ui);
        ui.add_space(6.0);
        for (spot, icon, label, pane) in [
            (SidebarSpot::Extensions, Icon::Grid, "Extensions", None),
            (
                SidebarSpot::Automations,
                Icon::Clock,
                "Automations",
                Some(Pane::Observability),
            ),
            (
                SidebarSpot::Sites,
                Icon::Window,
                "Sites",
                Some(Pane::Editor),
            ),
            (
                SidebarSpot::Knowledge,
                Icon::Book,
                "Knowledge Center",
                Some(Pane::Workflows),
            ),
        ] {
            let selected = state.spot == spot;
            if row(ui, icon, label, selected, false).clicked() {
                match pane {
                    Some(pane) => go_spot = Some((pane, spot)),
                    None => {
                        state.spot = spot;
                        state.open_model_dialog();
                    }
                }
            }
        }
    });

    if new_task {
        state.new_task();
    }
    if let Some((pane, spot)) = go_spot {
        state.go(pane, spot);
    }
}

/// The two-part switch at the top of the sidebar: the panel toggle and the mode
/// menu.
///
/// @param state the application state
/// @param ui the interface to draw into
fn mode_switch(state: &mut HarnessState, ui: &mut Ui) {
    let panel = state.agent_open;
    let height = 30.0;
    let gap = 4.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(34.0 + gap + 116.0, height), Sense::hover());
    let painter = ui.painter();

    // One capsule holds both segments, with the current one lifted onto white.
    let capsule = theme::static_radius(height / 2.0);
    painter.rect_filled(rect, CornerRadius::same(capsule), theme::ELEVATED);
    theme::outline(painter, rect, capsule, theme::BORDER);

    let segment = theme::static_radius((height - 6.0) / 2.0);
    let toggle_rect =
        Rect::from_min_size(rect.min + Vec2::new(3.0, 3.0), Vec2::splat(height - 6.0));
    let toggle = ui.interact(toggle_rect, ui.id().with("panel-toggle"), Sense::click());
    if panel {
        painter.rect_filled(toggle_rect, CornerRadius::same(segment), theme::PANEL);
        theme::outline(painter, toggle_rect, segment, theme::BORDER);
    } else if toggle.hovered() {
        painter.rect_filled(toggle_rect, CornerRadius::same(segment), theme::HOVER);
    }
    icons::paint(ui, Icon::Panel, toggle_rect.shrink(7.0), theme::TEXT);

    let pill_rect = Rect::from_min_size(
        egui::pos2(toggle_rect.right() + gap, rect.top() + 3.0),
        Vec2::new(
            rect.right() - 3.0 - (toggle_rect.right() + gap),
            height - 6.0,
        ),
    );
    let pill = ui.interact(pill_rect, ui.id().with("mode-menu"), Sense::click());
    if pill.hovered() {
        painter.rect_filled(pill_rect, CornerRadius::same(segment), theme::HOVER);
    }
    let inset = pill_rect.shrink2(Vec2::new(9.0, 0.0));
    let mark = Rect::from_min_size(
        egui::pos2(inset.left(), inset.center().y - 8.0),
        Vec2::splat(16.0),
    );
    icons::paint(ui, Icon::Sparkle, mark, theme::TEXT);
    painter.text(
        egui::pos2(mark.right() + 6.0, inset.center().y),
        egui::Align2::LEFT_CENTER,
        "General",
        theme::text(0.0),
        theme::TEXT,
    );
    icons::paint(
        ui,
        Icon::Chevron,
        Rect::from_center_size(
            egui::pos2(inset.right() - 7.0, inset.center().y),
            Vec2::splat(12.0),
        ),
        theme::FAINT,
    );

    if toggle.clicked() {
        state.agent_open = !state.agent_open;
        state.save_config();
    }
    egui::Popup::menu(&pill).show(|ui| {
        ui.set_min_width(190.0);
        for pane in Pane::ALL {
            if ui
                .selectable_label(state.pane == pane, pane.title())
                .clicked()
            {
                state.go(pane, SidebarSpot::Task);
                ui.close();
            }
        }
    });
}

/// The bottom row: who is working, and the two buttons beside the name.
///
/// @param state the application state
/// @param ui the interface to draw into
fn profile(state: &mut HarnessState, ui: &mut Ui) {
    let name = user_name();
    let mut menu: Option<ProfileAction> = None;
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        theme::avatar(ui, &name, 26.0);
        ui.add_space(8.0);
        let who = ui
            .add(
                egui::Label::new(RichText::new(&name).size(13.5).color(theme::TEXT))
                    .truncate()
                    .sense(Sense::click()),
            )
            .on_hover_text("the workspace this session is in — click for what you can do with it");
        egui::Popup::menu(&who).show(|ui| {
            ui.set_min_width(230.0);
            if ui.button("Open folder…  ⌘O").clicked() {
                menu = Some(ProfileAction::Folder);
                ui.close();
            }
            if ui.button("Copy the workspace path").clicked() {
                menu = Some(ProfileAction::CopyPath);
                ui.close();
            }
            if ui.button("Reveal in the file tree  ⌘B").clicked() {
                menu = Some(ProfileAction::Reveal);
                ui.close();
            }
            ui.separator();
            if ui.button("Models and routes…  ⌘,").clicked() {
                menu = Some(ProfileAction::Models);
                ui.close();
            }
            if ui.button("About this harness").clicked() {
                menu = Some(ProfileAction::About);
                ui.close();
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(10.0);
            let gear = icons::button(ui, Icon::Gear, theme::DIM, 24.0, theme::HOVER);
            if gear.on_hover_text("settings and about").clicked() {
                state.dialog = Some(Dialog::About);
            }
            ui.add_space(2.0);
            let palette = icons::button(ui, Icon::Grid, theme::DIM, 24.0, theme::HOVER);
            if palette.on_hover_text("command palette ⌘K").clicked() {
                state.palette = Some(Palette::new(PaletteMode::Commands));
                state.focus = Some(FocusRequest::Palette);
            }
        });
    });
    match menu {
        Some(ProfileAction::Folder) => state.open_folder_dialog(),
        Some(ProfileAction::CopyPath) => {
            let path = state.root.display().to_string();
            ui.ctx().copy_text(path.clone());
            state.toast(format!("copied {path}"));
        }
        Some(ProfileAction::Reveal) => {
            state.tree_open = true;
            state.save_config();
            state.toast("the tree is beside the conversation");
        }
        Some(ProfileAction::Models) => state.open_model_dialog(),
        Some(ProfileAction::About) => state.dialog = Some(Dialog::About),
        None => {}
    }
}

/// What the profile menu asked for.
enum ProfileAction {
    /// Work in a different folder.
    Folder,
    /// Put the workspace path on the clipboard.
    CopyPath,
    /// Show the file tree.
    Reveal,
    /// Open the model-route editor.
    Models,
    /// Open the about card.
    About,
}

/// The user's display name, from the environment.
///
/// @returns the name, capitalized
fn user_name() -> String {
    let user = std::env::var("USER").unwrap_or_default();
    let mut characters = user.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => "you".to_string(),
    }
}

/// One sidebar row: glyph, label, and whether it is current.
///
/// @param ui the interface to draw into
/// @param icon the row's mark
/// @param label the row's text
/// @param selected whether the row is current
/// @param indent whether the row belongs to a workspace
/// @returns the response of the row
fn row(ui: &mut Ui, icon: Icon, label: &str, selected: bool, indent: bool) -> egui::Response {
    let pad = if indent { 22.0 } else { 6.0 };
    theme::list_row(ui, selected, true, |ui| {
        ui.add_space(pad);
        icons::icon(
            ui,
            icon,
            if selected { theme::TEXT } else { theme::DIM },
            17.0,
        );
        ui.add_space(10.0);
        ui.add(
            egui::Label::new(RichText::new(label).size(13.5).color(if selected {
                theme::TEXT
            } else {
                theme::DIM
            }))
            .truncate(),
        );
    })
}

/// A dimmed section heading in the sidebar.
///
/// @param ui the interface to draw into
/// @param label the heading
fn heading(ui: &mut Ui, label: &str) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(RichText::new(label).size(12.5).color(theme::FAINT));
    });
}

/// The workspace area of the window: the two floating cards, the footer beneath
/// the conversation, and the help button.
///
/// @param state the application state
/// @param ui the interface to draw into
pub fn workspace(state: &mut HarnessState, ui: &mut Ui) {
    let area = ui.max_rect();
    let top = area.top() + 10.0;
    let bottom = area.bottom() - 8.0;
    let gutter = theme::GUTTER;

    let panel_width = if state.agent_open {
        theme::PANEL_WIDTH
    } else {
        0.0
    };
    let panel_gap = if state.agent_open { 12.0 } else { 0.0 };
    let right = area.right() - gutter;
    let panel_rect = Rect::from_min_max(
        egui::pos2(right - panel_width, top),
        egui::pos2(right, bottom),
    );
    let card_right = right - panel_width - panel_gap;
    let footer_height = 26.0;
    let card_rect = Rect::from_min_max(
        egui::pos2(area.left() + gutter, top),
        egui::pos2(card_right, bottom - footer_height),
    );

    theme::surface(ui, card_rect, theme::PANEL, theme::CARD_RADIUS);
    theme::inside(ui, card_rect.shrink(1.0), |ui| {
        ui.set_clip_rect(card_rect);
        crate::panes::card::show(state, ui);
    });

    if state.agent_open {
        theme::surface(ui, panel_rect, theme::PANEL, theme::CARD_RADIUS);
        theme::inside(ui, panel_rect.shrink(1.0), |ui| {
            ui.set_clip_rect(panel_rect);
            panel(state, ui);
        });
    }

    let footer = Rect::from_min_max(
        egui::pos2(card_rect.left() + 14.0, card_rect.bottom() + 1.0),
        egui::pos2(card_rect.right() - 14.0, bottom),
    );
    theme::inside(ui, footer, |ui| footer_row(state, ui));

    fab(state, ui, area);
    toast(state, ui, card_rect);
}

/// The footer under the conversation: where the work is happening, and how much
/// of the window and the budget are spent.
///
/// @param state the application state
/// @param ui the interface to draw into
fn footer_row(state: &mut HarnessState, ui: &mut Ui) {
    let name = state
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| state.root.display().to_string());
    ui.horizontal(|ui| {
        let folder = ui
            .horizontal(|ui| {
                icons::icon(ui, Icon::Folder, theme::DIM, 15.0);
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(shorten(&name, 30))
                            .size(12.5)
                            .color(theme::DIM),
                    )
                    .sense(Sense::click()),
                )
            })
            .inner;
        if folder
            .on_hover_text(format!(
                "{} — click to open a different folder",
                state.root.display()
            ))
            .clicked()
        {
            state.open_folder_dialog();
        }
        if let Some(engine) = running_engine(state) {
            ui.add_space(10.0);
            ui.label(RichText::new(engine).size(12.0).color(theme::FAINT));
        }
        // The meter takes the rest of this row from the right. It has to be laid
        // out inside the same `horizontal`: a sibling layout would start its own
        // row below, past the bottom of the window.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let meter = &state.agent.conversation.meter;
            let caps = &state.config.caps;
            if caps.token_budget_per_session > 0 && meter.used > 0 {
                let spent = meter.used as f32 / caps.token_budget_per_session as f32;
                let colour = if spent >= 1.0 {
                    theme::RED
                } else if spent >= caps.warn_at {
                    theme::AMBER
                } else {
                    theme::DIM
                };
                ui.label(
                    RichText::new(format!(
                        "budget {} / {}",
                        compact(meter.used),
                        compact(caps.token_budget_per_session)
                    ))
                    .size(12.0)
                    .color(colour),
                )
                .on_hover_text("tokens spent this session against the session budget");
                ui.add_space(14.0);
            }
            if let Some(percent) = state.context_percent() {
                ui.label(
                    RichText::new(percent)
                        .size(12.0)
                        .monospace()
                        .color(theme::DIM),
                );
                ui.add_space(6.0);
                let fraction = meter.context_fraction();
                let colour = if fraction >= 0.9 {
                    theme::RED
                } else if fraction >= 0.7 {
                    theme::AMBER
                } else {
                    theme::TEXT
                };
                let response = theme::hatch_meter(ui, fraction, 70.0, colour);
                response.on_hover_text(format!(
                    "context window: {} of {} tokens used",
                    compact(meter.used),
                    compact(meter.size)
                ));
            }
        });
    });
}

/// The engine's name and version, when one is running.
///
/// @param state the application state
/// @returns the label, or nothing
fn running_engine(state: &HarnessState) -> Option<String> {
    let conversation = &state.agent.conversation;
    if conversation.engine_name.is_empty() {
        return None;
    }
    Some(shorten(
        &format!(
            "{} {}",
            conversation.engine_name, conversation.engine_version
        ),
        34,
    ))
}

/// The right card: the session's notice, its delegations, and its commands.
///
/// @param state the application state
/// @param ui the interface to draw into
fn panel(state: &mut HarnessState, ui: &mut Ui) {
    ui.add_space(12.0);
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            let colour = match state.agent.conversation.state {
                RunState::Idle => theme::FAINT,
                RunState::Running => theme::ACCENT,
                RunState::AwaitingApproval { .. } => theme::AMBER,
                RunState::Failed { .. } => theme::RED,
            };
            ui.horizontal(|ui| {
                match state.agent.conversation.state {
                    RunState::Running => {
                        icons::icon(ui, Icon::Ring, colour, 15.0);
                    }
                    _ => {
                        icons::icon(ui, Icon::Sparkle, theme::GREEN, 15.0);
                    }
                }
                ui.add_space(7.0);
                ui.label(
                    RichText::new("Session")
                        .size(13.5)
                        .color(theme::TEXT)
                        .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let badge = theme::badge(ui, state.agent.conversation.state.label(), colour);
                    ui.add_space(4.0);
                    if let Some((stamp, _)) = state.notice() {
                        badge.on_hover_text(stamp);
                    }
                });
            });
        });
    ui.add_space(10.0);

    let mut reveal: Option<String> = None;
    let mut open_shell: Option<usize> = None;
    let rest = ui.available_height();
    egui::Frame::new()
        .inner_margin(egui::Margin { left: 12, right: 12, top: 0, bottom: 0 })
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("session-panel")
                .max_height(rest)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // What is happening right now, first, above everything else:
                    // the panel is the answer to "is it still working?", and it
                    // has to hold that answer without being scrolled to.
                    if let Some(activity) = state.activity() {
                        activity_card(ui, &activity, &model_label(state));
                        ui.add_space(16.0);
                    }

                    // The notice: when the session last moved, and what it said.
                    let (stamp, body) = state.notice().unwrap_or_else(|| {
                        (
                            format!("Updated at {}", state.last_activity.format("%H:%M")),
                            "Nothing has been asked yet. Whatever the engine does will be reported here."
                                .to_string(),
                        )
                    });
                    theme::card(ui, theme::ELEVATED, None, |ui| {
                        ui.label(RichText::new(stamp).size(12.0).color(theme::FAINT));
                        ui.add_space(4.0);
                        ui.label(RichText::new(body).size(13.0).color(theme::TEXT));
                    });
                    ui.add_space(16.0);

                    // Subagents: the delegations this session started.
                    let delegations = state.delegations();
                    if section_header(ui, "Subagents", delegations.len(), "subagents") {
                        for delegation in &delegations {
                            let label = format!("{} · {}", delegation.agent, delegation.task);
                            let clicked = theme::list_row(ui, false, false, |ui| {
                                theme::avatar(ui, &delegation.agent, 26.0);
                                ui.add_space(9.0);
                                ui.add(egui::Label::new(RichText::new(label).size(13.0).color(theme::TEXT)).truncate());
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    // Only work that is still out says anything here:
                                    // a finished delegation needs no badge to be done.
                                    if !delegation.done {
                                        icons::icon(ui, Icon::Ring, theme::ACCENT, 13.0);
                                    }
                                });
                            });
                            if clicked.on_hover_text("open this delegation").clicked() {
                                reveal = Some(delegation.call_id.clone());
                            }
                        }
                        ui.add_space(16.0);
                    }

                    // Background processes: the engine's commands, and the user's.
                    // Every shell keeps a row whether or not it is busy, so the
                    // list never empties out from under the reader.
                    let processes = state.processes();
                    if section_header(ui, "Background processes", processes.len(), "processes") {
                        for process in &processes {
                            let clicked = theme::list_row(ui, false, false, |ui| {
                                icons::tile(
                                    ui,
                                    Icon::Terminal,
                                    if process.running { theme::ACCENT } else { theme::DIM },
                                    26.0,
                                    theme::ELEVATED,
                                    7,
                                );
                                ui.add_space(9.0);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&process.command).size(12.5).monospace().color(theme::TEXT),
                                    )
                                    .truncate(),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if process.running {
                                        theme::spinner(ui, 12.0, theme::ACCENT);
                                        ui.add_space(6.0);
                                        ui.label(RichText::new(&process.state).size(12.0).color(theme::ACCENT));
                                    } else {
                                        ui.label(RichText::new(&process.state).size(12.0).color(theme::FAINT));
                                    }
                                });
                            });
                            if clicked.on_hover_text("open what this row is running").clicked() {
                                match &process.source {
                                    ProcessSource::Tool(call) => reveal = Some(call.clone()),
                                    ProcessSource::Shell(index) => open_shell = Some(*index),
                                }
                            }
                        }
                        ui.add_space(16.0);
                    }

                    if delegations.is_empty() && processes.is_empty() {
                        ui.label(
                            RichText::new(
                                "Agents the engine hands work to, and the commands it runs, appear here as they start.",
                            )
                            .size(12.5)
                            .color(theme::FAINT),
                        );
                    }
                });
        });

    if let Some(call) = reveal {
        state.reveal(call);
    }
    if let Some(index) = open_shell {
        state.active_shell = index;
        state.go(Pane::Terminal, SidebarSpot::Task);
    }
}

/// The live card at the top of the panel: what the turn is doing, and for how
/// long.
///
/// The transcript has a copy of this at its end, because the conversation is
/// what the reader watches; this one is always in the same place, so the panel
/// answers "is it still working?" without being scrolled. The elapsed count is
/// the honest part: it moves on its own, which is what tells a wait from a hang.
///
/// @param ui the interface to draw into
/// @param activity what the session is doing
/// @param model the route the engine is running on
fn activity_card(ui: &mut Ui, activity: &crate::state::Activity, model: &str) {
    let waiting = matches!(
        activity.headline.as_str(),
        "waiting for your approval" | "waiting for the model"
    );
    theme::card(
        ui,
        theme::ACCENT_DIM.gamma_multiply(0.4),
        Some(theme::ACCENT.gamma_multiply(0.35)),
        |ui| {
            ui.horizontal(|ui| {
                if waiting {
                    icons::icon(ui, Icon::Sparkle, theme::AMBER, 14.0);
                } else {
                    theme::spinner(ui, 14.0, theme::ACCENT);
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new(&activity.headline)
                        .size(13.0)
                        .color(theme::TEXT)
                        .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(elapsed) = activity.elapsed {
                        ui.label(
                            RichText::new(crate::state::elapsed_label(elapsed))
                                .size(12.0)
                                .monospace()
                                .color(theme::DIM),
                        )
                        .on_hover_text("how long this turn has been running");
                    }
                });
            });
            if let Some(detail) = &activity.detail {
                ui.add_space(5.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(detail)
                            .size(11.5)
                            .monospace()
                            .color(theme::DIM),
                    )
                    .truncate(),
                );
            }
            ui.add_space(5.0);
            ui.label(RichText::new(model).size(11.0).color(theme::FAINT));
        },
    );
}

/// Which route the engine is running on, as the activity card names it.
///
/// @param state the application state
/// @returns the model, and the engine when it is named
pub(crate) fn model_label(state: &HarnessState) -> String {
    let model = state
        .agent
        .applied_model
        .clone()
        .or_else(|| state.active_route.clone())
        .unwrap_or_else(|| "no model route".to_string());
    let engine = &state.agent.conversation.engine_name;
    if engine.is_empty() {
        format!("on {model}")
    } else {
        format!("on {model} · via {engine}")
    }
}

/// A collapsible section heading in the panel.
///
/// @param ui the interface to draw into
/// @param label the section's name
/// @param count how many rows it holds
/// @param salt a stable id for the openness memory
/// @returns whether the section is open
fn section_header(ui: &mut Ui, label: &str, count: usize, salt: &str) -> bool {
    let id = ui.id().with(salt);
    let mut open = ui.ctx().data_mut(|data| *data.get_temp_mut_or(id, true));
    ui.horizontal(|ui| {
        let response = ui
            .add(
                egui::Label::new(RichText::new(label).size(13.5).color(theme::TEXT).strong())
                    .sense(Sense::click()),
            )
            .on_hover_text("show or hide this section");
        if count > 0 {
            ui.label(
                RichText::new(count.to_string())
                    .size(11.5)
                    .monospace()
                    .color(theme::FAINT),
            );
        }
        let chevron = icons::button(ui, Icon::Chevron, theme::FAINT, 18.0, theme::HOVER_SOFT);
        if response.clicked() || chevron.clicked() {
            open = !open;
        }
    });
    ui.ctx().data_mut(|data| data.insert_temp(id, open));
    ui.add_space(2.0);
    open
}

/// The help button in the corner: a black disc with a question mark.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param area the workspace rectangle
fn fab(state: &mut HarnessState, ui: &mut Ui, area: Rect) {
    let size = 44.0;
    let rect = Rect::from_min_size(
        egui::pos2(area.right() - size - 22.0, area.bottom() - size - 20.0),
        Vec2::splat(size),
    );
    let response = ui.interact(rect, ui.id().with("help-fab"), Sense::click());
    let painter = ui.painter();
    let fill = if response.hovered() {
        Color32::from_rgb(0x3A, 0x3A, 0x3A)
    } else {
        theme::TEXT
    };
    painter.add(
        theme::card_shadow().as_shape(rect, CornerRadius::same(theme::static_radius(size / 2.0))),
    );
    painter.circle_filled(rect.center(), size / 2.0, fill);
    icons::paint(ui, Icon::Question, rect.shrink(14.0), theme::PANEL);
    if response.on_hover_text("keys").clicked() {
        state.help_open = true;
    }
}

/// The transient message, drawn where the news is: over the conversation.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param card the conversation card's rectangle
fn toast(state: &HarnessState, ui: &mut Ui, card: Rect) {
    let Some((message, age)) = state.current_toast() else {
        return;
    };
    let alpha = if age.as_secs_f32() > 5.0 { 0.5 } else { 1.0 };
    let painter = ui.painter();
    let galley = painter.layout(
        message.to_string(),
        theme::mono(12.0),
        theme::PANEL,
        (card.width() - 160.0).max(80.0),
    );
    let size = galley.size() + Vec2::new(24.0, 14.0);
    let radius = theme::static_radius(size.y / 2.0);
    let rect = Rect::from_center_size(
        egui::pos2(card.center().x, card.bottom() - 26.0 - size.y / 2.0),
        size,
    );
    painter.add(theme::card_shadow().as_shape(rect, CornerRadius::same(radius)));
    painter.rect_filled(
        rect,
        CornerRadius::same(radius),
        theme::TEXT.gamma_multiply(alpha * 0.92),
    );
    painter.galley(rect.min + Vec2::new(12.0, 7.0), galley, theme::PANEL);
}
