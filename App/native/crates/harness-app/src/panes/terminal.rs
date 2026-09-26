//! The terminal pane.
//!
//! Two views of one shell. **Screen** is the emulator's grid, drawn cell by
//! cell: it is what a full-screen program needs, and it is where you type.
//! **Blocks** is the list of finished commands, each with its own captured
//! output — the view you want when you are reading a build rather than running
//! one.
//!
//! Keys go to the shell only when the screen has focus, and they are sent as the
//! bytes a terminal would send: control chords become control bytes, arrows and
//! function keys become their escape sequences, alt-prefixed keys get an ESC.
//! Anything the application owns (`⌘K`, `⌘S`, …) was consumed before this pane
//! ever ran, so no shortcut can type itself into someone's shell.
//!
//! Bold text is drawn in the regular weight: the bundled monospace family has no
//! bold face, and inventing one by smearing a stroke would look worse than
//! plain. Underlines, colours, and the cursor are all drawn as the shell asked.

use eframe::egui::text::LayoutJob;
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Key, Layout, Modifiers, Rect, RichText, ScrollArea,
    Sense, Ui, Vec2,
};

use harness_core::term::{BlockSummary, ScreenSize, SnapshotCell, Terminal};

use crate::app::shorten;
use crate::code;
use crate::state::{HarnessState, ShellTab};
use crate::theme;

/// Rows of screen kept above the fold when the pane scrolls back.
const MIN_COLS: usize = 20;
/// Rows the pane will ask the PTY for at the very least.
const MIN_ROWS: usize = 4;
/// Lines of a block's output shown inline before it is cut.
const BLOCK_OUTPUT_LINES: usize = 600;

/// Draws the terminal pane.
///
/// @param state the application state
/// @param ui the interface to draw into
pub fn show(state: &mut HarnessState, ui: &mut Ui) {
    tabs(state, ui);
    ui.add_space(4.0);

    let font_size = state.config.terminal_font_size;
    let Some(shell) = state.shells.get(state.active_shell) else {
        empty(ui, "no shell", "⌘T starts one");
        return;
    };
    if let Some(error) = &shell.error {
        empty(ui, "the shell did not start", error);
        return;
    }
    match shell.tab {
        ShellTab::Screen => screen(state, ui, font_size),
        ShellTab::Blocks => blocks(state, ui, font_size),
    }
}

/// The shell tabs and the screen/blocks switch.
///
/// @param state the application state
/// @param ui the interface to draw into
fn tabs(state: &mut HarnessState, ui: &mut Ui) {
    let mut switch: Option<usize> = None;
    let mut new_shell = false;
    ui.horizontal(|ui| {
        for (index, shell) in state.shells.iter().enumerate() {
            let active = index == state.active_shell;
            let alive = shell.alive();
            let label = shell.name.clone();
            let response = theme::action(
                ui,
                &label,
                true,
                if active { theme::TEXT } else { theme::DIM },
            );
            let response = response.on_hover_text(if alive { "running" } else { "exited" });
            if response.clicked() {
                switch = Some(index);
            }
            if active {
                if !alive {
                    theme::badge(ui, "exited", theme::RED);
                }
            }
        }
        if theme::action(ui, "+", true, theme::ACCENT)
            .on_hover_text("New shell ⌘T")
            .clicked()
        {
            new_shell = true;
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(shell) = state.shells.get(state.active_shell) {
                let terminal = shell.terminal.as_ref();
                if let Some(terminal) = terminal {
                    let integration = if terminal.is_integrated() {
                        "blocks"
                    } else {
                        "screen only"
                    };
                    ui.label(RichText::new(integration).size(10.5).color(theme::FAINT));
                    if let Some(title) = terminal.title() {
                        ui.label(
                            RichText::new(shorten(&title, 28))
                                .size(10.5)
                                .color(theme::FAINT),
                        );
                    }
                    if terminal.at_prompt() {
                        ui.label(RichText::new("at prompt").size(10.5).color(theme::GREEN));
                    }
                }
                if terminal.is_none() {
                    if theme::action(ui, "start", true, theme::ACCENT).clicked() {
                        switch = Some(state.active_shell);
                    }
                }
            }
            // The switch between the two views of the same shell.
            let mut tab = state
                .shells
                .get(state.active_shell)
                .map(|shell| shell.tab)
                .unwrap_or(ShellTab::Screen);
            if theme::action(
                ui,
                "blocks",
                true,
                if tab == ShellTab::Blocks {
                    theme::TEXT
                } else {
                    theme::DIM
                },
            )
            .clicked()
            {
                tab = ShellTab::Blocks;
            }
            if theme::action(
                ui,
                "screen",
                true,
                if tab == ShellTab::Screen {
                    theme::TEXT
                } else {
                    theme::DIM
                },
            )
            .clicked()
            {
                tab = ShellTab::Screen;
            }
            if let Some(shell) = state.shells.get_mut(state.active_shell) {
                shell.tab = tab;
            }
        });
    });

    if let Some(index) = switch {
        state.active_shell = index;
        state.start_shell(index);
    }
    if new_shell {
        state.new_shell();
    }
}

/// The live screen.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param font_size terminal font size in points
fn screen(state: &mut HarnessState, ui: &mut Ui, font_size: f32) {
    let font = theme::mono(font_size);
    let (advance, line_height) = code::metrics(ui, &font);

    let available = ui.available_size();
    let cols = ((available.x - 8.0) / advance).floor().max(MIN_COLS as f32) as usize;
    let rows = ((available.y - 4.0) / line_height)
        .floor()
        .max(MIN_ROWS as f32) as usize;

    let Some(shell) = state.shells.get_mut(state.active_shell) else {
        return;
    };
    let Some(terminal) = shell.terminal.as_ref() else {
        empty(
            ui,
            "this shell is not running",
            "start it from the tab strip",
        );
        return;
    };

    // The PTY is told the grid it is being drawn into, so full-screen programs
    // and the shell's own wrapping agree with what is on screen.
    let size = ScreenSize { cols, rows };
    if terminal.size() != size {
        terminal.resize(size);
    }

    let snapshot = terminal.screen().snapshot();
    let grid_size = Vec2::new(cols as f32 * advance, rows as f32 * line_height);
    let (rect, response) = ui.allocate_exact_size(grid_size, Sense::click_and_drag());
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }

    // Scrolling back through history is the wheel's job; the wheel is only read
    // when the pointer is over the grid.
    if response.hovered() {
        let delta = ui.input(|input| input.smooth_scroll_delta.y);
        if delta.abs() > 0.5 {
            let lines = (delta / line_height).round() as i32;
            if lines != 0 {
                terminal.screen().scroll(lines);
                shell.follow = snapshot.display_offset == 0;
            }
        }
    }

    // The pane's own background goes down first: the shell's default background
    // is the palette background, so cells that share it cost no rectangles.
    let painter = ui.painter_at(rect);
    let palette_bg = theme::rgb(terminal.palette().background);
    painter.rect_filled(rect, CornerRadius::ZERO, palette_bg);

    for (row, line) in snapshot.lines.iter().enumerate() {
        let y = rect.top() + row as f32 * line_height;
        let mut index = 0;
        while index < line.cells.len() {
            let cell = line.cells[index];
            let mut end = index + 1;
            while end < line.cells.len() && same_style(&line.cells[end], &cell) {
                end += 1;
            }
            let run = &line.cells[index..end];
            let x = rect.left() + index as f32 * advance;
            let width = run.len() as f32 * advance;

            let bg = theme::rgb(cell.bg);
            if bg != palette_bg {
                painter.rect_filled(
                    Rect::from_min_size(egui::pos2(x, y), Vec2::new(width, line_height)),
                    CornerRadius::ZERO,
                    bg,
                );
            }
            if cell.cursor && snapshot.cursor.visible {
                painter.rect_filled(
                    Rect::from_min_size(egui::pos2(x, y), Vec2::new(advance, line_height)),
                    CornerRadius::ZERO,
                    theme::rgb(terminal.palette().cursor).gamma_multiply(0.55),
                );
            }
            let text: String = run
                .iter()
                .filter(|cell| !cell.cursor || !snapshot.cursor.visible)
                .map(|cell| cell.ch)
                .collect();
            if !text.trim().is_empty() {
                let job = LayoutJob::simple(text, font.clone(), theme::rgb(cell.fg), f32::INFINITY);
                let galley = painter.layout_job(job);
                painter.galley(egui::pos2(x, y), galley, theme::rgb(cell.fg));
            }
            if cell.underline {
                painter.hline(
                    egui::Rangef::new(x, x + width),
                    y + line_height - 1.5,
                    egui::Stroke::new(1.0, theme::rgb(cell.fg)),
                );
            }
            index = end;
        }
    }

    keyboard(ui, &response, terminal);

    // The scrollback indication sits at the bottom-right so it never covers the
    // prompt, and it is the only chrome the screen carries.
    if snapshot.display_offset > 0 {
        let label = format!(
            "-- {} ↑ of {} --",
            snapshot.display_offset, snapshot.history
        );
        painter.text(
            egui::pos2(rect.right() - 8.0, rect.bottom() - 2.0),
            egui::Align2::RIGHT_BOTTOM,
            label,
            theme::mono(font_size - 2.0),
            theme::AMBER,
        );
    }
    // The hint sits in the bottom-left, where a shell never writes, and the
    // scrollback counter in the bottom-right: the chrome never covers output.
    if !response.has_focus() {
        painter.text(
            egui::pos2(rect.left() + 6.0, rect.bottom() - 2.0),
            egui::Align2::LEFT_BOTTOM,
            "click to type",
            theme::mono(font_size - 2.0),
            theme::FAINT,
        );
    }
}

/// Whether two cells draw with the same colours and attributes.
///
/// @param a one cell
/// @param b another
/// @returns true when they can be drawn as one run
fn same_style(a: &SnapshotCell, b: &SnapshotCell) -> bool {
    a.fg == b.fg && a.bg == b.bg && a.underline == b.underline && a.cursor == b.cursor && !a.cursor
}

/// Sends the keys the screen received to the shell.
///
/// @param ui the interface, for its input
/// @param response the grid's response, which owns the focus
/// @param terminal the shell's PTY
fn keyboard(ui: &Ui, response: &egui::Response, terminal: &Terminal) {
    if !response.has_focus() {
        return;
    }
    let events = ui.input(|input| input.events.clone());
    let mut bytes = Vec::new();
    for event in events {
        match event {
            egui::Event::Text(text) => bytes.extend_from_slice(text.as_bytes()),
            egui::Event::Paste(text) => bytes.extend_from_slice(text.as_bytes()),
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                if let Some(sequence) = key_bytes(key, modifiers) {
                    bytes.extend_from_slice(&sequence);
                }
            }
            _ => {}
        }
    }
    if !bytes.is_empty() {
        terminal.send(bytes);
    }
}

/// The bytes a key press sends to a shell.
///
/// @param key the key
/// @param modifiers what was held
/// @returns the escape sequence, or `None` when the key produces text instead
fn key_bytes(key: Key, modifiers: Modifiers) -> Option<Vec<u8>> {
    // The application's own chords were consumed before this pane ran; anything
    // still holding the platform command key belongs to neither side.
    if modifiers.mac_cmd || modifiers.command {
        return None;
    }
    // Control chords are the shell's: they are how a person interrupts a build.
    if modifiers.ctrl {
        if let Some(byte) = control_byte(key) {
            return Some(vec![byte]);
        }
    }
    let sequence: &[u8] = match key {
        Key::Enter => b"\r",
        Key::Backspace => b"\x7f",
        Key::Tab if modifiers.shift => b"\x1b[Z",
        Key::Tab => b"\t",
        Key::Escape => b"\x1b",
        Key::ArrowUp => b"\x1b[A",
        Key::ArrowDown => b"\x1b[B",
        Key::ArrowRight => b"\x1b[C",
        Key::ArrowLeft => b"\x1b[D",
        Key::Home => b"\x1b[H",
        Key::End => b"\x1b[F",
        Key::Insert => b"\x1b[2~",
        Key::Delete => b"\x1b[3~",
        Key::PageUp => b"\x1b[5~",
        Key::PageDown => b"\x1b[6~",
        Key::F1 => b"\x1bOP",
        Key::F2 => b"\x1bOQ",
        Key::F3 => b"\x1bOR",
        Key::F4 => b"\x1bOS",
        Key::F5 => b"\x1b[15~",
        Key::F6 => b"\x1b[17~",
        Key::F7 => b"\x1b[18~",
        Key::F8 => b"\x1b[19~",
        Key::F9 => b"\x1b[20~",
        Key::F10 => b"\x1b[21~",
        Key::F11 => b"\x1b[23~",
        Key::F12 => b"\x1b[24~",
        // Anything printable arrives as text, and would otherwise be typed twice.
        _ => return None,
    };
    // Alt is the meta prefix on a terminal, which is what editors and readline
    // expect: alt-b moves by a word.
    if modifiers.alt {
        let mut prefixed = vec![0x1b];
        prefixed.extend_from_slice(sequence);
        return Some(prefixed);
    }
    Some(sequence.to_vec())
}

/// The control byte for a chord, e.g. Ctrl+C is `0x03`.
///
/// @param key the key held with control
/// @returns the byte, when the chord has one
fn control_byte(key: Key) -> Option<u8> {
    let name = key.name();
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) if ch.is_ascii_alphabetic() => {
            Some((ch.to_ascii_lowercase() as u8) & 0x1f)
        }
        _ => match key {
            Key::Space | Key::Num2 => Some(0x00),
            Key::OpenBracket => Some(0x1b),
            Key::CloseBracket => Some(0x1d),
            Key::Backslash => Some(0x1c),
            Key::Slash => Some(0x1f),
            _ => None,
        },
    }
}

/// The block log.
///
/// @param state the application state
/// @param ui the interface to draw into
/// @param font_size terminal font size in points
fn blocks(state: &mut HarnessState, ui: &mut Ui, font_size: f32) {
    state.refresh_blocks(state.active_shell);
    let font = theme::mono(font_size);

    // The selected block's output gets its own region, resizable, below the list.
    egui::Panel::bottom("block-output")
        .default_size(300.0)
        .size_range(80.0..=800.0)
        .resizable(true)
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .inner_margin(egui::Margin::symmetric(8, 6)),
        )
        .show(ui, |ui| {
            let Some(shell) = state.shells.get(state.active_shell) else {
                return;
            };
            match shell
                .selected_block
                .and_then(|index| shell.blocks.get(index))
            {
                Some(block) => block_output(ui, block, &font),
                None => {
                    ui.label(
                        RichText::new("no block selected")
                            .size(11.5)
                            .color(theme::FAINT),
                    );
                }
            }
        });

    let mut attach: Option<usize> = None;
    let mut select: Option<usize> = None;
    let mut run: Option<String> = None;
    ScrollArea::vertical()
        .id_salt("blocks")
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            let Some(shell) = state.shells.get(state.active_shell) else {
                return;
            };
            if shell.blocks.is_empty() {
                ui.label(
                    RichText::new("no commands yet")
                        .size(11.5)
                        .color(theme::FAINT),
                );
                ui.label(
                    RichText::new("commands run in this shell appear here with their output")
                        .size(11.0)
                        .color(theme::FAINT),
                );
                return;
            }
            for (index, block) in shell.blocks.iter().enumerate() {
                let selected = shell.selected_block == Some(index);
                let row = ui.horizontal(|ui| {
                    ui.add_space(2.0);
                    ui.label(RichText::new("❯").size(12.0).color(if selected {
                        theme::ACCENT
                    } else {
                        theme::FAINT
                    }));
                    let colour = if selected { theme::TEXT } else { theme::DIM };
                    let label = ui.add(
                        egui::Label::new(
                            RichText::new(shorten(&block.command, 90))
                                .size(12.0)
                                .monospace()
                                .color(colour),
                        )
                        .sense(Sense::click())
                        .truncate(),
                    );
                    if label.clicked() {
                        select = Some(index);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        match block.exit_code {
                            Some(0) => {
                                theme::badge(ui, "ok", theme::GREEN);
                            }
                            Some(code) => {
                                theme::badge(ui, &format!("exit {code}"), theme::RED);
                            }
                            None => {
                                theme::badge(ui, "running", theme::AMBER);
                            }
                        }
                        ui.label(
                            RichText::new(format!("{}ms", block.duration_ms))
                                .size(10.5)
                                .color(theme::FAINT),
                        );
                        if theme::action(ui, "rerun", true, theme::DIM).clicked() {
                            run = Some(block.command.clone());
                        }
                        if theme::action(ui, "ask agent", true, theme::ACCENT).clicked() {
                            attach = Some(index);
                        }
                        ui.label(
                            RichText::new(shorten(&block.cwd.display().to_string(), 30))
                                .size(10.0)
                                .color(theme::FAINT),
                        );
                    });
                });
                if selected {
                    let rect = row.response.rect;
                    ui.painter().rect_filled(
                        Rect::from_min_size(
                            egui::pos2(rect.left(), rect.bottom()),
                            Vec2::new(rect.width(), 1.0),
                        ),
                        CornerRadius::ZERO,
                        theme::ACCENT_DIM,
                    );
                }
            }
        });

    if let Some(index) = select {
        if let Some(shell) = state.shells.get_mut(state.active_shell) {
            shell.selected_block = Some(index);
        }
    }
    if let Some(command) = run {
        state.run_command(&command);
    }
    if let Some(index) = attach {
        state.attach_block(index);
    }
}

/// One block's captured output.
///
/// @param ui the interface to draw into
/// @param block the command
/// @param font the monospace font
fn block_output(ui: &mut Ui, block: &BlockSummary, font: &FontId) {
    ui.horizontal(|ui| {
        theme::section(ui, "output");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let cwd = shorten(&block.cwd.display().to_string(), 40);
            ui.label(RichText::new(cwd).size(10.5).color(theme::FAINT));
            match block.exit_code {
                Some(code) => ui.label(
                    RichText::new(format!("exit {code}"))
                        .size(10.5)
                        .color(if code == 0 { theme::GREEN } else { theme::RED }),
                ),
                None => ui.label(RichText::new("running").size(10.5).color(theme::AMBER)),
            };
        });
    });
    ui.add_space(2.0);
    let lines: Vec<&str> = block.output.lines().collect();
    let shown = lines.len().min(BLOCK_OUTPUT_LINES);
    ScrollArea::both()
        .id_salt("block-output-text")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for line in &lines[..shown] {
                let galley =
                    ui.painter()
                        .layout_no_wrap(line.to_string(), font.clone(), theme::TEXT);
                ui.add_space(0.0);
                let (rect, _) = ui.allocate_exact_size(
                    Vec2::new(galley.size().x.max(ui.available_width()), galley.size().y),
                    Sense::hover(),
                );
                ui.painter().galley(rect.min, galley, theme::TEXT);
            }
            if lines.len() > shown {
                ui.label(
                    RichText::new(format!("… {} of {} lines shown", shown, lines.len()))
                        .size(10.5)
                        .color(theme::AMBER),
                );
            }
        });
}

/// The empty state of a pane: a title, an explanation, and no apology.
///
/// @param ui the interface to draw into
/// @param title what is missing
/// @param detail what to do about it
pub fn empty(ui: &mut Ui, title: &str, detail: &str) {
    ui.add_space(30.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(title).size(14.0).color(theme::DIM));
        ui.add_space(4.0);
        ui.label(RichText::new(detail).size(11.5).color(theme::FAINT));
    });
}

/// The token count of a block's output, for the observability pane.
///
/// @param block the command
/// @returns an estimate in tokens
pub fn output_tokens(block: &BlockSummary) -> u64 {
    harness_core::agent::estimate_tokens(&block.output)
}

/// One block summarised in the status of a pane.
///
/// @param block the command
/// @returns e.g. `cargo test · ok · 420ms`
pub fn block_label(block: &BlockSummary) -> String {
    let status = match block.exit_code {
        Some(0) => "ok".to_string(),
        Some(code) => format!("exit {code}"),
        None => "running".to_string(),
    };
    format!(
        "{} · {status} · {}ms",
        shorten(&block.command, 40),
        block.duration_ms
    )
}

/// The colour a block's status should be drawn in.
///
/// @param block the command
/// @returns the colour
pub fn block_colour(block: &BlockSummary) -> Color32 {
    match block.exit_code {
        Some(0) => theme::GREEN,
        Some(_) => theme::RED,
        None => theme::AMBER,
    }
}
