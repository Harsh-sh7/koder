//! The observability pane: what this session has cost, and where it went.
//!
//! Every number here is one the app already had to compute, drawn where it can
//! be acted on rather than buried in a status line. Three questions, in order:
//! how much of the context window is spent and on what, what the tools have
//! been doing with their time, and what has been asked of the engine at all
//! (the protocol log, including the notifications this app does not model yet).
//!
//! Nothing here is estimated silently. A number that comes from the engine's own
//! accounting is labelled as such; a number derived from character counts says
//! "estimate" next to it. A harness that blurs the two teaches its user to trust
//! the wrong one.

use eframe::egui::{self, Align, Layout, RichText, ScrollArea, Ui};

use harness_core::agent::{Item, ToolStatus};

use crate::app::{compact, shorten};
use crate::panes::terminal::{block_colour, block_label, output_tokens};
use crate::state::HarnessState;
use crate::theme;

/// How many log lines the pane shows before it stops.
const LOG_LINES: usize = 300;

/// How many tool kinds the table lists.
const KINDS: usize = 12;

/// Draws the observability pane.
///
/// @param state the application state
/// @param ui the interface to draw into
pub fn show(state: &mut HarnessState, ui: &mut Ui) {
    ui.horizontal(|ui| {
        theme::section(ui, "meter");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::action(ui, "export transcript", true, theme::DIM).clicked() {
                match state.export_transcript() {
                    Ok(path) => state.toast(format!("wrote {}", path.display())),
                    Err(err) => state.toast(err),
                }
            }
        });
    });
    ui.add_space(4.0);
    ScrollArea::vertical()
        .id_salt("meter")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            tokens(state, ui);
            ui.add_space(6.0);
            context(state, ui);
            ui.add_space(6.0);
            tools(state, ui);
            ui.add_space(6.0);
            commands(state, ui);
            ui.add_space(6.0);
            engine(state, ui);
            ui.add_space(6.0);
            log(state, ui);
        });
}

/// The token accounting: what the engine reported, and what was estimated.
///
/// @param state the application state
/// @param ui the interface to draw into
fn tokens(state: &HarnessState, ui: &mut Ui) {
    let meter = &state.agent.conversation.meter;
    theme::card(ui, theme::ELEVATED, Some(theme::ACCENT), |ui| {
        theme::section(ui, "tokens");
        if meter.used == 0 && meter.updates == 0 {
            ui.label(
                RichText::new("the engine has not reported a token count yet; the first prompt will carry one")
                    .size(11.0)
                    .color(theme::FAINT),
            );
            return;
        }
        if meter.size > 0 {
            theme::stat_row(
                ui,
                "context window",
                &format!("{} used of {}k", compact(meter.used), meter.size / 1000),
                theme::TEXT,
            );
            theme::meter(
                ui,
                meter.context_fraction(),
                ui.available_width(),
                if meter.context_fraction() > 0.8 {
                    theme::AMBER
                } else {
                    theme::ACCENT
                },
            );
        }
        theme::stat_row(
            ui,
            "peak this session",
            &compact(meter.peak_used),
            theme::DIM,
        );
        theme::stat_row(
            ui,
            "updates from the engine",
            &meter.updates.to_string(),
            theme::DIM,
        );
        theme::stat_row(
            ui,
            "prompt tokens (estimate)",
            &compact(meter.prompt_tokens_estimate),
            theme::DIM,
        );
        theme::stat_row(
            ui,
            "completion tokens (estimate)",
            &compact(meter.completion_tokens_estimate),
            theme::DIM,
        );
        if let Some(cost) = meter.estimated_cost_usd() {
            theme::stat_row(ui, "estimated cost", &format!("${cost:.4}"), theme::GREEN);
        } else {
            theme::stat_row(
                ui,
                "estimated cost",
                "no price for this route",
                theme::FAINT,
            );
            ui.label(
                RichText::new("add input and output prices in the model editor to have the harness do this arithmetic")
                    .size(10.5)
                    .color(theme::FAINT),
            );
        }
        let caps = &state.config.caps;
        if caps.token_budget_per_session > 0 {
            let fraction = meter.used as f32 / caps.token_budget_per_session as f32;
            theme::stat_row(
                ui,
                "session budget",
                &format!(
                    "{} of {}",
                    compact(meter.used),
                    compact(caps.token_budget_per_session)
                ),
                theme::TEXT,
            );
            theme::meter(
                ui,
                fraction,
                ui.available_width(),
                if fraction >= caps.warn_at {
                    theme::AMBER
                } else {
                    theme::GREEN
                },
            );
            ui.label(
                RichText::new(format!(
                    "the engine warns at {:.0}% and grants {} tokens per approval, up to {} times",
                    caps.warn_at * 100.0,
                    compact(caps.grant_tokens),
                    caps.max_escalations
                ))
                .size(10.5)
                .color(theme::FAINT),
            );
        }
    });
}

/// Where the context is going: what is in the transcript, by kind.
///
/// @param state the application state
/// @param ui the interface to draw into
fn context(state: &HarnessState, ui: &mut Ui) {
    let mut prompt_chars = 0usize;
    let mut answer_chars = 0usize;
    let mut thought_chars = 0usize;
    let mut tool_chars = 0usize;
    let mut tools = 0usize;
    for item in &state.agent.conversation.items {
        match item {
            Item::User { text, .. } => prompt_chars += text.len(),
            Item::Assistant { text, thinking, .. } => {
                answer_chars += text.len();
                thought_chars += thinking.len();
            }
            Item::Tool(card) => {
                tool_chars += card.output.len() + card.title.len();
                tools += 1;
            }
            Item::Notice { text, .. } => prompt_chars += text.len(),
        }
    }
    let total = prompt_chars + answer_chars + thought_chars + tool_chars;

    theme::card(ui, theme::ELEVATED, None, |ui| {
        theme::section(ui, "context composition");
        if total == 0 {
            ui.label(
                RichText::new("the transcript is empty")
                    .size(11.0)
                    .color(theme::FAINT),
            );
            return;
        }
        let rows = [
            ("what was asked", prompt_chars, theme::ACCENT),
            ("what was answered", answer_chars, theme::GREEN),
            ("what was thought", thought_chars, theme::VIOLET),
            ("what tools returned", tool_chars, theme::AMBER),
        ];
        for (label, chars, colour) in rows {
            // The same four-characters-per-token heuristic the core estimates
            // with, applied to what the transcript actually holds.
            let tokens = chars as u64 / 4;
            let share = chars as f32 / total as f32;
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).size(11.5).color(theme::DIM));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{} tok · {:.0}%", compact(tokens), share * 100.0))
                            .size(11.0)
                            .monospace()
                            .color(colour),
                    );
                    theme::meter(ui, share, 90.0, colour);
                });
            });
        }
        theme::stat_row(
            ui,
            "transcript characters",
            &compact(total as u64),
            theme::DIM,
        );
        theme::stat_row(ui, "tool calls", &tools.to_string(), theme::DIM);
        ui.label(
            RichText::new("these five numbers are counted here, not reported by the engine; the meter above is the engine's own")
                .size(10.5)
                .color(theme::FAINT),
        );
    });
}

/// The tool table: what the agent spent its calls on.
///
/// @param state the application state
/// @param ui the interface to draw into
fn tools(state: &HarnessState, ui: &mut Ui) {
    let mut kinds: Vec<(String, usize, usize, u64)> = Vec::new(); // kind, calls, failures, ms
    let mut running = 0usize;
    for item in &state.agent.conversation.items {
        let Item::Tool(card) = item else { continue };
        if !card.status.done() {
            running += 1;
        }
        let entry = match kinds.iter_mut().find(|entry| entry.0 == card.kind) {
            Some(entry) => entry,
            None => {
                kinds.push((card.kind.clone(), 0, 0, 0));
                kinds.last_mut().expect("just pushed")
            }
        };
        entry.1 += 1;
        if card.status == ToolStatus::Failed {
            entry.2 += 1;
        }
        entry.3 += card.elapsed_ms.unwrap_or(0);
    }
    kinds.sort_by(|a, b| b.1.cmp(&a.1));
    kinds.truncate(KINDS);

    theme::card(ui, theme::ELEVATED, None, |ui| {
        theme::section(ui, "tool calls");
        if kinds.is_empty() {
            ui.label(
                RichText::new("no tool calls yet")
                    .size(11.0)
                    .color(theme::FAINT),
            );
            return;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("kind").size(10.0).color(theme::FAINT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new("total").size(10.0).color(theme::FAINT));
                ui.add_space(24.0);
                ui.label(RichText::new("failed").size(10.0).color(theme::FAINT));
                ui.add_space(18.0);
                ui.label(RichText::new("calls").size(10.0).color(theme::FAINT));
            });
        });
        for (kind, calls, failures, ms) in &kinds {
            theme::stat_row(
                ui,
                &shorten(kind, 24),
                &format!("{calls} · {failures} failed · {ms}ms"),
                if *failures > 0 {
                    theme::RED
                } else {
                    theme::DIM
                },
            );
        }
        if running > 0 {
            ui.label(
                RichText::new(format!("{running} call(s) still running"))
                    .size(10.5)
                    .color(theme::AMBER),
            );
        }
        if let Some(unhandled) = unmodeled(state) {
            ui.label(RichText::new(unhandled).size(10.5).color(theme::FAINT));
        }
    });
}

/// The shells' commands, with what each one cost in context.
///
/// @param state the application state
/// @param ui the interface to draw into
fn commands(state: &HarnessState, ui: &mut Ui) {
    theme::card(ui, theme::ELEVATED, None, |ui| {
        theme::section(ui, "commands");
        let mut any = false;
        for shell in &state.shells {
            if shell.blocks.is_empty() {
                continue;
            }
            any = true;
            ui.label(
                RichText::new(shorten(&shell.name, 30))
                    .size(11.0)
                    .color(theme::TEXT),
            );
            for block in shell.blocks.iter().rev().take(20) {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(block_label(block))
                            .size(11.0)
                            .monospace()
                            .color(block_colour(block)),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} tok if attached",
                                compact(output_tokens(block))
                            ))
                            .size(10.0)
                            .color(theme::FAINT),
                        );
                    });
                });
            }
        }
        if !any {
            ui.label(
                RichText::new("no finished commands yet")
                    .size(11.0)
                    .color(theme::FAINT),
            );
            ui.label(
                RichText::new("a command's output costs nothing until it is attached to a prompt")
                    .size(10.5)
                    .color(theme::FAINT),
            );
        }
    });
}

/// Where the engine is and what it is running on.
///
/// @param state the application state
/// @param ui the interface to draw into
fn engine(state: &HarnessState, ui: &mut Ui) {
    theme::card(ui, theme::PANEL, None, |ui| {
        theme::section(ui, "engine");
        let conversation = &state.agent.conversation;
        theme::stat_row(
            ui,
            "engine",
            &if conversation.engine_name.is_empty() {
                "not started".to_string()
            } else {
                format!(
                    "{} {}",
                    conversation.engine_name, conversation.engine_version
                )
            },
            theme::TEXT,
        );
        theme::stat_row(
            ui,
            "session",
            conversation
                .session_id
                .as_deref()
                .map(|id| shorten(id, 24))
                .as_deref()
                .unwrap_or("none"),
            theme::DIM,
        );
        theme::stat_row(
            ui,
            "model",
            state.agent.applied_model.as_deref().unwrap_or("unset"),
            theme::DIM,
        );
        theme::stat_row(
            ui,
            "workspace route",
            state
                .active_route
                .as_deref()
                .unwrap_or("no route configured"),
            theme::DIM,
        );
        match state.paths() {
            Ok(paths) => {
                theme::stat_row(
                    ui,
                    "harness home",
                    &shorten(&paths.dsh_home.display().to_string(), 46),
                    theme::FAINT,
                );
                theme::stat_row(
                    ui,
                    "engine repo",
                    &shorten(&paths.dsh_repo.display().to_string(), 46),
                    theme::FAINT,
                );
                theme::stat_row(
                    ui,
                    "node",
                    &shorten(&paths.node.display().to_string(), 46),
                    theme::FAINT,
                );
            }
            Err(err) => {
                theme::stat_row(ui, "engine layout", &shorten(err, 46), theme::RED);
            }
        }
        if let Some(git) = state.git.as_ref() {
            theme::stat_row(
                ui,
                "repository",
                &shorten(&git.root().display().to_string(), 46),
                theme::FAINT,
            );
        }
        theme::stat_row(
            ui,
            "repository state",
            &crate::panes::git::status_label(&state.git_view.status),
            theme::DIM,
        );
    });
}

/// The protocol log, including what the app does not model.
///
/// @param state the application state
/// @param ui the interface to draw into
fn log(state: &HarnessState, ui: &mut Ui) {
    let conversation = &state.agent.conversation;
    theme::card(ui, theme::PANEL, None, |ui| {
        theme::section(ui, "engine log");
        let lines: Vec<&String> = conversation.log.iter().rev().take(LOG_LINES).collect();
        if lines.is_empty() {
            ui.label(
                RichText::new("nothing logged yet")
                    .size(11.0)
                    .color(theme::FAINT),
            );
            return;
        }
        ScrollArea::vertical()
            .id_salt("engine-log")
            .max_height(240.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for line in lines.iter().rev() {
                    ui.add(
                        egui::Label::new(
                            RichText::new(shorten(line, 160))
                                .size(10.5)
                                .monospace()
                                .color(theme::DIM),
                        )
                        .truncate(),
                    );
                }
            });
    });
}

/// A sentence about notifications the app does not model.
///
/// The ACP surface grows; a harness that silently drops what it does not know
/// hides a real class of bug. Counting them is the honest minimum.
///
/// @param state the application state
/// @returns the sentence, when there is anything to say
fn unmodeled(state: &HarnessState) -> Option<String> {
    let unhandled = &state.agent.conversation.unhandled;
    if unhandled.is_empty() {
        return None;
    }
    let total: u32 = unhandled.values().sum();
    let names: Vec<&str> = unhandled.keys().take(3).map(String::as_str).collect();
    Some(format!(
        "{total} notification(s) this app does not model: {}",
        names.join(", ")
    ))
}
