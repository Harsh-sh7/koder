//! The visual language: one light theme, matched to the reference client.
//!
//! Every colour the app paints comes from here, so the whole interface moves
//! together when a token changes. The system it mirrors: a light gray chrome
//! (sidebar, top strip, the gutters between cards), opaque white surfaces that
//! float on top of it with a hairline border, near-black text, one blue accent
//! for selection and the agent's own voice, and colour used where it carries
//! meaning — a diff, a status, a token budget — rather than for decoration.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Layout, Rect, Response, RichText,
    Sense, Shadow, Stroke, StrokeKind, TextStyle, Ui, Vec2,
};

/// The chrome: sidebar, top strip, and the gutters between the cards.
pub const BG: Color32 = Color32::from_rgb(0xE7, 0xE7, 0xE4);
/// Opaque surface of a card: the conversation, the side panel, dialogs.
pub const PANEL: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
/// A surface one step in from a card: row tiles, code blocks, the composer's fill.
pub const ELEVATED: Color32 = Color32::from_rgb(0xF6, 0xF6, 0xF4);
/// Hover fill on chrome.
pub const HOVER: Color32 = Color32::from_rgb(0xDD, 0xDD, 0xD9);
/// Hover fill on a white card.
pub const HOVER_SOFT: Color32 = Color32::from_rgb(0xF0, 0xF0, 0xEE);
/// Selected fill on chrome.
pub const ACTIVE: Color32 = Color32::from_rgb(0xD3, 0xD3, 0xCE);
/// Hairline between regions, and the border of a card.
pub const BORDER: Color32 = Color32::from_rgb(0xE3, 0xE3, 0xDF);
/// Border of something that has focus or is current.
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0xC9, 0xC9, 0xC4);

/// Primary text.
pub const TEXT: Color32 = Color32::from_rgb(0x1A, 0x1A, 0x1A);
/// Secondary text: labels, metadata.
pub const DIM: Color32 = Color32::from_rgb(0x6E, 0x6E, 0x6C);
/// Tertiary text: hints, timestamps, disabled.
pub const FAINT: Color32 = Color32::from_rgb(0x9C, 0x9C, 0x99);

/// The one accent: selection, focus, links, the agent's own voice.
pub const ACCENT: Color32 = Color32::from_rgb(0x3B, 0x6F, 0xF5);
/// Accent at panel strength: a selected row's fill.
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0xE3, 0xEB, 0xFD);

/// Success: a passing command, a completed delegation.
pub const GREEN: Color32 = Color32::from_rgb(0x2E, 0x9E, 0x5B);
/// Warning: a budget approaching its cap, a tool still running.
pub const AMBER: Color32 = Color32::from_rgb(0xB4, 0x7A, 0x12);
/// Failure: a non-zero exit, a rejected tool call.
pub const RED: Color32 = Color32::from_rgb(0xD1, 0x3F, 0x3F);
/// Information: paths, links, the second hue in a diff.
pub const BLUE: Color32 = Color32::from_rgb(0x2C, 0x66, 0xE0);
/// Thinking, plans, the model's internal voice.
pub const VIOLET: Color32 = Color32::from_rgb(0x7C, 0x4D, 0xE0);

/// Background of an added diff line.
pub const DIFF_ADD_BG: Color32 = Color32::from_rgb(0xE2, 0xF4, 0xE7);
/// Background of a removed diff line.
pub const DIFF_DEL_BG: Color32 = Color32::from_rgb(0xFB, 0xE7, 0xE7);
/// Background of a hunk header.
pub const DIFF_HUNK_BG: Color32 = Color32::from_rgb(0xEE, 0xF1, 0xF6);

/// Text selection inside a field.
pub const SELECTION: Color32 = Color32::from_rgb(0xCB, 0xDE, 0xFB);
/// Selected text's own colour, which has to stay legible on [`SELECTION`].
pub const SELECTION_TEXT: Color32 = Color32::from_rgb(0x10, 0x23, 0x3F);

/// The empty part of the footer's hatched context meter.
pub const HATCH_TRACK: Color32 = Color32::from_rgb(0xEC, 0xEC, 0xE9);
/// The threads inside that empty part.
pub const HATCH_LINE: Color32 = Color32::from_rgb(0xD3, 0xD3, 0xCE);

/// How tall one row of interface chrome is.
pub const ROW: f32 = 26.0;

/// Rounded corner used by every framed surface.
pub const RADIUS: u8 = 6;

/// Corner radius of a floating card.
pub const CARD_RADIUS: u8 = 12;

pub const SIDEBAR: f32 = 236.0;

/// Width of the right panel.
pub const PANEL_WIDTH: f32 = 312.0;

/// The gutter between the window edge and a floating card.
pub const GUTTER: f32 = 8.0;

/// Installs the theme on a context, at the configured font sizes.
///
/// @param ctx the egui context
/// @param font_size interface text size in points
/// @param terminal_size terminal and code text size in points
pub fn apply(ctx: &egui::Context, font_size: f32, terminal_size: f32) {
    // One light theme, applied to both slots so the app cannot fall back to a
    // different palette when the system preference changes under it.
    ctx.set_theme(egui::Theme::Light);
    ctx.all_styles_mut(|style| {
        let visuals = &mut style.visuals;
        visuals.dark_mode = false;
        visuals.panel_fill = PANEL;
        visuals.window_fill = PANEL;
        visuals.window_stroke = Stroke::new(1.0, BORDER);
        visuals.window_corner_radius = CornerRadius::same(CARD_RADIUS);
        visuals.window_shadow = Shadow {
            offset: [0, 6],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(28),
        };
        visuals.popup_shadow = Shadow {
            offset: [0, 3],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(24),
        };
        visuals.menu_corner_radius = CornerRadius::same(RADIUS);
        visuals.extreme_bg_color = ELEVATED;
        visuals.faint_bg_color = ELEVATED;
        visuals.code_bg_color = ELEVATED;
        visuals.hyperlink_color = ACCENT;
        visuals.warn_fg_color = AMBER;
        visuals.error_fg_color = RED;
        visuals.selection.bg_fill = SELECTION;
        visuals.selection.stroke = Stroke::new(1.0, SELECTION_TEXT);
        visuals.button_frame = true;
        visuals.collapsing_header_frame = false;
        visuals.indent_has_left_vline = false;
        visuals.weak_text_alpha = 0.75;

        let widgets = &mut visuals.widgets;
        for widget in [
            &mut widgets.noninteractive,
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
            &mut widgets.open,
        ] {
            widget.corner_radius = CornerRadius::same(RADIUS);
            widget.bg_stroke = Stroke::new(1.0, BORDER);
        }
        widgets.noninteractive.bg_fill = PANEL;
        widgets.noninteractive.weak_bg_fill = PANEL;
        widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        widgets.inactive.bg_fill = PANEL;
        widgets.inactive.weak_bg_fill = PANEL;
        widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        widgets.hovered.bg_fill = HOVER_SOFT;
        widgets.hovered.weak_bg_fill = HOVER_SOFT;
        widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
        widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
        widgets.active.bg_fill = ELEVATED;
        widgets.active.weak_bg_fill = ELEVATED;
        widgets.active.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
        widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
        widgets.open.bg_fill = ELEVATED;
        widgets.open.weak_bg_fill = ELEVATED;
        widgets.open.fg_stroke = Stroke::new(1.0, TEXT);

        let spacing = &mut style.spacing;
        spacing.item_spacing = Vec2::new(6.0, 6.0);
        spacing.button_padding = Vec2::new(8.0, 4.0);
        spacing.menu_margin = eframe::egui::Margin::symmetric(6, 6);
        spacing.window_margin = eframe::egui::Margin::same(10);
        spacing.indent = 14.0;
        spacing.interact_size = Vec2::new(20.0, 20.0);
        spacing.scroll.bar_width = 8.0;
        spacing.scroll.bar_inner_margin = 2.0;

        style.text_styles = [
            (
                TextStyle::Heading,
                FontId::new(font_size + 3.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Body,
                FontId::new(font_size, FontFamily::Proportional),
            ),
            (
                TextStyle::Button,
                FontId::new(font_size, FontFamily::Proportional),
            ),
            (
                TextStyle::Small,
                FontId::new(font_size - 2.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(terminal_size, FontFamily::Monospace),
            ),
        ]
        .into();
    });
}

/// Installs the interface and monospace faces, preferring the system fonts the
/// reference client is drawn in.
///
/// A missing or unparsable file is skipped rather than fatal: egui keeps the
/// bundled faces for anything this fails to load, so the window always draws.
///
/// @param ctx the egui context
/// @returns the names of the faces that were installed, for the log
pub fn install_fonts(ctx: &egui::Context) -> Vec<String> {
    let mut fonts = egui::FontDefinitions::default();
    let mut installed = Vec::new();

    for (family, candidates) in [
        (
            FontFamily::Proportional,
            &[
                ("SF Pro", "/System/Library/Fonts/SFNS.ttf", 0),
                (
                    "Helvetica Neue",
                    "/System/Library/Fonts/HelveticaNeue.ttc",
                    0,
                ),
                ("Arial", "/System/Library/Fonts/Supplemental/Arial.ttf", 0),
            ][..],
        ),
        (
            FontFamily::Monospace,
            &[
                ("SF Mono", "/System/Library/Fonts/SFNSMono.ttf", 0),
                ("Menlo", "/System/Library/Fonts/Menlo.ttc", 0),
                (
                    "Andale Mono",
                    "/System/Library/Fonts/Supplemental/Andale Mono.ttf",
                    0,
                ),
            ][..],
        ),
    ] {
        let Some((name, bytes, index)) = candidates.iter().find_map(|(name, path, index)| {
            let bytes = std::fs::read(path).ok()?;
            Some((*name, bytes, *index))
        }) else {
            continue;
        };
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        fonts
            .font_data
            .insert(name.to_string(), std::sync::Arc::new(data));
        let entry = fonts.families.entry(family.clone()).or_default();
        // Ahead of the bundled faces: this is the face the whole window reads in.
        entry.insert(0, name.to_string());
        installed.push(format!("{name} ({})", family_name(&family)));
    }

    if !installed.is_empty() {
        ctx.set_fonts(fonts);
    }
    installed
}

/// A family's name for logs.
///
/// @param family the family
/// @returns a display name
fn family_name(family: &FontFamily) -> String {
    match family {
        FontFamily::Proportional => "text".to_string(),
        FontFamily::Monospace => "mono".to_string(),
        FontFamily::Name(name) => name.to_string(),
    }
}

/// The monospace font at a size, for the code and terminal views.
///
/// @param size points
/// @returns the font id
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// The interface font at a size, relative to the body size.
///
/// @param delta points to add to the body size
/// @returns the font id
pub fn text(delta: f32) -> FontId {
    FontId::new(13.5 + delta, FontFamily::Proportional)
}

/// Converts a terminal colour to an egui colour.
///
/// @param rgb the terminal's stored colour
/// @returns the same colour for painting
pub fn rgb(rgb: harness_core::term::Rgb) -> Color32 {
    Color32::from_rgb(rgb.r, rgb.g, rgb.b)
}

/// Draws a hairline border around a rounded rectangle.
///
/// @param painter the painter to draw with
/// @param rect the rectangle
/// @param radius the corner radius
/// @param colour the border colour
pub fn outline(painter: &egui::Painter, rect: Rect, radius: u8, colour: Color32) {
    painter.rect_stroke(
        rect,
        CornerRadius::same(radius),
        Stroke::new(1.0, colour),
        StrokeKind::Inside,
    );
}

/// Draws a small pill with a label — statuses, counts, kinds.
///
/// @param ui the interface to draw into
/// @param label text in the pill
/// @param colour the pill's colour; the fill is a faint tint of it
/// @returns the response of the pill as a widget
pub fn badge(ui: &mut Ui, label: &str, colour: Color32) -> Response {
    let font = text(-2.0);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, colour);
    let padding = Vec2::new(6.0, 2.0);
    let size = galley.size() + padding * 2.0;
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(4), colour.gamma_multiply(0.14));
    painter.galley(rect.min + padding, galley, colour);
    response
}

/// Draws a turning arc — the interface's one sign of work in progress.
///
/// The phase comes from egui's own clock rather than from a counter kept
/// anywhere, so every spinner in a frame agrees with every other, and the arc
/// asks for the next frame itself: it turns because something keeps repainting
/// it, which is exactly what should stop when the work does.
///
/// @param ui the interface to draw into
/// @param diameter the circle's diameter
/// @param colour the arc's colour
/// @returns the response of the spinner as a widget
pub fn spinner(ui: &mut Ui, diameter: f32, colour: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::hover());
    let phase = ui.input(|input| input.time) as f32;
    paint_spinner(ui.painter(), rect, colour, phase);
    ui.ctx().request_repaint();
    response
}

/// Draws a turning arc into a rectangle someone else measured.
///
/// @param painter the painter to draw with
/// @param rect the rectangle to turn inside
/// @param colour the arc's colour
/// @param phase seconds since the interface opened
pub fn paint_spinner(painter: &egui::Painter, rect: Rect, colour: Color32, phase: f32) {
    // One full turn takes this long; the arc covers three quarters of the circle
    // and is drawn as segments that fade towards its tail.
    const PERIOD: f32 = 1.1;
    const SPAN: f32 = std::f32::consts::PI * 1.5;
    const STEPS: usize = 18;

    let radius = (rect.width().min(rect.height()) / 2.0 - 1.0).max(2.0);
    let centre = rect.center();
    let start = phase * std::f32::consts::TAU / PERIOD;
    let point = |angle: f32| {
        egui::pos2(
            centre.x + radius * angle.cos(),
            centre.y + radius * angle.sin(),
        )
    };
    for step in 0..STEPS {
        let from = start + SPAN * (step as f32 / STEPS as f32);
        let to = start + SPAN * ((step + 1) as f32 / STEPS as f32);
        // The tail is faint and the head is solid, which is what makes a bare
        // arc read as turning rather than as a broken circle.
        let alpha = 0.12 + 0.88 * (step as f32 + 1.0) / STEPS as f32;
        painter.add(egui::Shape::line_segment(
            [point(from), point(to)],
            Stroke::new(1.6, colour.gamma_multiply(alpha)),
        ));
    }
}

/// Draws a small keyboard-style chip, e.g. `⌘K`.
///
/// @param ui the interface to draw into
/// @param label the key or chord
/// @returns the response of the chip as a widget
pub fn key_chip(ui: &mut Ui, label: &str) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), text(-3.0), DIM);
    let padding = Vec2::new(5.0, 2.0);
    let size = galley.size() + padding * 2.0;
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(3), ELEVATED);
    outline(painter, rect, 3, BORDER);
    painter.galley(rect.min + padding, galley, DIM);
    response
}

/// Draws a section label: small, spaced, dim, with the rest of the row empty.
///
/// @param ui the interface to draw into
/// @param label section name
pub fn section(ui: &mut Ui, label: &str) {
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        ui.label(
            RichText::new(label.to_uppercase())
                .size(10.0)
                .color(FAINT)
                .extra_letter_spacing(1.2),
        );
    });
}

/// A horizontal progress meter, used for the context window and token budget.
///
/// @param ui the interface to draw into
/// @param fraction how full, clamped to `0.0..=1.0`
/// @param width pixel width
/// @param colour the fill colour
/// @returns the response
pub fn meter(ui: &mut Ui, fraction: f32, width: f32, colour: Color32) -> Response {
    let height = 6.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(3), BORDER);
    let filled = (rect.width() * fraction.clamp(0.0, 1.0)).max(0.0);
    if filled > 0.0 {
        let bar = Rect::from_min_size(rect.min, Vec2::new(filled, rect.height()));
        painter.rect_filled(bar, CornerRadius::same(3), colour);
    }
    response
}

/// The footer's context meter: what is spent in solid ink, what is left hatched,
/// the way the reference client draws it.
///
/// @param ui the interface to draw into
/// @param fraction how full, clamped to `0.0..=1.0`
/// @param width pixel width
/// @param colour the fill colour
/// @returns the response
pub fn hatch_meter(ui: &mut Ui, fraction: f32, width: f32, colour: Color32) -> Response {
    let height = 7.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter();
    let radius = CornerRadius::same(static_radius(height / 2.0));
    painter.rect_filled(rect, radius, HATCH_TRACK);
    let fraction = fraction.clamp(0.0, 1.0);
    let filled = rect.width() * fraction;
    // The remainder is threaded with diagonals rather than filled: the two
    // halves of the bar are then one widget, not two.
    let remainder = Rect::from_min_max(egui::pos2(rect.left() + filled, rect.top()), rect.max);
    if remainder.width() > 2.0 {
        let painter = painter.with_clip_rect(remainder.intersect(rect));
        let mut x = remainder.left() - height;
        while x < remainder.right() {
            painter.line_segment(
                [
                    egui::pos2(x, rect.bottom() + 1.0),
                    egui::pos2(x + height + 1.0, rect.top() - 1.0),
                ],
                Stroke::new(1.0, HATCH_LINE),
            );
            x += 3.5;
        }
    }
    if filled > 0.0 {
        let bar = Rect::from_min_size(rect.min, Vec2::new(filled.max(height), rect.height()));
        painter.rect_filled(bar, radius, colour);
    }
    response
}

/// A floating surface: what a card is before anything is written on it.
///
/// @param ui the interface to draw into
/// @param rect the surface's rectangle
/// @param fill the surface colour
/// @param radius the corner radius
pub fn surface(ui: &Ui, rect: Rect, fill: Color32, radius: u8) {
    let painter = ui.painter();
    painter.add(card_shadow().as_shape(rect, CornerRadius::same(radius)));
    painter.rect_filled(rect, CornerRadius::same(radius), fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(radius),
        Stroke::new(1.0, BORDER),
        StrokeKind::Inside,
    );
}

/// The shadow every floating card casts.
///
/// @returns the shadow
pub fn card_shadow() -> Shadow {
    Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(16),
    }
}

/// Runs a closure in a rectangle, so a card's body is laid out where the card
/// was drawn rather than where the cursor happens to be.
///
/// @param ui the interface to draw into
/// @param rect the inner rectangle
/// @param add_contents the body
/// @returns the inner response of the body
pub fn inside<R>(ui: &mut Ui, rect: Rect, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    add_contents(&mut inner)
}

/// Lays out a label and a value on one row, the value right-aligned.
///
/// @param ui the interface to draw into
/// @param label the left text
/// @param value the right text
/// @param colour the value's colour
pub fn stat_row(ui: &mut Ui, label: &str, value: &str, colour: Color32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(12.0).color(DIM));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(value).size(12.0).monospace().color(colour));
        });
    });
}

/// A clickable label with hover feedback, for toolbar actions.
///
/// @param ui the interface to draw into
/// @param label the action's text
/// @param enabled whether the action can run now
/// @param colour the text colour
/// @returns the response
pub fn action(ui: &mut Ui, label: &str, enabled: bool, colour: Color32) -> Response {
    let colour = if enabled { colour } else { FAINT };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), text(-1.5), colour);
    let padding = Vec2::new(7.0, 3.0);
    let size = galley.size() + padding * 2.0;
    let (rect, response) = ui.allocate_exact_size(
        size,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let painter = ui.painter();
    if response.hovered() && enabled {
        painter.rect_filled(rect, CornerRadius::same(4), HOVER_SOFT);
    }
    painter.galley(rect.min + padding, galley, colour);
    response
}

/// A thin vertical or horizontal rule.
///
/// @param ui the interface to draw into
/// @param vertical whether the rule stands up
pub fn rule(ui: &mut Ui, vertical: bool) {
    let thickness = 1.0;
    let length = if vertical {
        ui.available_height()
    } else {
        ui.available_width()
    };
    let size = if vertical {
        Vec2::new(thickness, length)
    } else {
        Vec2::new(length, thickness)
    };
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if vertical {
        ui.painter()
            .vline(rect.center().x, rect.y_range(), Stroke::new(1.0, BORDER));
    } else {
        ui.painter()
            .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, BORDER));
    }
}

/// A framed surface for a card: agent tool calls, waves, notices.
///
/// @param ui the interface to draw into
/// @param fill the surface colour
/// @param accent an optional left edge marker
/// @param add_contents the card's body
/// @returns the inner response of the body
pub fn card<R>(
    ui: &mut Ui,
    fill: Color32,
    accent: Option<Color32>,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    let frame = egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(eframe::egui::Margin::symmetric(10, 8));
    let response = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        add_contents(ui)
    });
    if let Some(colour) = accent {
        // A 2px marker on the card's leading edge, inset so the corners stay
        // round. It is measured from the frame's own rectangle and not from the
        // body's `max_rect`: inside a scroll area that is the whole remaining
        // height, which would stripe the column rather than the card.
        let rect = response.response.rect;
        ui.painter().rect_filled(
            Rect::from_min_size(
                rect.min + Vec2::new(1.0, 4.0),
                Vec2::new(2.0, (rect.height() - 8.0).max(2.0)),
            ),
            CornerRadius::same(1),
            colour,
        );
    }
    response.inner
}

/// A floating white surface: the conversation card and the side panel.
///

/// A full-width row that behaves like a list item: hover fill, an optional
/// selected fill, an optional leading glyph, and a trailing chevron.
///
/// @param ui the interface to draw into
/// @param selected whether the row is current
/// @param on_chrome whether the row sits on the gray chrome (darker hover)
/// @param add_contents the row's body; it is laid out left to right
/// @returns the response of the whole row
pub fn list_row(
    ui: &mut Ui,
    selected: bool,
    on_chrome: bool,
    add_contents: impl FnOnce(&mut Ui),
) -> Response {
    let height = 30.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(
            rect,
            CornerRadius::same(6),
            if on_chrome { ACTIVE } else { ACCENT_DIM },
        );
    } else if response.hovered() {
        painter.rect_filled(
            rect,
            CornerRadius::same(6),
            if on_chrome { HOVER } else { HOVER_SOFT },
        );
    }
    // The contents are painted into the row's own rect, inset for the glyph.
    let mut content = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(8.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    add_contents(&mut content);
    response
}

/// A corner radius from a float, clamped to the `u8` the painter takes.
///
/// @param value the radius in points
/// @returns the radius as a `u8`
pub fn static_radius(value: f32) -> u8 {
    value.clamp(0.0, 255.0) as u8
}

/// A circle avatar with an initial, tinted by a hash of the name, matching the
/// reference's colourful delegation markers.
///
/// @param ui the interface to draw into
/// @param name the name to derive the tint and initial from
/// @param diameter the circle's diameter
/// @returns the response
pub fn avatar(ui: &mut Ui, name: &str, diameter: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::hover());
    let painter = ui.painter();
    let hue = hash_hue(name);
    let top = Color32::from_rgb(hue.0, hue.1, hue.2);
    let bottom = Color32::from_rgb(
        (hue.0 as f32 * 0.75) as u8,
        (hue.1 as f32 * 0.75) as u8,
        (hue.2 as f32 * 0.85) as u8,
    );
    // A vertical two-tone disc reads as a soft gradient at this size.
    painter.circle_filled(rect.center(), diameter / 2.0, top);
    let lower = Rect::from_min_max(
        egui::pos2(rect.left(), rect.center().y),
        egui::pos2(rect.right(), rect.bottom()),
    );
    painter.rect_filled(
        lower,
        CornerRadius::same(static_radius(diameter / 2.0)),
        bottom.gamma_multiply(0.55),
    );
    let initial = name
        .chars()
        .find(|ch| ch.is_alphanumeric())
        .map(|ch| ch.to_ascii_uppercase())
        .unwrap_or('•');
    let galley = painter.layout_no_wrap(initial.to_string(), text(-1.0), Color32::WHITE);
    painter.galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    response
}

/// A stable colour for a name.
///
/// @param name the name to hash
/// @returns an RGB triple
fn hash_hue(name: &str) -> (u8, u8, u8) {
    let mut hash: u32 = 2_166_136_261;
    for byte in name.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(16_777_619);
    }
    const TINTS: [(u8, u8, u8); 6] = [
        (0x6C, 0x8C, 0xF5),
        (0x4F, 0xB6, 0xA8),
        (0xE0, 0x8A, 0x5A),
        (0xA6, 0x7C, 0xE8),
        (0xE0, 0x6C, 0x9A),
        (0x5A, 0x9E, 0xE0),
    ];
    TINTS[(hash as usize) % TINTS.len()]
}
