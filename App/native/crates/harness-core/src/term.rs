//! PTY-backed terminals, their emulated screen, and their command blocks.
//!
//! One [`Terminal`] is one shell process. Three threads serve it: a reader that
//! decodes the PTY's byte stream, a writer that serializes everything the app
//! sends, and a monitor that reaps the child and reports its exit status.
//!
//! Two models come out of the same bytes:
//!
//! - **A screen.** An `alacritty_terminal` emulator turns the stream into a
//!   cell grid, which is what the terminal pane draws and what a full-screen
//!   program (an editor, a pager) needs to work at all.
//! - **Blocks.** A scanner in front of the emulator recognizes the semantic
//!   markers our shell integration emits — prompt start, the command line,
//!   output start, command finished with a status — and assembles the command
//!   list a block UI shows. Each finished block keeps its own emulator, so its
//!   output can be scrolled and recoloured long after it left the screen.
//!
//! The scanner only ever *classifies* bytes; the emulator still receives
//! everything, escape sequences included, so splitting the stream never breaks
//! a program's output.
//!
//! The integration is ordinary shell scripting (`OSC 133` / `OSC 633`, the same
//! vocabulary other terminals use), written to `<dir>/` and pointed at through
//! the shell's own startup path. A shell we have no integration for still gets a
//! fully working screen; it simply grows no blocks, and the terminal says so
//! rather than pretending.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::thread;

use alacritty_terminal::event::{Event as VtEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors as VtColors;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
/// A terminal colour, as the emulator stores it.
///
/// Re-exported so a front-end can name the colour type it reads out of a
/// [`Palette`] or a [`SnapshotCell`] without depending on the emulator crate.
pub use alacritty_terminal::vte::ansi::Rgb;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};
use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};

use crate::event::{CoreEvent, EventSink, TerminalId};

/// Lines of scrollback kept for one block's captured output.
const BLOCK_SCROLLBACK_LINES: usize = 2_000;
/// Blocks kept per terminal; the oldest are dropped.
const MAX_BLOCKS: usize = 500;
/// Upper bound on a partially received escape sequence.
const MARKER_CARRY_LIMIT: usize = 8 * 1024;
/// Bytes per PTY read.
const READ_BUFFER: usize = 64 * 1024;

/// A terminal's grid size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenSize {
    /// Columns.
    pub cols: usize,
    /// Visible rows.
    pub rows: usize,
}

impl Dimensions for ScreenSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// The 16 ANSI colours plus the special slots, resolved to RGB.
///
/// The palette is the harness's own: the terminal is part of the product, not a
/// guest in it. A colour a program sets explicitly (through OSC) wins over this
/// table; a named or indexed colour resolves here. These are the light
/// variants — dark ink on the card's own white — because a terminal is not a
/// window onto another theme: it is the part of this one that runs programs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// The sixteen ANSI colours.
    pub ansi: [Rgb; 16],
    /// Default text.
    pub foreground: Rgb,
    /// Default background.
    pub background: Rgb,
    /// The cursor block.
    pub cursor: Rgb,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            ansi: [
                Rgb {
                    r: 0x24,
                    g: 0x29,
                    b: 0x2f,
                },
                Rgb {
                    r: 0xcf,
                    g: 0x22,
                    b: 0x2e,
                },
                Rgb {
                    r: 0x11,
                    g: 0x63,
                    b: 0x29,
                },
                Rgb {
                    r: 0x95,
                    g: 0x38,
                    b: 0x00,
                },
                Rgb {
                    r: 0x09,
                    g: 0x69,
                    b: 0xda,
                },
                Rgb {
                    r: 0x82,
                    g: 0x50,
                    b: 0xdf,
                },
                Rgb {
                    r: 0x1b,
                    g: 0x7c,
                    b: 0x83,
                },
                Rgb {
                    r: 0x6e,
                    g: 0x77,
                    b: 0x81,
                },
                Rgb {
                    r: 0x57,
                    g: 0x60,
                    b: 0x6a,
                },
                Rgb {
                    r: 0xa4,
                    g: 0x0e,
                    b: 0x26,
                },
                Rgb {
                    r: 0x1a,
                    g: 0x7f,
                    b: 0x37,
                },
                Rgb {
                    r: 0xbf,
                    g: 0x87,
                    b: 0x00,
                },
                Rgb {
                    r: 0x21,
                    g: 0x8b,
                    b: 0xff,
                },
                Rgb {
                    r: 0xa4,
                    g: 0x75,
                    b: 0xf9,
                },
                Rgb {
                    r: 0x31,
                    g: 0x92,
                    b: 0xaa,
                },
                Rgb {
                    r: 0x8c,
                    g: 0x95,
                    b: 0x9f,
                },
            ],
            foreground: Rgb {
                r: 0x1f,
                g: 0x23,
                b: 0x28,
            },
            background: Rgb {
                r: 0xff,
                g: 0xff,
                b: 0xff,
            },
            cursor: Rgb {
                r: 0x3b,
                g: 0x6f,
                b: 0xf5,
            },
        }
    }
}

impl Palette {
    /// Resolves one terminal colour to RGB.
    ///
    /// @param color the cell's colour
    /// @param colors the emulator's table of colours the program set explicitly
    /// @returns the colour to draw
    pub fn resolve(&self, color: Color, colors: &VtColors) -> Rgb {
        match color {
            Color::Spec(rgb) => rgb,
            Color::Named(named) => self.named(named, colors),
            Color::Indexed(index) => self.indexed(index, colors),
        }
    }

    /// Resolves a named colour.
    fn named(&self, named: NamedColor, colors: &VtColors) -> Rgb {
        if let Some(rgb) = colors[named] {
            return rgb;
        }
        let index = named as usize;
        if index < 16 {
            return self.ansi[index];
        }
        match named {
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
            // The dim ramp is the same sixteen hues; without a dim set of its
            // own the palette answers with the hue rather than the default text
            // colour, which is closer to what the program asked for.
            NamedColor::DimBlack => self.ansi[0],
            NamedColor::DimRed => self.ansi[1],
            NamedColor::DimGreen => self.ansi[2],
            NamedColor::DimYellow => self.ansi[3],
            NamedColor::DimBlue => self.ansi[4],
            NamedColor::DimMagenta => self.ansi[5],
            NamedColor::DimCyan => self.ansi[6],
            NamedColor::DimWhite => self.ansi[7],
            // Foreground, its bright and dim variants, and the unused slots.
            _ => self.foreground,
        }
    }

    /// Resolves an indexed colour through the cube and the grayscale ramp.
    fn indexed(&self, index: u8, colors: &VtColors) -> Rgb {
        if let Some(rgb) = colors[index as usize] {
            return rgb;
        }
        match index {
            0..=15 => self.ansi[index as usize],
            16..=231 => {
                let index = index - 16;
                let steps = [0u8, 95, 135, 175, 215, 255];
                Rgb {
                    r: steps[(index / 36 % 6) as usize],
                    g: steps[(index / 6 % 6) as usize],
                    b: steps[(index % 6) as usize],
                }
            }
            232..=255 => {
                let level = 8 + (index - 232) * 10;
                Rgb {
                    r: level,
                    g: level,
                    b: level,
                }
            }
        }
    }
}

/// One cell of a snapshot, ready to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotCell {
    /// The character.
    pub ch: char,
    /// Foreground, already resolved.
    pub fg: Rgb,
    /// Background, already resolved.
    pub bg: Rgb,
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Underlined (any variant).
    pub underline: bool,
    /// Whether the cursor is on this cell.
    pub cursor: bool,
}

impl SnapshotCell {
    /// Whether the cell is a blank with default colours.
    ///
    /// @returns true when drawing nothing would look the same
    pub fn is_blank(&self) -> bool {
        self.ch == ' ' && !self.underline && !self.cursor
    }
}

/// One row of a snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotLine {
    /// Cells, left to right, untrimmed.
    pub cells: Vec<SnapshotCell>,
}

impl SnapshotLine {
    /// The row's text without trailing blanks, for search, copy, and tests.
    ///
    /// @returns trimmed text
    pub fn text(&self) -> String {
        let text: String = self.cells.iter().map(|cell| cell.ch).collect();
        text.trim_end().to_string()
    }
}

/// Where the cursor is, and whether it is visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotCursor {
    /// Row within the visible grid.
    pub row: usize,
    /// Column.
    pub col: usize,
    /// Whether the program left the cursor visible.
    pub visible: bool,
}

/// A screen's visible content at one instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenSnapshot {
    /// Columns.
    pub cols: usize,
    /// Visible rows.
    pub rows: usize,
    /// Rows top to bottom.
    pub lines: Vec<SnapshotLine>,
    /// Cursor position.
    pub cursor: SnapshotCursor,
    /// Whether the alternate screen is active (a full-screen program owns it).
    pub alt_screen: bool,
    /// Scrollback offset; non-zero means the view is scrolled up.
    pub display_offset: usize,
    /// How many history lines exist above the visible grid.
    pub history: usize,
}

impl ScreenSnapshot {
    /// The whole snapshot as text, one line per row.
    ///
    /// @returns trimmed lines joined by newlines
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(SnapshotLine::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The snapshot's text from the top through the last non-blank row.
    ///
    /// @returns text without trailing blank rows
    pub fn trimmed_text(&self) -> String {
        let last = self
            .lines
            .iter()
            .rposition(|line| !line.text().is_empty())
            .map(|index| index + 1)
            .unwrap_or(0);
        self.lines[..last]
            .iter()
            .map(SnapshotLine::text)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Marks a screen dirty so the UI knows to redraw it, and forwards the
/// emulator's own writes back to the PTY.
#[derive(Clone)]
struct Proxy {
    dirty: Arc<AtomicBool>,
    /// Where answers to the shell's queries go (cursor reports, device answers).
    sink: Option<std_mpsc::Sender<Vec<u8>>>,
}

impl EventListener for Proxy {
    fn send_event(&self, event: VtEvent) {
        match event {
            VtEvent::Wakeup
            | VtEvent::MouseCursorDirty
            | VtEvent::CursorBlinkingChange
            | VtEvent::Bell => {
                self.dirty.store(true, Ordering::Relaxed);
            }
            VtEvent::PtyWrite(text) => {
                if let Some(sink) = &self.sink {
                    let _ = sink.send(text.into_bytes());
                }
            }
            _ => {}
        }
    }
}

/// One emulated screen: a grid, its parser, and a dirty flag.
pub struct Screen {
    term: FairMutex<Term<Proxy>>,
    parser: Mutex<Processor>,
    size: Mutex<ScreenSize>,
    dirty: Arc<AtomicBool>,
    palette: Palette,
}

impl Screen {
    /// Creates a screen of a given size.
    ///
    /// @param size grid size
    /// @param scrollback history lines to keep
    /// @param sink where the emulator's own PTY writes go, when it should talk back
    /// @param palette colour table
    /// @returns the screen
    pub fn new(
        size: ScreenSize,
        scrollback: usize,
        sink: Option<std_mpsc::Sender<Vec<u8>>>,
        palette: Palette,
    ) -> Self {
        let dirty = Arc::new(AtomicBool::new(true));
        let proxy = Proxy {
            dirty: dirty.clone(),
            sink,
        };
        let config = TermConfig {
            scrolling_history: scrollback,
            ..TermConfig::default()
        };
        let term = Term::new(config, &size, proxy);
        Self {
            term: FairMutex::new(term),
            parser: Mutex::new(Processor::new()),
            size: Mutex::new(size),
            dirty,
            palette,
        }
    }

    /// Feeds PTY bytes through the parser into the grid.
    ///
    /// @param bytes raw bytes as read from the PTY
    pub fn feed(&self, bytes: &[u8]) {
        let mut term = self.term.lock();
        let mut parser = self.parser.lock().expect("screen parser");
        parser.advance(&mut *term, bytes);
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Resizes the grid, reflowing content.
    ///
    /// @param size new size
    pub fn resize(&self, size: ScreenSize) {
        {
            let mut current = self.size.lock().expect("screen size");
            if *current == size {
                return;
            }
            *current = size;
        }
        self.term.lock().resize(size);
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// The size the grid currently has.
    ///
    /// @returns grid size
    pub fn size(&self) -> ScreenSize {
        *self.size.lock().expect("screen size")
    }

    /// Whether the screen changed since the last snapshot.
    ///
    /// @returns true when a redraw is warranted
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    /// Takes the dirty flag, marking the screen drawn.
    ///
    /// @returns whether it had been dirty
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    /// Scrolls the view through history.
    ///
    /// @param lines positive scrolls up into history, negative scrolls down
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Scrolls back to the live view, when the view is not already there.
    pub fn scroll_to_bottom(&self) {
        let mut term = self.term.lock();
        let offset = term.grid().display_offset();
        if offset > 0 {
            term.scroll_display(Scroll::Delta(-(offset as i32)));
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// The visible screen as text, for tests and copy.
    ///
    /// @returns trimmed visible lines joined by newlines
    pub fn text(&self) -> String {
        self.snapshot().text()
    }

    /// The full cell grid at this instant.
    ///
    /// @returns a snapshot the UI can draw without holding the emulator lock
    pub fn snapshot(&self) -> ScreenSnapshot {
        let term = self.term.lock();
        let content = term.renderable_content();
        let palette = self.palette;
        let size = *self.size.lock().expect("screen size");
        let cursor = content.cursor.point;

        let mut lines: Vec<SnapshotLine> = (0..size.rows)
            .map(|_| SnapshotLine {
                cells: Vec::with_capacity(size.cols),
            })
            .collect();
        for indexed in content.display_iter {
            let Point { line, column } = indexed.point;
            // The iterator covers the visible window, whose top line is
            // `-display_offset`; rows are numbered from the top of the window.
            let row = line.0 + content.display_offset as i32;
            if row < 0 || row as usize >= size.rows || column.0 >= size.cols {
                continue;
            }
            let cell = indexed.cell;
            let mut fg = palette.resolve(cell.fg, content.colors);
            let mut bg = palette.resolve(cell.bg, content.colors);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            lines[row as usize].cells.push(SnapshotCell {
                ch: cell.c,
                fg,
                bg,
                bold: cell.flags.contains(Flags::BOLD),
                italic: cell.flags.contains(Flags::ITALIC),
                underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                cursor: false,
            });
        }
        let padding = SnapshotCell {
            ch: ' ',
            fg: palette.foreground,
            bg: palette.background,
            bold: false,
            italic: false,
            underline: false,
            cursor: false,
        };
        for line in &mut lines {
            line.cells.resize(size.cols, padding);
        }

        let cursor_row = (cursor.line.0 + content.display_offset as i32).max(0) as usize;
        let visible = content.mode.contains(TermMode::SHOW_CURSOR);
        if visible && cursor_row < lines.len() && cursor.column.0 < size.cols {
            lines[cursor_row].cells[cursor.column.0].cursor = true;
        }

        ScreenSnapshot {
            cols: size.cols,
            rows: size.rows,
            lines,
            cursor: SnapshotCursor {
                row: cursor_row,
                col: cursor.column.0,
                visible,
            },
            alt_screen: content.mode.contains(TermMode::ALT_SCREEN),
            display_offset: content.display_offset,
            history: term.grid().history_size(),
        }
    }

    /// The whole buffer as text: scrollback first, then the visible grid.
    ///
    /// A snapshot covers the visible rows, which is what drawing needs and not
    /// what reading a command back needs: a build log lives mostly in history.
    /// Rows lose their trailing blanks, trailing blank rows are dropped, and the
    /// spacer cells of a wide character are skipped so the text reads as it was
    /// written.
    ///
    /// @returns text, oldest line first
    pub fn full_text(&self) -> String {
        let term = self.term.lock();
        let grid = term.grid();
        let columns = grid.columns();
        let history = grid.history_size();
        // The grid iterator advances before it yields, so a read from the oldest
        // history line starts one column above the first cell it should return.
        let start = Point::new(
            Line(-(history as i32) - 1),
            Column(columns.saturating_sub(1)),
        );
        let mut rows: Vec<String> = Vec::new();
        let mut row: Vec<char> = Vec::with_capacity(columns);
        let mut row_of_line: Option<i32> = None;
        for indexed in grid.iter_from(start) {
            let line = indexed.point.line.0;
            if row_of_line != Some(line) {
                if row_of_line.is_some() {
                    rows.push(row.iter().collect::<String>().trim_end().to_string());
                    row.clear();
                }
                row_of_line = Some(line);
            }
            // A wide character stores a spacer in the cell after it.
            if indexed.cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            row.push(indexed.cell.c);
        }
        if row_of_line.is_some() {
            rows.push(row.iter().collect::<String>().trim_end().to_string());
        }
        while rows.last().is_some_and(String::is_empty) {
            rows.pop();
        }
        rows.join("\n")
    }
}

/// One semantic marker found in the PTY stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    /// `OSC 133;A` — the shell is about to draw a prompt.
    PromptStart,
    /// `OSC 633;E;<command>` — the command line the user submitted.
    CommandLine(String),
    /// `OSC 133;C` — the command's output is starting.
    OutputStart,
    /// `OSC 133;D;<code>` — the command finished with this status.
    CommandFinished(i32),
    /// `OSC 7;file://host/<path>` — the shell's working directory.
    Cwd(PathBuf),
    /// `OSC 0;<title>` or `OSC 2;<title>` — the window title.
    Title(String),
}

/// One marker, with the byte range it occupied in the read it was found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// First byte of the sequence, relative to the read (`0` when it began in
    /// an earlier read).
    pub start: usize,
    /// One past the last byte of the sequence.
    pub end: usize,
    /// What it was.
    pub marker: Marker,
}

/// Extracts semantic markers from a byte stream, holding split sequences.
///
/// Every other escape sequence is left alone for the emulator. Sequences split
/// across reads are carried until they complete, and a runaway payload is
/// dropped rather than buffered forever.
#[derive(Debug, Default)]
pub struct MarkerScanner {
    carry: Vec<u8>,
}

impl MarkerScanner {
    /// Creates an empty scanner.
    ///
    /// @returns the scanner
    pub fn new() -> Self {
        Self::default()
    }

    /// Consumes one read and returns the markers it completed.
    ///
    /// Offsets are relative to `chunk`; a marker that began in an earlier read
    /// starts at zero, which is what tells the caller not to re-send the bytes
    /// before it.
    ///
    /// @param chunk bytes as read from the PTY
    /// @returns markers, in the order they appeared
    pub fn scan(&mut self, chunk: &[u8]) -> Vec<Found> {
        let base = self.carry.len();
        self.carry.extend_from_slice(chunk);
        let mut found = Vec::new();
        let mut cursor = 0usize;
        let mut consumed = 0usize;
        while cursor + 1 < self.carry.len() {
            if self.carry[cursor] != 0x1b || self.carry[cursor + 1] != b']' {
                cursor += 1;
                continue;
            }
            match find_terminator(&self.carry[cursor + 2..]) {
                Some((body_len, terminator_len)) => {
                    let end = cursor + 2 + body_len + terminator_len;
                    let body = &self.carry[cursor + 2..cursor + 2 + body_len];
                    if let Some(marker) = parse_marker(body) {
                        found.push(Found {
                            start: cursor.saturating_sub(base),
                            end: end.saturating_sub(base).min(chunk.len()),
                            marker,
                        });
                    }
                    cursor = end;
                    consumed = cursor;
                }
                None => break,
            }
        }
        if consumed > 0 {
            self.carry.drain(..consumed);
        }
        if self.carry.len() > MARKER_CARRY_LIMIT {
            // A payload that never terminates cannot be trusted; drop it so a
            // noisy program cannot grow the buffer without bound.
            let tail = self.carry.split_off(self.carry.len() - 2);
            self.carry = tail;
        }
        found
    }
}

/// Finds the end of an OSC payload.
///
/// @param rest bytes after the `ESC ]` introducer
/// @returns payload length and terminator length, or `None` when unfinished
fn find_terminator(rest: &[u8]) -> Option<(usize, usize)> {
    let mut index = 0;
    while index < rest.len() {
        match rest[index] {
            0x07 => return Some((index, 1)),
            0x1b if index + 1 < rest.len() && rest[index + 1] == b'\\' => return Some((index, 2)),
            // A bare escape at the end may still become a terminator.
            0x1b if index + 1 >= rest.len() => return None,
            _ => index += 1,
        }
    }
    None
}

/// Parses one OSC payload into a marker, when it is one we act on.
///
/// @param body payload bytes without the introducer or terminator
/// @returns the marker, or `None` for payloads the emulator should handle
fn parse_marker(body: &[u8]) -> Option<Marker> {
    let text = String::from_utf8_lossy(body);
    let mut parts = text.splitn(2, ';');
    let code = parts.next()?;
    let rest = parts.next().unwrap_or("");
    match code {
        "133" => match rest.split(';').next()? {
            "A" => Some(Marker::PromptStart),
            "C" => Some(Marker::OutputStart),
            "D" => {
                let code = rest
                    .split(';')
                    .nth(1)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
                Some(Marker::CommandFinished(code))
            }
            _ => None,
        },
        "633" => match rest.split(';').next()? {
            "E" => Some(Marker::CommandLine(decode_osc_text(
                rest.splitn(2, ';').nth(1).unwrap_or(""),
            ))),
            _ => None,
        },
        "7" => cwd_from_url(rest).map(Marker::Cwd),
        "0" | "2" => Some(Marker::Title(decode_osc_text(rest))),
        _ => None,
    }
}

/// Decodes the escaped form our integration emits for command text.
///
/// The `633;E` payload quotes non-printable characters as `\xHH` so the marker
/// survives any command line; everything else is taken literally.
///
/// @param raw payload after the marker's own separator
/// @returns the command as the shell saw it
fn decode_osc_text(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() && bytes[index + 1] == b'x' {
            let hex = std::str::from_utf8(&bytes[index + 2..index + 4]).ok();
            if let Some(value) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(value);
                index += 4;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Reads a `file://` URL from an `OSC 7` payload.
///
/// @param url the URL, possibly with a host and percent-escapes
/// @returns the local path, when the URL names one
fn cwd_from_url(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // `file://host/path`; a local shell leaves the host empty.
    let path = match rest.find('/') {
        Some(0) => rest,
        Some(index) => &rest[index..],
        None => return None,
    };
    let decoded = percent_decode(path);
    if decoded.is_empty() {
        return None;
    }
    Some(PathBuf::from(decoded))
}

/// Decodes `%XX` escapes in a path.
///
/// @param text the raw path
/// @returns the decoded path
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(value) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// How a block ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockExit {
    /// The command's exit status.
    pub code: i32,
    /// How long it ran, in milliseconds.
    pub duration_ms: u64,
}

/// One command and its captured output.
pub struct Block {
    /// The command line, as the shell reported it.
    pub command: String,
    /// Directory the command ran in, when the shell reported one.
    pub cwd: PathBuf,
    /// When the command started.
    started: std::time::Instant,
    /// How it ended, once it has.
    exit: Option<BlockExit>,
    /// The command's output, in its own emulator.
    output: Arc<Screen>,
}

impl Block {
    /// Whether the command is still running.
    ///
    /// @returns true until the shell reports a status
    pub fn is_running(&self) -> bool {
        self.exit.is_none()
    }

    /// How the command ended, if it has.
    ///
    /// @returns the exit status and duration
    pub fn exit(&self) -> Option<BlockExit> {
        self.exit
    }

    /// How long the command ran, or has been running.
    ///
    /// @returns milliseconds
    pub fn elapsed_ms(&self) -> u64 {
        match self.exit {
            Some(exit) => exit.duration_ms,
            None => self.started.elapsed().as_millis() as u64,
        }
    }

    /// Whether the command succeeded.
    ///
    /// @returns true for exit status 0
    pub fn succeeded(&self) -> bool {
        matches!(self.exit, Some(BlockExit { code: 0, .. }))
    }

    /// The command's output, for drawing.
    ///
    /// @returns the output screen
    pub fn output(&self) -> &Arc<Screen> {
        &self.output
    }

    /// The command's output as text.
    ///
    /// This reads the block's whole buffer rather than its visible rows: a block
    /// is read back long after it finished, and by then a long output is mostly
    /// scrollback.
    ///
    /// @returns text without trailing blank rows
    pub fn output_text(&self) -> String {
        self.output.full_text()
    }
}

/// A terminal's block list and the state that assembles it.
struct BlockLog {
    blocks: VecDeque<Block>,
    /// Index of the block currently receiving output.
    active: Option<usize>,
    /// Command line seen before its output started.
    pending_command: Option<String>,
    /// Working directory reported by the shell.
    cwd: PathBuf,
    /// Whether the shell sits at a prompt.
    at_prompt: bool,
    /// The terminal's title, when the shell set one.
    title: Option<String>,
    /// Whether the shell emits the markers the block model needs.
    integrated: bool,
}

/// How to start one terminal.
pub struct TerminalOptions {
    /// Shell to run.
    pub shell: PathBuf,
    /// Arguments for the shell, when the integration needs any.
    pub args: Vec<String>,
    /// Working directory.
    pub cwd: PathBuf,
    /// Grid size.
    pub size: ScreenSize,
    /// Extra environment for the child.
    pub env: Vec<(String, String)>,
    /// Scrollback lines for the live screen.
    pub scrollback: usize,
}

/// One terminal session: shell process, screen, blocks.
pub struct Terminal {
    /// Identity the events refer to.
    pub id: TerminalId,
    /// Working directory the shell was started in.
    pub cwd: PathBuf,
    /// The shell binary.
    pub shell: String,
    /// Cells the grid was sized to.
    size: Mutex<ScreenSize>,
    /// The live screen.
    screen: Arc<Screen>,
    /// The block list.
    log: Arc<Mutex<BlockLog>>,
    /// Everything written to the PTY goes through this, from one writer thread.
    input: std_mpsc::Sender<Vec<u8>>,
    /// The PTY master, kept for resizing.
    master: Mutex<Box<dyn MasterPty + Send>>,
    /// How to stop the child.
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// Whether the child is still running.
    alive: Arc<AtomicBool>,
    /// The palette the screens resolve colours through.
    palette: Palette,
}

impl Terminal {
    /// Spawns a shell in a new PTY and starts its threads.
    ///
    /// @param id identity for events about this terminal
    /// @param options shell, size, and environment
    /// @param events sink for update, completion, and exit events
    /// @returns the terminal handle
    /// @throws `Err` naming the failure when the PTY or the child cannot start
    pub fn spawn(
        id: TerminalId,
        options: TerminalOptions,
        events: EventSink,
    ) -> Result<Self, String> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: options.size.rows as u16,
                cols: options.size.cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| format!("cannot open a pty: {err}"))?;

        // The writer thread drains this, so keystrokes, pastes, submitted
        // commands, and the emulator's own answers cannot interleave.
        let (input, input_rx) = std_mpsc::channel::<Vec<u8>>();
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|err| format!("no pty writer: {err}"))?;
        thread::Builder::new()
            .name("term-writer".into())
            .spawn(move || {
                while let Ok(bytes) = input_rx.recv() {
                    if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                        break;
                    }
                }
            })
            .map_err(|err| format!("cannot start the pty writer thread: {err}"))?;

        let mut command = CommandBuilder::new(&options.shell);
        for arg in &options.args {
            command.arg(arg);
        }
        command.cwd(&options.cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        for (key, value) in &options.env {
            command.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|err| format!("cannot start {}: {err}", options.shell.display()))?;
        drop(pair.slave);
        let killer = child.clone_killer();

        let palette = Palette::default();
        let screen = Arc::new(Screen::new(
            options.size,
            options.scrollback,
            Some(input.clone()),
            palette,
        ));
        let log = Arc::new(Mutex::new(BlockLog {
            blocks: VecDeque::new(),
            active: None,
            pending_command: None,
            cwd: options.cwd.clone(),
            at_prompt: false,
            title: None,
            integrated: false,
        }));
        let alive = Arc::new(AtomicBool::new(true));

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| format!("no pty reader: {err}"))?;
        spawn_reader(
            id,
            reader,
            screen.clone(),
            log.clone(),
            alive.clone(),
            events.clone(),
        );
        spawn_monitor(id, child, alive.clone(), events);

        Ok(Self {
            id,
            cwd: options.cwd,
            shell: options.shell.to_string_lossy().into_owned(),
            size: Mutex::new(options.size),
            screen,
            log,
            input,
            master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
            alive,
            palette,
        })
    }

    /// The live screen.
    ///
    /// @returns the screen, for snapshots and scrolling
    pub fn screen(&self) -> &Arc<Screen> {
        &self.screen
    }

    /// The palette the terminal resolves colours through.
    ///
    /// @returns the palette
    pub fn palette(&self) -> Palette {
        self.palette
    }

    /// Whether the child is still running.
    ///
    /// @returns true while the shell is alive
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// The shell's working directory, as it last reported it.
    ///
    /// @returns the directory the next command will run in
    pub fn cwd(&self) -> PathBuf {
        self.log.lock().expect("block log").cwd.clone()
    }

    /// The terminal's title, when the shell set one.
    ///
    /// @returns the title, or `None`
    pub fn title(&self) -> Option<String> {
        self.log.lock().expect("block log").title.clone()
    }

    /// Whether the shell emits block markers.
    ///
    /// @returns true once integration markers have been seen
    pub fn is_integrated(&self) -> bool {
        self.log.lock().expect("block log").integrated
    }

    /// Whether the shell is waiting at a prompt.
    ///
    /// @returns true between a command finishing and the next one starting
    pub fn at_prompt(&self) -> bool {
        self.log.lock().expect("block log").at_prompt
    }

    /// Number of blocks observed.
    ///
    /// @returns block count
    pub fn block_count(&self) -> usize {
        self.log.lock().expect("block log").blocks.len()
    }

    /// Whether a command is still running.
    ///
    /// @returns true while a block is open for output
    pub fn blocks_running(&self) -> bool {
        self.log.lock().expect("block log").active.is_some()
    }

    /// The command line the shell is running now, or the last one it ran.
    ///
    /// Unlike [`Terminal::recent_blocks`] this clones no output: it exists for a
    /// panel that watches the shell every frame.
    ///
    /// @returns the command, when one has been seen
    pub fn current_command(&self) -> Option<String> {
        let log = self.log.lock().expect("block log");
        if let Some(block) = log.active.and_then(|index| log.blocks.get(index)) {
            return Some(block.command.clone());
        }
        log.blocks
            .back()
            .map(|block| block.command.clone())
            .or_else(|| log.pending_command.clone())
    }

    /// Runs a closure over the blocks, newest last.
    ///
    /// @param visit called with each block in order
    pub fn for_each_block(&self, mut visit: impl FnMut(&Block)) {
        let log = self.log.lock().expect("block log");
        for block in &log.blocks {
            visit(block);
        }
    }

    /// The most recent blocks, newest first.
    ///
    /// @param limit how many to return
    /// @returns block summaries: command, exit status, duration, output text
    pub fn recent_blocks(&self, limit: usize) -> Vec<BlockSummary> {
        let log = self.log.lock().expect("block log");
        log.blocks
            .iter()
            .rev()
            .take(limit)
            .map(|block| BlockSummary {
                command: block.command.clone(),
                cwd: block.cwd.clone(),
                exit_code: block.exit.map(|exit| exit.code),
                duration_ms: block.elapsed_ms(),
                output: block.output_text(),
            })
            .collect()
    }

    /// Sends raw input to the shell.
    ///
    /// @param bytes keystrokes, paste text, or control sequences
    pub fn send(&self, bytes: impl Into<Vec<u8>>) {
        let _ = self.input.send(bytes.into());
    }

    /// Sends a line to the shell as if it were typed and submitted.
    ///
    /// @param command the command line
    pub fn run(&self, command: &str) {
        let mut bytes = command.as_bytes().to_vec();
        bytes.push(b'\r');
        self.send(bytes);
    }

    /// Resizes the grid and the PTY.
    ///
    /// @param size new size
    pub fn resize(&self, size: ScreenSize) {
        {
            let mut current = self.size.lock().expect("terminal size");
            if *current == size {
                return;
            }
            *current = size;
        }
        self.screen.resize(size);
        if let Ok(master) = self.master.lock() {
            let _ = master.resize(PtySize {
                rows: size.rows as u16,
                cols: size.cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        let log = self.log.lock().expect("block log");
        for block in &log.blocks {
            block.output.resize(size);
        }
    }

    /// The terminal's current size.
    ///
    /// @returns grid size
    pub fn size(&self) -> ScreenSize {
        *self.size.lock().expect("terminal size")
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Ok(mut killer) = self.killer.lock() {
            let _ = killer.kill();
        }
    }
}

/// One command's summary, for panes that do not need the emulator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockSummary {
    /// The command line.
    pub command: String,
    /// Directory it ran in.
    pub cwd: PathBuf,
    /// Exit status, absent while it runs.
    pub exit_code: Option<i32>,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Captured output text.
    pub output: String,
}

/// Reads the PTY until it closes, feeding the screen and the block log.
fn spawn_reader(
    id: TerminalId,
    mut reader: Box<dyn Read + Send>,
    screen: Arc<Screen>,
    log: Arc<Mutex<BlockLog>>,
    alive: Arc<AtomicBool>,
    events: EventSink,
) {
    let _ = thread::Builder::new()
        .name("term-reader".into())
        .spawn(move || {
            let mut scanner = MarkerScanner::new();
            let mut buffer = vec![0u8; READ_BUFFER];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        let chunk = &buffer[..count];
                        pump(id, chunk, &mut scanner, &screen, &log, &events);
                        let _ = events.send(CoreEvent::TerminalUpdated(id));
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            alive.store(false, Ordering::Relaxed);
            let _ = events.send(CoreEvent::TerminalUpdated(id));
        });
}

/// Reaps the child and reports its exit.
fn spawn_monitor(
    id: TerminalId,
    mut child: Box<dyn Child + Send + Sync>,
    alive: Arc<AtomicBool>,
    events: EventSink,
) {
    let _ = thread::Builder::new()
        .name("term-monitor".into())
        .spawn(move || {
            let code = child.wait().ok().map(|status| status.exit_code() as i32);
            alive.store(false, Ordering::Relaxed);
            let _ = events.send(CoreEvent::TerminalExited { id, code });
        });
}

/// Feeds one read through the whole model: screen, markers, block output.
///
/// The live screen receives every byte, escape sequences included, so the
/// emulator's parsing is never split. Block output receives only the ranges
/// *between* markers, so a block shows its command's output and not the
/// prompt or the next command's line.
fn pump(
    id: TerminalId,
    chunk: &[u8],
    scanner: &mut MarkerScanner,
    screen: &Arc<Screen>,
    log: &Arc<Mutex<BlockLog>>,
    events: &EventSink,
) {
    screen.feed(chunk);
    let found = scanner.scan(chunk);
    if found.is_empty() {
        feed_active_block(log, chunk);
        return;
    }
    let mut cursor = 0usize;
    for entry in found {
        let start = entry.start.min(chunk.len());
        if start > cursor {
            feed_active_block(log, &chunk[cursor..start]);
        }
        apply_marker(id, &entry.marker, log, screen, events);
        cursor = entry.end.max(start).min(chunk.len());
    }
    if cursor < chunk.len() {
        feed_active_block(log, &chunk[cursor..]);
    }
}

/// Sends bytes to the active block's screen, when a command is running.
fn feed_active_block(log: &Arc<Mutex<BlockLog>>, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let log = log.lock().expect("block log");
    if let Some(block) = log.active.and_then(|index| log.blocks.get(index)) {
        block.output.feed(bytes);
    }
}

/// Applies one marker to the block log.
fn apply_marker(
    id: TerminalId,
    marker: &Marker,
    log: &Arc<Mutex<BlockLog>>,
    screen: &Arc<Screen>,
    events: &EventSink,
) {
    let mut finished: Option<(String, BlockExit)> = None;
    {
        let mut log = log.lock().expect("block log");
        log.integrated = true;
        match marker {
            Marker::PromptStart => {
                // A prompt means whatever ran before it is over, even when the
                // status marker never arrived.
                finished = close_active(&mut log);
                log.at_prompt = true;
                log.pending_command = None;
            }
            Marker::CommandLine(command) => {
                log.pending_command = Some(command.clone());
            }
            Marker::OutputStart => {
                log.at_prompt = false;
                if let Some(done) = close_active(&mut log) {
                    finished = Some(done);
                }
                let command = log.pending_command.take().unwrap_or_default();
                let cwd = log.cwd.clone();
                let size = screen.size();
                log.blocks.push_back(Block {
                    command,
                    cwd,
                    started: std::time::Instant::now(),
                    exit: None,
                    output: Arc::new(Screen::new(
                        size,
                        BLOCK_SCROLLBACK_LINES,
                        None,
                        Palette::default(),
                    )),
                });
                log.active = Some(log.blocks.len() - 1);
                while log.blocks.len() > MAX_BLOCKS {
                    log.blocks.pop_front();
                    log.active = log.active.map(|index| index.saturating_sub(1));
                }
            }
            Marker::CommandFinished(code) => {
                if let Some(index) = log.active.take() {
                    if let Some(block) = log.blocks.get_mut(index) {
                        let exit = BlockExit {
                            code: *code,
                            duration_ms: block.elapsed_ms(),
                        };
                        block.exit = Some(exit);
                        finished = Some((block.command.clone(), exit));
                    }
                }
            }
            Marker::Cwd(path) => {
                log.cwd = path.clone();
            }
            Marker::Title(title) => {
                log.title = Some(title.clone());
            }
        }
    }
    if let Some((command, exit)) = finished {
        let _ = events.send(CoreEvent::BlockCompleted {
            id,
            exit_code: exit.code,
            command,
        });
    }
}

/// Marks the active block finished, if one is running.
///
/// @param log the block log, locked
/// @returns the finished command and its status
fn close_active(log: &mut BlockLog) -> Option<(String, BlockExit)> {
    let index = log.active.take()?;
    let block = log.blocks.get_mut(index)?;
    if block.exit.is_some() {
        return None;
    }
    let exit = BlockExit {
        code: 0,
        duration_ms: block.elapsed_ms(),
    };
    block.exit = Some(exit);
    Some((block.command.clone(), exit))
}

/// How to activate a shell's integration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellIntegration {
    /// Environment variables the child needs.
    pub env: Vec<(String, String)>,
    /// Extra arguments for the shell.
    pub args: Vec<String>,
    /// The file holding the snippet, for diagnostics.
    pub script: PathBuf,
}

/// Writes the integration snippet for a shell and returns how to use it.
///
/// @param dir directory to write under, e.g. `<workspace>/.harness/shell`
/// @param shell_name the shell binary's name
/// @returns activation instructions, or `None` for a shell we do not integrate
/// @throws `Err` when the snippet cannot be written
pub fn write_shell_integration(
    dir: &Path,
    shell_name: &str,
) -> Result<Option<ShellIntegration>, String> {
    let (name, snippet) = match shell_name {
        "zsh" => ("zsh", ZSH_INTEGRATION),
        "bash" => ("bash", BASH_INTEGRATION),
        _ => return Ok(None),
    };
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    // zsh only reads `.zshrc` from the directory it is pointed at, so the name is
    // fixed by the shell rather than by us.
    let file = match name {
        "zsh" => ".zshrc",
        _ => "bashrc",
    };
    let script = dir.join(file);
    std::fs::write(&script, snippet)
        .map_err(|err| format!("cannot write {}: {err}", script.display()))?;
    let dir_path = dir.to_string_lossy().into_owned();
    Ok(Some(match name {
        "zsh" => ShellIntegration {
            // zsh reads `.zshrc` from ZDOTDIR; the snippet sources the user's own
            // file and restores ZDOTDIR, so history still lands in $HOME.
            env: vec![
                ("ZDOTDIR".to_string(), dir_path),
                ("HARNESS_ZDOTDIR".to_string(), home_dir()),
            ],
            args: Vec::new(),
            script,
        },
        _ => ShellIntegration {
            env: vec![(
                "HARNESS_BASHRC".to_string(),
                PathBuf::from(home_dir())
                    .join(".bashrc")
                    .to_string_lossy()
                    .into_owned(),
            )],
            args: vec![
                "--rcfile".to_string(),
                script.to_string_lossy().into_owned(),
            ],
            script,
        },
    }))
}

/// The user's home directory as a string, or an empty string when unknown.
fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// The shell commands the harness gets the markers from, kept ASCII-only so the
/// bytes survive any locale.
const ZSH_INTEGRATION: &str = r#"# AI Harness shell integration: prompt and command markers for blocks.
# Sourced as .zshrc; the user's own .zshrc runs first and ZDOTDIR is restored
# so history and later sources still resolve to $HOME.
if [ -n "$HARNESS_ZDOTDIR" ] && [ -f "$HARNESS_ZDOTDIR/.zshrc" ]; then
  ZDOTDIR="$HARNESS_ZDOTDIR"
  . "$HARNESS_ZDOTDIR/.zshrc"
fi
export ZDOTDIR="${HARNESS_ZDOTDIR:-$HOME}"

_harness_preexec() {
  printf '\033]633;E;%s\007' "$1"
  printf '\033]133;C\007'
}

_harness_precmd() {
  printf '\033]133;D;%s\007' "$?"
  printf '\033]133;A\007'
  printf '\033]7;file://%s%s\007' "localhost" "$PWD"
}

autoload -Uz add-zsh-hook 2>/dev/null
if type add-zsh-hook >/dev/null 2>&1; then
  add-zsh-hook preexec _harness_preexec
  add-zsh-hook precmd _harness_precmd
fi
printf '\033]133;A\007'
"#;

/// bash integration: the same markers, through `PROMPT_COMMAND` and `DEBUG`.
const BASH_INTEGRATION: &str = r#"# AI Harness shell integration: prompt and command markers for blocks.
# Sourced through `bash --rcfile`; the user's own .bashrc runs first.
if [ -n "$HARNESS_BASHRC" ] && [ -f "$HARNESS_BASHRC" ]; then
  . "$HARNESS_BASHRC"
fi

_harness_preexec_last=

_harness_preexec() {
  [ -n "$COMP_LINE" ] && return
  [ "$BASH_COMMAND" = "$PROMPT_COMMAND" ] && return
  [ "$_harness_preexec_last" = "$BASH_COMMAND" ] && return
  _harness_preexec_last="$BASH_COMMAND"
  printf '\033]633;E;%s\007' "$BASH_COMMAND"
  printf '\033]133;C\007'
}

_harness_precmd() {
  printf '\033]133;D;%s\007' "$?"
  _harness_preexec_last=
  printf '\033]133;A\007'
  printf '\033]7;file://%s%s\007' "localhost" "$PWD"
}

trap '_harness_preexec' DEBUG
PROMPT_COMMAND="_harness_precmd${PROMPT_COMMAND:+; $PROMPT_COMMAND}"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// A palette for assertions.
    fn palette() -> Palette {
        Palette::default()
    }

    #[test]
    fn the_scanner_extracts_every_marker_kind() {
        let mut scanner = MarkerScanner::new();
        let stream = b"\x1b]133;A\x07\x1b]633;E;cargo test\x07\x1b]133;C\x07out\x1b]133;D;101\x07\x1b]7;file://host/tmp/dir\x07\x1b]2;my title\x07";
        let markers: Vec<Marker> = scanner
            .scan(stream)
            .into_iter()
            .map(|found| found.marker)
            .collect();
        assert_eq!(
            markers,
            vec![
                Marker::PromptStart,
                Marker::CommandLine("cargo test".to_string()),
                Marker::OutputStart,
                Marker::CommandFinished(101),
                Marker::Cwd(PathBuf::from("/tmp/dir")),
                Marker::Title("my title".to_string()),
            ]
        );
    }

    #[test]
    fn markers_report_the_byte_ranges_they_occupied() {
        let mut scanner = MarkerScanner::new();
        let read: &[u8] = b"before\x1b]133;D;0\x07after";
        let found = scanner.scan(read);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].start, 6, "the range starts at the escape");
        assert_eq!(
            &read[found[0].start..found[0].end],
            b"\x1b]133;D;0\x07".as_slice()
        );
    }

    #[test]
    fn the_scanner_survives_markers_split_across_reads() {
        let mut scanner = MarkerScanner::new();
        assert!(
            scanner.scan(b"\x1b]133").is_empty(),
            "an incomplete marker yields nothing"
        );
        assert!(scanner.scan(b";A").is_empty(), "still incomplete");
        let found = scanner.scan(b"\x07");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].marker, Marker::PromptStart);
        assert_eq!(
            found[0].start, 0,
            "a marker begun earlier starts at this read's first byte"
        );

        // The escape itself split from its introducer is the other half of the
        // same hazard.
        let mut scanner = MarkerScanner::new();
        assert!(scanner.scan(b"\x1b").is_empty());
        let found = scanner.scan(b"]133;D;3\x07rest");
        assert_eq!(found[0].marker, Marker::CommandFinished(3));
        assert_eq!(found[0].end, 9, "the range ends after the terminator");
    }

    #[test]
    fn the_scanner_handles_string_terminated_markers_and_escapes() {
        let mut scanner = MarkerScanner::new();
        let found = scanner.scan(b"\x1b]633;E;echo \\x41\x1b\\");
        assert_eq!(
            found[0].marker,
            Marker::CommandLine("echo A".to_string()),
            "ST-terminated, \\xHH decoded"
        );
    }

    #[test]
    fn the_scanner_leaves_other_sequences_for_the_emulator() {
        let mut scanner = MarkerScanner::new();
        assert!(scanner
            .scan(b"\x1b]8;;https://example.com\x07link\x1b]8;;\x07")
            .is_empty());
        assert!(scanner.scan(b"\x1b[31mred\x1b[0m").is_empty());
    }

    #[test]
    fn a_runaway_payload_does_not_grow_the_carry_buffer() {
        let mut scanner = MarkerScanner::new();
        let mut payload = b"\x1b]0;".to_vec();
        payload.extend(std::iter::repeat(b'x').take(MARKER_CARRY_LIMIT * 2));
        assert!(scanner.scan(&payload).is_empty());
        assert!(
            scanner.carry.len() <= 2,
            "the unterminated payload is dropped"
        );
    }

    #[test]
    fn a_screen_renders_text_attributes_and_colors() {
        let screen = Screen::new(ScreenSize { cols: 20, rows: 3 }, 100, None, palette());
        screen.feed(b"\x1b[1;32mgreen\x1b[0m plain");
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.lines[0].text(), "green plain");

        let first = snapshot.lines[0].cells[0];
        assert_eq!(first.ch, 'g');
        assert!(first.bold, "the SGR bold flag reaches the snapshot");
        assert_eq!(
            first.fg,
            palette().ansi[2],
            "SGR 32 resolves through the palette"
        );
        let space = snapshot.lines[0].cells[5];
        assert_eq!(space.ch, ' ', "the space before 'plain'");
        assert!(!space.bold, "the reset applies");
        assert_eq!(space.fg, palette().foreground);
    }

    #[test]
    fn a_screens_full_text_reads_the_scrollback_the_snapshot_cannot_show() {
        let screen = Screen::new(ScreenSize { cols: 12, rows: 3 }, 100, None, palette());
        screen.feed(b"first\r\nsecond\r\nthird\r\nfourth\r\nfifth");

        let snapshot = screen.snapshot();
        assert!(
            !snapshot.text().contains("first"),
            "the snapshot is the visible rows only"
        );
        assert!(snapshot.history >= 2);

        let full = screen.full_text();
        assert_eq!(full, "first\nsecond\nthird\nfourth\nfifth");
        assert_eq!(
            screen.full_text().lines().count(),
            5,
            "trailing blank rows are dropped"
        );
    }

    #[test]
    fn a_screen_reports_its_cursor_and_can_be_scrolled() {
        let screen = Screen::new(ScreenSize { cols: 10, rows: 2 }, 100, None, palette());
        screen.feed(b"first\r\nsecond\r\nthird");
        let snapshot = screen.snapshot();
        assert!(
            snapshot.history >= 1,
            "content beyond the visible rows becomes history"
        );
        assert_eq!(
            snapshot.lines[1].text(),
            "third",
            "the newest line is at the bottom"
        );
        assert_eq!(snapshot.cursor.row, 1);
        assert_eq!(snapshot.cursor.col, 5, "after 'third'");
        assert!(
            snapshot.lines[1].cells[5].cursor,
            "the cursor cell is marked"
        );

        screen.scroll(1);
        assert_eq!(screen.snapshot().display_offset, 1);
        screen.scroll_to_bottom();
        assert_eq!(screen.snapshot().display_offset, 0);
    }

    /// A block log for reducer tests.
    fn test_log(cwd: &str) -> Arc<Mutex<BlockLog>> {
        Arc::new(Mutex::new(BlockLog {
            blocks: VecDeque::new(),
            active: None,
            pending_command: None,
            cwd: PathBuf::from(cwd),
            at_prompt: false,
            title: None,
            integrated: false,
        }))
    }

    #[test]
    fn blocks_are_assembled_from_marker_sequences() {
        let log = test_log("/tmp");
        let screen = Arc::new(Screen::new(
            ScreenSize { cols: 20, rows: 4 },
            10,
            None,
            palette(),
        ));
        let (tx, rx) = crossbeam_channel::unbounded();
        let id = TerminalId(7);

        for marker in [
            Marker::PromptStart,
            Marker::CommandLine("ls -la".to_string()),
            Marker::OutputStart,
            Marker::CommandFinished(2),
        ] {
            apply_marker(id, &marker, &log, &screen, &tx);
        }

        let log_guard = log.lock().unwrap();
        assert_eq!(log_guard.blocks.len(), 1);
        let block = &log_guard.blocks[0];
        assert_eq!(block.command, "ls -la");
        assert_eq!(block.cwd, PathBuf::from("/tmp"));
        assert_eq!(block.exit().map(|exit| exit.code), Some(2));
        assert!(!block.succeeded());
        assert!(!log_guard.at_prompt, "the status marker closed the command");
        drop(log_guard);

        match rx.try_recv() {
            Ok(CoreEvent::BlockCompleted {
                id: got,
                exit_code,
                command,
            }) => {
                assert_eq!(got, id);
                assert_eq!(exit_code, 2);
                assert_eq!(command, "ls -la");
            }
            other => panic!("expected a block completion, got {other:?}"),
        }
    }

    #[test]
    fn a_command_that_never_reports_its_status_is_closed_by_the_next_prompt() {
        let log = test_log("/tmp");
        let screen = Arc::new(Screen::new(
            ScreenSize { cols: 20, rows: 4 },
            10,
            None,
            palette(),
        ));
        let (tx, _rx) = crossbeam_channel::unbounded();
        let id = TerminalId(1);
        for marker in [
            Marker::CommandLine("sleep 1".into()),
            Marker::OutputStart,
            Marker::PromptStart,
        ] {
            apply_marker(id, &marker, &log, &screen, &tx);
        }
        let log = log.lock().unwrap();
        assert_eq!(log.blocks.len(), 1);
        assert!(
            log.blocks[0].exit().is_some(),
            "the prompt closed the block"
        );
        assert!(log.at_prompt);
    }

    #[test]
    fn only_the_bytes_between_markers_reach_a_block() {
        let log = test_log("/tmp");
        let screen = Arc::new(Screen::new(
            ScreenSize { cols: 40, rows: 6 },
            10,
            None,
            palette(),
        ));
        let (tx, _rx) = crossbeam_channel::unbounded();
        let id = TerminalId(1);
        let mut scanner = MarkerScanner::new();

        // A prompt, a submitted command, its output, and the status: one read.
        let read = b"\x1b]133;A\x07$ \x1b]633;E;echo hi\x07\x1b]133;C\x07hi\r\n\x1b]133;D;0\x07";
        pump(id, read, &mut scanner, &screen, &log, &tx);

        let log = log.lock().unwrap();
        assert_eq!(log.blocks.len(), 1);
        let block = &log.blocks[0];
        assert_eq!(block.command, "echo hi");
        assert_eq!(
            block.output_text(),
            "hi",
            "the prompt and the markers stay out of the block"
        );
        assert_eq!(block.exit().map(|exit| exit.code), Some(0));
        drop(log);

        // The live screen sees everything, so the emulator's own state is intact.
        assert!(
            screen.text().contains("$"),
            "the prompt is on the live screen"
        );
    }

    #[test]
    fn indexed_colors_resolve_through_the_cube_and_the_ramp() {
        let palette = Palette::default();
        let colors = VtColors::default();
        assert_eq!(palette.indexed(1, &colors), palette.ansi[1]);
        assert_eq!(
            palette.indexed(196, &colors),
            Rgb { r: 255, g: 0, b: 0 },
            "196 is the cube's red"
        );
        assert_eq!(
            palette.indexed(232, &colors),
            Rgb { r: 8, g: 8, b: 8 },
            "232 is the first gray step"
        );
    }

    #[test]
    fn integration_is_written_for_the_shells_we_support() {
        let dir = std::env::temp_dir().join(format!("harness-shell-{}", std::process::id()));
        let zsh = write_shell_integration(&dir, "zsh")
            .expect("zsh integration")
            .expect("supported");
        assert!(zsh.script.is_file());
        assert!(zsh.env.iter().any(|(key, _)| key == "ZDOTDIR"));
        assert!(zsh.args.is_empty());

        let bash = write_shell_integration(&dir, "bash")
            .expect("bash integration")
            .expect("supported");
        assert_eq!(bash.args.first().map(String::as_str), Some("--rcfile"));

        assert!(
            write_shell_integration(&dir, "fish")
                .expect("no integration is not an error")
                .is_none(),
            "an unsupported shell runs plainly instead of failing"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
