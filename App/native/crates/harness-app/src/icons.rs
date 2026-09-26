//! The interface's glyphs, drawn rather than typed.
//!
//! A harness that ships its own renderer does not get to borrow a symbol font:
//! the window's own icon set is a few dozen line drawings, and drawing them
//! means they inherit the theme's colour, sit exactly on the pixel grid, and
//! scale with the font size instead of with a sprite sheet.
//!
//! Every glyph is described in a unit square and mapped onto the rectangle it is
//! given, so one routine draws the same mark at 14 points in a list row and at
//! 22 points in a toolbar. Strokes are sized from the box, which is what keeps a
//! 14-point glyph from turning into a smudge.

use eframe::egui::{
    self, Color32, CornerRadius, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};

use crate::theme;

/// Every mark the interface can draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// A panel with a side column: the sidebar toggle.
    Panel,
    /// A four-point star: the agent, and a fresh notice.
    Sparkle,
    /// A circled pen: start something new.
    NewTask,
    /// A magnifier.
    Search,
    /// The harness mark: a small block with two eyes.
    Harness,
    /// An open book.
    Book,
    /// A window with a title bar.
    Window,
    /// A clock face.
    Clock,
    /// Four tiles.
    Grid,
    /// A downward chevron.
    Chevron,
    /// A folder.
    Folder,
    /// Two stacked sheets.
    Copy,
    /// A thumbs up.
    ThumbUp,
    /// A thumbs down.
    ThumbDown,
    /// A corner-down arrow.
    Reply,
    /// A circular arrow: ask again.
    Refresh,
    /// A terminal tile: a prompt inside a frame.
    Terminal,
    /// An open arc, the shape of work in flight.
    Ring,
    /// A check.
    Check,
    /// A plus.
    Plus,
    /// A paperclip.
    Clip,
    /// A circled play triangle.
    Play,
    /// A microphone.
    Mic,
    /// An arrow pointing up.
    ArrowUp,
    /// A gear.
    Gear,
    /// A question mark.
    Question,
    /// A page with a folded corner: a file to edit.
    Doc,
    /// A branch off a trunk: the repository.
    Branch,
    /// A half-dial with a needle: the meter.
    Gauge,
    /// Three dots: more actions.
    More,
}

/// Draws one glyph and returns its rectangle as a widget slot.
///
/// @param ui the interface to draw into
/// @param icon which mark
/// @param colour the stroke colour
/// @param size the square's side in points
/// @returns the response of the allocated box
pub fn icon(ui: &mut Ui, icon: Icon, colour: Color32, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint(ui, icon, rect, colour);
    response
}

/// Draws one glyph into a rectangle, allocating nothing.
///
/// @param ui the interface to draw into
/// @param icon which mark
/// @param rect the box to draw in
/// @param colour the stroke colour
pub fn paint(ui: &Ui, icon: Icon, rect: Rect, colour: Color32) {
    let pen = Pen {
        painter: ui.painter(),
        rect,
        colour,
        width: (rect.width() * 0.085).clamp(1.0, 2.2),
    };
    match icon {
        Icon::Panel => pen.panel(),
        Icon::Sparkle => pen.sparkle(),
        Icon::NewTask => pen.new_task(),
        Icon::Search => pen.search(),
        Icon::Harness => pen.harness(),
        Icon::Book => pen.book(),
        Icon::Window => pen.window(),
        Icon::Clock => pen.clock(),
        Icon::Grid => pen.grid(),
        Icon::Chevron => pen.chevron(),
        Icon::Folder => pen.folder(),
        Icon::Copy => pen.copy(),
        Icon::ThumbUp => pen.thumb(true),
        Icon::ThumbDown => pen.thumb(false),
        Icon::Reply => pen.reply(),
        Icon::Refresh => pen.refresh(),
        Icon::Terminal => pen.terminal(),
        Icon::Ring => pen.ring(),
        Icon::Check => pen.check(),
        Icon::Plus => pen.plus(),
        Icon::Clip => pen.clip(),
        Icon::Play => pen.play(),
        Icon::Mic => pen.mic(),
        Icon::ArrowUp => pen.arrow_up(),
        Icon::Gear => pen.gear(),
        Icon::Question => pen.question(),
        Icon::Doc => pen.doc(),
        Icon::Branch => pen.branch(),
        Icon::Gauge => pen.gauge(),
        Icon::More => pen.more(),
    }
}

/// A clickable glyph: the box, the hover fill, and the mark.
///
/// @param ui the interface to draw into
/// @param icon which mark
/// @param colour the stroke colour
/// @param size the box's side in points
/// @param fill the hover fill
/// @returns the response
pub fn button(ui: &mut Ui, icon: Icon, colour: Color32, size: f32, fill: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(theme::static_radius(size * 0.28)),
            fill,
        );
    }
    paint(ui, icon, rect.shrink(size * 0.16), colour);
    response
}

/// Draws a glyph inside a tinted tile — the panel's process rows and the
/// composer's marks sit in one.
///
/// @param ui the interface to draw into
/// @param icon which mark
/// @param colour the stroke colour
/// @param size the tile's side in points
/// @param tint the tile's fill
/// @param radius the tile's corner radius
/// @returns the response
pub fn tile(
    ui: &mut Ui,
    icon: Icon,
    colour: Color32,
    size: f32,
    tint: Color32,
    radius: u8,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(radius), tint);
    paint(ui, icon, rect.shrink(size * 0.22), colour);
    response
}

/// The drawing surface: unit coordinates in, geometry out.
struct Pen<'a> {
    painter: &'a egui::Painter,
    rect: Rect,
    colour: Color32,
    width: f32,
}

impl Pen<'_> {
    /// A point in the glyph's unit square.
    ///
    /// @param x 0..1 across
    /// @param y 0..1 down
    /// @returns the point in the box
    fn at(&self, x: f32, y: f32) -> Pos2 {
        Pos2::new(
            self.rect.min.x + self.rect.width() * x,
            self.rect.min.y + self.rect.height() * y,
        )
    }

    /// A stroke at the glyph's weight.
    ///
    /// @param factor weight multiplier
    /// @returns the stroke
    fn stroke(&self, factor: f32) -> Stroke {
        Stroke::new(self.width * factor, self.colour)
    }

    /// An open polyline.
    ///
    /// @param points the vertices, in unit coordinates
    fn line(&self, points: &[(f32, f32)]) {
        let path: Vec<Pos2> = points.iter().map(|(x, y)| self.at(*x, *y)).collect();
        self.painter.add(Shape::line(path, self.stroke(1.0)));
    }

    /// A straight segment.
    ///
    /// @param ax start x
    /// @param ay start y
    /// @param bx end x
    /// @param by end y
    fn seg(&self, ax: f32, ay: f32, bx: f32, by: f32) {
        self.painter
            .line_segment([self.at(ax, ay), self.at(bx, by)], self.stroke(1.0));
    }

    /// A circle outline.
    ///
    /// @param cx centre x
    /// @param cy centre y
    /// @param r radius as a fraction of the box
    fn circle(&self, cx: f32, cy: f32, r: f32) {
        self.painter
            .circle_stroke(self.at(cx, cy), self.rect.width() * r, self.stroke(1.0));
    }

    /// An arc, drawn as a polyline so its ends stay round-free like the rest.
    ///
    /// @param cx centre x
    /// @param cy centre y
    /// @param r radius as a fraction of the box
    /// @param from start angle in radians
    /// @param to end angle in radians
    /// @param factor stroke weight
    fn arc(&self, cx: f32, cy: f32, r: f32, from: f32, to: f32, factor: f32) {
        let centre = self.at(cx, cy);
        let radius = self.rect.width() * r;
        let steps = 18;
        let path: Vec<Pos2> = (0..=steps)
            .map(|step| {
                let angle = from + (to - from) * step as f32 / steps as f32;
                centre + Vec2::new(angle.cos(), angle.sin()) * radius
            })
            .collect();
        self.painter.add(Shape::line(path, self.stroke(factor)));
    }

    /// A rounded rectangle outline.
    ///
    /// @param min minimum corner in unit coordinates
    /// @param max maximum corner in unit coordinates
    /// @param radius corner radius in points
    fn frame(&self, min: (f32, f32), max: (f32, f32), radius: f32) {
        let rect = Rect::from_min_max(self.at(min.0, min.1), self.at(max.0, max.1));
        self.painter.rect_stroke(
            rect,
            CornerRadius::same(theme::static_radius(radius)),
            self.stroke(1.0),
            StrokeKind::Inside,
        );
    }

    /// A filled dot.
    ///
    /// @param x centre x
    /// @param y centre y
    /// @param r radius as a fraction of the box
    fn dot(&self, x: f32, y: f32, r: f32) {
        self.painter
            .circle_filled(self.at(x, y), self.rect.width() * r, self.colour);
    }

    /// A filled polygon from unit coordinates.
    ///
    /// @param points the vertices, in unit coordinates
    fn fill(&self, points: &[(f32, f32)]) {
        let path: Vec<Pos2> = points.iter().map(|(x, y)| self.at(*x, *y)).collect();
        self.painter
            .add(Shape::convex_polygon(path, self.colour, Stroke::NONE));
    }

    /// A panel with a divided side column.
    fn panel(&self) {
        self.frame((0.12, 0.18), (0.88, 0.82), 2.0);
        self.seg(0.38, 0.18, 0.38, 0.82);
    }

    /// A four-point star with a smaller companion, like the reference's mark.
    fn sparkle(&self) {
        let star = |cx: f32, cy: f32, r: f32| {
            vec![
                (cx, cy - r),
                (cx + r * 0.28, cy - r * 0.28),
                (cx + r, cy),
                (cx + r * 0.28, cy + r * 0.28),
                (cx, cy + r),
                (cx - r * 0.28, cy + r * 0.28),
                (cx - r, cy),
                (cx - r * 0.28, cy - r * 0.28),
            ]
        };
        self.fill(&star(0.42, 0.58, 0.38));
        self.fill(&star(0.78, 0.22, 0.19));
    }

    /// A circled pen.
    fn new_task(&self) {
        self.circle(0.5, 0.5, 0.36);
        self.seg(0.36, 0.64, 0.66, 0.34);
        self.seg(0.60, 0.28, 0.72, 0.40);
    }

    /// A magnifier: a ring and its handle.
    fn search(&self) {
        self.circle(0.44, 0.44, 0.28);
        self.seg(0.65, 0.65, 0.84, 0.84);
    }

    /// The harness mark: a block with two eyes and a foot.
    fn harness(&self) {
        self.frame((0.14, 0.24), (0.86, 0.74), 3.0);
        self.dot(0.36, 0.46, 0.07);
        self.dot(0.64, 0.46, 0.07);
        self.seg(0.32, 0.86, 0.32, 0.74);
        self.seg(0.68, 0.86, 0.68, 0.74);
    }

    /// An open book: two leaves over a spine.
    fn book(&self) {
        self.frame((0.12, 0.22), (0.88, 0.80), 2.0);
        self.seg(0.5, 0.22, 0.5, 0.80);
        self.seg(0.12, 0.44, 0.5, 0.36);
        self.seg(0.5, 0.36, 0.88, 0.44);
    }

    /// A window with a title bar.
    fn window(&self) {
        self.frame((0.12, 0.20), (0.88, 0.80), 3.0);
        self.seg(0.12, 0.40, 0.88, 0.40);
        self.dot(0.26, 0.30, 0.055);
    }

    /// A clock face: a ring with two hands.
    fn clock(&self) {
        self.circle(0.5, 0.5, 0.36);
        self.seg(0.5, 0.5, 0.5, 0.28);
        self.seg(0.5, 0.5, 0.68, 0.60);
    }

    /// Four tiles.
    fn grid(&self) {
        for (x, y) in [(0.12, 0.12), (0.56, 0.12), (0.12, 0.56), (0.56, 0.56)] {
            self.frame((x, y), (x + 0.32, y + 0.32), 2.0);
        }
    }

    /// A downward chevron.
    fn chevron(&self) {
        self.line(&[(0.30, 0.42), (0.5, 0.62), (0.70, 0.42)]);
    }

    /// A folder with a tab.
    fn folder(&self) {
        self.frame((0.10, 0.26), (0.90, 0.78), 2.0);
        self.seg(0.10, 0.26, 0.34, 0.26);
        self.seg(0.10, 0.26, 0.10, 0.20);
        self.seg(0.10, 0.20, 0.42, 0.20);
    }

    /// Two overlapping sheets.
    fn copy(&self) {
        self.frame((0.12, 0.12), (0.66, 0.66), 2.0);
        self.line(&[(0.34, 0.80), (0.80, 0.80), (0.80, 0.34)]);
    }

    /// A thumb, up or down.
    ///
    /// @param up whether the thumb points up
    fn thumb(&self, up: bool) {
        // One shape, mirrored: the cuff sits at the bottom for an up thumb.
        let mirror = |y: f32| if up { y } else { 1.0 - y };
        self.frame((0.16, mirror(0.46)), (0.42, mirror(0.86)), 2.0);
        self.line(&[
            (0.42, mirror(0.80)),
            (0.60, mirror(0.80)),
            (0.80, mirror(0.62)),
            (0.80, mirror(0.34)),
            (0.48, mirror(0.34)),
            (0.48, mirror(0.14)),
            (0.34, mirror(0.40)),
        ]);
    }

    /// A corner-down arrow: the reply and suggestion mark.
    fn reply(&self) {
        self.line(&[(0.24, 0.24), (0.24, 0.66), (0.76, 0.66)]);
        self.line(&[(0.56, 0.46), (0.78, 0.66), (0.56, 0.86)]);
    }

    /// A circular arrow, open at the top right where its head points.
    fn refresh(&self) {
        self.arc(0.5, 0.5, 0.32, 0.45, 5.62, 1.0);
        self.fill(&[(0.58, 0.16), (0.88, 0.30), (0.58, 0.44)]);
    }

    /// A prompt inside a frame.
    fn terminal(&self) {
        self.frame((0.10, 0.16), (0.90, 0.84), 2.0);
        self.line(&[(0.28, 0.40), (0.40, 0.50), (0.28, 0.60)]);
        self.seg(0.52, 0.62, 0.72, 0.62);
    }

    /// An open arc with a gap at the top right: work in flight.
    fn ring(&self) {
        self.arc(0.5, 0.5, 0.36, 0.5, 5.5, 1.1);
    }

    /// A check.
    fn check(&self) {
        self.line(&[(0.20, 0.52), (0.42, 0.74), (0.80, 0.28)]);
    }

    /// A plus.
    fn plus(&self) {
        self.seg(0.5, 0.22, 0.5, 0.78);
        self.seg(0.22, 0.5, 0.78, 0.5);
    }

    /// A paperclip.
    fn clip(&self) {
        self.line(&[
            (0.62, 0.22),
            (0.32, 0.52),
            (0.32, 0.74),
            (0.52, 0.86),
            (0.72, 0.74),
            (0.72, 0.38),
            (0.56, 0.22),
            (0.44, 0.30),
            (0.44, 0.62),
        ]);
    }

    /// A circled play triangle.
    fn play(&self) {
        self.circle(0.5, 0.5, 0.36);
        self.fill(&[(0.42, 0.34), (0.68, 0.50), (0.42, 0.66)]);
    }

    /// A microphone: a capsule, a cradle, and a stem.
    fn mic(&self) {
        self.frame((0.38, 0.12), (0.62, 0.58), 3.0);
        self.arc(0.5, 0.50, 0.30, 0.2, 2.94, 1.0);
        self.seg(0.5, 0.80, 0.5, 0.90);
    }

    /// An arrow pointing up, the send mark.
    fn arrow_up(&self) {
        self.seg(0.5, 0.80, 0.5, 0.22);
        self.line(&[(0.30, 0.42), (0.5, 0.22), (0.70, 0.42)]);
    }

    /// A gear: a ring with teeth.
    fn gear(&self) {
        self.circle(0.5, 0.5, 0.20);
        for step in 0..8 {
            let angle = std::f32::consts::TAU * step as f32 / 8.0;
            let (sin, cos) = angle.sin_cos();
            self.seg(
                0.5 + cos * 0.26,
                0.5 + sin * 0.26,
                0.5 + cos * 0.38,
                0.5 + sin * 0.38,
            );
        }
    }

    /// A question mark, as three strokes: a hook, a dot.
    fn question(&self) {
        self.arc(0.47, 0.33, 0.19, 3.4, 6.0, 1.1);
        self.seg(0.47, 0.48, 0.47, 0.62);
        self.dot(0.47, 0.79, 0.075);
    }

    /// Three dots.
    fn more(&self) {
        self.dot(0.24, 0.5, 0.075);
        self.dot(0.50, 0.5, 0.075);
        self.dot(0.76, 0.5, 0.075);
    }

    /// A page with a folded corner and two lines of text.
    fn doc(&self) {
        self.line(&[
            (0.24, 0.10),
            (0.62, 0.10),
            (0.80, 0.28),
            (0.80, 0.90),
            (0.20, 0.90),
            (0.20, 0.10),
            (0.24, 0.10),
        ]);
        self.line(&[(0.62, 0.10), (0.62, 0.28), (0.80, 0.28)]);
        self.seg(0.34, 0.52, 0.66, 0.52);
        self.seg(0.34, 0.68, 0.56, 0.68);
    }

    /// A branch off a trunk: two commits on the trunk, one beside them.
    fn branch(&self) {
        self.circle(0.30, 0.22, 0.085);
        self.circle(0.30, 0.78, 0.085);
        self.circle(0.70, 0.50, 0.085);
        self.seg(0.30, 0.305, 0.30, 0.695);
        self.seg(0.385, 0.50, 0.615, 0.50);
    }

    /// A half-dial with a needle.
    fn gauge(&self) {
        self.arc(0.5, 0.62, 0.36, 3.35, 6.08, 1.1);
        self.seg(0.5, 0.62, 0.66, 0.40);
        self.dot(0.5, 0.62, 0.06);
    }
}
