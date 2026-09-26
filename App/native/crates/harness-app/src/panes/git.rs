//! The git pane: what changed, what a change says, and one commit button.
//!
//! The layout answers the three questions in the order they are asked. The left
//! column is the working tree, grouped the way git groups it — staged, changed,
//! untracked, conflicted — because that grouping is the model the commit box
//! below it acts on. The middle is the selected path's patch, drawn with the
//! same scanner the editor uses. The right column is where you are: branches,
//! and the last commits, so a diff can be read in the context of the history it
//! lands in.
//!
//! Every write here is deliberate and reversible: staging and unstaging are one
//! click each, and the commit needs a message before it will do anything.

use eframe::egui::{self, Align, Layout, RichText, ScrollArea, Sense, TextEdit, Ui};

use harness_core::git::{ChangeKind, FileChange, RepoStatus};

use crate::app::shorten;
use crate::code;
use crate::state::HarnessState;
use crate::theme;

/// Patch lines drawn for one file before the pane stops.
const PATCH_LIMIT: usize = 20_000;

/// Draws the git pane.
///
/// @param state the application state
/// @param ui the interface to draw into
pub fn show(state: &mut HarnessState, ui: &mut Ui) {
    if state.git.is_none() {
        empty(ui);
        return;
    }
    if !state.git_view.loaded {
        state.refresh_git();
    }
    let mut select: Option<(String, bool)> = None;
    let mut restage: Option<(String, bool)> = None;

    toolbar(state, ui);
    ui.add_space(4.0);
    if let Some(error) = state.git_view.error.clone() {
        ui.label(RichText::new(error).size(11.0).color(theme::RED));
        ui.add_space(2.0);
    }

    // The commit box spans the pane, under everything it acts on: the message
    // and the button are about the whole working tree, not about one column.
    egui::Panel::bottom("git-commit")
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .inner_margin(egui::Margin::symmetric(8, 6)),
        )
        .show(ui, |ui| commit_box(state, ui));

    egui::Panel::right("git-history")
        .default_size(250.0)
        .size_range(160.0..=460.0)
        .resizable(true)
        .frame(
            egui::Frame::new()
                .fill(theme::BG)
                .inner_margin(egui::Margin::symmetric(8, 6)),
        )
        .show(ui, |ui| history(state, ui));

    egui::Panel::left("git-changes")
        .default_size(280.0)
        .size_range(180.0..=520.0)
        .resizable(true)
        .frame(
            egui::Frame::new()
                .fill(theme::BG)
                .inner_margin(egui::Margin::symmetric(8, 6)),
        )
        .show(ui, |ui| changes(state, ui, &mut select, &mut restage));

    patch(state, ui);

    if let Some((path, staged)) = select {
        state.git_view.selected = Some(path.clone());
        state.git_view.staged = staged;
        state.load_diff(&path, staged);
    }
    if let Some((path, stage)) = restage {
        state.set_staged(&path, stage);
    }
}

/// The pane's head: where HEAD is, and the actions that re-read it.
///
/// @param state the application state
/// @param ui the interface to draw into
fn toolbar(state: &mut HarnessState, ui: &mut Ui) {
    let mut refresh = false;
    let mut stage_all = false;
    let status = &state.git_view.status;
    let clean = status.is_clean();

    ui.horizontal(|ui| {
        theme::section(ui, "git");
        ui.label(
            RichText::new(&status.branch)
                .size(12.0)
                .monospace()
                .color(theme::TEXT),
        );
        if status.ahead > 0 {
            theme::badge(ui, &format!("{} ahead", status.ahead), theme::BLUE);
        }
        if status.behind > 0 {
            theme::badge(ui, &format!("{} behind", status.behind), theme::AMBER);
        }
        if clean {
            theme::badge(ui, "clean", theme::GREEN);
        } else {
            theme::badge(
                ui,
                &format!("{} changed", status.change_count()),
                theme::AMBER,
            );
        }
        if let Some(upstream) = &status.upstream {
            ui.label(
                RichText::new(shorten(upstream, 30))
                    .size(10.5)
                    .color(theme::FAINT),
            );
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::action(ui, "refresh", true, theme::DIM).clicked() {
                refresh = true;
            }
            let stageable =
                status.unstaged.len() + status.untracked.len() + status.conflicted.len();
            if theme::action(
                ui,
                &format!("stage all ({stageable})"),
                stageable > 0,
                theme::ACCENT,
            )
            .on_hover_text("git add -A")
            .clicked()
            {
                stage_all = true;
            }
        });
    });

    if refresh {
        state.refresh_git();
    }
    if stage_all {
        let paths: Vec<String> = state
            .git_view
            .status
            .unstaged
            .iter()
            .chain(&state.git_view.status.untracked)
            .chain(&state.git_view.status.conflicted)
            .map(|change| change.path.clone())
            .collect();
        for path in paths {
            state.set_staged(&path, true);
        }
        state.toast("staged everything that changed");
    }
}

/// The working tree, grouped the way git groups it.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param select set to a path and its side when one is clicked
/// @param restage set to a path and the action when stage/unstage is clicked
fn changes(
    state: &HarnessState,
    ui: &mut Ui,
    select: &mut Option<(String, bool)>,
    restage: &mut Option<(String, bool)>,
) {
    let status = &state.git_view.status;
    if status.is_clean() {
        ui.add_space(10.0);
        ui.label(
            RichText::new("nothing changed")
                .size(11.5)
                .color(theme::FAINT),
        );
        ui.label(
            RichText::new("the working tree matches HEAD")
                .size(10.5)
                .color(theme::FAINT),
        );
        return;
    }

    ScrollArea::vertical()
        .id_salt("git-changes-list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            group(ui, "staged", &status.staged, true, state, select, restage);
            group(
                ui,
                "changed",
                &status.unstaged,
                false,
                state,
                select,
                restage,
            );
            group(
                ui,
                "untracked",
                &status.untracked,
                false,
                state,
                select,
                restage,
            );
            group(
                ui,
                "conflicted",
                &status.conflicted,
                false,
                state,
                select,
                restage,
            );
        });
}

/// One group of changes, with its rows.
///
/// @param ui the interface to draw into
/// @param title the group's name
/// @param changes the paths in it
/// @param staged whether these paths are on the staged side
/// @param state the application state
/// @param select set when a path is clicked
/// @param restage set when the row's action is clicked
fn group(
    ui: &mut Ui,
    title: &str,
    changes: &[FileChange],
    staged: bool,
    state: &HarnessState,
    select: &mut Option<(String, bool)>,
    restage: &mut Option<(String, bool)>,
) {
    if changes.is_empty() {
        return;
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        theme::section(ui, &format!("{title} · {}", changes.len()));
    });
    for change in changes {
        let selected = state.git_view.selected.as_deref() == Some(change.path.as_str())
            && state.git_view.staged == staged;
        let colour = kind_colour(change.kind);
        // The band goes down before the row's own text, so the selection reads
        // as a row background rather than as a wash over the words.
        if selected {
            let band = egui::Rect::from_min_size(
                ui.cursor().min,
                egui::Vec2::new(ui.available_width(), theme::ROW),
            );
            ui.painter()
                .rect_filled(band, egui::CornerRadius::same(3), theme::ACCENT_DIM);
        }
        ui.horizontal(|ui| {
            // The row is laid out from its right end so the path is told how much
            // room the kind label and the stage action leave it, and truncates to
            // fit instead of running underneath them.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(2.0);
                let (text, action) = if staged {
                    ("unstage", false)
                } else {
                    ("stage", true)
                };
                if theme::action(
                    ui,
                    text,
                    true,
                    if staged { theme::FAINT } else { theme::ACCENT },
                )
                .clicked()
                {
                    *restage = Some((change.path.clone(), action));
                }
                ui.label(RichText::new(change.kind.label()).size(9.5).color(colour));
                ui.add_space(6.0);
                // A path is cut in the middle rather than at its end, so the file
                // name — the part that says which file this is — survives.
                let (advance, _) = code::metrics(ui, &theme::mono(11.5));
                let room = ui.available_width();
                let fit = ((room / advance).floor() as usize).saturating_sub(2);
                let shown = shorten(&format!("{} {}", change.kind.letter(), change.path), fit);
                // Given the whole of the room left over rather than just the width
                // of its text: a label in a right-to-left layout ends where it is
                // placed, which would right-align a column of paths of different
                // lengths.
                let label = ui.add_sized(
                    [room, 18.0],
                    egui::Label::new(
                        RichText::new(shown)
                            .size(11.5)
                            .monospace()
                            .color(if selected { theme::TEXT } else { theme::DIM }),
                    )
                    .sense(Sense::click())
                    .truncate(),
                );
                if label.clicked() {
                    *select = Some((change.path.clone(), staged));
                }
                label.on_hover_text(change.kind.label());
            });
        });
    }
}

/// The selected path's patch.
///
/// A patch long enough to be a wall of text — a lockfile, a generated file —
/// opens collapsed, and the header carries the switch between the two views.
///
/// @param state the application state
/// @param ui the interface to draw into
fn patch(state: &mut HarnessState, ui: &mut Ui) {
    let font = theme::mono(state.config.terminal_font_size - 0.5);
    let (_, row_height) = code::metrics(ui, &font);
    let mut switch = false;
    {
        let Some((path, text)) = state.git_view.diff.as_ref() else {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("select a change")
                        .size(12.0)
                        .color(theme::DIM),
                );
                ui.add_space(2.0);
                ui.label(
                    RichText::new("its patch is shown here, and the agent can be asked about it")
                        .size(10.5)
                        .color(theme::FAINT),
                );
            });
            return;
        };

        let (added, removed, files) = code::patch_stats(text);
        let lines = text.lines().count();
        let compact = state.git_view.compact;
        ui.horizontal(|ui| {
            theme::section(ui, "diff");
            ui.add(
                egui::Label::new(
                    RichText::new(shorten(path, 70))
                        .size(11.0)
                        .monospace()
                        .color(theme::TEXT),
                )
                .truncate(),
            );
            theme::badge(ui, &format!("+{added}"), theme::GREEN);
            theme::badge(ui, &format!("-{removed}"), theme::RED);
            if files > 1 {
                theme::badge(ui, &format!("{files} files"), theme::DIM);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if state.git_view.staged {
                    theme::badge(ui, "index vs HEAD", theme::BLUE);
                } else {
                    theme::badge(ui, "worktree vs index", theme::AMBER);
                }
                let label = if compact { "full diff" } else { "summary" };
                if theme::action(ui, label, true, theme::DIM)
                    .on_hover_text(format!(
                        "{lines} patch lines; switch between all of them and the changes alone"
                    ))
                    .clicked()
                {
                    switch = true;
                }
            });
        });
        ui.add_space(2.0);
        ScrollArea::both()
            .id_salt("git-diff")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if compact {
                    code::draw_patch_compact(ui, text, &font, row_height, PATCH_LIMIT);
                } else {
                    code::draw_patch(ui, text, &font, row_height, PATCH_LIMIT);
                }
            });
    }
    if switch {
        state.git_view.compact = !state.git_view.compact;
    }
}

/// Branches and recent history.
///
/// @param state the application state
/// @param ui the interface to draw into
fn history(state: &HarnessState, ui: &mut Ui) {
    theme::section(ui, "branches");
    ScrollArea::vertical()
        .id_salt("git-branches")
        .max_height(140.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for branch in &state.git_view.branches {
                let marker = if branch.current { "●" } else { "○" };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(marker).size(10.0).color(if branch.current {
                        theme::ACCENT
                    } else {
                        theme::FAINT
                    }));
                    ui.add(
                        egui::Label::new(RichText::new(&branch.name).size(11.5).monospace().color(
                            if branch.current {
                                theme::TEXT
                            } else {
                                theme::DIM
                            },
                        ))
                        .truncate(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(&branch.target).size(10.0).color(theme::FAINT));
                    });
                });
            }
        });
    ui.add_space(6.0);
    theme::section(ui, "history");
    ScrollArea::vertical()
        .id_salt("git-log")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for commit in &state.git_view.log {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&commit.short)
                            .size(10.5)
                            .monospace()
                            .color(theme::BLUE),
                    );
                    ui.add(
                        egui::Label::new(
                            RichText::new(shorten(&commit.summary, 90))
                                .size(11.0)
                                .color(theme::TEXT),
                        )
                        .truncate(),
                    );
                });
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(shorten(&format!("{} · {}", commit.author, commit.when), 60))
                            .size(9.5)
                            .color(theme::FAINT),
                    );
                });
                ui.add_space(3.0);
            }
        });
}

/// The commit box: a message, and what it will commit.
///
/// @param state the application state
/// @param ui the interface to draw into
fn commit_box(state: &mut HarnessState, ui: &mut Ui) {
    let staged = state.git_view.status.staged.len();
    let conflicted = state.git_view.status.conflicted.len();
    let font = theme::mono(state.config.terminal_font_size - 1.0);
    let mut commit = false;

    theme::rule(ui, false);
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        theme::section(ui, "commit");
        ui.label(
            RichText::new(match staged {
                0 => "nothing staged".to_string(),
                1 => "1 staged file".to_string(),
                n => format!("{n} staged files"),
            })
            .size(11.0)
            .color(if staged == 0 {
                theme::FAINT
            } else {
                theme::TEXT
            }),
        );
        if conflicted > 0 {
            theme::badge(ui, &format!("{conflicted} conflicted"), theme::RED);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let ready = staged > 0 && conflicted == 0 && !state.git_view.message.trim().is_empty();
            if theme::action(ui, "commit", ready, theme::GREEN).clicked() && ready {
                commit = true;
            }
        });
    });
    let response = ui.add(
        TextEdit::multiline(&mut state.git_view.message)
            .id(egui::Id::new("git-commit-message"))
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .hint_text("what this change does")
            .font(font),
    );
    // ⌘↩ commits from the message field, the same chord the prompt box uses.
    if response.has_focus()
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
    {
        commit = staged > 0 && conflicted == 0 && !state.git_view.message.trim().is_empty();
    }
    if commit {
        state.commit();
    }
}

/// The colour a change kind is drawn in.
///
/// @param kind the change
/// @returns the colour
fn kind_colour(kind: ChangeKind) -> egui::Color32 {
    match kind {
        ChangeKind::Added => theme::GREEN,
        ChangeKind::Modified => theme::AMBER,
        ChangeKind::Deleted => theme::RED,
        ChangeKind::Renamed => theme::BLUE,
        ChangeKind::TypeChange => theme::VIOLET,
        ChangeKind::Conflicted => theme::RED,
        ChangeKind::Untracked => theme::FAINT,
    }
}

/// The empty state for a workspace that is not a repository.
///
/// @param ui the interface to draw into
fn empty(ui: &mut Ui) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("no repository here")
                .size(13.0)
                .color(theme::DIM),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new("this workspace is not inside a git repository")
                .size(11.0)
                .color(theme::FAINT),
        );
        ui.label(
            RichText::new("the terminal can run git init, and this pane will pick it up")
                .size(11.0)
                .color(theme::FAINT),
        );
    });
}

/// A one-line summary of the status, for headers elsewhere.
///
/// @param status the repository status
/// @returns e.g. `main · 3 changed`
pub fn status_label(status: &RepoStatus) -> String {
    if status.is_clean() {
        format!("{} · clean", status.branch)
    } else {
        format!("{} · {} changed", status.branch, status.change_count())
    }
}
