//! The editor pane: read a file, or change it.
//!
//! Two modes, on purpose. **View** is the one a coding harness needs most: it
//! scrolls a large file without asking the layout engine to measure every line
//! (`ScrollArea::show_rows` draws only the band on screen), it numbers the
//! lines, and it colours them with the same scanner the diffs use. **Edit** is a
//! plain text area — no language server, no completion, no ceremony — for the
//! small change you do not want to leave the harness to make.
//!
//! The pane never writes without being asked: `⌘S` and the toolbar's save are
//! the only paths to disk, and both go through the state, which reports what
//! happened in the status bar.

use eframe::egui::{self, Align, Layout, RichText, ScrollArea, Sense, TextEdit, Ui, Vec2};

use crate::code;
use crate::state::{Buffer, FocusRequest, HarnessState};
use crate::theme;

/// A file this long is drawn as text no matter what.
const HUGE_LINES: usize = 400_000;

/// Draws the editor pane.
///
/// @param state the application state
/// @param ui the interface to draw into
pub fn show(state: &mut HarnessState, ui: &mut Ui) {
    if state.editor.buffer.is_none() {
        empty(ui);
        return;
    }
    toolbar(state, ui);
    ui.add_space(4.0);
    if let Some(error) = state.editor.error.clone() {
        ui.label(RichText::new(error).size(11.0).color(theme::RED));
        ui.add_space(2.0);
    }
    let font = theme::mono(state.config.terminal_font_size);
    let Some(buffer) = state.editor.buffer.as_ref() else {
        return;
    };
    if buffer.binary {
        binary_notice(ui, buffer);
        return;
    }
    if buffer.truncated {
        truncated_notice(ui, buffer);
    }
    if buffer.editing {
        edit(state, ui, &font);
    } else {
        view(state, ui, &font);
    }
}

/// The pane's head: what is open, and what can be done to it.
///
/// @param state the application state
/// @param ui the interface to draw into
fn toolbar(state: &mut HarnessState, ui: &mut Ui) {
    let mut save = false;
    let mut reload = false;
    let mut close = false;
    let mut edit = None;
    let mut goto_line: Option<u32> = None;
    let Some(buffer) = state.editor.buffer.as_ref() else {
        return;
    };

    let dirty = buffer.dirty();
    let relative = buffer.relative.clone();
    let language = buffer.language;
    let bytes = buffer.bytes;
    let lines = buffer.text.lines().count();
    let editing = buffer.editing;

    ui.horizontal(|ui| {
        theme::section(ui, "editor");
        if theme::action(ui, &crate::app::shorten(&relative, 70), true, theme::TEXT)
            .on_hover_text(relative.clone())
            .clicked()
        {
            // Clicking the path is a request to reveal it, not to close it.
            if let Some(buffer) = state.editor.buffer.as_ref() {
                state.toast(format!(
                    "{} · {} bytes · {} line(s)",
                    buffer.relative, bytes, lines
                ));
            }
        }
        theme::badge(ui, language, theme::BLUE);
        if dirty {
            theme::badge(ui, "unsaved", theme::AMBER);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::action(ui, "close", true, theme::DIM)
                .on_hover_text("⌘W")
                .clicked()
            {
                close = true;
            }
            if theme::action(
                ui,
                "save",
                dirty || editing,
                if dirty { theme::AMBER } else { theme::DIM },
            )
            .on_hover_text("⌘S")
            .clicked()
            {
                save = true;
            }
            if theme::action(ui, "reload", true, theme::DIM)
                .on_hover_text("re-read from disk")
                .clicked()
            {
                reload = true;
            }
            if editing {
                if theme::action(ui, "view", true, theme::ACCENT).clicked() {
                    edit = Some(false);
                }
            } else if theme::action(ui, "edit", true, theme::ACCENT).clicked() {
                edit = Some(true);
            }
        });
    });

    // A line box, because reading a stack trace's line number out of the
    // transcript and into the file is the most common thing anyone does here.
    ui.horizontal(|ui| {
        ui.label(RichText::new("line").size(10.5).color(theme::FAINT));
        let mut text = String::new();
        let response = ui.add(
            TextEdit::singleline(&mut text)
                .id(egui::Id::new("editor-goto"))
                .desired_width(52.0)
                .hint_text("nº")
                .font(theme::mono(11.0)),
        );
        if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            goto_line = text.trim().parse::<u32>().ok();
        }
        if let Some(goto) = goto_line {
            let line = goto.clamp(1, lines.max(1) as u32) as usize;
            if let Some(buffer) = state.editor.buffer.as_mut() {
                let offset = buffer
                    .text
                    .lines()
                    .take(line.saturating_sub(1))
                    .map(|line| line.len() + 1)
                    .sum::<usize>();
                buffer.scroll_line = Some(line);
                buffer.editing = false;
                state.editor.goto = Some(offset);
            }
        }
        ui.label(
            RichText::new(format!("{lines} line(s) · {bytes} bytes"))
                .size(10.5)
                .color(theme::FAINT),
        );
    });

    if let Some(editing) = edit {
        if let Some(buffer) = state.editor.buffer.as_mut() {
            buffer.editing = editing;
        }
    }
    if let Some(buffer) = state.editor.buffer.as_ref() {
        if let Some(line) = buffer.scroll_line {
            if state.editor.goto.is_none() && !buffer.editing {
                state.editor.goto = Some(line.saturating_sub(1));
            }
        }
    }
    if save {
        state.save_buffer();
    }
    if reload {
        let path = state
            .editor
            .buffer
            .as_ref()
            .map(|buffer| buffer.path.clone());
        if let Some(path) = path {
            state.open_file(&path);
        }
    }
    if close {
        state.editor.buffer = None;
    }
}

/// The read-only view: line numbers and coloured rows, drawn a screenful at a time.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param font the monospace font
fn view(state: &mut HarnessState, ui: &mut Ui, font: &egui::FontId) {
    let (advance, row_height) = code::metrics(ui, font);

    // A byte offset from a search hit becomes the row the view opens on. The
    // offset arrives once and is consumed here, so scrolling stays the user's.
    let mut start_row = None;
    if let Some(offset) = state.editor.goto.take() {
        if let Some(buffer) = state.editor.buffer.as_mut() {
            let row = buffer.text[..offset.min(buffer.text.len())]
                .matches('\n')
                .count();
            buffer.scroll_line = Some(row + 1);
            start_row = Some(row as f32 * row_height);
        }
    }

    let Some(buffer) = state.editor.buffer.as_ref() else {
        return;
    };
    let lines: Vec<&str> = buffer.text.lines().collect();
    let total = lines.len().min(HUGE_LINES);
    let digits = total.max(1).to_string().len();
    let language = buffer.language;
    let marked = buffer.scroll_line;
    let gutter = (digits as f32 + 2.0) * advance;

    let mut area = ScrollArea::vertical()
        .id_salt("editor-view")
        .auto_shrink([false, false]);
    if let Some(offset) = start_row {
        area = area.vertical_scroll_offset(offset);
    }
    area.show_rows(ui, row_height, total, |ui, range| {
        let width = ui.available_width();
        for index in range {
            let line = lines[index];
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(width, row_height), Sense::hover());
            if response.hovered() {
                ui.painter()
                    .rect_filled(rect, egui::CornerRadius::ZERO, theme::PANEL);
            }
            let number = format!("{:>width$}", index + 1, width = digits);
            ui.painter().text(
                egui::pos2(rect.left() + 2.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                number,
                font.clone(),
                if marked == Some(index + 1) {
                    theme::ACCENT
                } else {
                    theme::FAINT
                },
            );
            let job = code::highlight(language, line, font, theme::TEXT);
            let galley = ui.painter().layout_job(job);
            ui.painter().galley(
                egui::pos2(rect.left() + gutter, rect.top()),
                galley,
                theme::TEXT,
            );
        }
    });
}

/// The editable view: one text area, monospace, no decoration.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param font the monospace font
fn edit(state: &mut HarnessState, ui: &mut Ui, font: &egui::FontId) {
    let Some(buffer) = state.editor.buffer.as_mut() else {
        return;
    };
    let height = ui.available_height() - 4.0;
    let response = ui.add_sized(
        Vec2::new(ui.available_width(), height),
        TextEdit::multiline(&mut buffer.text)
            .font(font.clone())
            .code_editor()
            .desired_width(f32::INFINITY),
    );
    if state
        .focus
        .take_if(|focus| *focus == FocusRequest::Search)
        .is_some()
    {
        response.request_focus();
    }
    let rows = buffer.text.lines().count();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{rows} line(s)"))
                .size(10.5)
                .color(theme::FAINT),
        );
        if buffer.dirty() {
            ui.label(
                RichText::new("unsaved changes · ⌘S writes them")
                    .size(10.5)
                    .color(theme::AMBER),
            );
        } else {
            ui.label(RichText::new("⌘S saves").size(10.5).color(theme::FAINT));
        }
    });
}

/// The notice for a file that is not text.
///
/// @param ui the interface to draw into
/// @param buffer the open file
fn binary_notice(ui: &mut Ui, buffer: &Buffer) {
    ui.add_space(20.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("this is a binary file")
                .size(13.0)
                .color(theme::DIM),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("{} · {} bytes", buffer.relative, buffer.bytes))
                .size(11.0)
                .color(theme::FAINT),
        );
        ui.label(
            RichText::new("editing it here would corrupt it, so the harness will not")
                .size(11.0)
                .color(theme::FAINT),
        );
    });
}

/// The notice for a file the load limit cut short.
///
/// @param ui the interface to draw into
/// @param buffer the open file
fn truncated_notice(ui: &mut Ui, buffer: &Buffer) {
    ui.label(
        RichText::new(format!(
            "showing the first {} bytes of {} — the rest is on disk and left alone",
            buffer.text.len(),
            buffer.bytes
        ))
        .size(10.5)
        .color(theme::AMBER),
    );
    ui.add_space(2.0);
}

/// The empty state.
///
/// @param ui the interface to draw into
fn empty(ui: &mut Ui) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("no file open").size(13.0).color(theme::DIM));
        ui.add_space(4.0);
        ui.label(
            RichText::new("⌘P opens one by name · ⌘⇧F searches what is inside them")
                .size(11.0)
                .color(theme::FAINT),
        );
        ui.label(
            RichText::new("the tree on the left opens one too")
                .size(11.0)
                .color(theme::FAINT),
        );
    });
}
