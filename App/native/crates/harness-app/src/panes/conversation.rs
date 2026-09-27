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

/// How many lines of a wave's result ride on its closed card: one per part.
const WAVE_RESULT_LINES: usize = 14;

/// The widest the transcript's reading column gets, in points: past this, lines
/// are too long to read comfortably, so a wide window centres the column.
const READING_COLUMN: f32 = 880.0;

/// Code blocks longer than this many lines fold, with a "show all" to open them.
const CODE_FOLD_LINES: usize = 24;

/// The most lines an opened code block draws; the copy button carries the rest.
const CODE_MAX_LINES: usize = 600;

/// Draws the conversation view inside the card's body.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param area the card's body rectangle
pub fn show(state: &mut HarnessState, ui: &mut Ui, area: Rect) {
    let inset = 20.0;
    let left = area.left() + inset;
    let right = area.right() - inset;

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
    let composer_top = composer.top() - 10.0;
    let asks_rect = Rect::from_min_max(
        egui::pos2(left, composer_top - ask_height),
        egui::pos2(right, composer_top - 4.0),
    );
    let transcript = Rect::from_min_max(
        egui::pos2(left, area.top() + 6.0),
        egui::pos2(right, asks_rect.top() - 6.0),
    );

    transcript_view(state, ui, transcript);
    if !state.agent.pending.is_empty() {
        asks(state, ui, asks_rect);
    }
    composer_view(state, ui, composer);
}

/// The scrolling record itself.
///
/// The record is a reading column: capped at a comfortable line length and
/// centred in wide windows, full width in narrow ones. It sticks to the newest
/// line while the reader is there, and offers a way back down when they have
/// scrolled up to read.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param rect the rectangle the transcript owns
fn transcript_view(state: &mut HarnessState, ui: &mut Ui, rect: Rect) {
    let column = rect.width().min(READING_COLUMN);
    let content = Rect::from_min_max(
        egui::pos2(rect.center().x - column / 2.0, rect.top()),
        egui::pos2(rect.center().x + column / 2.0, rect.bottom()),
    );
    // The markdown cache is taken out for the frame so the transcript can be
    // read while the cache is written.
    let mut markdown = std::mem::take(&mut state.markdown);
    let jump = std::mem::take(&mut state.scroll_to_bottom);
    let conversation = &state.agent.conversation;
    let skipped = conversation.items.len().saturating_sub(TRANSCRIPT_ITEMS);
    let reveal = state.reveal_call.clone();
    let plan: Vec<(String, String)> = conversation
        .plan
        .iter()
        .map(|entry| (entry.content.clone(), entry.status.clone()))
        .collect();
    let todos = current_todos(&conversation.items);
    let running = conversation.state.busy();

    let mut toggle_thinking: Option<usize> = None;
    let mut reply: Option<String> = None;
    let mut retry: Option<String> = None;
    let mut rated: Option<(String, Option<i8>)> = None;
    let mut export = false;
    let mut scrolled = false;
    let mut retry_last = false;
    let mut open_models = false;
    let mut suggestion: Option<String> = None;
    let mut open_file: Option<String> = None;
    let activity = state.activity();
    let failure = match &state.agent.conversation.state {
        harness_core::agent::RunState::Failed { message } => Some(message.clone()),
        _ => None,
    };
    let can_retry = state.agent.last_prompt.is_some();
    let model = crate::shell::model_label(state);
    let welcome = welcome_lines(state);

    let output = theme::inside(ui, content, |ui| {
        let mut area = egui::ScrollArea::vertical()
            .id_salt("transcript")
            .auto_shrink([false, false])
            .stick_to_bottom(true);
        if jump {
            area = area.vertical_scroll_offset(f32::MAX);
        }
        area.show(ui, |ui| {
            ui.set_width((ui.available_width().min(column) - 12.0).max(1.0));
            // Horizontal notice rows must not enlarge every subsequent card.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            ui.add_space(12.0);
            if conversation.items.is_empty() {
                if let Some(prompt) = empty(ui, &welcome) {
                    suggestion = Some(prompt);
                }
            }
            if skipped > 0 {
                ui.label(
                    RichText::new(format!(
                        "… {skipped} earlier item(s) not drawn; the exported transcript has all of them"
                    ))
                    .size(11.5)
                    .color(theme::FAINT),
                );
                ui.add_space(6.0);
            }
            // The prompt the last answer was given, for its retry action.
            let mut asked: Option<String> = None;
            let last = conversation.items.len().saturating_sub(1);
            for (index, item) in conversation.items[skipped..].iter().enumerate() {
                let absolute = skipped + index;
                match item {
                    Item::User { text, .. } => {
                        asked = Some(text.clone());
                        user_bubble(ui, text);
                    }
                    Item::Assistant { id, text, thinking, thinking_open, rating } => {
                        let streaming = running && absolute == last;
                        if !thinking.is_empty()
                            && thinking_view(ui, thinking, *thinking_open, streaming && text.trim().is_empty())
                        {
                            toggle_thinking = Some(absolute);
                        }
                        answer(ui, &mut markdown, text, absolute);
                        if !text.trim().is_empty() && !streaming {
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
                            if change_card(ui, card, &change, reveal.as_deref()) {
                                open_file = Some(change.path.clone());
                            }
                        } else if card.is_todo_write() && !card.todos().is_empty() {
                            // Nothing drawn here: the live card at the end of
                            // the transcript is the newest list, and one card
                            // that updates is the readable shape of a list
                            // the engine replaces whole.
                        } else {
                            tool_row(ui, card, reveal.as_deref());
                        }
                    }
                    // The failure's own notice is what the failure row at the
                    // end already says; drawing both says it twice.
                    Item::Notice { level: NoticeLevel::Error, text } if failure.as_deref() == Some(text.as_str()) => {
                        continue;
                    }
                    Item::Notice { level, text } => notice(ui, *level, text),
                }
                ui.add_space(10.0);
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
            // A turn that failed is over, so the record ends with the reason
            // and the way out rather than with a spinner.
            if let Some(message) = &failure {
                match failed_row(ui, message, can_retry, &model) {
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
            ui.add_space(8.0);
        })
    });
    state.markdown = markdown;

    // Scrolled up to read: a round button brings the newest line back.
    let below = output.content_size.y - output.state.offset.y - output.inner_rect.height();
    if below > 160.0 {
        let button = Rect::from_center_size(
            egui::pos2(rect.center().x, rect.bottom() - 22.0),
            Vec2::splat(32.0),
        );
        let response = ui.interact(button, ui.id().with("jump-to-latest"), Sense::click());
        let painter = ui.painter();
        painter.add(theme::card_shadow().as_shape(button, CornerRadius::same(16)));
        painter.circle_filled(button.center(), 16.0, theme::PANEL);
        painter.circle_stroke(
            button.center(),
            16.0,
            Stroke::new(1.0, if response.hovered() { theme::BORDER_STRONG } else { theme::BORDER }),
        );
        icons::paint(ui, Icon::ArrowDown, button.shrink(9.0), theme::DIM);
        if response.on_hover_text("jump to the latest").clicked() {
            state.scroll_to_bottom = true;
        }
    }

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
    if let Some(prompt) = suggestion {
        state.agent.input = prompt;
        state.focus = Some(FocusRequest::Agent);
    }
    if let Some(path) = open_file {
        let path = if std::path::Path::new(&path).is_absolute() {
            std::path::PathBuf::from(path)
        } else {
            state.root.join(path)
        };
        state.open_file(&path);
        state.go(crate::state::Pane::Editor, crate::state::SidebarSpot::Task);
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
    let hint = "Ask for a change, attach a command's output, or describe what to build";
    let mut send = false;
    let mut stop = false;
    let mut chosen_route: Option<(String, String)> = None;
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
            // The reference's send disc is green, not the accent blue.
            const SEND_FILL: egui::Color32 = egui::Color32::from_rgb(0x2F, 0x7D, 0x55);
            const SEND_HOVER: egui::Color32 = egui::Color32::from_rgb(0x25, 0x66, 0x45);
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
                    // The chevron is part of the control: a mark drawn beside a
                    // clickable label that did nothing when clicked read as broken.
                    let chevron = icons::icon(ui, Icon::Chevron, theme::FAINT, 12.0)
                        .interact(Sense::click());
                    clicked |= label.clicked() || chevron.clicked();
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
                let (rect, response) = ui.allocate_exact_size(Vec2::splat(SEND), Sense::click());
                let painter = ui.painter();
                if busy && !can_send {
                    // While a turn runs and nothing new is typed, the button is
                    // the way to stop it — in the same place the send was.
                    let fill = if response.hovered() { theme::RED } else { theme::TEXT };
                    painter.rect_filled(rect, CornerRadius::same(9), fill);
                    icons::paint(ui, Icon::Stop, rect.shrink(8.0), theme::PANEL);
                    if response.on_hover_text("stop the turn").clicked() {
                        stop = true;
                    }
                } else {
                    // The send mark keeps its disc whether or not there is
                    // anything to send — where it is does not move — and only
                    // lightens when it has nothing to carry.
                    let fill = if response.hovered() && can_send {
                        SEND_HOVER
                    } else if can_send {
                        SEND_FILL
                    } else {
                        SEND_FILL.gamma_multiply(0.35)
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
                }
                let mic = icons::button(ui, Icon::Mic, theme::FAINT, MIC, theme::HOVER_SOFT);
                if mic
                    .on_hover_text("dictation belongs to the platform: press fn twice and talk")
                    .clicked()
                {
                    state
                        .toast("dictation is the Mac's — press fn twice, then talk into any field");
                }
                // The model chip: the route this task runs on, and a menu of the
                // others. Choosing one restarts the engine on it.
                let model = state
                    .agent
                    .applied_model
                    .clone()
                    .or_else(|| state.active_route.clone())
                    .unwrap_or_else(|| "Default route".to_string());
                let label = if show_name { shorten(&model, 28) } else { String::new() };
                let galley = ui.painter().layout_no_wrap(
                    label.clone(),
                    egui::FontId::proportional(12.5),
                    theme::TEXT,
                );
                let text_width = if show_name { galley.size().x.min(name_budget) } else { 0.0 };
                let chip_width = CHIP + 8.0 + text_width + if show_name { 6.0 + CHEVRON + 8.0 } else { 4.0 };
                let (chip, chip_response) =
                    ui.allocate_exact_size(Vec2::new(chip_width, 26.0), Sense::click());
                if chip_response.hovered() {
                    ui.painter()
                        .rect_filled(chip, CornerRadius::same(7), theme::HOVER_SOFT);
                }
                let tile = Rect::from_min_size(egui::pos2(chip.left() + 4.0, chip.center().y - CHIP / 2.0), Vec2::splat(CHIP));
                ui.painter().rect_filled(tile, CornerRadius::same(6), theme::ACCENT_DIM);
                icons::paint(ui, Icon::Harness, tile.shrink(4.0), theme::ACCENT);
                if show_name {
                    let clip = Rect::from_min_size(
                        egui::pos2(tile.right() + 6.0, chip.top()),
                        Vec2::new(text_width, chip.height()),
                    );
                    ui.painter().with_clip_rect(clip).galley(
                        egui::pos2(clip.left(), chip.center().y - galley.size().y / 2.0),
                        galley,
                        theme::TEXT,
                    );
                    icons::paint(
                        ui,
                        Icon::Chevron,
                        Rect::from_center_size(egui::pos2(chip.right() - 10.0, chip.center().y), Vec2::splat(CHEVRON)),
                        theme::FAINT,
                    );
                }
                let chip_response = chip_response.on_hover_text(format!("{model} — choose the model"));
                egui::Popup::menu(&chip_response).show(|ui| {
                    ui.set_min_width(260.0);
                    ui.label(RichText::new("Model").size(11.5).color(theme::FAINT));
                    let active = state.active_route.clone();
                    let mut any = false;
                    for (route, profile) in &state.models.providers {
                        for entry in profile.models.iter().flatten() {
                            any = true;
                            let key = format!("{route}/{}", entry.id);
                            let label = format!(
                                "{} · {}",
                                entry.name.clone().unwrap_or_else(|| entry.id.clone()),
                                profile.display_name.clone().unwrap_or_else(|| route.clone())
                            );
                            if ui.selectable_label(active.as_deref() == Some(key.as_str()), label).clicked() {
                                chosen_route = Some((route.clone(), entry.id.clone()));
                                ui.close();
                            }
                        }
                    }
                    if !any {
                        ui.label(
                            RichText::new("the default route: AI_MODEL in .env, or OpenRouter's DeepSeek")
                                .size(12.0)
                                .color(theme::DIM),
                        );
                    }
                    ui.separator();
                    if ui.button("Models and routes…  ⌘,").clicked() {
                        open_models = true;
                        ui.close();
                    }
                });
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
    if stop {
        state.stop_turn();
    }
    if let Some((route, model)) = chosen_route {
        state.activate_route(&route, &model);
    }
}

/// What the welcome screen says: a headline, what the agent will run on, and
/// starter prompts that fit this workspace.
struct Welcome {
    /// What the engine will run on, or why it cannot start.
    route: String,
    /// Why the engine could not start, when it could not.
    error: Option<String>,
    /// Prompts worth one click.
    starters: Vec<String>,
}

/// Gathers what the welcome screen shows.
///
/// @param state the application state
/// @returns the welcome content
fn welcome_lines(state: &HarnessState) -> Welcome {
    let route = state
        .agent
        .applied_model
        .clone()
        .or_else(|| state.active_route.clone())
        .map(|route| format!("runs on {route}"))
        .unwrap_or_else(|| "runs on the default route — AI_MODEL in .env picks another".to_string());
    let mut starters = vec![
        "Explain how this project is organised and how to run it".to_string(),
        "Run the tests and fix whatever fails".to_string(),
        "Build a small feature: describe it, and I will plan, build and verify it".to_string(),
    ];
    if state.git.is_some() {
        starters.push("Review the uncommitted changes and point out problems".to_string());
    }
    Welcome {
        route,
        error: state.agent.start_error.clone(),
        starters,
    }
}

/// The welcome screen of a task nothing has been asked in yet.
///
/// @param ui the interface to draw into
/// @param welcome what it says
/// @returns a starter prompt that was clicked, if any
fn empty(ui: &mut Ui, welcome: &Welcome) -> Option<String> {
    let mut chosen = None;
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        crate::icons::tile(ui, Icon::Sparkle, theme::ACCENT, 40.0, theme::ACCENT_DIM, 12);
        ui.add_space(14.0);
        ui.label(RichText::new("What can I build for you?").size(22.0).strong().color(theme::TEXT));
        ui.add_space(6.0);
        ui.label(RichText::new(&welcome.route).size(12.5).color(theme::FAINT));
        if let Some(error) = &welcome.error {
            ui.add_space(8.0);
            ui.label(RichText::new(shorten(error, 160)).size(12.0).color(theme::RED));
        }
    });
    ui.add_space(26.0);
    for starter in &welcome.starters {
        let row = theme::list_row(ui, false, false, |ui| {
            ui.add_space(6.0);
            icons::icon(ui, Icon::Reply, theme::FAINT, 15.0);
            ui.add_space(10.0);
            ui.add(egui::Label::new(RichText::new(starter).size(13.0).color(theme::DIM)).truncate());
        });
        if row.on_hover_text("put this in the prompt").clicked() {
            chosen = Some(starter.clone());
        }
    }
    chosen
}

/// A message the user sent: right-aligned, wrapped inside its bubble.
///
/// The bubble is measured before it is placed. A label in a right-to-left
/// layout does not wrap, so a long prompt used to run out of the bubble and off
/// the card's left edge, dragging every later full-width row out with it.
///
/// @param ui the interface to draw into
/// @param text the message
fn user_bubble(ui: &mut Ui, text: &str) {
    let width = ui.available_width();
    let max_text = (width * 0.78 - 28.0).max(60.0);
    let galley = ui.painter().layout(
        text.trim_end().to_string(),
        egui::FontId::proportional(13.5),
        theme::TEXT,
        max_text,
    );
    let size = galley.size() + Vec2::new(28.0, 20.0);
    let (row, response) =
        ui.allocate_exact_size(Vec2::new(width, size.y), Sense::click());
    let bubble = Rect::from_min_size(egui::pos2(row.right() - size.x, row.top()), size);
    ui.painter()
        .rect_filled(bubble, CornerRadius::same(theme::CARD_RADIUS), theme::ELEVATED);
    ui.painter().galley(bubble.min + Vec2::new(14.0, 10.0), galley, theme::TEXT);
    let copy = response.clone();
    copy.context_menu(|ui| {
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(text.to_string());
            ui.close();
        }
    });
    if response.hovered() {
        let mark = Rect::from_center_size(
            egui::pos2(bubble.left() - 16.0, bubble.bottom() - 12.0),
            Vec2::splat(20.0),
        );
        let hit = ui.interact(mark, ui.id().with(("copy-prompt", row.top() as i64)), Sense::click());
        icons::paint(ui, Icon::Copy, mark.shrink(3.0), if hit.hovered() { theme::DIM } else { theme::FAINT });
        if hit.on_hover_text("copy this message").clicked() {
            ui.ctx().copy_text(text.to_string());
        }
    }
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

/// The row under a transcript whose turn failed: what went wrong, and the two
/// ways out of the dead end.
///
/// @param ui the interface to draw into
/// @param message why the turn failed, in one sentence
/// @param can_retry whether there is a prompt to ask again
/// @param model the route the session was on
/// @returns what the user asked for, if anything
fn failed_row(ui: &mut Ui, message: &str, can_retry: bool, model: &str) -> Option<FailedAction> {
    let mut action = None;
    egui::Frame::new()
        .fill(theme::RED.gamma_multiply(0.05))
        .stroke(egui::Stroke::new(1.0, theme::RED.gamma_multiply(0.28)))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                icons::icon(ui, Icon::Cross, theme::RED, 14.0);
                ui.add_space(6.0);
                ui.label(RichText::new("The turn did not finish").size(13.0).strong().color(theme::TEXT));
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
            ui.add_space(6.0);
            ui.add(egui::Label::new(RichText::new(message).size(12.5).color(theme::TEXT)).wrap());
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
/// While it streams it is drawn open, with its newest lines; once the answer
/// starts it folds to one line that says how long it was.
///
/// @param ui the interface to draw into
/// @param thinking the reasoning text
/// @param open whether it is expanded
/// @param live whether it is still streaming
/// @returns true when the header was clicked
fn thinking_view(ui: &mut Ui, thinking: &str, open: bool, live: bool) -> bool {
    let lines = thinking.lines().filter(|line| !line.trim().is_empty()).count();
    let (row, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
    let chevron = Rect::from_center_size(egui::pos2(row.left() + 8.0, row.center().y), Vec2::splat(12.0));
    if open || live {
        icons::paint(ui, Icon::Chevron, chevron, theme::FAINT);
    } else {
        icons::paint(ui, Icon::ArrowRight, chevron.shrink(1.0), theme::FAINT);
    }
    let label = if live {
        "Thinking…".to_string()
    } else {
        format!("Thought · {lines} line(s)")
    };
    ui.painter().text(
        egui::pos2(row.left() + 20.0, row.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.5),
        if response.hovered() { theme::DIM } else { theme::FAINT },
    );
    let clicked = response.clicked();
    if open || live {
        egui::Frame::new()
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(theme::RADIUS))
            .inner_margin(egui::Margin { left: 12, right: 10, top: 6, bottom: 6 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let shown: Vec<&str> = if live {
                    let all: Vec<&str> = thinking.lines().collect();
                    all[all.len().saturating_sub(6)..].to_vec()
                } else {
                    thinking.lines().take(THINKING_LINES).collect()
                };
                ui.add(
                    egui::Label::new(
                        RichText::new(shown.join("\n"))
                            .size(12.0)
                            .italics()
                            .color(theme::DIM),
                    )
                    .wrap(),
                );
            });
    }
    clicked
}

/// One piece of an answer: markdown prose, or a fenced block kept verbatim.
enum Segment<'a> {
    /// Markdown between fences.
    Prose(String),
    /// A fenced block: its info string and its lines.
    Code(&'a str, Vec<&'a str>),
}

/// Splits an answer at its code fences.
///
/// Fenced blocks are drawn by this pane rather than by the markdown renderer
/// because they carry the things a renderer wraps and breaks: ASCII diagrams,
/// tables drawn in text, mermaid sources, and long lines of code. Here they
/// keep every column, scroll sideways, and copy whole. An unterminated fence —
/// an answer still streaming — is code up to the end.
///
/// @param text the answer
/// @returns the segments, in order
fn segments(text: &str) -> Vec<Segment<'_>> {
    let mut out = Vec::new();
    let mut prose: Vec<&str> = Vec::new();
    let mut fence: Option<(&str, &str, Vec<&str>)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        match fence.as_mut() {
            Some((marker, _, body)) => {
                if trimmed.starts_with(*marker) && trimmed.trim_start_matches(|c| c == '`' || c == '~').trim().is_empty() {
                    let (_, info, body) = fence.take().expect("open fence");
                    out.push(Segment::Code(info, body));
                } else {
                    body.push(line);
                }
            }
            None => {
                let marker = if trimmed.starts_with("```") {
                    Some("```")
                } else if trimmed.starts_with("~~~") {
                    Some("~~~")
                } else {
                    None
                };
                match marker {
                    Some(marker) => {
                        if !prose.is_empty() {
                            out.push(Segment::Prose(prose.join("\n")));
                            prose.clear();
                        }
                        fence = Some((marker, trimmed[3..].trim(), Vec::new()));
                    }
                    None => prose.push(line),
                }
            }
        }
    }
    if let Some((_, info, body)) = fence {
        out.push(Segment::Code(info, body));
    }
    if !prose.is_empty() {
        out.push(Segment::Prose(prose.join("\n")));
    }
    out
}

/// An answer: markdown prose — headings, lists, tables, quotes, emphasis,
/// inline code — with its fenced blocks drawn as code.
///
/// @param ui the interface to draw into
/// @param cache the markdown renderer's cache
/// @param text the answer
/// @param index the item's place in the transcript, for stable block ids
fn answer(ui: &mut Ui, cache: &mut egui_commonmark::CommonMarkCache, text: &str, index: usize) {
    if text.trim().is_empty() {
        return;
    }
    for (block, segment) in segments(text).into_iter().enumerate() {
        match segment {
            Segment::Prose(prose) => {
                if prose.trim().is_empty() {
                    continue;
                }
                ui.scope(|ui| {
                    ui.style_mut().override_font_id = None;
                    ui.spacing_mut().item_spacing.y = 6.0;
                    egui_commonmark::CommonMarkViewer::new()
                        .default_width(Some(ui.available_width() as usize))
                        .show(ui, cache, &prose);
                });
            }
            Segment::Code(info, body) => {
                ui.add_space(4.0);
                code_block(ui, info, &body, ui.id().with(("code", index, block)));
                ui.add_space(4.0);
            }
        }
    }
}

/// A fenced block from an answer: its language, a copy button, and its lines
/// exactly as written — never wrapped, scrolling sideways when wide, folded
/// when long.
///
/// Diagrams are the reason for "exactly": box drawings and mermaid sources
/// only read when every column stays where the model put it.
///
/// @param ui the interface to draw into
/// @param language the fence's info string
/// @param body the lines
/// @param id a stable id for the block's scroll and fold state
fn code_block(ui: &mut Ui, language: &str, body: &[&str], id: egui::Id) {
    let language = language.split_whitespace().next().unwrap_or("");
    let label = match language {
        "" => "text",
        "mermaid" => "mermaid diagram",
        other => other,
    };
    let open_id = id.with("open");
    let mut open = ui.ctx().data_mut(|data| *data.get_temp_mut_or(open_id, false));
    let long = body.len() > CODE_FOLD_LINES;
    let shown = if long && !open { CODE_FOLD_LINES } else { body.len().min(CODE_MAX_LINES) };
    let font = theme::mono(12.0);
    egui::Frame::new()
        .fill(theme::ELEVATED)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(theme::RADIUS + 2))
        .inner_margin(egui::Margin { left: 12, right: 8, top: 6, bottom: 8 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).size(11.0).color(theme::FAINT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let copy = icons::button(ui, Icon::Copy, theme::FAINT, 22.0, theme::HOVER);
                    if copy.on_hover_text("copy").clicked() {
                        ui.ctx().copy_text(body.join("\n"));
                    }
                    ui.label(RichText::new(format!("{} line(s)", body.len())).size(11.0).color(theme::FAINT));
                });
            });
            ui.add_space(2.0);
            egui::ScrollArea::horizontal()
                .id_salt(id.with("scroll"))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let highlight = language != "mermaid" && !language.is_empty();
                    for line in &body[..shown] {
                        let job = if highlight {
                            crate::code::highlight(language, line, &font, theme::TEXT)
                        } else {
                            let mut job = egui::text::LayoutJob::default();
                            job.append(line, 0.0, egui::TextFormat::simple(font.clone(), theme::TEXT));
                            job
                        };
                        ui.add(egui::Label::new(job).extend());
                    }
                });
            if long {
                ui.add_space(4.0);
                let text = if open {
                    "show less".to_string()
                } else {
                    format!("show all {} lines", body.len())
                };
                if theme::action(ui, &text, true, theme::ACCENT).clicked() {
                    open = !open;
                }
            }
            if body.len() > CODE_MAX_LINES && open {
                ui.label(
                    RichText::new(format!("… {} more lines — copy the block to read them all", body.len() - CODE_MAX_LINES))
                        .size(11.0)
                        .color(theme::AMBER),
                );
            }
        });
    ui.ctx().data_mut(|data| data.insert_temp(open_id, open));
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
    // The row names what the call acted on — the command, the file, the
    // pattern — and falls back to the engine's own summary only without one.
    let summary = card
        .input_text(&["command", "cmd", "file_path", "path", "pattern", "query", "url", "name"])
        .unwrap_or_else(|| card.summary());
    let verb = tool_verb(card);
    let elapsed = card
        .elapsed_ms
        .map(duration_label)
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
                ui.label(
                    RichText::new(verb)
                        .size(12.5)
                        .strong()
                        .color(if escalation { theme::VIOLET } else { theme::TEXT }),
                );
                if !summary.is_empty() {
                    ui.add_space(4.0);
                    ui.scope(|ui| {
                        ui.set_max_width((ui.available_width() - 110.0).max(40.0));
                        ui.add(
                            egui::Label::new(
                                RichText::new(first_line(&summary))
                                    .size(12.0)
                                    .monospace()
                                    .color(theme::DIM),
                            )
                            .truncate(),
                        );
                    });
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    status_mark(ui, card.status, colour);
                    ui.add_space(6.0);
                    if !elapsed.is_empty() {
                        ui.label(RichText::new(elapsed).size(11.0).color(theme::FAINT));
                    }
                    if has_detail {
                        icons::icon(ui, if open { Icon::Chevron } else { Icon::ArrowRight }, theme::FAINT, 12.0);
                    }
                });
            });
            if open {
                ui.add_space(6.0);
                if let Some(input) = &card.input {
                    let text =
                        serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
                    detail(ui, "input", &shorten_lines(&text, 40), theme::DIM, &card.id);
                }
                if !card.output.trim().is_empty() {
                    detail(ui, "output", &card.output, theme::TEXT, &card.id);
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
/// @param call the call it belongs to: scroll areas need ids unique to the call
fn detail(ui: &mut Ui, label: &str, text: &str, colour: egui::Color32, call: &str) {
    ui.label(RichText::new(label).size(11.0).color(theme::FAINT));
    egui::Frame::new()
        .fill(theme::ELEVATED)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::horizontal()
                .id_salt(("detail", call, label))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let lines: Vec<&str> = text.lines().take(TOOL_OUTPUT_LINES).collect();
                    ui.add(
                        egui::Label::new(RichText::new(lines.join("\n")).size(12.0).monospace().color(colour))
                            .extend(),
                    );
                    let total = text.lines().count();
                    if total > TOOL_OUTPUT_LINES {
                        ui.label(
                            RichText::new(format!("… {} more line(s)", total - TOOL_OUTPUT_LINES))
                                .size(11.0)
                                .color(theme::FAINT),
                        );
                    }
                });
        });
    ui.add_space(4.0);
}

/// A call's status as a mark: a turning ring while it runs, a check, or a cross.
///
/// @param ui the interface to draw into
/// @param status the call's status
/// @param colour the status colour
fn status_mark(ui: &mut Ui, status: ToolStatus, colour: egui::Color32) {
    match status {
        ToolStatus::Pending | ToolStatus::Running => {
            theme::spinner(ui, 13.0, colour);
        }
        ToolStatus::Completed => {
            icons::icon(ui, Icon::Check, colour, 13.0);
        }
        ToolStatus::Failed => {
            icons::icon(ui, Icon::Cross, colour, 13.0);
        }
    }
}

/// A duration the way a row prints it: `420 ms`, `3.2 s`, `1 m 04 s`.
///
/// @param ms milliseconds
/// @returns the label
fn duration_label(ms: u64) -> String {
    match ms {
        0..=999 => format!("{ms} ms"),
        1_000..=59_999 => format!("{:.1} s", ms as f64 / 1_000.0),
        _ => format!("{} m {:02} s", ms / 60_000, (ms % 60_000) / 1_000),
    }
}

/// What a tool call did, as the verb its row leads with.
///
/// @param card the call
/// @returns a short past-tense verb
fn tool_verb(card: &ToolCard) -> String {
    let name = card.title.trim().to_ascii_lowercase();
    let verb = match name.as_str() {
        "read" | "read_file" | "view" => "Read",
        "grep" | "search" | "rg" => "Searched",
        "glob" | "ls" | "list" => "Listed",
        "bash" | "pwsh" | "shell" | "execute" | "bash_persistent" | "pwsh_persistent" => "Ran",
        "write" => "Wrote",
        "edit" | "str_replace_editor" => "Edited",
        "todo_write" => "Updated the plan",
        "web_fetch" | "fetch" => "Fetched",
        "web_search" => "Searched the web",
        "skill" => "Loaded a skill",
        "spec_get" => "Read the spec",
        "spec_amend" => "Amended the spec",
        "wave_plan" => "Planned waves",
        "job_output" => "Read job output",
        "job_list" => "Listed jobs",
        "job_kill" => "Stopped a job",
        _ => return card.title.trim().to_string(),
    };
    verb.to_string()
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
    // A wave is one call and several agents: it is headed by how many parts it
    // ran, and its closed card shows every part's line rather than the first few.
    let wave = card.title.trim().eq_ignore_ascii_case("run_wave");
    let parts = card
        .input
        .as_ref()
        .and_then(|input| input.get("parts"))
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let headline = if wave {
        format!("Ran {parts} part(s) as parallel subagents")
    } else {
        card.input_text(&["description", "decision", "task", "summary"])
            .unwrap_or_else(|| card.title.clone())
    };
    let handed = if wave {
        None
    } else {
        card.input_text(&["prompt", "evidence"])
    };
    let elapsed = card
        .elapsed_ms
        .map(duration_label)
        .unwrap_or_default();
    let result: Vec<&str> = card
        .output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(if wave { WAVE_RESULT_LINES } else { AGENT_RESULT_LINES })
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
                    detail(ui, "input", &shorten_lines(&text, 40), theme::DIM, &card.id);
                }
                if !card.output.trim().is_empty() {
                    detail(ui, "result", &card.output, theme::TEXT, &card.id);
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

/// A file the agent created or changed, as a file card: its type, its name and
/// folder, the tally, and an Open button; the lines themselves below.
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
/// @returns true when Open was clicked
fn change_card(ui: &mut Ui, card: &ToolCard, change: &FileChange, reveal: Option<&str>) -> bool {
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
    let (folder, name) = match change.path.rsplit_once(['/', '\\']) {
        Some((folder, name)) => (folder.to_string(), name.to_string()),
        None => (String::new(), change.path.clone()),
    };
    let created = change.removed.is_empty();
    let mut open_file = false;
    let mut toggle = false;

    egui::Frame::new()
        .fill(theme::PANEL)
        .stroke(Stroke::new(1.0, if open { theme::BORDER_STRONG } else { theme::BORDER }))
        .corner_radius(CornerRadius::same(theme::RADIUS + 2))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let header = ui.horizontal(|ui| {
                crate::shell::file_tile(ui, &name, 34.0);
                ui.add_space(8.0);
                ui.scope(|ui| {
                    ui.set_max_width((ui.available_width() - 150.0).max(60.0));
                    ui.vertical(|ui| {
                        ui.add(
                            egui::Label::new(RichText::new(&name).size(13.0).strong().color(theme::TEXT))
                                .truncate()
                                .selectable(false),
                        );
                        let sub = format!(
                            "{}{}",
                            if created { "created" } else { "edited" },
                            if folder.is_empty() { String::new() } else { format!(" · {folder}") }
                        );
                        ui.add(
                            egui::Label::new(RichText::new(sub).size(11.5).color(theme::FAINT))
                                .truncate()
                                .selectable(false),
                        );
                    });
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::action(ui, "Open", card.status.done(), theme::ACCENT)
                        .on_hover_text("open the file in the editor")
                        .clicked()
                    {
                        open_file = true;
                    }
                    status_mark(ui, card.status, colour);
                    ui.add_space(4.0);
                    if !change.removed.is_empty() {
                        ui.label(
                            RichText::new(format!("-{}", change.removed.len()))
                                .size(11.5)
                                .monospace()
                                .color(theme::RED),
                        );
                    }
                    ui.label(
                        RichText::new(format!("+{}", change.added.len()))
                            .size(11.5)
                            .monospace()
                            .color(theme::GREEN),
                    );
                });
            });
            let header_hit = ui.interact(
                header.response.rect,
                ui.id().with(("change-head", card.id.as_str())),
                Sense::click(),
            );
            if header_hit
                .on_hover_text(if open { "fold the change" } else { "show the whole change" })
                .clicked()
            {
                toggle = true;
            }
            if open {
                ui.add_space(6.0);
                diff_lines(ui, change, CHANGE_LINES_OPEN);
                if !card.output.trim().is_empty() {
                    ui.add_space(4.0);
                    detail(ui, "result", &card.output, theme::TEXT, &card.id);
                }
            } else if !change.added.is_empty() || !change.removed.is_empty() {
                ui.add_space(6.0);
                diff_lines(ui, change, CHANGE_LINES);
            }
        });

    if toggle {
        open = !open;
    }
    ui.ctx().data_mut(|data| data.insert_temp(open_id, open));
    open_file
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
        ui.add(egui::Label::new(RichText::new(text).size(12.5).color(colour)).wrap());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_transcript_rows_stay_within_the_reading_column() {
        for width in [320.0, 480.0, 860.0] {
            let ctx = egui::Context::default();
            // `answer()` rasterises glyphs the first time they are drawn, which
            // queues a texture delta in the output; epaint's own drop guard
            // panics if that delta is discarded unhandled, so it has to be
            // cleared explicitly — a test-harness detail, not app behaviour.
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(width, 900.0))),
                ..Default::default()
            }, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                let bound = ui.available_width();
                notice(ui, NoticeLevel::Info, &"Model route selected; the key is read from the workspace environment. ".repeat(14));
                assert!(ui.min_rect().width() <= bound + 1.0, "notice overflow at {width}");
                thinking_view(ui, &"Inspecting the workspace and following the user's original request. ".repeat(14), true, false);
                assert!(ui.min_rect().width() <= bound + 1.0, "thought overflow at {width}");
                let mut cache = egui_commonmark::CommonMarkCache::default();
                answer(ui, &mut cache, &"A detailed response with enough prose to wrap across several lines. ".repeat(14), 0);
                assert!(ui.min_rect().width() <= bound + 1.0, "answer overflow at {width}");
                failed_row(ui, &"The provider is temporarily rate-limited. ".repeat(14), true, "a/very/long/model/route");
                assert!(ui.min_rect().width() <= bound + 1.0, "error overflow at {width}");
            });
            output.textures_delta.clear();
        }
    }
}
