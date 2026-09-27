//! The dialogs: the model routes, a workflow, a note, the folder picker, and the
//! about card.
//!
//! Each one takes its draft out of the state, edits the draft, and puts it back
//! only if it should stay open. That is why nothing here mutates a document on
//! a keystroke: writing `.harness/models.json` is a decision, not a side effect
//! of typing, and the engine only re-reads it when it starts — so the save is
//! also the point where the app says what will happen next.
//!
//! The model dialog is the one that matters most: it is where a workspace gets a
//! provider at all. Nothing is bundled, nothing is guessed, and the credential
//! is written to the workspace `.env` — the one file meant to hold a value —
//! while the route document records only the variable's name.

use std::path::PathBuf;

use eframe::egui::{self, Align, Layout, RichText, TextEdit};

use harness_core::workflow::{Workflow, WorkflowKind};

use crate::icons::{self, Icon};
use crate::state::{
    folder_shortcuts, Dialog, FolderDraft, HarnessState, ModelDraft, NoteDraft, WorkflowDraft,
};
use crate::theme;

/// Draws whichever dialog is open.
///
/// @param state the application state
/// @param ctx the egui context
pub fn show(state: &mut HarnessState, ctx: &egui::Context) {
    let Some(dialog) = state.dialog.take() else {
        return;
    };
    match dialog {
        Dialog::Models(mut draft) => {
            if models(state, ctx, &mut draft) {
                state.dialog = Some(Dialog::Models(draft));
            }
        }
        Dialog::Workflow(mut draft) => {
            if workflow(state, ctx, &mut draft) {
                state.dialog = Some(Dialog::Workflow(draft));
            }
        }
        Dialog::Note(mut draft) => {
            if note(state, ctx, &mut draft) {
                state.dialog = Some(Dialog::Note(draft));
            }
        }
        Dialog::Folder(mut draft) => {
            if folder(state, ctx, &mut draft) {
                state.dialog = Some(Dialog::Folder(draft));
            }
        }
        Dialog::About => {
            if about(ctx) {
                state.dialog = Some(Dialog::About);
            }
        }
    }
}

/// The model-route editor.
///
/// @param state the application state
/// @param ctx the egui context
/// @param draft the route being edited
/// @returns true when the dialog should stay open
fn models(state: &mut HarnessState, ctx: &egui::Context, draft: &mut ModelDraft) -> bool {
    let mut open = true;
    let mut save = false;
    let mut remove = false;
    let mut activate: Option<(String, String)> = None;
    let existing: Vec<(String, String, Vec<String>)> = state
        .models
        .providers
        .iter()
        .map(|(route, profile)| {
            let label = profile
                .display_name
                .clone()
                .unwrap_or_else(|| route.clone());
            let models: Vec<String> = profile
                .models
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|model| model.id)
                .collect();
            (route.clone(), label, models)
        })
        .collect();
    let active = state.models.active.clone();

    egui::Window::new("model routes")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
        .show(ctx, |ui| {
            ui.label(
                RichText::new("a route is a provider, its API root, and the models it serves. The key is read from the environment; pasting one below writes it to the workspace .env, which git ignores — never to models.json.")
                    .size(11.0)
                    .color(theme::DIM),
            );
            ui.add_space(6.0);

            // What is already configured, and one click to switch to it.
            if !existing.is_empty() {
                theme::section(ui, "configured");
                for (route, label, models) in &existing {
                    ui.horizontal(|ui| {
                        let current = active.as_ref().is_some_and(|active| &active.provider == route);
                        let marker = if current { "●" } else { "○" };
                        ui.label(RichText::new(marker).size(10.0).color(if current { theme::ACCENT } else { theme::FAINT }));
                        ui.label(RichText::new(label).size(12.0).color(if current { theme::TEXT } else { theme::DIM }));
                        ui.label(RichText::new(format!("({route})")).size(10.5).color(theme::FAINT));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if theme::action(ui, "edit", true, theme::DIM).clicked() {
                                *draft = state.model_draft_for(route);
                            }
                            for model in models.iter().take(3) {
                                let selected = active.as_ref().is_some_and(|active| &active.provider == route && &active.model == model);
                                if theme::action(ui, model, !selected, theme::BLUE).clicked() {
                                    activate = Some((route.clone(), model.clone()));
                                }
                            }
                        });
                    });
                }
                ui.add_space(8.0);
            }

            theme::section(ui, if draft.existing { "edit route" } else { "new route" });
            ui.add_space(4.0);
            egui::Grid::new("model-fields").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label(RichText::new("route key").size(11.0).color(theme::DIM));
                ui.add(TextEdit::singleline(&mut draft.route).desired_width(340.0).hint_text("deepseek"));
                ui.end_row();
                ui.label(RichText::new("display name").size(11.0).color(theme::DIM));
                ui.add(TextEdit::singleline(&mut draft.display_name).desired_width(340.0).hint_text("DeepSeek"));
                ui.end_row();
                ui.label(RichText::new("base URL").size(11.0).color(theme::DIM));
                ui.add(TextEdit::singleline(&mut draft.base_url).desired_width(340.0).hint_text("https://api.example.com/v1"));
                ui.end_row();
                ui.label(RichText::new("key variable").size(11.0).color(theme::DIM));
                ui.add(TextEdit::singleline(&mut draft.api_key_env).desired_width(340.0).hint_text("AI_API_KEY"));
                ui.end_row();
                ui.label(RichText::new("api key").size(11.0).color(theme::DIM));
                ui.vertical(|ui| {
                    ui.add(
                        TextEdit::singleline(&mut draft.api_key)
                            .password(true)
                            .desired_width(340.0)
                            .hint_text("paste the key value — saved to .env, never to models.json"),
                    );
                    let var = if draft.api_key_env.trim().is_empty() { "AI_API_KEY" } else { draft.api_key_env.trim() };
                    let standing = match std::env::var(var) {
                        Ok(value) if value.trim() == "mock-key" => {
                            format!("{var} is still the probe placeholder (mock-key); paste a real one")
                        }
                        Ok(_) => format!(
                            "{var} is set — a saved key is never shown again; leave this blank to keep it, or paste a new one to replace it"
                        ),
                        Err(_) => format!("{var} is not set: paste a key here, or add one to the workspace .env"),
                    };
                    ui.label(RichText::new(standing).size(10.0).color(theme::FAINT));
                });
                ui.end_row();
                ui.label(RichText::new("models").size(11.0).color(theme::DIM));
                ui.add(
                    TextEdit::multiline(&mut draft.models)
                        .desired_rows(4)
                        .desired_width(340.0)
                        .hint_text("model-id | display name | context window"),
                );
                ui.end_row();
                ui.label(RichText::new("active model").size(11.0).color(theme::DIM));
                ui.add(TextEdit::singleline(&mut draft.active).desired_width(340.0).hint_text("the model id to run on"));
                ui.end_row();
                ui.label(RichText::new("price in/out").size(11.0).color(theme::DIM));
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut draft.price_input).desired_width(80.0).hint_text("in"));
                    ui.add(TextEdit::singleline(&mut draft.price_output).desired_width(80.0).hint_text("out"));
                    ui.label(RichText::new("USD per million tokens").size(10.0).color(theme::FAINT));
                });
                ui.end_row();
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if theme::action(ui, "save route", !draft.route.trim().is_empty(), theme::ACCENT).clicked() {
                    save = true;
                }
                if draft.existing && theme::action(ui, "remove", true, theme::RED).clicked() {
                    remove = true;
                }
                if theme::action(ui, "new", true, theme::DIM).clicked() {
                    *draft = ModelDraft::empty();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new("the engine starts on the saved route; a change restarts it")
                            .size(10.0)
                            .color(theme::FAINT),
                    );
                });
            });
        });

    if let Some((route, model)) = activate {
        state.activate_route(&route, &model);
        open = false;
    }
    if remove {
        let route = draft.route.clone();
        state.remove_route(&route);
        open = false;
    }
    if save {
        state.save_model_draft(draft);
        open = false;
    }
    open
}

/// The workflow editor: name, kind, body, and the placeholders it carries.
///
/// @param state the application state
/// @param ctx the egui context
/// @param draft the workflow being edited
/// @returns true when the dialog should stay open
fn workflow(state: &mut HarnessState, ctx: &egui::Context, draft: &mut WorkflowDraft) -> bool {
    let mut open = true;
    let mut save = false;
    let mut run = false;
    let mut delete = false;
    let placeholders = draft.workflow.placeholders();

    egui::Window::new(if draft.is_new { "new workflow" } else { "workflow" })
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(540.0)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
        .show(ctx, |ui| {
            egui::Grid::new("workflow-fields").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label(RichText::new("name").size(11.0).color(theme::DIM));
                ui.add(TextEdit::singleline(&mut draft.workflow.name).desired_width(380.0));
                ui.end_row();
                ui.label(RichText::new("when to use it").size(11.0).color(theme::DIM));
                ui.add(TextEdit::singleline(&mut draft.workflow.description).desired_width(380.0));
                ui.end_row();
                ui.label(RichText::new("kind").size(11.0).color(theme::DIM));
                ui.horizontal(|ui| {
                    let command = draft.workflow.kind == WorkflowKind::Command;
                    if theme::action(ui, "command", !command, theme::BLUE).clicked() {
                        draft.workflow.kind = WorkflowKind::Command;
                    }
                    if theme::action(ui, "prompt", command, theme::ACCENT).clicked() {
                        draft.workflow.kind = WorkflowKind::Prompt;
                    }
                    ui.label(
                        RichText::new(if command { "goes to the shell" } else { "goes to the agent" })
                            .size(10.0)
                            .color(theme::FAINT),
                    );
                });
                ui.end_row();
                ui.label(RichText::new("body").size(11.0).color(theme::DIM));
                ui.add(
                    TextEdit::multiline(&mut draft.workflow.body)
                        .desired_rows(5)
                        .desired_width(380.0)
                        .hint_text("cargo test {{filter}}\nor: explain the failure in {{file}}"),
                );
                ui.end_row();
                ui.label(RichText::new("tags").size(11.0).color(theme::DIM));
                ui.add(TextEdit::singleline(&mut draft.tags).desired_width(380.0).hint_text("build, test"));
                ui.end_row();
            });

            if !placeholders.is_empty() {
                ui.add_space(8.0);
                theme::section(ui, "values");
                ui.label(RichText::new("these are the placeholders the body uses; they are remembered with the workflow").size(10.5).color(theme::FAINT));
                for name in &placeholders {
                    let value = draft.values.entry(name.clone()).or_default();
                    ui.horizontal(|ui| {
                        ui.add_sized([140.0, theme::ROW], egui::Label::new(RichText::new(name).size(11.5).monospace().color(theme::BLUE)));
                        ui.add(TextEdit::singleline(value).desired_width(300.0));
                    });
                }
            }

            ui.add_space(8.0);
            let rendered = draft.workflow.render(&draft.values);
            theme::card(ui, theme::PANEL, None, |ui| {
                theme::section(ui, "it will run");
                ui.label(RichText::new(rendered.text.trim()).size(11.5).monospace().color(theme::TEXT));
                if !rendered.missing.is_empty() {
                    ui.label(
                        RichText::new(format!("still missing: {}", rendered.missing.join(", ")))
                            .size(10.5)
                            .color(theme::AMBER),
                    );
                }
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::action(ui, "save", !draft.workflow.name.trim().is_empty(), theme::ACCENT).clicked() {
                    save = true;
                }
                let ready = rendered.missing.is_empty() && !draft.workflow.body.trim().is_empty();
                if theme::action(ui, "run now", ready, theme::GREEN).clicked() && ready {
                    run = true;
                }
                if !draft.is_new && theme::action(ui, "delete", true, theme::RED).clicked() {
                    delete = true;
                }
            });
        });

    if delete {
        let id = draft.workflow.id.clone();
        match state.workflows.remove(&id) {
            Ok(true) => state.toast(format!("removed {id}")),
            Ok(false) => state.toast(format!("{id} was not there")),
            Err(err) => state.toast(err),
        }
        return false;
    }
    if save {
        if draft.is_new {
            draft.workflow.id = harness_core::workflow::slug(&draft.workflow.name);
            draft.workflow.seeded = false;
        }
        let mut workflow: Workflow = draft.workflow.clone();
        workflow.variables = placeholders
            .iter()
            .map(|name| {
                let mut variable = harness_core::workflow::WorkflowVariable::new(name);
                if let Some(value) = draft.values.get(name) {
                    if !value.is_empty() {
                        variable = variable.defaulting_to(value);
                    }
                }
                variable
            })
            .collect();
        workflow.tags = draft
            .tags
            .split(',')
            .map(|tag| tag.trim().to_string())
            .filter(|tag| !tag.is_empty())
            .collect();
        match state.workflows.upsert(workflow) {
            Ok(()) => {
                state.toast(format!("saved {}", draft.workflow.name));
                return false;
            }
            Err(err) => state.toast(err),
        }
    }
    if run {
        let rendered = draft.workflow.render(&draft.values);
        match draft.workflow.kind {
            WorkflowKind::Command => state.run_command(&rendered.text),
            WorkflowKind::Prompt => {
                state.agent.input = rendered.text;
                state.agent_open = true;
                state.focus = Some(crate::state::FocusRequest::Agent);
            }
        }
        return false;
    }
    open
}

/// The note editor.
///
/// @param state the application state
/// @param ctx the egui context
/// @param draft the note being edited
/// @returns true when the dialog should stay open
fn note(state: &mut HarnessState, ctx: &egui::Context, draft: &mut NoteDraft) -> bool {
    let mut open = true;
    let mut save = false;

    egui::Window::new("note")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(480.0)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 110.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("name").size(11.0).color(theme::DIM));
                ui.add(
                    TextEdit::singleline(&mut draft.name)
                        .desired_width(280.0)
                        .hint_text("what this note is about"),
                );
                ui.label(RichText::new(".md").size(11.0).color(theme::FAINT));
            });
            ui.add_space(4.0);
            ui.add(
                TextEdit::multiline(&mut draft.text)
                    .desired_rows(12)
                    .desired_width(f32::INFINITY)
                    .hint_text("markdown; the agent can read this file like any other"),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::action(ui, "save", !draft.name.trim().is_empty(), theme::ACCENT).clicked()
                {
                    save = true;
                }
                ui.label(
                    RichText::new("notes live in .harness/notes and open in the editor")
                        .size(10.0)
                        .color(theme::FAINT),
                );
            });
        });

    if save {
        let name = draft.name.trim().to_string();
        match state.save_note(&name, &draft.text) {
            Ok(path) => {
                // A renamed note is a move, not a copy: the file it came from
                // goes away, so the notes list does not hold the same text twice.
                if let Some(original) = draft.original.as_deref() {
                    if original != name {
                        state.remove_note(original);
                    }
                }
                state.refresh_notes();
                state.toast(format!("wrote {}", path.display()));
                return false;
            }
            Err(err) => state.toast(err),
        }
    }
    open
}

/// The folder picker: choose the folder this harness works in.
///
/// Browsing is the app's own: the list, the field, and the shortcuts are one
/// path seen three ways, and the engine is told about the new folder only when
/// the picker is answered — by then it is restarted on it.
///
/// @param state the application state
/// @param ctx the egui context
/// @param draft the picker's browsing state
/// @returns true when the window should stay open
fn folder(state: &mut HarnessState, ctx: &egui::Context, draft: &mut FolderDraft) -> bool {
    let mut open = true;
    let mut go: Option<PathBuf> = None;
    let mut chosen = false;
    let mut cancel = false;

    let shortcuts = folder_shortcuts(
        &state.root,
        state
            .paths
            .as_ref()
            .ok()
            .map(|paths| paths.repo_root.as_path()),
    );
    let listing = draft.dirs();
    let browsed = draft.dir.clone();
    let same = browsed == state.root;

    egui::Window::new("open folder")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(560.0)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
        .show(ctx, |ui| {
            ui.label(
                RichText::new("the harness works in one folder at a time: the tree, the editor, the shells, and the agent's tool calls all point at it. Opening another folder starts a fresh session — the model route carries over when the new folder has none.")
                    .size(11.0)
                    .color(theme::DIM),
            );
            ui.add_space(8.0);

            theme::section(ui, "shortcuts");
            ui.horizontal_wrapped(|ui| {
                if theme::action(ui, "↑ up", browsed.parent().is_some(), theme::DIM).clicked() {
                    draft.up();
                }
                for (label, path) in &shortcuts {
                    if theme::action(ui, label, true, theme::BLUE).clicked() {
                        go = Some(path.clone());
                    }
                }
            });
            ui.add_space(8.0);

            theme::section(ui, "path");
            ui.horizontal(|ui| {
                let response = ui.add(
                    TextEdit::singleline(&mut draft.typed)
                        .desired_width(400.0)
                        .font(theme::mono(11.0))
                        .hint_text("/a/folder/to/work/in"),
                );
                if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    go = Some(PathBuf::from(draft.typed.trim()));
                }
                if theme::action(ui, "go", !draft.typed.trim().is_empty(), theme::ACCENT).clicked() {
                    go = Some(PathBuf::from(draft.typed.trim()));
                }
            });
            if let Some(error) = draft.error.clone() {
                ui.label(RichText::new(error).size(10.5).color(theme::RED));
            }
            ui.add_space(8.0);

            theme::section(ui, "subfolders");
            egui::ScrollArea::vertical()
                .id_salt("folder-list")
                .max_height(230.0)
                .auto_shrink([false, false])
                .show(ui, |ui| match &listing {
                    Err(err) => {
                        ui.label(RichText::new(err).size(11.0).color(theme::RED));
                    }
                    Ok(dirs) if dirs.is_empty() => {
                        ui.label(RichText::new("no subfolders here").size(11.5).color(theme::FAINT));
                    }
                    Ok(dirs) => {
                        for (name, path, hidden) in dirs {
                            let clicked = theme::list_row(ui, false, true, |ui| {
                                icons::icon(ui, Icon::Folder, if *hidden { theme::FAINT } else { theme::BLUE }, 15.0);
                                ui.add_space(9.0);
                                ui.label(
                                    RichText::new(name)
                                        .size(12.5)
                                        .color(if *hidden { theme::FAINT } else { theme::TEXT }),
                                );
                            });
                            if clicked.on_hover_text(path.display().to_string()).clicked() {
                                go = Some(path.clone());
                            }
                        }
                    }
                });
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                let ready = browsed.is_dir() && !same;
                if theme::action(ui, "open this folder", ready, theme::ACCENT).clicked() && ready {
                    chosen = true;
                }
                if theme::action(ui, "cancel", true, theme::DIM).clicked() {
                    cancel = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(if same {
                            "this is the folder already open"
                        } else {
                            "the engine restarts on the new folder · ⌘O reopens this"
                        })
                        .size(10.0)
                        .color(theme::FAINT),
                    );
                });
            });
        });

    if let Some(path) = go {
        draft.enter(path);
    }
    if chosen {
        state.switch_workspace(browsed);
        return false;
    }
    if cancel {
        open = false;
    }
    open
}

/// The about card: what this is, and the things it will not do.
///
/// @param ctx the egui context
/// @returns true when the window should stay open
fn about(ctx: &egui::Context) -> bool {
    let mut open = true;
    egui::Window::new("about ai harness")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 120.0))
        .show(ctx, |ui| {
            ui.label(RichText::new("A coding harness with a terminal in it.").size(12.5).color(theme::TEXT));
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "The shell and the agent share one workspace: a command's output is attached to a prompt as it is, and the agent's tool calls are shown as the cards they are. The engine runs behind the Agent Client Protocol, so the app talks to it in a protocol rather than in its source.",
                )
                .size(11.5)
                .color(theme::DIM),
            );
            ui.add_space(8.0);
            theme::section(ui, "what it will not do");
            for line in [
                "no model is bundled: a route is added here and its key is read from the environment",
                "no credential is written to the workspace, ever",
                "no repository is dumped into a prompt; a command's output is attached deliberately",
                "no background spend: closing the app ends the engine process it started",
            ] {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("·").size(11.0).color(theme::FAINT));
                    ui.label(RichText::new(line).size(11.0).color(theme::DIM));
                });
            }
            ui.add_space(8.0);
            theme::section(ui, "where things live");
            for (what, where_) in [
                ("settings", ".harness/config.toml"),
                ("model routes", ".harness/models.json"),
                ("token prices", ".harness/prices.json"),
                ("workflows", ".harness/workflows"),
                ("notes", ".harness/notes"),
                ("transcripts", ".harness/transcripts"),
            ] {
                theme::stat_row(ui, what, where_, theme::FAINT);
            }
        });
    open
}
