//! The command palette: one field, three searches, and the app's verbs.
//!
//! The palette is the fastest path to everything the app can do, and it is built
//! around that: `⌘K` lists commands, `⌘P` lists files by name, `⌘⇧F` searches
//! inside them, and `tab` cycles the three without closing it. What it does not
//! do is make the user remember which mode a thing is in — an unmatched command
//! stays in the list, below the matches, because hiding the thing someone opened
//! the palette to find is the worst possible answer.
//!
//! Content search runs on its own thread (`HarnessState::run_search`), so the
//! field stays typeable while a repository is being walked; results that belong
//! to a query already typed over are dropped rather than shown.

use eframe::egui::{
    self, Align, Key, Layout, Modal, Modifiers, RichText, ScrollArea, Sense, TextEdit, Ui,
};

use crate::app::shorten;
use crate::state::{FocusRequest, HarnessState, Palette, PaletteAction, PaletteHit, PaletteMode};
use crate::theme;

/// How many hits are drawn at once.
const VISIBLE: usize = 14;

/// How wide the panel is.
const WIDTH: f32 = 560.0;

/// Draws the palette, when one is open.
///
/// @param state the application state
/// @param ctx the egui context
pub fn show(state: &mut HarnessState, ctx: &egui::Context) {
    let Some(mut palette) = state.palette.take() else {
        return;
    };

    // Keys are read before the field is drawn: the arrows belong to the list,
    // and the field is single-line, so it has no use for them.
    let mut run = false;
    let mut moved = 0i32;
    let mut cycle = false;
    ctx.input_mut(|input| {
        if input.consume_key(Modifiers::NONE, Key::ArrowDown) {
            moved = 1;
        }
        if input.consume_key(Modifiers::NONE, Key::ArrowUp) {
            moved = -1;
        }
        if input.consume_key(Modifiers::NONE, Key::Tab) {
            cycle = true;
        }
        if input.consume_key(Modifiers::NONE, Key::Enter) {
            run = true;
        }
    });

    if cycle {
        palette.mode = palette.mode.next();
        palette.selection = 0;
        palette.dirty = true;
    }
    let count = palette.hits.len();
    if moved != 0 && count > 0 {
        let current = palette.selection as i32;
        palette.selection = (current + moved).clamp(0, count as i32 - 1) as usize;
    }

    let mut close = false;
    let mut chosen: Option<PaletteAction> = None;
    let mut requested: Option<PaletteMode> = None;
    let area = egui::Area::new(egui::Id::new("palette"))
        .kind(egui::UiKind::Modal)
        .sense(Sense::hover())
        .anchor(egui::Align2::CENTER_TOP, egui::Vec2::new(0.0, 64.0))
        .order(egui::Order::Foreground)
        .interactable(true);
    let frame = egui::Frame::new()
        .fill(theme::ELEVATED)
        .stroke(egui::Stroke::new(1.0, theme::BORDER_STRONG))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 8));

    let response = Modal::new(egui::Id::new("palette-modal"))
        .area(area)
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(WIDTH);
            requested = modes(ui, &palette, &mut close);
            if requested.is_some() {
                return;
            }
            ui.add_space(4.0);

            let field = ui.add(
                TextEdit::singleline(&mut palette.query)
                    .id(egui::Id::new("palette-query"))
                    .desired_width(f32::INFINITY)
                    .hint_text(match palette.mode {
                        PaletteMode::Commands => "run a command, open a pane, start a workflow",
                        PaletteMode::Files => "open a file by name",
                        PaletteMode::Contents => "search inside every file",
                    }),
            );
            if state
                .focus
                .take_if(|focus| *focus == FocusRequest::Palette)
                .is_some()
            {
                field.request_focus();
            }
            if field.changed() {
                palette.selection = 0;
                palette.dirty = true;
            }

            ui.add_space(4.0);
            if palette.mode == PaletteMode::Contents && state.search.pending {
                ui.label(RichText::new("searching…").size(11.0).color(theme::AMBER));
            }
            if palette.hits.is_empty() {
                ui.label(
                    RichText::new(match palette.mode {
                        PaletteMode::Contents => "no matches in this workspace",
                        _ => "nothing matches",
                    })
                    .size(11.5)
                    .color(theme::FAINT),
                );
            }
            let row_height = theme::ROW + ui.spacing().item_spacing.y;
            let mut list = ScrollArea::vertical()
                .id_salt("palette-hits")
                .max_height(330.0)
                .auto_shrink([false, true]);
            if moved != 0 {
                // The arrows move the selection, and the selection moves the list.
                let anchor =
                    palette.selection as f32 * row_height - VISIBLE as f32 * row_height * 0.5;
                list = list.vertical_scroll_offset(anchor.max(0.0));
            }
            list.show_rows(ui, row_height, palette.hits.len(), |ui, range| {
                for index in range {
                    let hit = &palette.hits[index];
                    let selected = index == palette.selection;
                    if row(ui, hit, selected).clicked() {
                        chosen = Some(hit.action.clone());
                    }
                }
            });
            ui.add_space(4.0);
            footer(ui, &palette, state);
        });

    if run {
        if let Some(hit) = palette.hits.get(palette.selection) {
            chosen = Some(hit.action.clone());
        }
    }
    if response.should_close() {
        close = true;
    }

    if let Some(mode) = requested {
        palette.mode = mode;
        palette.selection = 0;
        palette.dirty = true;
        state.palette = Some(palette);
        state.refresh_palette();
        return;
    }
    if let Some(action) = chosen {
        state.palette = None;
        state.apply(&action);
        return;
    }
    if close {
        state.palette = None;
        return;
    }
    if palette.dirty {
        state.palette = Some(palette);
        state.refresh_palette();
        return;
    }
    state.palette = Some(palette);
}

/// The mode chips.
///
/// @param ui the interface to draw into
/// @param palette the palette
/// @param close set when the close chip is clicked
/// @returns the mode a chip asked for, when one did
fn modes(ui: &mut Ui, palette: &Palette, close: &mut bool) -> Option<PaletteMode> {
    let mut requested = None;
    ui.horizontal(|ui| {
        for (mode, label, hint) in [
            (PaletteMode::Commands, "commands", "⌘K"),
            (PaletteMode::Files, "files", "⌘P"),
            (PaletteMode::Contents, "contents", "⌘⇧F"),
        ] {
            let selected = palette.mode == mode;
            let response = theme::action(
                ui,
                label,
                true,
                if selected {
                    theme::ACCENT
                } else {
                    theme::FAINT
                },
            );
            if selected {
                ui.painter().hline(
                    response.rect.x_range(),
                    response.rect.bottom() + 1.0,
                    egui::Stroke::new(1.0, theme::ACCENT),
                );
            }
            if response
                .on_hover_text(format!("{hint} · tab cycles"))
                .clicked()
                && !selected
            {
                requested = Some(mode);
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::action(ui, "esc", true, theme::FAINT).clicked() {
                *close = true;
            }
        });
    });
    requested
}

/// One hit.
///
/// @param ui the interface to draw into
/// @param hit the hit
/// @param selected whether it is the highlighted one
/// @returns the row's response
fn row(ui: &mut Ui, hit: &PaletteHit, selected: bool) -> egui::Response {
    let height = theme::ROW;
    let (rect, response) = ui.allocate_exact_size(
        egui::Vec2::new(ui.available_width(), height),
        Sense::click(),
    );
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, egui::CornerRadius::same(4), theme::ACCENT_DIM);
    } else if response.hovered() {
        painter.rect_filled(rect, egui::CornerRadius::same(4), theme::HOVER);
    }
    let label = painter.layout_no_wrap(
        shorten(&hit.label, 60),
        theme::text(0.0),
        if selected { theme::TEXT } else { theme::DIM },
    );
    painter.galley(
        rect.min + egui::Vec2::new(8.0, (height - label.size().y) / 2.0),
        label,
        theme::TEXT,
    );
    if !hit.detail.is_empty() {
        let detail =
            painter.layout_no_wrap(shorten(&hit.detail, 46), theme::mono(10.5), theme::FAINT);
        painter.galley(
            egui::pos2(
                rect.right() - detail.size().x - 8.0,
                rect.center().y - detail.size().y / 2.0,
            ),
            detail,
            theme::FAINT,
        );
    }
    response
}

/// The footer: what the list is made of, and what the keys do.
///
/// @param ui the interface to draw into
/// @param palette the palette
/// @param state the application state
fn footer(ui: &mut Ui, palette: &Palette, state: &HarnessState) {
    ui.horizontal(|ui| {
        let count = palette.hits.len();
        let noun = match palette.mode {
            PaletteMode::Commands => "command(s)",
            PaletteMode::Files => "file(s)",
            PaletteMode::Contents => "match(es)",
        };
        ui.label(
            RichText::new(format!("{count} {noun}"))
                .size(10.0)
                .color(theme::FAINT),
        );
        if palette.mode == PaletteMode::Contents {
            if let Some(outcome) = &state.search.outcome {
                let note = if outcome.truncated {
                    " · truncated"
                } else {
                    ""
                };
                ui.label(
                    RichText::new(format!("{} file(s) searched{note}", outcome.files_searched))
                        .size(10.0)
                        .color(theme::FAINT),
                );
                if let Some(error) = &outcome.error {
                    ui.label(
                        RichText::new(shorten(error, 40))
                            .size(10.0)
                            .color(theme::RED),
                    );
                }
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            theme::key_chip(ui, "↵");
            ui.label(RichText::new("run").size(10.0).color(theme::FAINT));
            ui.add_space(6.0);
            theme::key_chip(ui, "↑↓");
            ui.label(RichText::new("move").size(10.0).color(theme::FAINT));
            ui.add_space(6.0);
            theme::key_chip(ui, "tab");
            ui.label(RichText::new("mode").size(10.0).color(theme::FAINT));
        });
    });
}
