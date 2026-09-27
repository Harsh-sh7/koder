//! The window's chrome: the sidebar, the floating cards, and the footer.
//!
//! The layout is the reference client's: a gray chrome with a task sidebar on
//! the left — the window's own traffic lights sit in its top corner, beside the
//! sidebar toggle and the back and forward arrows — a white conversation card
//! floating in the middle with its composer and a footer underneath, and a
//! second white card on the right for what the engine is doing: its
//! delegations, the skills and servers it can use, what it produced, and what
//! it is running.
//!
//! Nothing here decides anything. The sidebar's rows set the current view and
//! open tasks from the history; the panel's rows open the tool call they name;
//! every number drawn is read from the session in [`HarnessState`], so the
//! chrome cannot disagree with the transcript it frames.

use eframe::egui::{self, Align, Color32, CornerRadius, Layout, Rect, RichText, Sense, Ui, Vec2};

use harness_core::agent::RunState;

use crate::app::{compact, shorten};
use crate::icons::{self, Icon};
use crate::state::{
    Dialog, FocusRequest, HarnessState, Palette, PaletteMode, Pane, ProcessSource, SidebarSpot,
};
use crate::theme;

/// Height of the strip the window's traffic lights and the navigation sit in.
pub const CHROME: f32 = 44.0;

/// Where controls may start beside the traffic lights: macOS draws its window
/// buttons into the content, other platforms keep a title bar of their own.
pub const TRAFFIC_LIGHTS: f32 = if cfg!(target_os = "macos") { 80.0 } else { 12.0 };

/// Lets the window be dragged, and zoomed with a double click, by a strip the
/// app draws itself — the title bar is gone, so something has to stand in.
///
/// Registered before the controls on the strip, so they sit above it and keep
/// their clicks.
///
/// @param ui the interface to draw into
/// @param rect the strip
/// @param salt a stable id for the strip
pub fn drag_strip(ui: &mut Ui, rect: Rect, salt: &str) {
    let response = ui.interact(rect, ui.id().with(salt), Sense::click_and_drag());
    if response.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if response.double_clicked() {
        let maximized = ui.ctx().input(|input| input.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    }
}

/// The sidebar toggle and the back and forward arrows.
///
/// The same three controls sit in the sidebar's top strip, or at the start of
/// the card's title row once the sidebar is retracted, so the way back to it
/// never disappears with it.
///
/// @param state the application state
/// @param ui the interface to draw into
pub fn nav_buttons(state: &mut HarnessState, ui: &mut Ui) {
    ui.spacing_mut().item_spacing.x = 2.0;
    let toggle = icons::button(ui, Icon::Panel, theme::DIM, 26.0, theme::HOVER);
    if toggle
        .on_hover_text(if state.sidebar_open {
            "hide the sidebar ⌘\\"
        } else {
            "show the sidebar ⌘\\"
        })
        .clicked()
    {
        state.sidebar_open = !state.sidebar_open;
        state.save_config();
    }
    let back = state.can_go_back();
    let colour = |enabled: bool| if enabled { theme::DIM } else { theme::FAINT.gamma_multiply(0.6) };
    let left = icons::button(ui, Icon::ArrowLeft, colour(back), 26.0, theme::HOVER);
    if left.on_hover_text("back ⌘[").clicked() && back {
        state.navigate(false);
    }
    let forward = state.can_go_forward();
    let right = icons::button(ui, Icon::ArrowRight, colour(forward), 26.0, theme::HOVER);
    if right.on_hover_text("forward ⌘]").clicked() && forward {
        state.navigate(true);
    }
}

/// The sidebar: navigation, the mode switch, the task history, the tools, and
/// who is working.
///
/// In a narrow window the sidebar folds into a rail of marks: every row keeps
/// its click and names itself on hover, so folding costs width and nothing else.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param rail whether the sidebar is folded into an icon rail
pub fn sidebar(state: &mut HarnessState, ui: &mut Ui, rail: bool) {
    let full = ui.max_rect();
    drag_strip(ui, Rect::from_min_size(full.min, Vec2::new(full.width(), CHROME)), "sidebar-drag");
    if rail {
        ui.add_space(CHROME);
        ui.vertical_centered(|ui| {
            let toggle = icons::button(ui, Icon::Panel, theme::DIM, 28.0, theme::HOVER);
            if toggle.on_hover_text("hide the sidebar ⌘\\").clicked() {
                state.sidebar_open = false;
                state.save_config();
            }
        });
    } else {
        theme::inside(
            ui,
            Rect::from_min_max(
                egui::pos2(full.left() + TRAFFIC_LIGHTS, full.top() + 9.0),
                egui::pos2(full.right() - 8.0, full.top() + CHROME - 9.0),
            ),
            |ui| ui.horizontal_centered(|ui| nav_buttons(state, ui)),
        );
        ui.add_space(CHROME + 2.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            mode_switch(state, ui);
        });
    }

    ui.add_space(12.0);
    let mut go_spot: Option<(Pane, SidebarSpot)> = None;
    let mut new_task = false;

    if row(ui, Icon::NewTask, "New Task", false, false, rail)
        .on_hover_text("start a new task ⌘N")
        .clicked()
    {
        new_task = true;
    }
    if row(ui, Icon::Search, "Search", false, false, rail)
        .on_hover_text("search file contents ⌘⇧F")
        .clicked()
    {
        state.palette = Some(Palette::new(PaletteMode::Contents));
        state.focus = Some(FocusRequest::Palette);
    }

    ui.add_space(12.0);
    if !rail {
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(RichText::new("Workspaces").size(12.5).color(theme::FAINT));
        });
    }
    ui.add_space(2.0);
    let workspace = state
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| state.root.display().to_string());
    if row(ui, Icon::Harness, &workspace, false, false, rail)
        .on_hover_text(format!(
            "{} — click to open a different folder (⌘O)",
            state.root.display()
        ))
        .clicked()
    {
        state.open_folder_dialog();
    }

    // The bottom block is laid out from the bottom up first, so the task
    // history knows exactly how much room it has and scrolls within it rather
    // than pushing the tools out of the window.
    let bottom_height = if rail { 4.0 * 30.0 + 52.0 } else { 4.0 * 30.0 + 56.0 };
    let history_height = (ui.available_height() - bottom_height - 8.0).max(0.0);
    let history_rect = Rect::from_min_size(
        ui.cursor().min,
        Vec2::new(ui.available_width(), history_height),
    );
    theme::inside(ui, history_rect, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("task-history")
            .max_height(history_height)
            .auto_shrink([false, false])
            .show(ui, |ui| task_rows(state, ui, rail));
    });
    ui.advance_cursor_after_rect(history_rect);

    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
        ui.add_space(10.0);
        profile(state, ui, rail);
        ui.add_space(6.0);
        for (spot, icon, label, hint, pane) in [
            (
                SidebarSpot::Extensions,
                Icon::Grid,
                "Models & keys",
                "model routes, providers and API keys (⌘,)",
                None,
            ),
            (
                SidebarSpot::Automations,
                Icon::Gauge,
                "Usage",
                "tokens, cost and everything the engine ran",
                Some(Pane::Observability),
            ),
            (
                SidebarSpot::Sites,
                Icon::Folder,
                "Files",
                "browse and edit this workspace's files",
                Some(Pane::Editor),
            ),
            (
                SidebarSpot::Knowledge,
                Icon::Book,
                "Workflows",
                "saved prompts, commands and notes",
                Some(Pane::Workflows),
            ),
        ] {
            let selected = state.spot == spot;
            if row(ui, icon, label, selected, false, rail)
                .on_hover_text(hint)
                .clicked()
            {
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

/// The workspace's tasks: the one on screen when it is new, then the history,
/// newest first. A row opens its task; its menu renames or deletes it.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param rail whether the sidebar is folded
fn task_rows(state: &mut HarnessState, ui: &mut Ui, rail: bool) {
    let mut open: Option<String> = None;
    let mut rename: Option<(String, String)> = None;
    let mut delete: Option<String> = None;
    let mut start_rename: Option<(String, String)> = None;
    let busy = state.agent.conversation.state.busy();

    // A task that has not been asked anything yet is not in the history, but
    // it is where the user is; it gets a row until its first prompt saves it.
    if state.current_task.is_none() {
        let title = shorten(&state.session_title(), 40);
        let response = row(ui, Icon::Sparkle, &title, state.spot == SidebarSpot::Task, true, rail);
        if response.on_hover_text("the task on screen").clicked() {
            state.go(Pane::Conversation, SidebarSpot::Task);
        }
    }

    let tasks = state.tasks.clone();
    for task in &tasks {
        let current = state.current_task.as_deref() == Some(task.id.as_str());
        if let Some((id, text)) = state.renaming.as_mut().filter(|(id, _)| *id == task.id) {
            ui.horizontal(|ui| {
                ui.add_space(if rail { 4.0 } else { 30.0 });
                let edit = ui.add(
                    egui::TextEdit::singleline(text)
                        .desired_width(ui.available_width() - 12.0)
                        .font(theme::text(0.0)),
                );
                if !edit.has_focus() && !edit.lost_focus() {
                    edit.request_focus();
                }
                let done = edit.lost_focus();
                let submitted = done && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if submitted {
                    rename = Some((id.clone(), text.clone()));
                } else if done {
                    rename = Some((id.clone(), String::new()));
                }
            });
            continue;
        }
        let spinning = current && busy;
        let response = task_row(ui, &task.title, current && state.spot == SidebarSpot::Task, spinning, rail);
        let hovered = response.hovered();
        let clicked = response.clicked();
        let secondary = response.secondary_clicked();
        let response = response.on_hover_text(format!("{}\nlast active {}", task.title, when(&task.updated)));
        // The menu: the row's own ⋯ on hover, or a right click anywhere on it.
        let menu_id = ui.id().with(("task-menu", &task.id));
        if !rail && (hovered || egui::Popup::is_id_open(ui.ctx(), menu_id)) {
            let dots = Rect::from_center_size(
                egui::pos2(response.rect.right() - 20.0, response.rect.center().y),
                Vec2::splat(22.0),
            );
            let more = ui.interact(dots, ui.id().with(("task-more", &task.id)), Sense::click());
            if more.hovered() {
                ui.painter()
                    .rect_filled(dots, CornerRadius::same(5), theme::ACTIVE);
            }
            icons::paint(ui, Icon::More, dots.shrink(4.0), theme::DIM);
            if more.clicked() {
                egui::Popup::toggle_id(ui.ctx(), menu_id);
            }
        } else if clicked {
            open = Some(task.id.clone());
        }
        if clicked && !rail && response.hover_pos().is_some_and(|pos| pos.x < response.rect.right() - 34.0) {
            open = Some(task.id.clone());
        }
        if secondary {
            egui::Popup::open_id(ui.ctx(), menu_id);
        }
        egui::Popup::new(menu_id, ui.ctx().clone(), response.rect, ui.layer_id())
            .open_memory(None)
            .show(|ui| {
                ui.set_min_width(170.0);
                if ui.button("Open").clicked() {
                    open = Some(task.id.clone());
                    ui.close();
                }
                if ui.button("Rename").clicked() {
                    start_rename = Some((task.id.clone(), task.title.clone()));
                    ui.close();
                }
                if ui.button("Copy task id").clicked() {
                    ui.ctx().copy_text(task.id.clone());
                    ui.close();
                }
                ui.separator();
                if ui
                    .button(RichText::new("Delete").color(theme::RED))
                    .clicked()
                {
                    delete = Some(task.id.clone());
                    ui.close();
                }
            });
    }
    if tasks.is_empty() && state.current_task.is_none() && !rail {
        ui.horizontal(|ui| {
            ui.add_space(32.0);
            ui.label(
                RichText::new("tasks you start are kept here")
                    .size(11.5)
                    .color(theme::FAINT),
            );
        });
    }

    if let Some(pair) = start_rename {
        state.renaming = Some(pair);
    }
    if let Some((id, title)) = rename {
        state.renaming = None;
        if !title.trim().is_empty() {
            state.rename_task(&id, &title);
        }
    }
    if let Some(id) = delete {
        state.delete_task(&id);
    }
    if let Some(id) = open {
        state.open_task(&id);
    }
}

/// When a task was last active, in the words a history row uses.
///
/// @param stamp RFC 3339 time
/// @returns e.g. `today 14:02`, `yesterday`, `Sep 21`
fn when(stamp: &str) -> String {
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(stamp) else {
        return stamp.to_string();
    };
    let at = at.with_timezone(&chrono::Local);
    let today = chrono::Local::now().date_naive();
    match (today - at.date_naive()).num_days() {
        0 => format!("today {}", at.format("%H:%M")),
        1 => "yesterday".to_string(),
        _ => at.format("%b %-d").to_string(),
    }
}

/// One history row: indented under its workspace, with a live spinner while
/// its turn runs.
///
/// @param ui the interface to draw into
/// @param title the task's name
/// @param selected whether it is the task on screen
/// @param spinning whether its turn is running
/// @param rail whether the sidebar is folded
/// @returns the row's response
fn task_row(ui: &mut Ui, title: &str, selected: bool, spinning: bool, rail: bool) -> egui::Response {
    if rail {
        return theme::list_row(ui, selected, true, |ui| {
            ui.add_space(((ui.available_width() - 15.0) / 2.0).max(0.0));
            if spinning {
                theme::spinner(ui, 15.0, theme::ACCENT);
            } else {
                icons::icon(ui, Icon::Sparkle, if selected { theme::TEXT } else { theme::FAINT }, 15.0);
            }
        });
    }
    // Aligned with the rows above it: the same indent, a mark-sized slot (a
    // spinner while the task works), and the same gap before the name.
    theme::list_row(ui, selected, true, |ui| {
        ui.add_space(22.0);
        if spinning {
            theme::spinner(ui, 17.0, theme::ACCENT);
        } else {
            ui.add_space(17.0);
        }
        ui.add_space(10.0);
        ui.scope(|ui| {
            ui.set_max_width((ui.available_width() - 26.0).max(20.0));
            ui.add(
                egui::Label::new(RichText::new(title).size(13.5).color(if selected {
                    theme::TEXT
                } else {
                    theme::DIM
                }))
                .truncate()
                .selectable(false),
            );
        });
    })
}

/// The two-part switch at the top of the sidebar: the coding mode, which is
/// the conversation, and the terminal.
///
/// @param state the application state
/// @param ui the interface to draw into
fn mode_switch(state: &mut HarnessState, ui: &mut Ui) {
    let height = 30.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(116.0 + 4.0 + 34.0, height), Sense::hover());
    let painter = ui.painter().clone();
    let capsule = theme::static_radius(height / 2.0);
    painter.rect_filled(rect, CornerRadius::same(capsule), theme::ACTIVE.gamma_multiply(0.55));

    let segment = theme::static_radius((height - 6.0) / 2.0);
    let coding_rect = Rect::from_min_size(rect.min + Vec2::new(3.0, 3.0), Vec2::new(112.0, height - 6.0));
    let terminal_rect = Rect::from_min_size(
        egui::pos2(coding_rect.right() + 4.0, rect.top() + 3.0),
        Vec2::splat(height - 6.0),
    );
    let coding = ui.interact(coding_rect, ui.id().with("mode-coding"), Sense::click());
    let terminal = ui.interact(terminal_rect, ui.id().with("mode-terminal"), Sense::click());
    let on_terminal = state.pane == Pane::Terminal;

    for (segment_rect, response, selected) in [
        (coding_rect, &coding, !on_terminal),
        (terminal_rect, &terminal, on_terminal),
    ] {
        if selected {
            painter.add(theme::card_shadow().as_shape(segment_rect, CornerRadius::same(segment)));
            painter.rect_filled(segment_rect, CornerRadius::same(segment), theme::PANEL);
        } else if response.hovered() {
            painter.rect_filled(segment_rect, CornerRadius::same(segment), theme::HOVER);
        }
    }
    let inset = coding_rect.shrink2(Vec2::new(10.0, 0.0));
    let mark = Rect::from_min_size(egui::pos2(inset.left(), inset.center().y - 8.0), Vec2::splat(16.0));
    icons::paint(ui, Icon::Doc, mark, if on_terminal { theme::DIM } else { theme::TEXT });
    painter.text(
        egui::pos2(mark.right() + 7.0, inset.center().y),
        egui::Align2::LEFT_CENTER,
        "Coding",
        theme::text(0.0),
        if on_terminal { theme::DIM } else { theme::TEXT },
    );
    icons::paint(
        ui,
        Icon::Terminal,
        terminal_rect.shrink(5.0),
        if on_terminal { theme::TEXT } else { theme::DIM },
    );
    if coding.on_hover_text("the conversation ⌘1").clicked() {
        state.go(Pane::Conversation, SidebarSpot::Task);
    }
    if terminal.on_hover_text("the terminal ⌘2").clicked() {
        state.go(Pane::Terminal, SidebarSpot::Task);
    }
}

/// The bottom row: who is working, and the two buttons beside the name.
///
/// Folded into the rail, the row is the avatar alone, and its menu carries the
/// history and settings entries the two buttons would otherwise hold.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param rail whether the sidebar is folded into an icon rail
fn profile(state: &mut HarnessState, ui: &mut Ui, rail: bool) {
    let name = user_name();
    let mut menu: Option<ProfileAction> = None;
    ui.horizontal(|ui| {
        ui.add_space(if rail { (ui.available_width() - 26.0) / 2.0 } else { 14.0 });
        let avatar = theme::avatar(ui, &name, 26.0);
        let who = if rail {
            avatar
                .interact(Sense::click())
                .on_hover_text(format!("{name} — click for the workspace menu"))
        } else {
            ui.add_space(8.0);
            // The two buttons on the right are laid out after the name, so the
            // name's budget has to leave them room or it runs underneath them.
            ui.scope(|ui| {
                ui.set_max_width((ui.available_width() - 72.0).max(24.0));
                ui.add(
                    egui::Label::new(RichText::new(&name).size(13.5).color(theme::TEXT))
                        .truncate()
                        .selectable(false)
                        .sense(Sense::click()),
                )
            })
            .inner
            .on_hover_text("the workspace this session is in — click for what you can do with it")
        };
        egui::Popup::menu(&who).show(|ui| {
            ui.set_min_width(230.0);
            if ui.button("Command palette  ⌘K").clicked() {
                menu = Some(ProfileAction::Palette);
                ui.close();
            }
            if ui.button("Keys").clicked() {
                menu = Some(ProfileAction::Keys);
                ui.close();
            }
            ui.separator();
            if ui.button("Open folder…  ⌘O").clicked() {
                menu = Some(ProfileAction::Folder);
                ui.close();
            }
            if ui.button("Copy the workspace path").clicked() {
                menu = Some(ProfileAction::CopyPath);
                ui.close();
            }
            if ui.button("Show the file tree  ⌘B").clicked() {
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
        if rail {
            return;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(10.0);
            let gear = icons::button(ui, Icon::Gear, theme::DIM, 24.0, theme::HOVER);
            if gear.on_hover_text("settings: models, routes and keys ⌘,").clicked() {
                menu = Some(ProfileAction::Models);
            }
            ui.add_space(2.0);
            let history = icons::button(ui, Icon::Clock, theme::DIM, 24.0, theme::HOVER)
                .on_hover_text("recent tasks");
            egui::Popup::menu(&history).show(|ui| {
                ui.set_min_width(260.0);
                if state.tasks.is_empty() {
                    ui.label(RichText::new("no saved tasks yet").size(12.0).color(theme::FAINT));
                }
                for task in state.tasks.iter().take(12) {
                    if ui
                        .button(shorten(&task.title, 40))
                        .on_hover_text(when(&task.updated))
                        .clicked()
                    {
                        menu = Some(ProfileAction::Open(task.id.clone()));
                        ui.close();
                    }
                }
            });
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
        Some(ProfileAction::Palette) => {
            state.palette = Some(Palette::new(PaletteMode::Commands));
            state.focus = Some(FocusRequest::Palette);
        }
        Some(ProfileAction::Keys) => state.help_open = true,
        Some(ProfileAction::Open(id)) => state.open_task(&id),
        None => {}
    }
}

/// What the profile row asked for.
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
    /// Open the command palette.
    Palette,
    /// Show the key reference.
    Keys,
    /// Open a task from the recent list.
    Open(String),
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
/// @param rail whether the sidebar is folded, so the row is its mark alone
/// @returns the response of the row
fn row(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    selected: bool,
    indent: bool,
    rail: bool,
) -> egui::Response {
    if rail {
        let colour = if selected { theme::TEXT } else { theme::DIM };
        return theme::list_row(ui, selected, true, |ui| {
            ui.add_space(((ui.available_width() - 17.0) / 2.0).max(0.0));
            icons::icon(ui, icon, colour, 17.0);
        })
        .on_hover_text(label);
    }
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

/// How far above the card's bottom a floating session panel stops: the
/// composer's band (field, controls, and its margins) stays uncovered.
const COMPOSER_CLEARANCE: f32 = 150.0;

/// The workspace area of the window: the two floating cards, the footer beneath
/// the conversation, and the help button.
///
/// The session panel docks beside the conversation card while the card keeps
/// [`theme::CARD_MIN`] of width; below that it floats over the card's right
/// edge instead, so opening the panel never crushes the conversation.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param panel_width how wide the session panel is drawn at this window size
pub fn workspace(state: &mut HarnessState, ui: &mut Ui, panel_width: f32) {
    let area = ui.max_rect();
    let top = area.top() + 10.0;
    let bottom = area.bottom() - 8.0;
    let gutter = theme::GUTTER;
    let right = area.right() - gutter;
    let panel_gap = 12.0;
    let footer_height = 26.0;

    // The strip above the cards is the window's title bar now.
    drag_strip(
        ui,
        Rect::from_min_max(area.min, egui::pos2(area.right(), top)),
        "workspace-drag",
    );

    let docked = state.agent_open
        && right - panel_width - panel_gap - (area.left() + gutter) >= theme::CARD_MIN;
    // Floating, the panel is never wider than most of the card it covers.
    let panel_width = if docked {
        panel_width
    } else {
        panel_width.min((area.width() - 2.0 * gutter) * 0.85)
    };
    // Floating, the panel also stops above the composer: it may cover the
    // transcript, but never the field and the send button beneath it.
    let panel_bottom = if docked {
        bottom
    } else {
        (bottom - footer_height - COMPOSER_CLEARANCE).max(top + 160.0)
    };
    // Floating, it also starts below the card's title row, whose toolbar
    // holds the toggle that closes it.
    let panel_top = if docked { top } else { top + 50.0 };
    let panel_rect = Rect::from_min_max(
        egui::pos2(right - panel_width, panel_top),
        egui::pos2(right, panel_bottom),
    );
    let card_right = if docked {
        right - panel_width - panel_gap
    } else {
        right
    };
    let card_rect = Rect::from_min_max(
        egui::pos2(area.left() + gutter, top),
        egui::pos2(card_right, bottom - footer_height),
    );

    theme::surface(ui, card_rect, theme::PANEL, theme::CARD_RADIUS);
    theme::inside(ui, card_rect.shrink(1.0), |ui| {
        ui.set_clip_rect(card_rect);
        crate::panes::card::show(state, ui);
    });

    // The help button sits in the panel's corner when the panel is docked. With
    // no docked panel that corner is the composer's send button, so the button
    // moves to the end of the footer strip, under the card, where it covers
    // nothing.
    let fab_in_footer = !docked;
    let footer = Rect::from_min_max(
        egui::pos2(card_rect.left() + 14.0, card_rect.bottom() + 1.0),
        egui::pos2(
            card_rect.right() - if fab_in_footer { 44.0 } else { 14.0 },
            bottom,
        ),
    );
    theme::inside(ui, footer, |ui| footer_row(state, ui));

    if state.agent_open {
        if !docked {
            // A floating panel is a layer over the card: anything under it
            // must not also take its clicks.
            ui.interact(panel_rect, ui.id().with("panel-overlay"), Sense::click_and_drag());
        }
        theme::surface(ui, panel_rect, theme::PANEL, theme::CARD_RADIUS);
        theme::inside(ui, panel_rect.shrink(1.0), |ui| {
            ui.set_clip_rect(panel_rect);
            panel(state, ui, !fab_in_footer);
        });
    }

    let fab_rect = if fab_in_footer {
        Rect::from_center_size(
            egui::pos2(card_rect.right() - 18.0, card_rect.bottom() + footer_height / 2.0 + 1.0),
            Vec2::splat(24.0),
        )
    } else {
        let size = 40.0;
        Rect::from_min_size(
            egui::pos2(panel_rect.right() - size - 14.0, panel_rect.bottom() - size - 12.0),
            Vec2::splat(size),
        )
    };
    fab(state, ui, fab_rect);
    toast(state, ui, card_rect);
}

/// The footer under the conversation: the folder, where it runs, the branch,
/// and how full the context window is.
///
/// @param state the application state
/// @param ui the interface to draw into
fn footer_row(state: &mut HarnessState, ui: &mut Ui) {
    let name = state
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| state.root.display().to_string());
    // The git view's last status carries the branch; asking the repository
    // only when that has not been read yet keeps the footer off the disk.
    let branch = Some(state.git_view.status.branch.clone())
        .filter(|name| !name.is_empty())
        .or_else(|| state.git.as_ref().map(|repo| repo.branch_name()))
        .filter(|name| !name.is_empty());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        // Status only: opening another folder is the composer's folder mark
        // now, so this chip is not a second, differently-shaped way to do it.
        chip(ui, Icon::Folder, &shorten(&name, 28)).on_hover_text(state.root.display().to_string());
        ui.add_space(8.0);
        let local = chip(ui, Icon::Window, "Local");
        let engine = running_engine(state);
        local.on_hover_text(match engine {
            Some(engine) => format!("the agent runs on this machine · {engine}"),
            None => "the agent runs on this machine; its engine starts with the first prompt".to_string(),
        });
        if let Some(branch) = &branch {
            ui.add_space(8.0);
            if chip(ui, Icon::Branch, &shorten(branch, 24))
                .on_hover_text("the branch — click for the git view")
                .clicked()
            {
                state.go(Pane::Git, SidebarSpot::Task);
            }
        }
        // The meter takes the rest of this row from the right. It has to be laid
        // out inside the same `horizontal`: a sibling layout would start its own
        // row below, past the bottom of the window.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let meter = &state.agent.conversation.meter;
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
                let response = theme::hatch_meter(ui, fraction, 64.0, colour);
                response.on_hover_text(format!(
                    "context window: {} of {} tokens used",
                    compact(meter.used),
                    compact(meter.size)
                ));
            }
        });
    });
}

/// A footer chip: a mark and a word, clickable.
///
/// @param ui the interface to draw into
/// @param icon the mark
/// @param label the word
/// @returns the chip's response
fn chip(ui: &mut Ui, icon: Icon, label: &str) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::text(-1.0), theme::DIM);
    let size = Vec2::new(14.0 + 6.0 + galley.size().x + 12.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(5), theme::HOVER);
    }
    icons::paint(
        ui,
        icon,
        Rect::from_center_size(egui::pos2(rect.left() + 13.0, rect.center().y), Vec2::splat(14.0)),
        theme::DIM,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 26.0, rect.center().y - galley.size().y / 2.0),
        galley,
        theme::DIM,
    );
    response
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

/// The right card: what the session is doing, who it delegated to, what it can
/// be extended with, what it produced, and what it is running.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param fab_inside whether the help button floats in this card's corner, so
///        the list keeps room below its last row to scroll clear of it
fn panel(state: &mut HarnessState, ui: &mut Ui, fab_inside: bool) {
    ui.add_space(12.0);
    egui::Frame::new()
        .inner_margin(egui::Margin { left: 14, right: 12, top: 0, bottom: 0 })
        .show(ui, |ui| {
            let phase = SessionPhase::of(state);
            let colour = phase.colour();
            ui.horizontal(|ui| {
                match phase {
                    SessionPhase::Working => {
                        theme::spinner(ui, 15.0, colour);
                    }
                    _ => {
                        icons::icon(ui, phase.icon(), colour, 15.0);
                    }
                }
                ui.add_space(7.0);
                ui.label(RichText::new("Session").size(13.5).color(theme::TEXT).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let close = icons::button(ui, Icon::Cross, theme::FAINT, 20.0, theme::HOVER_SOFT);
                    if close.on_hover_text("hide this panel ⌘J").clicked() {
                        state.agent_open = false;
                        state.save_config();
                    }
                    ui.add_space(4.0);
                    theme::badge(ui, phase.label(), colour);
                });
            });
        });
    ui.add_space(10.0);

    let mut reveal: Option<String> = None;
    let mut open_shell: Option<usize> = None;
    let mut open_file: Option<String> = None;
    let mut use_skill: Option<String> = None;
    let mut stop = false;
    let mut open_mcp = false;
    let rest = ui.available_height();
    let busy = state.agent.conversation.state.busy();
    let activity = state.activity();
    let model = model_label(state);
    let notice = state.notice();
    let last_activity = state.last_activity;
    let phase = SessionPhase::of(state);
    let delegations = state.delegations();
    let artifacts = state.artifacts();
    let processes = state.processes();
    let (skills, mcp) = {
        let extensions = state.extensions();
        (extensions.skills.clone(), extensions.mcp.clone())
    };

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
                    if let Some(activity) = &activity {
                        if activity_card(ui, activity, &model) {
                            stop = true;
                        }
                        ui.add_space(14.0);
                    }

                    // The notice: when the session last moved, and what it said.
                    // Headed by the outcome in words ("Finished at 21:07"), not a
                    // bare timestamp, and kept to a few lines: the transcript
                    // holds the whole answer; this is the glance.
                    let (_, body) = notice.unwrap_or_else(|| {
                        (
                            String::new(),
                            "Ask for a change or describe what to build. What the agent does, who it hands work to, and what it builds shows up here."
                                .to_string(),
                        )
                    });
                    let body = crate::app::shorten_words(&body, 190);
                    let stamp = match phase {
                        SessionPhase::Ready => "Ready".to_string(),
                        SessionPhase::Working => format!("Working · since {}", last_activity.format("%H:%M")),
                        SessionPhase::NeedsYou => "Waiting for your answer".to_string(),
                        SessionPhase::Done => format!("Finished at {}", last_activity.format("%H:%M")),
                        SessionPhase::Stopped => format!("Stopped at {}", last_activity.format("%H:%M")),
                        SessionPhase::Failed => format!("Failed at {}", last_activity.format("%H:%M")),
                    };
                    theme::card(ui, theme::ELEVATED, None, |ui| {
                        ui.label(RichText::new(stamp).size(12.0).color(phase.colour()));
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(RichText::new(body).size(13.0).color(theme::TEXT)).wrap());
                    });
                    ui.add_space(16.0);

                    // Subagents: every agent this task handed work to, with its
                    // state; a running one can be stopped with its turn.
                    let turn_live = state.agent.conversation.state.busy();
                    if section_header(ui, "Subagents", delegations.len(), None, "subagents") {
                        if delegations.is_empty() {
                            empty_line(ui, "Parts of a job the harness runs in parallel appear here.");
                        }
                        for delegation in &delegations {
                            let label = format!("{} · {}", delegation.agent, delegation.task);
                            let mut stop_row = false;
                            let clicked = theme::list_row(ui, false, false, |ui| {
                                theme::avatar(ui, &delegation.agent, 22.0);
                                ui.add_space(9.0);
                                ui.scope(|ui| {
                                    ui.set_max_width((ui.available_width() - 30.0).max(20.0));
                                    ui.add(
                                        egui::Label::new(RichText::new(label).size(12.5).color(theme::TEXT))
                                            .truncate(),
                                    );
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if !delegation.done && !turn_live {
                                        // Its turn is over and it never reported back:
                                        // nothing is running it any more.
                                        icons::icon(ui, Icon::Stop, theme::FAINT, 11.0)
                                            .on_hover_text("stopped — the turn ended before this part reported back");
                                    } else if !delegation.done {
                                        let stop_mark = icons::button(ui, Icon::Stop, theme::FAINT, 20.0, theme::HOVER_SOFT);
                                        if stop_mark
                                            .on_hover_text("stop — ends the turn this agent runs in")
                                            .clicked()
                                        {
                                            stop_row = true;
                                        }
                                        theme::spinner(ui, 12.0, theme::ACCENT);
                                    } else if delegation.failed {
                                        icons::icon(ui, Icon::Cross, theme::RED, 13.0);
                                    } else {
                                        icons::icon(ui, Icon::Check, theme::GREEN, 13.0);
                                    }
                                });
                            });
                            if stop_row {
                                stop = true;
                            } else if clicked.on_hover_text(&delegation.task).clicked() {
                                reveal = Some(delegation.call_id.clone());
                            }
                        }
                        ui.add_space(14.0);
                    }

                    // Skills and MCP servers: what a session can load and call.
                    let servers = mcp.as_ref().map(Vec::len).unwrap_or(0);
                    let suggestions = format!(
                        "{} {}",
                        skills.len() + servers,
                        if skills.len() + servers == 1 { "suggestion" } else { "suggestions" }
                    );
                    if section_header(ui, "Skills & MCP", 0, Some(&suggestions), "skills") {
                        for skill in &skills {
                            let clicked = theme::list_row(ui, false, false, |ui| {
                                icons::tile(ui, Icon::Book, theme::DIM, 22.0, theme::ELEVATED, 6);
                                ui.add_space(9.0);
                                ui.add(
                                    egui::Label::new(RichText::new(&skill.name).size(12.5).color(theme::TEXT))
                                        .truncate(),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(RichText::new(skill.source).size(11.0).color(theme::FAINT));
                                });
                            });
                            if clicked
                                .on_hover_text(format!("{}\n\nclick to ask the agent to use it", skill.description))
                                .clicked()
                            {
                                use_skill = Some(skill.name.clone());
                            }
                        }
                        match &mcp {
                            Ok(list) => {
                                for server in list {
                                    let clicked = theme::list_row(ui, false, false, |ui| {
                                        icons::tile(ui, Icon::Plug, theme::ACCENT, 22.0, theme::ACCENT_DIM, 6);
                                        ui.add_space(9.0);
                                        ui.add(
                                            egui::Label::new(RichText::new(&server.name).size(12.5).color(theme::TEXT))
                                                .truncate(),
                                        );
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            ui.label(RichText::new(server.transport).size(11.0).color(theme::FAINT));
                                        });
                                    });
                                    if clicked.on_hover_text(format!("{} · {}", server.transport, server.target)).clicked() {
                                        open_mcp = true;
                                    }
                                }
                                if servers == 0 {
                                    let clicked = theme::list_row(ui, false, false, |ui| {
                                        icons::tile(ui, Icon::Plug, theme::FAINT, 22.0, theme::ELEVATED, 6);
                                        ui.add_space(9.0);
                                        ui.label(RichText::new("Add an MCP server").size(12.5).color(theme::DIM));
                                    });
                                    if clicked
                                        .on_hover_text("declare servers in .harness/mcp.json (\"mcpServers\"); new sessions get them")
                                        .clicked()
                                    {
                                        open_mcp = true;
                                    }
                                }
                            }
                            Err(err) => {
                                ui.label(RichText::new(shorten(err, 90)).size(11.5).color(theme::RED));
                            }
                        }
                        ui.add_space(14.0);
                    }

                    // Artifacts: what this task built or changed.
                    if section_header(ui, "Artifact", artifacts.len(), None, "artifacts") {
                        if artifacts.is_empty() {
                            empty_line(ui, "Files the agent creates or edits in this task appear here.");
                        }
                        for artifact in &artifacts {
                            let name = artifact
                                .path
                                .rsplit(['/', '\\'])
                                .next()
                                .unwrap_or(&artifact.path)
                                .to_string();
                            let clicked = theme::list_row(ui, false, false, |ui| {
                                file_tile(ui, &name, 22.0);
                                ui.add_space(9.0);
                                ui.scope(|ui| {
                                    ui.set_max_width((ui.available_width() - 64.0).max(20.0));
                                    ui.add(
                                        egui::Label::new(RichText::new(&name).size(12.5).color(theme::TEXT))
                                            .truncate(),
                                    );
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if artifact.added == 0 && artifact.removed == 0 {
                                        ui.label(RichText::new("built").size(11.0).color(theme::FAINT));
                                        return;
                                    }
                                    if artifact.removed > 0 {
                                        ui.label(
                                            RichText::new(format!("-{}", artifact.removed))
                                                .size(11.0)
                                                .monospace()
                                                .color(theme::RED),
                                        );
                                    }
                                    ui.label(
                                        RichText::new(format!("+{}", artifact.added))
                                            .size(11.0)
                                            .monospace()
                                            .color(theme::GREEN),
                                    );
                                });
                            });
                            if clicked
                                .on_hover_text(format!("{} — click to open it", artifact.path))
                                .clicked()
                            {
                                open_file = Some(artifact.path.clone());
                            }
                        }
                        ui.add_space(14.0);
                    }

                    // Background processes: the engine's commands, and the user's.
                    // Every shell keeps a row whether or not it is busy, so the
                    // list never empties out from under the reader.
                    if section_header(ui, "Processes", processes.len(), None, "processes") {
                        for process in &processes {
                            let clicked = theme::list_row(ui, false, false, |ui| {
                                icons::tile(
                                    ui,
                                    Icon::Terminal,
                                    if process.running { theme::ACCENT } else { theme::DIM },
                                    22.0,
                                    theme::ELEVATED,
                                    6,
                                );
                                ui.add_space(9.0);
                                ui.scope(|ui| {
                                    ui.set_max_width((ui.available_width() - 80.0).max(20.0));
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&process.command).size(12.0).monospace().color(theme::TEXT),
                                        )
                                        .truncate(),
                                    );
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if process.running {
                                        theme::spinner(ui, 12.0, theme::ACCENT);
                                    } else {
                                        ui.label(RichText::new(&process.state).size(11.5).color(theme::FAINT));
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
                        ui.add_space(8.0);
                    }
                    if fab_inside {
                        ui.add_space(60.0);
                    }
                });
        });

    let _ = busy;
    if stop {
        state.stop_turn();
    }
    if let Some(call) = reveal {
        state.reveal(call);
    }
    if let Some(index) = open_shell {
        state.active_shell = index;
        state.go(Pane::Terminal, SidebarSpot::Task);
    }
    if let Some(path) = open_file {
        let path = if std::path::Path::new(&path).is_absolute() {
            std::path::PathBuf::from(path)
        } else {
            state.root.join(path)
        };
        state.open_file(&path);
        state.go(Pane::Editor, SidebarSpot::Task);
    }
    if let Some(name) = use_skill {
        let prefix = format!("Use the `{name}` skill: ");
        if !state.agent.input.starts_with(&prefix) {
            state.agent.input = format!("{prefix}{}", state.agent.input);
        }
        state.pane = Pane::Conversation;
        state.focus = Some(FocusRequest::Agent);
    }
    if open_mcp {
        let path = harness_core::extensions::mcp_path(&state.root);
        if !path.exists() {
            let template = "{\n  \"mcpServers\": {\n  }\n}\n";
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(err) = std::fs::write(&path, template) {
                state.toast(format!("could not create {}: {err}", path.display()));
            }
        }
        state.open_file(&path);
        state.go(Pane::Editor, SidebarSpot::Task);
        state.toast("declare servers under \"mcpServers\"; new sessions pick them up");
    }
}

/// A file's type mark: its extension on a tinted tile.
///
/// @param ui the interface to draw into
/// @param name the file's name
/// @param size the tile's side
/// @returns the tile's response
pub fn file_tile(ui: &mut Ui, name: &str, size: f32) -> egui::Response {
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_uppercase())
        .filter(|ext| ext.len() <= 4)
        .unwrap_or_else(|| "FILE".to_string());
    let tint = match extension.as_str() {
        "MD" | "TXT" => theme::BLUE,
        "RS" | "PY" | "TS" | "JS" | "GO" | "C" | "CPP" | "JAVA" => theme::VIOLET,
        "JSON" | "YML" | "YAML" | "TOML" => theme::AMBER,
        _ => theme::DIM,
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(5), tint.gamma_multiply(0.14));
    let font = egui::FontId::proportional((size * 0.34).clamp(7.0, 11.0));
    let label = if extension.len() > 3 { &extension[..3] } else { extension.as_str() };
    ui.painter()
        .text(rect.center(), egui::Align2::CENTER_CENTER, label, font, tint);
    response
}

/// A quiet line saying what a section will hold, when it holds nothing yet.
///
/// @param ui the interface to draw into
/// @param text the line
fn empty_line(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(theme::FAINT)).wrap());
    ui.add_space(2.0);
}

/// The live card at the top of the panel: what the turn is doing, for how long,
/// and the stop button.
///
/// The transcript has a copy of this at its end, because the conversation is
/// what the reader watches; this one is always in the same place, so the panel
/// answers "is it still working?" without being scrolled. The elapsed count is
/// the honest part: it moves on its own, which is what tells a wait from a hang.
///
/// @param ui the interface to draw into
/// @param activity what the session is doing
/// @param model the route the engine is running on
/// @returns true when stop was clicked
fn activity_card(ui: &mut Ui, activity: &crate::state::Activity, model: &str) -> bool {
    let waiting = matches!(
        activity.headline.as_str(),
        "waiting for your approval" | "waiting for the model"
    );
    let mut stop = false;
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
                ui.scope(|ui| {
                    ui.set_max_width((ui.available_width() - 70.0).max(40.0));
                    ui.add(
                        egui::Label::new(
                            RichText::new(&activity.headline)
                                .size(13.0)
                                .color(theme::TEXT)
                                .strong(),
                        )
                        .truncate(),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icons::button(ui, Icon::Stop, theme::DIM, 20.0, theme::HOVER_SOFT)
                        .on_hover_text("stop this turn")
                        .clicked()
                    {
                        stop = true;
                    }
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
            ui.add(egui::Label::new(RichText::new(model).size(11.0).color(theme::FAINT)).truncate());
        },
    );
    stop
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
        .unwrap_or_else(|| "the default route".to_string());
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
/// @param badge a tag drawn beside the name, when the section has one
/// @param salt a stable id for the openness memory
/// @returns whether the section is open
fn section_header(ui: &mut Ui, label: &str, count: usize, badge: Option<&str>, salt: &str) -> bool {
    let id = ui.id().with(salt);
    let mut open = ui.ctx().data_mut(|data| *data.get_temp_mut_or(id, true));
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(5), theme::HOVER_SOFT);
    }
    let painter = ui.painter();
    let name = painter.layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(13.5),
        theme::TEXT,
    );
    let name_width = name.size().x;
    painter.galley(egui::pos2(rect.left() + 2.0, rect.center().y - name.size().y / 2.0), name, theme::TEXT);
    let mut x = rect.left() + 2.0 + name_width + 8.0;
    if count > 0 {
        let galley = painter.layout_no_wrap(count.to_string(), theme::mono(11.0), theme::FAINT);
        let width = galley.size().x;
        painter.galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, theme::FAINT);
        x += width + 8.0;
    }
    let chevron = Rect::from_center_size(egui::pos2(x + 5.0, rect.center().y), Vec2::splat(12.0));
    if open {
        icons::paint(ui, Icon::Chevron, chevron, theme::FAINT);
    } else {
        icons::paint(ui, Icon::ArrowRight, chevron.shrink(1.0), theme::FAINT);
    }
    if let Some(badge) = badge {
        let galley = ui
            .painter()
            .layout_no_wrap(badge.to_string(), egui::FontId::proportional(11.0), theme::ACCENT);
        let pill = Rect::from_min_size(
            egui::pos2(rect.right() - galley.size().x - 14.0, rect.center().y - 9.0),
            Vec2::new(galley.size().x + 12.0, 18.0),
        );
        ui.painter()
            .rect_filled(pill, CornerRadius::same(5), theme::ACCENT_DIM);
        ui.painter().galley(
            egui::pos2(pill.left() + 6.0, pill.center().y - galley.size().y / 2.0),
            galley,
            theme::ACCENT,
        );
    }
    if response.clicked() {
        open = !open;
    }
    ui.ctx().data_mut(|data| data.insert_temp(id, open));
    ui.add_space(4.0);
    open
}

/// The help button: a black disc with a question mark.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param rect where the button is drawn — the panel's corner, or the footer
fn fab(state: &mut HarnessState, ui: &mut Ui, rect: Rect) {
    let size = rect.width();
    let response = ui.interact(rect, ui.id().with("help-fab"), Sense::click());
    let painter = ui.painter();
    let fill = if response.hovered() {
        Color32::from_rgb(0x3A, 0x3A, 0x3A)
    } else {
        theme::TEXT
    };
    if size > 30.0 {
        painter.add(
            theme::card_shadow()
                .as_shape(rect, CornerRadius::same(theme::static_radius(size / 2.0))),
        );
    }
    painter.circle_filled(rect.center(), size / 2.0, fill);
    icons::paint(ui, Icon::Question, rect.shrink(size * 0.32), theme::PANEL);
    if response.on_hover_text("keyboard shortcuts").clicked() {
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

/// Where the session stands, in the words its panel uses.
///
/// Derived for display from the run state and the transcript — it changes
/// nothing about how a turn runs. "Idle" alone threw away the one thing a
/// person glancing at the panel wants to know: whether the last thing asked
/// actually finished.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SessionPhase {
    /// Nothing asked yet in this task.
    Ready,
    /// A turn is running.
    Working,
    /// The engine is waiting on a permission answer.
    NeedsYou,
    /// The last turn ended with an answer.
    Done,
    /// The last turn ended without one (stopped by the user).
    Stopped,
    /// The last turn failed.
    Failed,
}

impl SessionPhase {
    /// @param state the application state
    /// @returns the phase
    fn of(state: &HarnessState) -> Self {
        use harness_core::agent::Item;
        let conversation = &state.agent.conversation;
        match conversation.state {
            RunState::Running => return Self::Working,
            RunState::AwaitingApproval { .. } => return Self::NeedsYou,
            RunState::Failed { .. } => return Self::Failed,
            RunState::Idle => {}
        }
        let last_user = conversation.items.iter().rposition(|item| matches!(item, Item::User { .. }));
        let Some(last_user) = last_user else { return Self::Ready };
        // Done means the turn ended the way a finished turn does: with an
        // answer as its last word. A tool call as the last thing is a turn
        // that was cut off mid-step (stopped, or the app closed under it).
        let ended_with_answer = conversation.items[last_user..]
            .iter()
            .rev()
            .find(|item| matches!(item, Item::Assistant { .. } | Item::Tool(_)))
            .is_some_and(|item| matches!(item, Item::Assistant { text, .. } if !text.trim().is_empty()));
        if ended_with_answer { Self::Done } else { Self::Stopped }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Working => "working",
            Self::NeedsYou => "needs you",
            Self::Done => "done",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }

    fn colour(self) -> egui::Color32 {
        match self {
            Self::Ready | Self::Stopped => theme::FAINT,
            Self::Working => theme::ACCENT,
            Self::NeedsYou => theme::AMBER,
            Self::Done => theme::GREEN,
            Self::Failed => theme::RED,
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Done => Icon::Check,
            Self::Failed => Icon::Cross,
            Self::Stopped => Icon::Stop,
            _ => Icon::Sparkle,
        }
    }
}
