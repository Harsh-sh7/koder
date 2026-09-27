//! The workflows pane: what the harness knows how to do here, and what it knows.
//!
//! A workflow is a saved command or a saved prompt — Warp Drive's equivalent, in
//! a form a harness can use: it carries `{{placeholders}}`, so the same saved
//! "run the failing test" prompt is one thing in the list and a filled-in request
//! when it runs. The list groups them by kind, because a command goes to the
//! shell and a prompt goes to the agent, and those are different verbs.
//!
//! Notes live beside them. A note is a markdown file under `.harness/notes`, so
//! it opens in the editor like any other file: the harness adds no private
//! format for text a person wrote.

use eframe::egui::{self, Align, Layout, RichText, ScrollArea, Sense, Ui};

use harness_core::workflow::{Rendered, Workflow, WorkflowKind};

use crate::app::shorten;
use crate::state::{Dialog, HarnessState};
use crate::theme;

/// How much of a workflow's body is shown in the list.
const BODY_PREVIEW: usize = 120;

/// Draws the workflows pane.
///
/// @param state the application state
/// @param ui the interface to draw into
pub fn show(state: &mut HarnessState, ui: &mut Ui) {
    toolbar(state, ui);
    ui.add_space(4.0);

    let mut open: Option<Workflow> = None;
    let mut run: Option<Workflow> = None;
    let mut remove: Option<String> = None;

    egui::Panel::right("flow-notes")
        .default_size(300.0)
        .size_range(180.0..=520.0)
        .resizable(true)
        .frame(
            egui::Frame::new()
                .fill(theme::BG)
                .inner_margin(egui::Margin::symmetric(8, 6)),
        )
        .show(ui, |ui| notes(state, ui));

    ScrollArea::vertical()
        .id_salt("flow-list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut commands: Vec<&Workflow> = state.workflows.of_kind(WorkflowKind::Command);
            let mut prompts: Vec<&Workflow> = state.workflows.of_kind(WorkflowKind::Prompt);
            commands.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            prompts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

            if commands.is_empty() && prompts.is_empty() {
                ui.add_space(10.0);
                ui.label(
                    RichText::new("no saved workflows")
                        .size(11.5)
                        .color(theme::FAINT),
                );
                ui.label(
                    RichText::new("save a command or a prompt once and it is here next time")
                        .size(10.5)
                        .color(theme::FAINT),
                );
                return;
            }
            section(ui, "commands", commands.len());
            for workflow in commands {
                row(ui, workflow, &mut open, &mut run, &mut remove);
            }
            ui.add_space(8.0);
            section(ui, "prompts", prompts.len());
            for workflow in prompts {
                row(ui, workflow, &mut open, &mut run, &mut remove);
            }
        });

    if let Some(workflow) = open {
        state.dialog = Some(Dialog::Workflow(Box::new(crate::state::workflow_draft(
            workflow, false,
        ))));
    }
    if let Some(workflow) = run {
        run_workflow(state, &workflow);
    }
    if let Some(id) = remove {
        match state.workflows.remove(&id) {
            Ok(true) => state.toast(format!("removed {id}")),
            Ok(false) => state.toast(format!("{id} was not there")),
            Err(err) => state.toast(err),
        }
    }
}

/// The pane's head: how many are saved, and how to add one.
///
/// @param state the application state
/// @param ui the interface to draw into
fn toolbar(state: &mut HarnessState, ui: &mut Ui) {
    ui.horizontal(|ui| {
        theme::section(ui, "workflows");
        ui.label(
            RichText::new(format!("{} saved", state.workflows.all().len()))
                .size(10.5)
                .color(theme::FAINT),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::action(ui, "new note", true, theme::DIM).clicked() {
                state.dialog = Some(Dialog::Note(crate::state::NoteDraft {
                    name: String::new(),
                    text: String::new(),
                    original: None,
                }));
            }
            if theme::action(ui, "new workflow", true, theme::ACCENT).clicked() {
                let workflow = Workflow::prompt("new-workflow", "New workflow", "");
                state.dialog = Some(Dialog::Workflow(Box::new(crate::state::workflow_draft(
                    workflow, true,
                ))));
            }
        });
    });
    ui.label(
        RichText::new(
            "commands go to the shell; prompts go to the agent. Both take {{placeholders}}.",
        )
        .size(10.5)
        .color(theme::FAINT),
    );
}

/// A group heading.
///
/// @param ui the interface to draw into
/// @param title the group
/// @param count how many rows follow
fn section(ui: &mut Ui, title: &str, count: usize) {
    ui.add_space(4.0);
    theme::section(ui, &format!("{title} · {count}"));
}

/// One workflow row: what it is, and the three things to do with it.
///
/// @param ui the interface to draw into
/// @param workflow the workflow
/// @param open set when edit is clicked
/// @param run set when run is clicked
/// @param remove set when remove is clicked
fn row(
    ui: &mut Ui,
    workflow: &Workflow,
    open: &mut Option<Workflow>,
    run: &mut Option<Workflow>,
    remove: &mut Option<String>,
) {
    let rendered: Rendered = workflow.render(&std::collections::BTreeMap::new());
    let missing = rendered.missing.len();
    let kind_colour = match workflow.kind {
        WorkflowKind::Command => theme::BLUE,
        WorkflowKind::Prompt => theme::ACCENT,
    };

    // The card carries no accent stripe: the kind badge already says what the
    // workflow is, and five stripes down a light list read as noise.
    theme::card(ui, theme::ELEVATED, None, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&workflow.name).size(12.0).color(theme::TEXT));
            theme::badge(ui, workflow.kind.label(), kind_colour);
            if missing > 0 {
                theme::badge(ui, &format!("{missing} to fill"), theme::AMBER);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::action(ui, "remove", true, theme::FAINT).clicked() {
                    *remove = Some(workflow.id.clone());
                }
                if theme::action(ui, "edit", true, theme::DIM).clicked() {
                    *open = Some(workflow.clone());
                }
                if theme::action(ui, "run", true, kind_colour).clicked() {
                    *run = Some(workflow.clone());
                }
            });
        });
        if !workflow.description.is_empty() {
            ui.add(
                egui::Label::new(
                    RichText::new(&workflow.description)
                        .size(11.0)
                        .color(theme::DIM),
                )
                .sense(Sense::hover())
                .truncate(),
            );
        }
        // The preview is prose, not a path: it is cut at its end, where the eye
        // expects to stop reading, rather than in the middle.
        let preview = if missing > 0 {
            workflow.body.clone()
        } else {
            rendered.text
        };
        let preview = preview.split_whitespace().collect::<Vec<_>>().join(" ");
        let preview = head(&preview, BODY_PREVIEW);
        ui.add(
            egui::Label::new(
                RichText::new(preview)
                    .size(10.5)
                    .monospace()
                    .color(theme::FAINT),
            )
            .sense(Sense::hover())
            .truncate(),
        );
    });
    ui.add_space(4.0);
}

/// The head of a text, with an ellipsis when anything was cut.
///
/// @param text the text
/// @param limit how many characters to keep
/// @returns the head
fn head(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let cut: String = text.chars().take(limit).collect();
    format!("{}…", cut.trim_end())
}

/// The notes column.
///
/// @param state the application state
/// @param ui the interface to draw into
fn notes(state: &mut HarnessState, ui: &mut Ui) {
    let mut open: Option<std::path::PathBuf> = None;
    let mut edit: Option<(String, String)> = None;
    let mut refresh = false;

    ui.horizontal(|ui| {
        theme::section(ui, "notes");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::action(ui, "refresh", true, theme::FAINT).clicked() {
                refresh = true;
            }
            if theme::action(ui, "new", true, theme::ACCENT).clicked() {
                state.dialog = Some(Dialog::Note(crate::state::NoteDraft {
                    name: String::new(),
                    text: String::new(),
                    original: None,
                }));
            }
        });
    });
    ui.add_space(2.0);
    if state.notes.is_empty() {
        ui.label(RichText::new("no notes yet").size(11.0).color(theme::FAINT));
        ui.label(
            RichText::new("notes are markdown files under .harness/notes; the agent can read them")
                .size(10.5)
                .color(theme::FAINT),
        );
    }
    ScrollArea::vertical()
        .id_salt("notes")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for note in &state.notes {
                let name = note
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("note")
                    .to_string();
                ui.horizontal(|ui| {
                    let label = ui.add(
                        egui::Label::new(
                            RichText::new(shorten(&name, 40))
                                .size(11.5)
                                .color(theme::TEXT),
                        )
                        .sense(Sense::click())
                        .truncate(),
                    );
                    if label.clicked() {
                        open = Some(note.clone());
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::action(ui, "edit", true, theme::DIM).clicked() {
                            if let Ok(text) = std::fs::read_to_string(note) {
                                edit = Some((name.clone(), text));
                            }
                        }
                    });
                });
            }
        });

    if refresh {
        state.refresh_notes();
    }
    if let Some(path) = open {
        state.open_file(&path);
    }
    if let Some((name, text)) = edit {
        state.dialog = Some(Dialog::Note(crate::state::NoteDraft {
            name: name.clone(),
            text,
            original: Some(name),
        }));
    }
}

/// Runs a workflow: a command goes to the shell, a prompt to the agent.
///
/// A workflow with unfilled placeholders opens the dialog instead, because
/// running `cargo test {{filter}}` literally would be a worse answer than asking.
///
/// @param state the application state
/// @param workflow the workflow
fn run_workflow(state: &mut HarnessState, workflow: &Workflow) {
    let rendered = workflow.render(&std::collections::BTreeMap::new());
    if !rendered.missing.is_empty() {
        state.dialog = Some(Dialog::Workflow(Box::new(crate::state::workflow_draft(
            workflow.clone(),
            false,
        ))));
        return;
    }
    match workflow.kind {
        WorkflowKind::Command => state.run_command(&rendered.text),
        WorkflowKind::Prompt => {
            state.agent.input = rendered.text;
            state.agent_open = true;
            state.focus = Some(crate::state::FocusRequest::Agent);
            state.save_config();
        }
    }
}
