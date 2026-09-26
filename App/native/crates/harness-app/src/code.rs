//! Code and patch views.
//!
//! Two drawings live here, because two panes need the same two things. The
//! editor wants source with the syntax coloured; the git pane and the agent's
//! tool cards want a patch whose added and removed lines are legible at a
//! glance. Both are drawn as text painted into a scrolled region, so only the
//! rows on screen are laid out.
//!
//! The colouring is a line scanner, not a parser. It knows comments, strings,
//! numbers, keywords, and call sites, which is what a diff or a build script
//! needs and what a full grammar would cost hundreds of milliseconds to give.

use eframe::egui::text::LayoutJob;
use eframe::egui::{self, Color32, CornerRadius, FontId, Rect, TextFormat, Ui, Vec2};

use crate::theme;

/// Keywords, control flow, declarations.
pub const KEYWORD: Color32 = Color32::from_rgb(0xCF, 0x22, 0x2E);
/// Quoted text and template literals.
pub const STRING: Color32 = Color32::from_rgb(0x0A, 0x30, 0x69);
/// Numeric literals.
pub const NUMBER: Color32 = Color32::from_rgb(0x05, 0x50, 0xAE);
/// Call sites and definitions.
pub const FUNCTION: Color32 = Color32::from_rgb(0x82, 0x50, 0xDF);
/// Types, from a capitalised name.
pub const TYPE: Color32 = Color32::from_rgb(0x95, 0x38, 0x00);
/// Comments and documentation.
pub const COMMENT: Color32 = Color32::from_rgb(0x6E, 0x77, 0x81);
/// Brackets, semicolons, commas.
pub const PUNCT: Color32 = Color32::from_rgb(0x57, 0x60, 0x6A);

/// How one language's line scanner should behave.
struct Syntax {
    /// Words to colour as keywords.
    keywords: &'static [&'static str],
    /// Markers that comment out the rest of a line.
    line_comments: &'static [&'static str],
}

/// Shared keyword lists, so a language only names the ones it has.
const C_LIKE: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "dyn",
    "else",
    "enum",
    "export",
    "extends",
    "extern",
    "false",
    "final",
    "finally",
    "fn",
    "for",
    "from",
    "func",
    "function",
    "if",
    "impl",
    "import",
    "in",
    "interface",
    "is",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "new",
    "null",
    "of",
    "package",
    "private",
    "pub",
    "public",
    "return",
    "self",
    "static",
    "struct",
    "super",
    "switch",
    "this",
    "throw",
    "trait",
    "true",
    "try",
    "type",
    "typeof",
    "union",
    "unsafe",
    "use",
    "var",
    "void",
    "where",
    "while",
    "yield",
];
const PYTHON: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "self", "True", "try",
    "while", "with", "yield",
];
const SHELL: &[&str] = &[
    "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if", "in",
    "local", "return", "select", "then", "until", "while", "set", "unset", "source",
];
const TOML: &[&str] = &["true", "false"];
const YAML: &[&str] = &["true", "false", "null", "yes", "no", "on", "off"];
const MAKEFILE: &[&str] = &[
    "ifeq", "ifneq", "ifdef", "ifndef", "else", "endif", "include", "define", "endef", "export",
    "override", "unexport", "vpath", "error", "warning", "info", "shell",
];

/// The scanner's configuration for a language id from `language_of`.
///
/// @param language the id
/// @returns its syntax; an unknown id gets a plain scanner
fn syntax_of(language: &str) -> Syntax {
    match language {
        "rust" | "c" | "cpp" | "go" | "javascript" | "typescript" | "json" => Syntax {
            keywords: C_LIKE,
            line_comments: &["//"],
        },
        "python" => Syntax {
            keywords: PYTHON,
            line_comments: &["#"],
        },
        "shell" => Syntax {
            keywords: SHELL,
            line_comments: &["#"],
        },
        "toml" => Syntax {
            keywords: TOML,
            line_comments: &["#"],
        },
        "yaml" => Syntax {
            keywords: YAML,
            line_comments: &["#"],
        },
        "make" => Syntax {
            keywords: MAKEFILE,
            line_comments: &["#"],
        },
        "css" => Syntax {
            keywords: &[],
            line_comments: &["/*"],
        },
        "html" | "markdown" => Syntax {
            keywords: &[],
            line_comments: &["<!--"],
        },
        _ => Syntax {
            keywords: &[],
            line_comments: &[],
        },
    }
}

/// Whether a character can start an identifier.
///
/// @param ch the character
/// @returns true for a letter or underscore
fn ident_start(ch: char) -> bool {
    ch.is_alphabetic() || ch == '_'
}

/// Whether a character can continue an identifier.
///
/// @param ch the character
/// @returns true for a letter, digit, or underscore
fn ident_part(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Appends a coloured run to a job.
///
/// @param job the job being built
/// @param text the run's text
/// @param colour the run's colour
/// @param font the font to lay it out in
fn push(job: &mut LayoutJob, text: &str, colour: Color32, font: &FontId) {
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font.clone(),
            color: colour,
            ..TextFormat::default()
        },
    );
}

/// Colourises one line of source.
///
/// @param language language id from `language_of`
/// @param line the line, without its newline
/// @param font the monospace font to use
/// @param base the colour for everything unclassified
/// @returns a job ready for `Painter::layout_job`
pub fn highlight(language: &str, line: &str, font: &FontId, base: Color32) -> LayoutJob {
    let syntax = syntax_of(language);
    let mut job = LayoutJob::default();
    let chars: Vec<char> = line.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let rest = &chars[index..];

        // A comment takes the rest of the line.
        if syntax
            .line_comments
            .iter()
            .any(|marker| starts_with(rest, marker))
        {
            let comment: String = rest.iter().collect();
            push(&mut job, &comment, COMMENT, font);
            break;
        }

        let ch = rest[0];
        if ch == '"' || ch == '\'' || ch == '`' {
            let mut end = 1;
            while end < rest.len() {
                if rest[end] == '\\' {
                    end += 2;
                    continue;
                }
                if rest[end] == ch {
                    end += 1;
                    break;
                }
                end += 1;
            }
            let text: String = rest[..end.min(rest.len())].iter().collect();
            push(&mut job, &text, STRING, font);
            index += end.min(rest.len());
            continue;
        }

        if ch.is_ascii_digit() {
            let mut end = 1;
            while end < rest.len()
                && (rest[end].is_alphanumeric() || rest[end] == '.' || rest[end] == '_')
            {
                end += 1;
            }
            let text: String = rest[..end].iter().collect();
            push(&mut job, &text, NUMBER, font);
            index += end;
            continue;
        }

        if ident_start(ch) {
            let mut end = 1;
            while end < rest.len() && ident_part(rest[end]) {
                end += 1;
            }
            let word: String = rest[..end].iter().collect();
            let after = rest[end..]
                .iter()
                .position(|ch| !ch.is_whitespace())
                .map(|offset| rest[end + offset]);
            let colour = if syntax.keywords.contains(&word.as_str()) {
                KEYWORD
            } else if after == Some('(') || after == Some('!') {
                FUNCTION
            } else if word.chars().next().is_some_and(char::is_uppercase) {
                TYPE
            } else {
                base
            };
            push(&mut job, &word, colour, font);
            index += end;
            continue;
        }

        if "(){}[];,".contains(ch) {
            push(&mut job, &ch.to_string(), PUNCT, font);
        } else {
            push(&mut job, &ch.to_string(), base, font);
        }
        index += 1;
    }
    job
}

/// Whether a slice of characters begins with a marker.
///
/// @param chars the text from the current position
/// @param marker the marker to look for
/// @returns true when it matches
fn starts_with(chars: &[char], marker: &str) -> bool {
    let marker: Vec<char> = marker.chars().collect();
    chars.len() >= marker.len() && chars[..marker.len()] == marker[..]
}

/// What a patch line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    /// `diff --git`, `index`, `---`, `+++`: the file headers.
    File,
    /// `new file`, `deleted file`, mode and rename notes.
    Meta,
    /// `@@ … @@`: where the hunk starts.
    Hunk,
    /// An added line.
    Add,
    /// A removed line.
    Del,
    /// An unchanged line shown for context.
    Context,
}

/// One line of a patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// What the line is.
    pub kind: DiffKind,
    /// The line's text, including its leading marker.
    pub text: String,
}

/// Splits a patch into classified lines.
///
/// @param patch unified diff text
/// @returns the lines, in order
pub fn parse_patch(patch: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    for line in patch.lines() {
        let kind = if line.starts_with("diff --git")
            || line.starts_with("index ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
        {
            DiffKind::File
        } else if line.starts_with("new file")
            || line.starts_with("deleted file")
            || line.starts_with("old mode")
            || line.starts_with("new mode")
            || line.starts_with("similarity")
            || line.starts_with("rename ")
            || line.starts_with("\\ No newline")
        {
            DiffKind::Meta
        } else if line.starts_with("@@") {
            DiffKind::Hunk
        } else if line.starts_with('+') {
            DiffKind::Add
        } else if line.starts_with('-') {
            DiffKind::Del
        } else {
            DiffKind::Context
        };
        lines.push(DiffLine {
            kind,
            text: line.to_string(),
        });
    }
    lines
}

/// Counts what a patch changes.
///
/// @param patch unified diff text
/// @returns added lines, removed lines, and files touched
pub fn patch_stats(patch: &str) -> (usize, usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    let mut files = 0;
    for line in parse_patch(patch) {
        match line.kind {
            DiffKind::Add => added += 1,
            DiffKind::Del => removed += 1,
            DiffKind::File if line.text.starts_with("diff --git") => files += 1,
            _ => {}
        }
    }
    (added, removed, files)
}

/// The background tint for a patch line.
///
/// @param kind the line's kind
/// @returns the fill, or `None` for an untinted line
fn diff_fill(kind: DiffKind) -> Option<Color32> {
    match kind {
        DiffKind::Add => Some(theme::DIFF_ADD_BG),
        DiffKind::Del => Some(theme::DIFF_DEL_BG),
        DiffKind::Hunk => Some(theme::DIFF_HUNK_BG),
        _ => None,
    }
}

/// The text colour for a patch line.
///
/// @param kind the line's kind
/// @param size the font size, used to know whether the row is a header
/// @returns the colour
fn diff_text_colour(kind: DiffKind) -> Color32 {
    match kind {
        DiffKind::File => theme::DIM,
        DiffKind::Meta => theme::FAINT,
        DiffKind::Hunk => theme::BLUE,
        DiffKind::Add => theme::GREEN,
        DiffKind::Del => theme::RED,
        DiffKind::Context => theme::TEXT,
    }
}

/// Draws a patch into a scrolled region, only laying out the rows on screen.
///
/// @param ui the interface to draw into
/// @param patch unified diff text
/// @param font the monospace font
/// @param row_height the height of one row, from [`metrics`]
/// @param limit the most lines to draw
/// @returns the number of lines drawn, and whether the patch was cut
pub fn draw_patch(
    ui: &mut Ui,
    patch: &str,
    font: &FontId,
    row_height: f32,
    limit: usize,
) -> (usize, bool) {
    let lines = parse_patch(patch);
    let total = lines.len();
    let shown = total.min(limit);
    let widest = lines
        .iter()
        .take(shown)
        .map(|line| line.text.chars().count())
        .max()
        .unwrap_or(0);
    // One advance per character is exact enough for a tint that must cover the
    // text, and it costs nothing to compute.
    let advance = ui
        .painter()
        .layout_no_wrap(" ".to_string(), font.clone(), theme::TEXT)
        .size()
        .x;
    let content_width = (widest as f32 + 4.0) * advance;
    let viewport = ui.clip_rect();
    let left = viewport.left() + 6.0;
    let top = ui.cursor().top();

    ui.set_height(row_height * shown as f32);
    ui.set_width(content_width.max(viewport.width()));

    let first = ((viewport.top() - top) / row_height).floor().max(0.0) as usize;
    let last = (((viewport.bottom() - top) / row_height).ceil() as usize + 1).min(shown);
    let painter = ui.painter().clone();
    for (index, line) in lines[first..last].iter().enumerate() {
        let index = first + index;
        let row = Rect::from_min_size(
            egui::pos2(left, top + index as f32 * row_height),
            Vec2::new(content_width.max(viewport.width()), row_height),
        );
        if let Some(fill) = diff_fill(line.kind) {
            painter.rect_filled(row, CornerRadius::ZERO, fill);
        }
        let job = LayoutJob::simple(
            line.text.clone(),
            font.clone(),
            diff_text_colour(line.kind),
            f32::INFINITY,
        );
        let galley = painter.layout_job(job);
        painter.galley(egui::pos2(left + 2.0, row.top()), galley, theme::TEXT);
    }
    if total > shown {
        let row = Rect::from_min_size(
            egui::pos2(left, top + shown as f32 * row_height),
            Vec2::new(viewport.width(), row_height),
        );
        painter.rect_filled(row, 0.0, theme::ELEVATED);
        painter.text(
            egui::pos2(left + 8.0, row.top()),
            egui::Align2::LEFT_TOP,
            format!("… {} of {total} patch lines shown", shown),
            font.clone(),
            theme::DIM,
        );
    }
    (shown, total > shown)
}

/// Draws a patch as a compact summary: the file header, each hunk header, every
/// change, and the context lines that bracket them.
///
/// @param ui the interface to draw into
/// @param patch unified diff text
/// @param font the monospace font
/// @param row_height the height of one row
/// @param limit the most lines to draw
/// @returns nothing
pub fn draw_patch_compact(ui: &mut Ui, patch: &str, font: &FontId, row_height: f32, limit: usize) {
    let rows = compact_rows(&parse_patch(patch), limit);
    let viewport = ui.clip_rect();
    let left = viewport.left() + 6.0;
    let top = ui.cursor().top();
    ui.set_height(row_height * rows.len() as f32);
    let painter = ui.painter().clone();
    for (index, (kind, text)) in rows.iter().enumerate() {
        let y = top + index as f32 * row_height;
        if y + row_height < viewport.top() {
            continue;
        }
        if y > viewport.bottom() {
            break;
        }
        let row = Rect::from_min_size(egui::pos2(left, y), Vec2::new(viewport.width(), row_height));
        if let Some(fill) = diff_fill(*kind) {
            painter.rect_filled(row, CornerRadius::ZERO, fill);
        }
        let galley = painter.layout_no_wrap(text.clone(), font.clone(), diff_text_colour(*kind));
        painter.galley(egui::pos2(left + 2.0, y), galley, theme::TEXT);
    }
}

/// Chooses the lines a compact patch view shows.
///
/// @param lines the parsed patch
/// @param limit the most lines to return
/// @returns the lines to draw
fn compact_rows(lines: &[DiffLine], limit: usize) -> Vec<(DiffKind, String)> {
    /// Keeps the first and last line of a run of context, which is what brackets
    /// a change; everything between them is what a collapsed diff hides.
    fn flush(run: &mut Vec<(DiffKind, String)>, rows: &mut Vec<(DiffKind, String)>) {
        match run.len() {
            0 => {}
            1 => rows.push(run[0].clone()),
            count => {
                rows.push(run[0].clone());
                rows.push(run[count - 1].clone());
            }
        }
        run.clear();
    }

    let mut rows: Vec<(DiffKind, String)> = Vec::new();
    let mut run: Vec<(DiffKind, String)> = Vec::new();
    let mut header_seen = false;
    for line in lines {
        match line.kind {
            DiffKind::Context => run.push((line.kind, line.text.clone())),
            kind => {
                flush(&mut run, &mut rows);
                let header = matches!(kind, DiffKind::File | DiffKind::Meta);
                if !header || !header_seen {
                    header_seen |= header;
                    rows.push((kind, line.text.clone()));
                }
            }
        }
        if rows.len() >= limit {
            break;
        }
    }
    flush(&mut run, &mut rows);
    rows.truncate(limit);
    rows
}

/// Measures the monospace advance and row height for a font size.
///
/// @param ui the interface, for its font definitions
/// @param font the monospace font
/// @returns character advance and row height, both in points
pub fn metrics(ui: &Ui, font: &FontId) -> (f32, f32) {
    let galley = ui
        .painter()
        .layout_no_wrap("0123456789".to_string(), font.clone(), theme::TEXT);
    (galley.size().x / 10.0, galley.size().y.max(1.0))
}
