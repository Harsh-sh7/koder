//! The conversation: what was asked, what the engine did, and the box to ask again.
//!
//! The transcript is the product's record, so it is drawn as one: the user's own
//! messages as bubbles, the engine's answers as prose with their code kept as
//! code, and every tool call as a row that opens. Above the composer sit the
//! follow-ups that make sense right now — real prompts, derived from the state
//! of the workspace, not a canned list.
//!
//! Everything the engine says about price is in the footer under the card, not
//! here; the conversation stays about the work.

use eframe::egui::{self, Align, CornerRadius, Layout, Rect, RichText, Sense, Stroke, Ui, Vec2};

use harness_core::agent::{FileChange, Item, NoticeLevel, ToolCard, ToolStatus};

use crate::app::shorten;
use crate::icons::{self, Icon};
use crate::state::{first_line, FocusRequest, HarnessState};
use crate::theme;

/// How many transcript items are drawn, counting back from the end.
///
/// A session can run for hours and thousands of tool calls; drawing all of them
/// every frame to show the last screenful is work nobody sees. The transcript is
/// kept whole in memory and in the export — this only bounds the drawing.
const TRANSCRIPT_ITEMS: usize = 400;

/// How many lines of a tool call's output an opened card shows.
const TOOL_OUTPUT_LINES: usize = 60;

/// How many lines of the model's reasoning an opened thought shows.
const THINKING_LINES: usize = 200;

/// How many task-list rows the live card shows before it stops counting.
const TODO_ROWS: usize = 8;

/// How many lines of a change the closed card shows, per side.
const CHANGE_LINES: usize = 4;

/// How many lines of a change the opened card shows, per side.
const CHANGE_LINES_OPEN: usize = 40;

/// How many lines of a delegation's answer ride on the closed card.
const AGENT_RESULT_LINES: usize = 3;

/// Draws the conversation view inside the card's body.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param area the card's body rectangle
pub fn show(state: &mut HarnessState, ui: &mut Ui, area: Rect) {
    let inset = 20.0;
    let left = area.left() + inset;
    let right = area.right() - inset;

    let busy = state.agent.conversation.state.busy();
    let suggestions = if busy {
        Vec::new()
    } else {
        state.suggestions()
    };
    let suggestion_height = if suggestions.is_empty() {
        0.0
    } else {
        suggestions.len() as f32 * 32.0 + 10.0
    };
    let ask_height = if state.agent.pending.is_empty() {
        0.0
    } else {
        state.agent.pending.len().min(2) as f32 * 96.0 + 10.0
    };
    let composer_height = 116.0;

    let bottom = area.bottom() - 12.0;
    let composer = Rect::from_min_max(
        egui::pos2(left, bottom - composer_height),
        egui::pos2(right, bottom),
    );
    let composer_top = composer.top() - 16.0;
    let suggestions_rect = Rect::from_min_max(
        egui::pos2(left, composer_top - suggestion_height + 8.0),
        egui::pos2(right, composer_top - 6.0),
    );
    let asks_rect = Rect::from_min_max(
        egui::pos2(left, suggestions_rect.top() - ask_height),
        egui::pos2(right, suggestions_rect.top() - 4.0),
    );
    let transcript = Rect::from_min_max(
        egui::pos2(left, area.top() + 6.0),
        egui::pos2(right, asks_rect.top() - 6.0),
    );

    transcript_view(state, ui, transcript);
    if !state.agent.pending.is_empty() {
        asks(state, ui, asks_rect);
    }
    if !suggestions.is_empty() {
        suggestions_view(state, ui, suggestions_rect, &suggestions);
    }
    composer_view(state, ui, composer);
}

/// The scrolling record itself.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param rect the rectangle the transcript owns
fn transcript_view(state: &mut HarnessState, ui: &mut Ui, rect: Rect) {
    let conversation = &state.agent.conversation;
    let skipped = conversation.items.len().saturating_sub(TRANSCRIPT_ITEMS);
    let font = theme::mono(state.config.terminal_font_size - 1.0);
    let reveal = state.reveal_call.clone();
    let plan: Vec<(String, String)> = conversation
        .plan
        .iter()
        .map(|entry| (entry.content.clone(), entry.status.clone()))
        .collect();
    let todos = current_todos(&conversation.items);

    let mut toggle_thinking: Option<usize> = None;
    let mut reply: Option<String> = None;
    let mut retry: Option<String> = None;
    let mut rated: Option<(String, Option<i8>)> = None;
    let mut export = false;
    let mut scrolled = false;
    let mut retry_last = false;
    let mut open_models = false;
    let activity = state.activity();
    let failed = matches!(
        state.agent.conversation.state,
        harness_core::agent::RunState::Failed { .. }
    );
    let can_retry = state.agent.last_prompt.is_some();
    let model = crate::shell::model_label(state);

    theme::inside(ui, rect, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("transcript")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if conversation.items.is_empty() {
                    empty(ui, state, rect.width());
                }
                if skipped > 0 {
                    ui.label(
                        RichText::new(format!(
                            "… {skipped} earlier item(s) not drawn; the transcript file has all of them"
                        ))
                        .size(11.5)
                        .color(theme::FAINT),
                    );
                    ui.add_space(6.0);
                }
                // The prompt the last answer was given, for its retry action.
                let mut asked: Option<String> = None;
                for (index, item) in conversation.items[skipped..].iter().enumerate() {
                    match item {
                        Item::User { text, .. } => {
                            asked = Some(text.clone());
                            user_bubble(ui, rect.width(), text);
                        }
                        Item::Assistant { id, text, thinking, thinking_open, rating } => {
                            if !thinking.is_empty() && thinking_view(ui, thinking, *thinking_open) {
                                toggle_thinking = Some(skipped + index);
                            }
                            prose(ui, text, &font);
                            if !text.trim().is_empty() {
                                if let Some(action) = message_actions(ui, text, *rating, asked.is_some()) {
                                    match action {
                                        MessageAction::Quote(quote) => reply = Some(quote),
                                        MessageAction::Retry => retry = asked.clone(),
                                        MessageAction::Rate(value) => rated = Some((id.clone(), value)),
                                        MessageAction::Export => export = true,
                                    }
                                }
                            }
                        }
                        Item::Tool(card) => {
                            if card.is_delegation() {
                                agent_card(ui, card, reveal.as_deref());
                            } else if let Some(change) = card.change().filter(|change| {
                                !change.path.is_empty() || !change.removed.is_empty() || !change.added.is_empty()
                            }) {
                                change_card(ui, card, &change, reveal.as_deref());
                            } else if card.is_todo_write() && !card.todos().is_empty() {
                                // Nothing drawn here: the live card at the end of
                                // the transcript is the newest list, and one card
                                // that updates is the readable shape of a list
                                // the engine replaces whole.
                            } else {
                                tool_row(ui, card, reveal.as_deref());
                            }
                        }
                        Item::Notice { level, text } => notice(ui, *level, text),
                    }
                    ui.add_space(8.0);
                }
                if !todos.is_empty() {
                    todo_card(ui, &todos);
                    ui.add_space(8.0);
                }
                if !plan.is_empty() {
                    plan_card(ui, &plan);
                    ui.add_space(8.0);
                }
                // The live row: what the engine is doing, right now, at the end
                // of the record. A turn can run for minutes, and a transcript
                // that stops at the last finished thing reads as a stall.
                if let Some(activity) = &activity {
                    working_row(ui, activity);
                }
                // A turn that failed is over, so the record ends with the way
                // out rather than with a spinner.
                if failed {
                    match failed_row(ui, can_retry, &model) {
                        Some(FailedAction::Retry) => retry_last = true,
                        Some(FailedAction::Route) => open_models = true,
                        None => {}
                    }
                }
                if let Some(call) = &reveal {
                    // The panel asked for this card: show it, and bring it in.
                    if conversation.items.iter().any(|item| matches!(item, Item::Tool(card) if &card.id == call)) {
                        let focus = ui.min_rect();
                        ui.scroll_to_rect(focus, Some(Align::BOTTOM));
                        scrolled = true;
                    }
                }
            });
    });

    if let Some(index) = toggle_thinking {
        if let Some(Item::Assistant { thinking_open, .. }) =
            state.agent.conversation.items.get_mut(index)
        {
            *thinking_open = !*thinking_open;
        }
    }
    if let Some((id, value)) = rated {
        if state.agent.conversation.rate(&id, value) {
            state.toast(match value {
                Some(rating) if rating > 0 => "noted as useful",
                Some(_) => "noted as off the mark",
                None => "rating cleared",
            });
        }
    }
    if export {
        match state.export_transcript() {
            Ok(path) => state.toast(format!("wrote {}", path.display())),
            Err(err) => state.toast(err),
        }
    }
    if let Some(text) = reply {
        state.agent.input = format!("> {text}\n\n");
        state.focus = Some(FocusRequest::Agent);
    }
    if let Some(prompt) = retry {
        state.send_prompt(&prompt);
    }
    if retry_last {
        state.retry_last_prompt();
    }
    if open_models {
        state.open_model_dialog();
    }
    if scrolled {
        state.reveal_call = None;
    }
}

/// The engine's current task list: the input of the last `todo_write` call.
///
/// dsh replaces the whole list on every call and never maps it to an ACP plan,
/// so the newest call's input is the current state — which is what makes one
/// card at the end of the transcript live rather than a row per call.
///
/// @param items the transcript
/// @returns the (content, status) pairs, empty when the engine wrote none
fn current_todos(items: &[Item]) -> Vec<(String, String)> {
    items
        .iter()
        .rev()
        .find_map(|item| match item {
            Item::Tool(card) if card.is_todo_write() => {
                let todos = card.todos();
                (!todos.is_empty()).then_some(todos)
            }
            _ => None,
        })
        .unwrap_or_default()
}

/// The follow-up prompts under the transcript.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param rect the rectangle the follow-ups own
/// @param suggestions the prompts to offer
fn suggestions_view(state: &mut HarnessState, ui: &mut Ui, rect: Rect, suggestions: &[String]) {
    theme::inside(ui, rect, |ui| {
        for prompt in suggestions {
            let row = theme::list_row(ui, false, false, |ui| {
                ui.add_space(6.0);
                icons::icon(ui, Icon::Reply, theme::FAINT, 15.0);
                ui.add_space(10.0);
                ui.add(
                    egui::Label::new(RichText::new(prompt).size(13.0).color(theme::DIM)).truncate(),
                );
            });
            if row.clicked() {
                state.agent.input = prompt.clone();
                state.focus = Some(FocusRequest::Agent);
            }
        }
    });
}

/// The permission asks the engine is waiting on.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param rect the rectangle the asks own
fn asks(state: &mut HarnessState, ui: &mut Ui, rect: Rect) {
    let mut answer: Option<(usize, Option<String>)> = None;
    theme::inside(ui, rect, |ui| {
        for (index, ask) in state.agent.pending.iter().enumerate().take(2) {
            if index > 0 {
                ui.add_space(6.0);
            }
            egui::Frame::new()
                .fill(theme::AMBER.gamma_multiply(0.08))
                .stroke(Stroke::new(1.0, theme::AMBER.gamma_multiply(0.4)))
                .corner_radius(CornerRadius::same(theme::RADIUS))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("the engine is waiting")
                                .size(12.0)
                                .color(theme::AMBER)
                                .strong(),
                        );
                        ui.add_space(8.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(shorten(&ask.title, 90))
                                    .size(12.5)
                                    .monospace()
                                    .color(theme::TEXT),
                            )
                            .truncate(),
                        );
                    });
                    if let Some(reason) = &ask.reason {
                        ui.label(
                            RichText::new(shorten(reason, 200))
                                .size(12.0)
                                .color(theme::DIM),
                        );
                    }
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        for option in &ask.options {
                            let colour = match option.kind.as_str() {
                                "allow_always" => theme::BLUE,
                                "allow_once" => theme::GREEN,
                                "reject_once" | "reject_always" => theme::RED,
                                _ => theme::DIM,
                            };
                            if theme::action(ui, &option.name, true, colour).clicked() {
                                answer = Some((index, Some(option.option_id.clone())));
                            }
                        }
                        if theme::action(ui, "cancel", true, theme::FAINT).clicked() {
                            answer = Some((index, None));
                        }
                    });
                });
        }
    });
    if let Some((index, option)) = answer {
        state.answer_permission(index, option);
    }
}

/// The composer: the prompt box, the two switches beside it, and send.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param rect the composer card's rectangle
fn composer_view(state: &mut HarnessState, ui: &mut Ui, rect: Rect) {
    // Cloned so the card's own edges can be painted while its contents borrow
    // the interface mutably.
    let painter = ui.painter().clone();
    painter.add(theme::card_shadow().as_shape(rect, CornerRadius::same(14)));
    painter.rect_filled(rect, CornerRadius::same(14), theme::PANEL);
    painter.rect_stroke(
        rect,
        CornerRadius::same(14),
        Stroke::new(1.0, theme::BORDER_STRONG),
        egui::StrokeKind::Inside,
    );

    let control_height = 32.0;
    let inner = rect.shrink2(Vec2::new(14.0, 12.0));
    let rule_y = inner.bottom() - control_height - 8.0;
    let text_rect = Rect::from_min_max(inner.min, egui::pos2(inner.right(), rule_y - 8.0));
    let control = Rect::from_min_max(egui::pos2(inner.left(), rule_y + 8.0), inner.max);

    // The attached-command chip rides above the prompt, since it is about what
    // will be sent rather than about what is being typed.
    let attached = state.agent.attached_block.clone();
    let mut drop_attachment = false;
    if let Some(block) = &attached {
        theme::inside(
            ui,
            Rect::from_min_size(text_rect.min, Vec2::new(text_rect.width(), 20.0)),
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("attached").size(11.5).color(theme::FAINT));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(shorten(block, 58))
                            .size(11.5)
                            .monospace()
                            .color(theme::BLUE),
                    );
                    if icons::icon(ui, Icon::Plus, theme::FAINT, 13.0)
                        .on_hover_text("drop the attachment")
                        .clicked()
                    {
                        drop_attachment = true;
                    }
                });
            },
        );
    }
    if drop_attachment {
        state.agent.attached_block = None;
    }

    let rows = 2;
    let hint = if state.models.active.is_some() {
        "Ask for a change, attach a command's output, or describe what to build"
    } else {
        "Add a model route first — over ⌘, — the engine has nothing to run on"
    };
    let mut send = false;
    let mut open_models = false;
    let mut toggle_auto = false;
    let mut attach_last = false;
    let mut open_add = false;

    let text_top = if attached.is_some() {
        text_rect.top() + 24.0
    } else {
        text_rect.top()
    };
    theme::inside(
        ui,
        Rect::from_min_max(egui::pos2(text_rect.left(), text_top), text_rect.max),
        |ui| {
            let response = ui.add(
                egui::TextEdit::multiline(&mut state.agent.input)
                    .id(egui::Id::new("agent-prompt"))
                    .frame(egui::Frame::NONE)
                    .desired_rows(rows)
                    .desired_width(f32::INFINITY)
                    .hint_text(RichText::new(hint).size(13.0).color(theme::FAINT)),
            );
            if state
                .focus
                .take_if(|focus| *focus == FocusRequest::Agent)
                .is_some()
            {
                response.request_focus();
            }
            // ⌘↩ sends; a bare return is a newline, because prompts are paragraphs.
            if response.has_focus()
                && ui.input_mut(|input| {
                    input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter)
                })
            {
                send = true;
            }
        },
    );

    // The dashed separator the reference draws over the control row.
    let mut x = inner.left();
    while x < inner.right() {
        let end = (x + 4.0).min(inner.right());
        painter.hline(x..=end, rule_y, Stroke::new(1.0, theme::BORDER));
        x += 8.0;
    }

    theme::inside(ui, control, |ui| {
        ui.horizontal(|ui| {
            // Both halves of this row must agree on where the seam is, so the
            // right group's width is computed from the marks it always draws
            // plus whatever the model's name may take. Guessing a fraction of
            // the row instead lets that group slide left over this one when the
            // window narrows. The name is the only elastic thing here: it takes
            // what the fixed marks leave, and is dropped when that is too little
            // to read.
            const GAP: f32 = 8.0;
            const SEND: f32 = 30.0;
            const MIC: f32 = 26.0;
            const CHIP: f32 = 20.0;
            const CHEVRON: f32 = 12.0;
            const LEFT_MARKS: f32 = 26.0 * 3.0 + GAP * 2.0; // plus, clip, play
            const RIGHT_MARKS: f32 = SEND + GAP + MIC + GAP + CHIP;
            const NAME_TAIL: f32 = CHEVRON + GAP + GAP; // and a gap on either side of the name
            let row = ui.available_width();
            let name_budget = (row - LEFT_MARKS - RIGHT_MARKS - NAME_TAIL - GAP).clamp(0.0, 200.0);
            let show_name = name_budget >= 40.0;
            let reserved = RIGHT_MARKS
                + if show_name {
                    NAME_TAIL + name_budget
                } else {
                    0.0
                };
            ui.scope(|ui| {
                ui.set_max_width((row - reserved - GAP).max(LEFT_MARKS));
                ui.spacing_mut().item_spacing.x = GAP;
                if icons::button(ui, Icon::Plus, theme::DIM, 26.0, theme::HOVER_SOFT)
                    .on_hover_text("add something to the prompt")
                    .clicked()
                {
                    open_add = true;
                }
                if icons::button(ui, Icon::Clip, theme::DIM, 26.0, theme::HOVER_SOFT)
                    .on_hover_text("attach the last finished command's output")
                    .clicked()
                {
                    attach_last = true;
                }
                let auto = state.agent.auto_approve;
                let colour = if auto { theme::ACCENT } else { theme::DIM };
                let play = icons::button(ui, Icon::Play, colour, 26.0, theme::HOVER_SOFT);
                // In a narrow window the label is dropped rather than clipped
                // to an ellipsis: the play mark still says what it does, and a
                // lone "…" says nothing.
                let mut clicked = play.clicked();
                if ui.available_width() > 80.0 {
                    let label = ui
                        .add(
                            egui::Label::new(
                                RichText::new("Auto approve").size(12.5).color(colour),
                            )
                            .sense(Sense::click())
                            .truncate(),
                        )
                        .on_hover_text("answer permission asks with their narrowest allow option");
                    icons::icon(ui, Icon::Chevron, theme::FAINT, 12.0);
                    clicked |= label.clicked();
                } else {
                    play.on_hover_text("auto approve is off");
                }
                if clicked {
                    toggle_auto = true;
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                let can_send = !state.agent.input.trim().is_empty();
                let busy = state.agent.conversation.state.busy();
                let size = Vec2::splat(30.0);
                let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                let painter = ui.painter();
                // The send mark keeps its dark disc whether or not there is
                // anything to send — where it is does not move — and only
                // lightens when it has nothing to carry.
                let fill = if response.hovered() && can_send {
                    theme::ACCENT
                } else if can_send {
                    theme::TEXT
                } else {
                    theme::TEXT.gamma_multiply(0.45)
                };
                painter.rect_filled(rect, CornerRadius::same(9), fill);
                icons::paint(ui, Icon::ArrowUp, rect.shrink(8.0), theme::PANEL);
                if response
                    .on_hover_text(if !can_send {
                        "write a prompt first"
                    } else if busy {
                        "queue this prompt — the engine is working"
                    } else {
                        "send ⌘↩"
                    })
                    .clicked()
                    && can_send
                {
                    send = true;
                }
                let mic = icons::button(ui, Icon::Mic, theme::FAINT, 26.0, theme::HOVER_SOFT);
                if mic
                    .on_hover_text("dictation belongs to the platform: press fn twice and talk")
                    .clicked()
                {
                    state
                        .toast("dictation is the Mac's — press fn twice, then talk into any field");
                }
                let model = state
                    .agent
                    .applied_model
                    .clone()
                    .or_else(|| state.active_route.clone())
                    .unwrap_or_else(|| "no model configured".to_string());
                // Sized rather than merely truncated: a label in a right-to-left
                // layout sizes to its text, so an unbounded name would push the
                // whole group left past the seam. The scope keeps the name in
                // its budget while still letting the text be as wide as it is,
                // so the mark and the name stay next to each other.
                let mut named = false;
                let mut chevron = None;
                if show_name {
                    chevron = Some(icons::icon(ui, Icon::Chevron, theme::FAINT, 12.0));
                    let name = ui
                        .scope(|ui| {
                            ui.set_max_width(name_budget);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(shorten(&model, 30))
                                        .size(12.5)
                                        .color(theme::TEXT)
                                        .strong(),
                                )
                                .sense(Sense::click())
                                .truncate(),
                            )
                        })
                        .inner
                        .on_hover_text("model route — click to change");
                    named = name.clicked();
                }
                let tile =
                    icons::tile(ui, Icon::Harness, theme::ACCENT, 20.0, theme::ACCENT_DIM, 6);
                let tile = tile
                    .on_hover_text(format!("model route: {model} — click to change the route"))
                    .interact(Sense::click());
                if named || tile.clicked() || chevron.is_some_and(|chevron| chevron.clicked()) {
                    open_models = true;
                }
            });
        });
    });

    if open_add {
        state.palette = Some(crate::state::Palette::new(crate::state::PaletteMode::Files));
        state.focus = Some(FocusRequest::Palette);
    }
    if attach_last {
        let index = state.active_shell;
        state.refresh_blocks(index);
        match state
            .shells
            .get(index)
            .and_then(|shell| shell.blocks.len().checked_sub(1))
        {
            Some(last) => state.attach_block(last),
            None => state.toast("no finished command to attach yet"),
        }
    }
    if toggle_auto {
        state.agent.auto_approve = !state.agent.auto_approve;
    }
    if send {
        let text = state.agent.input.trim().to_string();
        if !text.is_empty() {
            state.send_prompt(&text);
        }
    }
    if open_models {
        state.open_model_dialog();
    }
}

/// The empty state: what is missing, and the one click that fixes it.
///
/// @param ui the interface to draw into
/// @param state the application state
/// @param width the transcript's width
fn empty(ui: &mut Ui, state: &HarnessState, width: f32) {
    ui.add_space(30.0);
    let missing_model = state.models.active.is_none();
    let (headline, lines): (&str, [&str; 2]) = if missing_model {
        (
            "no model route yet",
            [
                "a route is a provider, a base URL, and the models it serves",
                "the key is read from the environment; nothing is bundled here",
            ],
        )
    } else if state.agent.start_error.is_some() {
        (
            "the engine did not start",
            [
                "set the route in .harness/models.json, then press restart",
                "the model editor opens over ⌘,",
            ],
        )
    } else {
        (
            "nothing asked yet",
            [
                "type below, and every tool call the engine makes appears here",
                "the panel on the right lists what it delegates and runs",
            ],
        )
    };
    theme::inside(
        ui,
        Rect::from_min_size(ui.cursor().min, Vec2::new(width, 160.0)),
        |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                ui.label(RichText::new(headline).size(14.0).color(theme::DIM));
                ui.add_space(6.0);
                for line in lines {
                    ui.label(RichText::new(line).size(12.5).color(theme::FAINT));
                }
                if let Some(error) = &state.agent.start_error {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(shorten(error, 140))
                            .size(12.0)
                            .color(theme::RED),
                    );
                }
            });
        },
    );
}

/// A message the user sent.
///
/// @param ui the interface to draw into
/// @param width the transcript's width
/// @param text the message
fn user_bubble(ui: &mut Ui, width: f32, text: &str) {
    let bubble_width = (width * 0.72).min(width - 40.0);
    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
        egui::Frame::new()
            .fill(theme::ELEVATED)
            .corner_radius(CornerRadius::same(theme::CARD_RADIUS))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_max_width(bubble_width);
                ui.label(RichText::new(text).size(13.5).color(theme::TEXT));
            });
    });
}

/// What a click on the answer's action row asked for.
enum MessageAction {
    /// Quote the answer and ask again from it.
    Quote(String),
    /// Send the prompt this answer was given, again.
    Retry,
    /// Record what the user thought of the answer, or clear it.
    Rate(Option<i8>),
    /// Write the transcript to `.harness/transcripts`.
    Export,
}

/// The row of actions under an answer.
///
/// @param ui the interface to draw into
/// @param text the answer's text, for the clipboard
/// @param rating what the user already said about it, if anything
/// @param can_retry whether a previous prompt exists to send again
/// @returns what was clicked, if anything
fn message_actions(
    ui: &mut Ui,
    text: &str,
    rating: Option<i8>,
    can_retry: bool,
) -> Option<MessageAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let copy = icons::button(ui, Icon::Copy, theme::FAINT, 24.0, theme::HOVER_SOFT);
        if copy.on_hover_text("copy this answer").clicked() {
            ui.ctx().copy_text(text.to_string());
        }
        let up = icons::button(
            ui,
            Icon::ThumbUp,
            if rating == Some(1) {
                theme::GREEN
            } else {
                theme::FAINT
            },
            24.0,
            theme::HOVER_SOFT,
        );
        if up
            .on_hover_text(if rating == Some(1) {
                "rated useful — click to clear"
            } else {
                "rate this answer useful"
            })
            .clicked()
        {
            action = Some(MessageAction::Rate(if rating == Some(1) {
                None
            } else {
                Some(1)
            }));
        }
        let down = icons::button(
            ui,
            Icon::ThumbDown,
            if rating == Some(-1) {
                theme::AMBER
            } else {
                theme::FAINT
            },
            24.0,
            theme::HOVER_SOFT,
        );
        if down
            .on_hover_text(if rating == Some(-1) {
                "rated off the mark — click to clear"
            } else {
                "rate this answer off the mark"
            })
            .clicked()
        {
            action = Some(MessageAction::Rate(if rating == Some(-1) {
                None
            } else {
                Some(-1)
            }));
        }
        if can_retry {
            let again = icons::button(ui, Icon::Refresh, theme::FAINT, 24.0, theme::HOVER_SOFT);
            if again
                .on_hover_text("ask again — send the previous prompt a second time")
                .clicked()
            {
                action = Some(MessageAction::Retry);
            }
        }
        let answer = icons::button(ui, Icon::Reply, theme::FAINT, 24.0, theme::HOVER_SOFT);
        if answer
            .on_hover_text("quote this answer and ask again")
            .clicked()
        {
            let line = text
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("")
                .trim();
            action = Some(MessageAction::Quote(shorten(line, 120)));
        }
        let more = icons::button(ui, Icon::More, theme::FAINT, 24.0, theme::HOVER_SOFT)
            .on_hover_text("more");
        let first = text
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_string();
        egui::Popup::menu(&more).show(|ui| {
            ui.set_min_width(180.0);
            if ui.button("copy the answer").clicked() {
                ui.ctx().copy_text(text.to_string());
                ui.close();
            }
            if ui.button("quote and ask again").clicked() {
                action = Some(MessageAction::Quote(shorten(&first, 120)));
                ui.close();
            }
            if can_retry && ui.button("ask the same thing again").clicked() {
                action = Some(MessageAction::Retry);
                ui.close();
            }
            if ui.button("export the transcript").clicked() {
                action = Some(MessageAction::Export);
                ui.close();
            }
        });
    });
    action
}

/// What the failure row offers.
enum FailedAction {
    /// Ask the last prompt again, on this route.
    Retry,
    /// Open the route dialog, to run it elsewhere.
    Route,
}

/// The row under a transcript whose turn failed: the two ways out of a dead end.
///
/// The reason is already in the transcript above — the failure wrote a notice —
/// so this row is the repair: ask again, on this route or on another.
///
/// @param ui the interface to draw into
/// @param can_retry whether there is a prompt to ask again
/// @param model the route the session was on
/// @returns what the user asked for, if anything
fn failed_row(ui: &mut Ui, can_retry: bool, model: &str) -> Option<FailedAction> {
    let mut action = None;
    egui::Frame::new()
        .fill(theme::RED.gamma_multiply(0.05))
        .stroke(egui::Stroke::new(1.0, theme::RED.gamma_multiply(0.28)))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                icons::icon(ui, Icon::Sparkle, theme::RED, 14.0);
                ui.add_space(9.0);
                ui.label(
                    RichText::new("the turn did not finish")
                        .size(12.5)
                        .color(theme::TEXT),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::action(ui, "another route", true, theme::DIM)
                        .on_hover_text("open models and routes (⌘,)")
                        .clicked()
                    {
                        action = Some(FailedAction::Route);
                    }
                    ui.add_space(2.0);
                    if theme::action(ui, "try again", can_retry, theme::ACCENT)
                        .on_hover_text(if can_retry {
                            "ask the last request again, on the same route"
                        } else {
                            "nothing has been asked yet"
                        })
                        .clicked()
                    {
                        action = Some(FailedAction::Retry);
                    }
                });
            });
            ui.add_space(4.0);
            ui.label(RichText::new(model).size(11.5).color(theme::FAINT));
        });
    action
}

/// The live row under the transcript: the turning arc, what is happening, and
/// how long it has been happening.
///
/// @param ui the interface to draw into
/// @param activity what the session is doing
fn working_row(ui: &mut Ui, activity: &crate::state::Activity) {
    egui::Frame::new()
        .fill(theme::ELEVATED)
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::spinner(ui, 14.0, theme::ACCENT);
                ui.add_space(9.0);
                ui.label(
                    RichText::new(&activity.headline)
                        .size(12.5)
                        .color(theme::TEXT),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(elapsed) = activity.elapsed {
                        ui.label(
                            RichText::new(crate::state::elapsed_label(elapsed))
                                .size(11.5)
                                .monospace()
                                .color(theme::FAINT),
                        )
                        .on_hover_text("how long this turn has been running");
                    }
                });
            });
            if let Some(detail) = &activity.detail {
                ui.add_space(3.0);
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
        });
}

/// The model's reasoning, folded away until it is asked for.
///
/// @param ui the interface to draw into
/// @param thinking the reasoning text
/// @param open whether it is expanded
/// @returns true when the header was clicked
fn thinking_view(ui: &mut Ui, thinking: &str, open: bool) -> bool {
    let lines = thinking.lines().count();
    let mut clicked = false;
    ui.horizontal(|ui| {
        let chevron = icons::button(ui, Icon::Chevron, theme::VIOLET, 20.0, theme::HOVER_SOFT);
        let label = ui.add(
            egui::Label::new(
                RichText::new(format!("thought for {lines} line(s)"))
                    .size(12.0)
                    .color(theme::VIOLET.gamma_multiply(0.9)),
            )
            .sense(Sense::click()),
        );
        clicked = chevron.clicked() || label.clicked();
    });
    if open {
        egui::Frame::new()
            .fill(theme::VIOLET.gamma_multiply(0.06))
            .corner_radius(CornerRadius::same(theme::RADIUS))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for line in thinking.lines().take(THINKING_LINES) {
                    ui.label(
                        RichText::new(line)
                            .size(12.0)
                            .italics()
                            .color(theme::VIOLET.gamma_multiply(0.8)),
                    );
                }
            });
    }
    // Open thought is a folded list, not a paragraph: the chevron points down.
    let _ = open;
    clicked
}

/// An answer: prose with its code kept as code, and its inline code as chips.
///
/// @param ui the interface to draw into
/// @param text the answer
/// @param font the monospace font
fn prose(ui: &mut Ui, text: &str, font: &egui::FontId) {
    let mut fence: Option<(String, Vec<&str>)> = None;
    let mut paragraph: Vec<&str> = Vec::new();

    for line in text.lines() {
        if let Some(info) = line.trim_start().strip_prefix("```") {
            match fence.take() {
                Some((language, body)) => code_block(ui, &language, &body),
                None => {
                    flush(ui, &mut paragraph);
                    fence = Some((info.trim().to_string(), Vec::new()));
                }
            }
            continue;
        }
        match fence.as_mut() {
            Some((_, body)) => body.push(line),
            None => paragraph.push(line),
        }
    }
    // An unterminated fence is still the model's answer, so it is shown as code.
    if let Some((language, body)) = fence {
        code_block(ui, &language, &body);
    }
    flush(ui, &mut paragraph);
    let _ = font;
}

/// Draws the prose lines gathered so far: bullets, then a wrapping paragraph.
///
/// @param ui the interface to draw into
/// @param paragraph the lines, drained
fn flush(ui: &mut Ui, paragraph: &mut Vec<&str>) {
    let lines: Vec<String> = paragraph.iter().map(|line| line.to_string()).collect();
    paragraph.clear();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim_end();
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        let trimmed = line.trim_start();
        let bullet =
            trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("• ");
        if bullet {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                inline_runs(ui, "• ", theme::DIM, false);
                inline_runs(ui, trimmed[2..].trim_start(), theme::TEXT, false);
            });
            index += 1;
            continue;
        }
        // A paragraph runs until a blank line, a bullet, or a fence.
        let mut body = Vec::new();
        while index < lines.len() {
            let next = lines[index].trim_end();
            let next_trimmed = next.trim_start();
            let stops = next_trimmed.is_empty()
                || next_trimmed.starts_with("- ")
                || next_trimmed.starts_with("* ")
                || next_trimmed.starts_with("• ");
            if stops && !body.is_empty() {
                break;
            }
            if next_trimmed.is_empty() {
                index += 1;
                continue;
            }
            body.push(next.trim());
            index += 1;
        }
        let joined = body.join(" ");
        let heading = joined.starts_with('#');
        let text = if heading {
            joined.trim_start_matches('#').trim_start()
        } else {
            joined.as_str()
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            inline_runs(
                ui,
                text,
                if heading { theme::TEXT } else { theme::TEXT },
                heading,
            );
        });
    }
}

/// Draws one line's inline runs: plain words, `code` chips, and **emphasis**.
///
/// @param ui the interface to draw into
/// @param text the line
/// @param colour the text colour
/// @param heading whether the line is a heading
fn inline_runs(ui: &mut Ui, text: &str, colour: egui::Color32, heading: bool) {
    for run in runs(text) {
        match run {
            Run::Code(code) => {
                let galley =
                    ui.painter()
                        .layout_no_wrap(code.clone(), theme::mono(12.5), theme::TEXT);
                let padding = Vec2::new(4.0, 2.0);
                let (rect, _) =
                    ui.allocate_exact_size(galley.size() + padding * 2.0, Sense::hover());
                let painter = ui.painter();
                painter.rect_filled(rect, CornerRadius::same(4), theme::ELEVATED);
                painter.galley(rect.min + padding, galley, theme::TEXT);
                ui.add_space(3.0);
            }
            Run::Bold(strong) => {
                for word in words(&strong) {
                    ui.label(RichText::new(word).size(13.5).strong().color(colour));
                }
            }
            Run::Plain(plain) => {
                for word in words(&plain) {
                    ui.label(
                        RichText::new(word)
                            .size(if heading { 14.5 } else { 13.5 })
                            .color(colour),
                    );
                }
            }
        }
    }
}

/// One piece of a line, by how it is marked up.
enum Run {
    /// Ordinary text.
    Plain(String),
    /// A `code` span.
    Code(String),
    /// A **strong** span.
    Bold(String),
}

/// Splits a line into runs by its inline markup.
///
/// @param text the line
/// @returns the runs, in order
fn runs(text: &str) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let code_at = rest.find('`');
        let bold_at = rest.find("**");
        let next = match (code_at, bold_at) {
            (Some(code), Some(bold)) => Some(code.min(bold)),
            (Some(code), None) => Some(code),
            (None, Some(bold)) => Some(bold),
            (None, None) => None,
        };
        let Some(at) = next else {
            push_run(&mut runs, Run::Plain(rest.to_string()));
            break;
        };
        if at > 0 {
            push_run(&mut runs, Run::Plain(rest[..at].to_string()));
        }
        let tail = &rest[at..];
        if tail.starts_with('`') {
            match tail[1..].find('`') {
                Some(end) => {
                    push_run(&mut runs, Run::Code(tail[1..1 + end].to_string()));
                    rest = &tail[end + 2..];
                }
                None => {
                    push_run(&mut runs, Run::Plain(tail.to_string()));
                    break;
                }
            }
        } else {
            match tail[2..].find("**") {
                Some(end) => {
                    push_run(&mut runs, Run::Bold(tail[2..2 + end].to_string()));
                    rest = &tail[end + 4..];
                }
                None => {
                    push_run(&mut runs, Run::Plain(tail.to_string()));
                    break;
                }
            }
        }
    }
    runs
}

/// Adds a run, merging adjacent plain runs so a line does not become a dozen.
///
/// @param runs the runs so far
/// @param run the run to add
fn push_run(runs: &mut Vec<Run>, run: Run) {
    match (runs.last_mut(), run) {
        (Some(Run::Plain(last)), Run::Plain(text)) => last.push_str(&text),
        (_, run) => runs.push(run),
    }
}

/// Splits text into words that keep their trailing space, so wrapping is even.
///
/// @param text the text
/// @returns the words
fn words(text: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for (index, word) in text.split(' ').enumerate() {
        if index == 0 {
            words.push(word.to_string());
        } else {
            words.push(format!(" {word}"));
        }
    }
    words.retain(|word| !word.is_empty());
    words
}

/// A fenced code block from an answer.
///
/// @param ui the interface to draw into
/// @param language the fence's info string
/// @param body the lines
fn code_block(ui: &mut Ui, language: &str, body: &[&str]) {
    let shown = body.len().min(400);
    let language = if language.is_empty() {
        "text"
    } else {
        language
    };
    let font = theme::mono(12.0);
    egui::Frame::new()
        .fill(theme::ELEVATED)
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(language).size(11.0).color(theme::FAINT));
                ui.label(
                    RichText::new(format!("{} line(s)", body.len()))
                        .size(11.0)
                        .color(theme::FAINT),
                );
            });
            ui.add_space(2.0);
            egui::ScrollArea::horizontal()
                .id_salt(language)
                .show(ui, |ui| {
                    for line in &body[..shown] {
                        let job = crate::code::highlight(language, line, &font, theme::TEXT);
                        ui.label(job);
                    }
                    if body.len() > shown {
                        ui.label(
                            RichText::new(format!("… {} of {} lines", shown, body.len()))
                                .size(11.0)
                                .color(theme::AMBER),
                        );
                    }
                });
        });
}

/// One tool call: a row that opens into its input and output.
///
/// @param ui the interface to draw into
/// @param card the call
/// @param reveal the call the panel asked to open
fn tool_row(ui: &mut Ui, card: &ToolCard, reveal: Option<&str>) {
    let colour = match card.status {
        ToolStatus::Pending | ToolStatus::Running => theme::AMBER,
        ToolStatus::Completed => theme::GREEN,
        ToolStatus::Failed => theme::RED,
    };
    let escalation = card.is_budget_escalation();
    let open_id = ui.id().with(("tool-open", card.id.as_str()));
    let mut open = ui
        .ctx()
        .data_mut(|data| *data.get_temp_mut_or(open_id, false));
    if reveal == Some(card.id.as_str()) {
        open = true;
    }
    let icon = tool_icon(card);
    let summary = card.summary();
    let elapsed = card
        .elapsed_ms
        .map(|ms| format!("{ms} ms"))
        .unwrap_or_default();
    let has_detail = !card.output.trim().is_empty() || card.input.is_some();

    let response = egui::Frame::new()
        .fill(theme::PANEL)
        .stroke(Stroke::new(
            1.0,
            if open {
                theme::BORDER_STRONG
            } else {
                theme::BORDER
            },
        ))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                icons::tile(ui, icon, colour, 22.0, theme::ELEVATED, 6);
                ui.add_space(9.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(shorten(&card.title, 40))
                            .size(12.5)
                            .monospace()
                            .color(if escalation {
                                theme::VIOLET
                            } else {
                                theme::TEXT
                            }),
                    )
                    .truncate(),
                );
                if !summary.is_empty() {
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(shorten(&summary, 90))
                                .size(12.0)
                                .color(theme::DIM),
                        )
                        .truncate(),
                    );
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    match card.status {
                        ToolStatus::Pending | ToolStatus::Running => {
                            icons::icon(ui, Icon::Ring, colour, 13.0);
                        }
                        ToolStatus::Completed => {
                            icons::icon(ui, Icon::Check, colour, 13.0);
                        }
                        ToolStatus::Failed => {
                            icons::icon(ui, Icon::More, colour, 13.0);
                        }
                    }
                    ui.add_space(6.0);
                    if !elapsed.is_empty() {
                        ui.label(RichText::new(elapsed).size(11.0).color(theme::FAINT));
                    }
                    if has_detail {
                        icons::icon(ui, Icon::Chevron, theme::FAINT, 12.0);
                    }
                });
            });
            if open {
                ui.add_space(6.0);
                if let Some(input) = &card.input {
                    let text =
                        serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
                    detail(ui, "input", &shorten_lines(&text, 40), theme::DIM);
                }
                if !card.output.trim().is_empty() {
                    detail(ui, "output", &card.output, theme::TEXT);
                }
                if !card.locations.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        for location in card.locations.iter().take(6) {
                            ui.label(
                                RichText::new(shorten(location, 70))
                                    .size(11.5)
                                    .monospace()
                                    .color(theme::BLUE),
                            );
                        }
                    });
                }
            }
        })
        .response;

    if response.interact(Sense::click()).clicked() && has_detail {
        open = !open;
    }
    ui.ctx().data_mut(|data| data.insert_temp(open_id, open));
}

/// One labelled block of a tool call's detail.
///
/// @param ui the interface to draw into
/// @param label the block's name
/// @param text the block's text
/// @param colour the text colour
fn detail(ui: &mut Ui, label: &str, text: &str, colour: egui::Color32) {
    ui.label(RichText::new(label).size(11.0).color(theme::FAINT));
    egui::Frame::new()
        .fill(theme::ELEVATED)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::horizontal()
                .id_salt(label)
                .show(ui, |ui| {
                    for line in text.lines().take(TOOL_OUTPUT_LINES) {
                        ui.label(RichText::new(line).size(12.0).monospace().color(colour));
                    }
                });
        });
    ui.add_space(4.0);
}

/// A delegation: who is working, on what, and what came back.
///
/// The engine does not stream a child's steps to the ACP client — the parent's
/// one call and its result are all that is on the wire — so this is the honest
/// shape of a sub-agent: the task it was handed, whether it is still running,
/// and its answer once it lands. Clicking opens the call and its whole result.
///
/// @param ui the interface to draw into
/// @param card the call
/// @param reveal the call the panel asked to open
fn agent_card(ui: &mut Ui, card: &ToolCard, reveal: Option<&str>) {
    let colour = match card.status {
        ToolStatus::Pending | ToolStatus::Running => theme::AMBER,
        ToolStatus::Completed => theme::GREEN,
        ToolStatus::Failed => theme::RED,
    };
    let open_id = ui.id().with(("agent-open", card.id.as_str()));
    let mut open = ui
        .ctx()
        .data_mut(|data| *data.get_temp_mut_or(open_id, false));
    if reveal == Some(card.id.as_str()) {
        open = true;
    }
    let headline = card
        .input_text(&["description", "decision", "task", "summary"])
        .unwrap_or_else(|| card.title.clone());
    let handed = card.input_text(&["prompt", "evidence"]);
    let elapsed = card
        .elapsed_ms
        .map(|ms| format!("{ms} ms"))
        .unwrap_or_default();
    let result: Vec<&str> = card
        .output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(AGENT_RESULT_LINES)
        .collect();

    // The right group needs room for a status word, its mark, and a time; the
    // headline column takes what is left so the two never overlap.
    const RIGHT: f32 = 132.0;
    let response = egui::Frame::new()
        .fill(theme::PANEL)
        .stroke(Stroke::new(
            1.0,
            if open {
                theme::BORDER_STRONG
            } else {
                theme::BORDER
            },
        ))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                icons::tile(ui, Icon::Harness, theme::ACCENT, 22.0, theme::ACCENT_DIM, 6);
                ui.add_space(9.0);
                let room = ui.available_width();
                ui.scope(|ui| {
                    ui.set_max_width((room - RIGHT).max(80.0));
                    ui.vertical(|ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(shorten(&headline, 94))
                                    .size(12.5)
                                    .strong()
                                    .color(theme::TEXT),
                            )
                            .truncate(),
                        );
                        if let Some(handed) = &handed {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(shorten(&first_line(handed), 110))
                                        .size(11.5)
                                        .monospace()
                                        .color(theme::DIM),
                                )
                                .truncate(),
                            );
                        }
                    });
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    match card.status {
                        ToolStatus::Pending | ToolStatus::Running => {
                            icons::icon(ui, Icon::Ring, colour, 13.0);
                        }
                        ToolStatus::Completed => {
                            icons::icon(ui, Icon::Check, colour, 13.0);
                        }
                        ToolStatus::Failed => {
                            icons::icon(ui, Icon::More, colour, 13.0);
                        }
                    }
                    ui.add_space(5.0);
                    ui.label(RichText::new(card.status.label()).size(11.5).color(colour));
                    if !elapsed.is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new(elapsed).size(11.0).color(theme::FAINT));
                    }
                });
            });
            if open {
                ui.add_space(6.0);
                if let Some(input) = &card.input {
                    let text =
                        serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
                    detail(ui, "input", &shorten_lines(&text, 40), theme::DIM);
                }
                if !card.output.trim().is_empty() {
                    detail(ui, "result", &card.output, theme::TEXT);
                }
            } else if !result.is_empty() {
                // A line of rule, then the answer's opening: enough to see how
                // it went without opening the card.
                ui.add_space(6.0);
                let (rule, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
                ui.painter()
                    .rect_filled(rule, CornerRadius::same(1), theme::BORDER);
                ui.add_space(6.0);
                for line in &result {
                    ui.add(
                        egui::Label::new(
                            RichText::new(shorten(line, 120))
                                .size(11.5)
                                .monospace()
                                .color(theme::DIM),
                        )
                        .truncate(),
                    );
                }
            }
        })
        .response;

    if response.interact(Sense::click()).clicked() {
        open = !open;
    }
    ui.ctx().data_mut(|data| data.insert_temp(open_id, open));
}

/// A file change: the path, the tally, and the lines themselves.
///
/// The engine sends the call's arguments and its one-line confirmation, not a
/// patch, so the diff is derived from the arguments the model wrote — the text
/// it asked to remove and the text it asked to add. That is exactly the change
/// that was requested, which is what the card should show.
///
/// @param ui the interface to draw into
/// @param card the call
/// @param change the derived change
/// @param reveal the call the panel asked to open
fn change_card(ui: &mut Ui, card: &ToolCard, change: &FileChange, reveal: Option<&str>) {
    let colour = match card.status {
        ToolStatus::Pending | ToolStatus::Running => theme::AMBER,
        ToolStatus::Completed => theme::GREEN,
        ToolStatus::Failed => theme::RED,
    };
    let open_id = ui.id().with(("change-open", card.id.as_str()));
    let mut open = ui
        .ctx()
        .data_mut(|data| *data.get_temp_mut_or(open_id, false));
    if reveal == Some(card.id.as_str()) {
        open = true;
    }
    let elapsed = card
        .elapsed_ms
        .map(|ms| format!("{ms} ms"))
        .unwrap_or_default();

    let response = egui::Frame::new()
        .fill(theme::PANEL)
        .stroke(Stroke::new(
            1.0,
            if open {
                theme::BORDER_STRONG
            } else {
                theme::BORDER
            },
        ))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                icons::tile(ui, Icon::Doc, theme::GREEN, 22.0, theme::ELEVATED, 6);
                ui.add_space(9.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(shorten(&change.path, 64))
                            .size(12.0)
                            .monospace()
                            .color(theme::TEXT),
                    )
                    .truncate(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    match card.status {
                        ToolStatus::Pending | ToolStatus::Running => {
                            icons::icon(ui, Icon::Ring, colour, 13.0);
                        }
                        ToolStatus::Completed => {
                            icons::icon(ui, Icon::Check, colour, 13.0);
                        }
                        ToolStatus::Failed => {
                            icons::icon(ui, Icon::More, colour, 13.0);
                        }
                    }
                    ui.add_space(6.0);
                    if !elapsed.is_empty() {
                        ui.label(RichText::new(elapsed).size(11.0).color(theme::FAINT));
                        ui.add_space(6.0);
                    }
                    if change.removed.is_empty() {
                        ui.label(
                            RichText::new(format!("+{} line(s)", change.added.len()))
                                .size(11.0)
                                .color(theme::GREEN),
                        );
                    } else {
                        ui.label(
                            RichText::new(format!("-{}", change.removed.len()))
                                .size(11.0)
                                .color(theme::RED),
                        );
                        ui.label(
                            RichText::new(format!("+{}", change.added.len()))
                                .size(11.0)
                                .color(theme::GREEN),
                        );
                    }
                });
            });
            ui.add_space(5.0);
            let budget = if open {
                CHANGE_LINES_OPEN
            } else {
                CHANGE_LINES
            };
            diff_lines(ui, change, budget);
            if open {
                if let Some(input) = &card.input {
                    ui.add_space(6.0);
                    let text =
                        serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
                    detail(ui, "input", &shorten_lines(&text, 40), theme::DIM);
                }
                if !card.output.trim().is_empty() {
                    detail(ui, "output", &card.output, theme::TEXT);
                }
            }
        })
        .response;

    if response.interact(Sense::click()).clicked() {
        open = !open;
    }
    ui.ctx().data_mut(|data| data.insert_temp(open_id, open));
}

/// A change's lines: what was removed, then what was added, each per side.
///
/// @param ui the interface to draw into
/// @param change the derived change
/// @param budget the most lines to show per side
fn diff_lines(ui: &mut Ui, change: &FileChange, budget: usize) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for line in change.removed.iter().take(budget) {
            diff_row(ui, "-", line, theme::RED, theme::DIFF_DEL_BG);
        }
        if change.removed.len() > budget {
            more_lines(ui, change.removed.len() - budget);
        }
        for line in change.added.iter().take(budget) {
            diff_row(ui, "+", line, theme::GREEN, theme::DIFF_ADD_BG);
        }
        if change.added.len() > budget {
            more_lines(ui, change.added.len() - budget);
        }
    });
}

/// One line of a change: a sign, the text, and the tint that says which way.
///
/// The row is drawn by hand rather than as a widget because the tint has to
/// span the card while the text stays left-aligned inside it.
///
/// @param ui the interface to draw into
/// @param sign the `+` or `-`
/// @param text the line itself
/// @param colour the text colour
/// @param bg the row's tint
fn diff_row(ui: &mut Ui, sign: &str, text: &str, colour: egui::Color32, bg: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 15.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(3), bg);
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(5.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    row.add(
        egui::Label::new(
            RichText::new(format!("{sign} {text}"))
                .size(11.5)
                .monospace()
                .color(colour),
        )
        .truncate(),
    );
}

/// The marker a cut diff ends with.
///
/// @param ui the interface to draw into
/// @param count how many lines were not drawn
fn more_lines(ui: &mut Ui, count: usize) {
    ui.label(
        RichText::new(format!("… {count} more line(s)"))
            .size(11.0)
            .color(theme::FAINT),
    );
}

/// The engine's task list, live: the newest `todo_write` says what remains.
///
/// @param ui the interface to draw into
/// @param todos the (content, status) pairs
fn todo_card(ui: &mut Ui, todos: &[(String, String)]) {
    let done = todos
        .iter()
        .filter(|(_, status)| status == "completed")
        .count();
    egui::Frame::new()
        .fill(theme::ELEVATED)
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(format!("to-dos · {done} of {} done", todos.len()))
                    .size(12.0)
                    .color(theme::DIM),
            );
            ui.add_space(5.0);
            progress(ui, done, todos.len());
            ui.add_space(6.0);
            for (content, status) in todos.iter().take(TODO_ROWS) {
                let (mark, colour) = match status.as_str() {
                    "completed" => (Icon::Check, theme::GREEN),
                    "in_progress" => (Icon::Ring, theme::ACCENT),
                    "cancelled" => (Icon::More, theme::FAINT),
                    _ => (Icon::Sparkle, theme::FAINT),
                };
                ui.horizontal(|ui| {
                    icons::icon(ui, mark, colour, 12.0);
                    ui.add_space(6.0);
                    let mut text = RichText::new(shorten(content, 92))
                        .size(12.5)
                        .color(theme::TEXT);
                    if status == "completed" {
                        text = text.color(theme::FAINT).strikethrough();
                    }
                    ui.add(egui::Label::new(text).truncate());
                });
            }
            if todos.len() > TODO_ROWS {
                ui.label(
                    RichText::new(format!("… {} more", todos.len() - TODO_ROWS))
                        .size(11.0)
                        .color(theme::FAINT),
                );
            }
        });
}

/// A thin bar under the task list: how much of it is done.
///
/// @param ui the interface to draw into
/// @param done how many entries are complete
/// @param total how many entries there are
fn progress(ui: &mut Ui, done: usize, total: usize) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 4.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(2), theme::HATCH_TRACK);
    let fraction = if total == 0 {
        0.0
    } else {
        done as f32 / total as f32
    };
    if fraction > 0.0 {
        let filled = Rect::from_min_size(rect.min, Vec2::new(width * fraction, 4.0));
        ui.painter()
            .rect_filled(filled, CornerRadius::same(2), theme::GREEN);
    }
}

/// The mark a tool call gets, by what it does.
///
/// @param card the call
/// @returns the icon
fn tool_icon(card: &ToolCard) -> Icon {
    let name = card.title.to_ascii_lowercase();
    if name.contains("subagent") || name.contains("agent") || name.contains("committee") {
        Icon::Harness
    } else if name.contains("bash")
        || name.contains("pwsh")
        || name.contains("shell")
        || name.contains("exec")
    {
        Icon::Terminal
    } else if name.contains("grep") || name.contains("glob") || name.contains("search") {
        Icon::Search
    } else if name.contains("read") || name.contains("book") || name.contains("knowledge") {
        Icon::Book
    } else if name.contains("write")
        || name.contains("edit")
        || name.contains("patch")
        || name.contains("todo")
    {
        Icon::NewTask
    } else if name.contains("web") || name.contains("fetch") || name.contains("http") {
        Icon::Window
    } else if name.contains("plan") || name.contains("wave") {
        Icon::Sparkle
    } else {
        Icon::Sparkle
    }
}

/// A notice in the transcript: one line that says what happened.
///
/// @param ui the interface to draw into
/// @param level the notice's severity
/// @param text what it says
fn notice(ui: &mut Ui, level: NoticeLevel, text: &str) {
    let colour = match level {
        NoticeLevel::Info => theme::FAINT,
        NoticeLevel::Warn => theme::AMBER,
        NoticeLevel::Error => theme::RED,
    };
    ui.horizontal(|ui| {
        icons::icon(ui, Icon::Sparkle, colour, 13.0);
        ui.add_space(6.0);
        ui.label(RichText::new(text).size(12.5).color(colour));
    });
}

/// The model's current plan, under the last thing it said.
///
/// @param ui the interface to draw into
/// @param plan the plan's entries, as content and status
fn plan_card(ui: &mut Ui, plan: &[(String, String)]) {
    let done = plan
        .iter()
        .filter(|(_, status)| status == "completed")
        .count();
    egui::Frame::new()
        .fill(theme::ELEVATED)
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(format!("the plan · {done} of {} done", plan.len()))
                    .size(12.0)
                    .color(theme::DIM),
            );
            ui.add_space(4.0);
            for (content, status) in plan {
                let (mark, colour) = match status.as_str() {
                    "completed" => (Icon::Check, theme::GREEN),
                    "in_progress" => (Icon::Ring, theme::ACCENT),
                    "cancelled" => (Icon::More, theme::FAINT),
                    _ => (Icon::Sparkle, theme::FAINT),
                };
                ui.horizontal(|ui| {
                    icons::icon(ui, mark, colour, 12.0);
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(shorten(content, 90))
                            .size(12.5)
                            .color(theme::TEXT),
                    );
                });
            }
        });
}

/// Cuts a block of text to a line budget.
///
/// @param text the text
/// @param lines the most lines to keep
/// @returns the text, marked when it was cut
fn shorten_lines(text: &str, lines: usize) -> String {
    let kept: Vec<&str> = text.lines().take(lines).collect();
    let mut out = kept.join("\n");
    if text.lines().count() > lines {
        out.push_str("\n… (truncated)");
    }
    out
}
