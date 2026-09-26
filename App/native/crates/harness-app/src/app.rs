//! The application shell: window chrome, keyboard, and the pane router.
//!
//! The layout is the reference client's: gray chrome with a task sidebar on the
//! left, a white card floating in the middle that holds the conversation and its
//! composer, and a second card on the right for what the engine is doing. The
//! file tree rides along as an optional column when it is asked for, since a
//! harness without a way to browse the tree would be a worse harness.
//!
//! Two rules shape this file. First, the shell is a function of state plus
//! events: [`HarnessState::pump`] has already drained every background channel by
//! the time anything here runs, so a pane never waits on I/O. Second, keys are
//! consumed deliberately, in one place, before any pane draws: a shortcut that
//! reaches the terminal by accident is a shortcut that typed a control character
//! into someone's shell.

use std::time::Duration;

use eframe::egui::{self, Frame, Key, Modifiers, RichText, Ui};

use crate::state::{FocusRequest, HarnessState, Palette, PaletteMode, Pane};
use crate::theme;
use crate::{dialogs, palette, panes, shell};

/// A one-frame screenshot request: draw for a moment, write the frame, exit.
///
/// The interface is verified by looking at it, and looking at it should not
/// require a person at the keyboard — this is what lets a test capture the
/// window it just opened.
pub struct Shot {
    /// Where to write the frame, as a binary PPM.
    pub path: std::path::PathBuf,
    /// How long to let the interface settle before capturing.
    pub delay: Duration,
}

/// The application.
pub struct HarnessApp {
    /// Everything the panes read and write.
    pub state: HarnessState,
    /// The prompt the caller opened the harness on, sent once the engine is up.
    ask: Option<String>,
    /// The pending screenshot, when one was asked for.
    shot: Option<Shot>,
    /// When the window opened, so the delay is measured from real work.
    opened: std::time::Instant,
    /// Whether the capture has been requested from the renderer.
    asked: bool,
    /// Whether the launch has stopped asking to be brought to the front.
    fronted: bool,
    /// The workspace the window title was last written for.
    titled: std::path::PathBuf,
}

impl HarnessApp {
    /// Builds the application for a workspace.
    ///
    /// @param cc the creation context, used to install the theme
    /// @param root workspace root
    /// @param dsh_home explicit harness home, when the caller named one
    /// @param ask a prompt to open on, when the caller named one
    /// @param shot a screenshot to take and exit on, for tests
    /// @returns the application
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        root: std::path::PathBuf,
        dsh_home: Option<std::path::PathBuf>,
        ask: Option<String>,
        shot: Option<Shot>,
    ) -> Self {
        let state = HarnessState::new(root, dsh_home);
        // The system faces are installed before the style is applied, so the
        // first frame is measured against the fonts that will actually draw it.
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(
            &cc.egui_ctx,
            state.config.font_size,
            state.config.terminal_font_size,
        );
        let titled = state.root.clone();
        Self {
            state,
            ask,
            shot,
            opened: std::time::Instant::now(),
            asked: false,
            fronted: false,
            titled,
        }
    }

    /// Drains the background channels and decides when to draw again.
    ///
    /// Called before every pass, including while the window is hidden, so a
    /// command that finishes in the background is in the transcript by the time
    /// anyone looks.
    fn logic(&mut self) {
        self.state.pump();
        // A prompt named on the command line goes in as soon as the interface
        // has pumped once. `send_prompt` waits for the engine's session itself,
        // so this does not care whether the engine is up yet.
        if let Some(ask) = self.ask.take() {
            self.state.send_prompt(&ask);
        }
    }

    /// Handles the keyboard, consuming what it uses.
    ///
    /// @param ctx the egui context
    fn keys(&mut self, ctx: &egui::Context) {
        let command = Modifiers::COMMAND;
        let command_shift = Modifiers {
            command: true,
            shift: true,
            ..Modifiers::default()
        };

        if ctx.input_mut(|input| input.consume_key(command_shift, Key::F)) {
            self.open_palette(PaletteMode::Contents);
        }
        if ctx.input_mut(|input| input.consume_key(command, Key::K))
            || ctx.input_mut(|input| input.consume_key(command, Key::P))
        {
            self.open_palette(PaletteMode::Commands);
        }
        if ctx.input_mut(|input| input.consume_key(command, Key::B)) {
            self.state.tree_open = !self.state.tree_open;
            self.state.save_config();
        }
        if ctx.input_mut(|input| input.consume_key(command, Key::J)) {
            self.state.agent_open = !self.state.agent_open;
            self.state.save_config();
        }
        if ctx.input_mut(|input| input.consume_key(command, Key::T)) {
            self.state.new_shell();
            self.state
                .go(Pane::Terminal, crate::state::SidebarSpot::Task);
        }
        if ctx.input_mut(|input| input.consume_key(command, Key::S)) {
            self.state.save_buffer();
        }
        if ctx.input_mut(|input| input.consume_key(command_shift, Key::E)) {
            self.state
                .apply(&crate::state::PaletteAction::ExportTranscript);
        }
        if ctx.input_mut(|input| input.consume_key(command, Key::Comma)) {
            self.state.open_model_dialog();
        }
        if ctx.input_mut(|input| input.consume_key(command, Key::O)) {
            self.state.open_folder_dialog();
        }
        if ctx.input_mut(|input| input.consume_key(command, Key::W)) {
            self.state.editor.buffer = None;
        }
        for (key, pane) in [
            (Key::Num1, Pane::Conversation),
            (Key::Num2, Pane::Terminal),
            (Key::Num3, Pane::Editor),
            (Key::Num4, Pane::Git),
            (Key::Num5, Pane::Workflows),
            (Key::Num6, Pane::Observability),
        ] {
            if ctx.input_mut(|input| input.consume_key(command, key)) {
                self.state.go(pane, crate::state::SidebarSpot::Task);
            }
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
            if self.state.palette.is_some() {
                self.state.palette = None;
            } else if self.state.dialog.is_some() {
                self.state.dialog = None;
            } else {
                self.state.help_open = false;
            }
        }
    }

    /// Opens the palette in a mode, asking the keyboard for its field.
    ///
    /// @param mode what the palette should search
    fn open_palette(&mut self, mode: PaletteMode) {
        let mut palette = self
            .state
            .palette
            .take()
            .unwrap_or_else(|| Palette::new(mode));
        if palette.mode != mode {
            palette.mode = mode;
            palette.dirty = true;
            palette.selection = 0;
        }
        palette.dirty = true;
        self.state.palette = Some(palette);
        self.state.focus = Some(FocusRequest::Palette);
    }

    /// The key reference, drawn as a floating card.
    ///
    /// @param ctx the egui context
    fn help(&mut self, ctx: &egui::Context) {
        if !self.state.help_open {
            return;
        }
        let mut open = true;
        egui::Window::new("keys")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(egui::pos2(ctx.content_rect().center().x - 170.0, 120.0))
            .show(ctx, |ui| {
                for (keys, what) in [
                    ("⌘K", "command palette"),
                    ("⌘P", "go to file"),
                    ("⌘⇧F", "search file contents"),
                    (
                        "⌘1…6",
                        "conversation · terminal · editor · git · workflows · meter",
                    ),
                    ("⌘T", "new shell"),
                    ("⌘B", "file tree"),
                    ("⌘J", "session panel"),
                    ("⌘S", "save the open file"),
                    ("⌘,", "models & routes"),
                    ("⌘O", "open folder"),
                    ("⌘↩", "send the prompt"),
                    ("⌘⇧E", "export the transcript"),
                    ("esc", "close what is open"),
                ] {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [46.0, theme::ROW],
                            egui::Label::new(
                                RichText::new(keys).monospace().size(11.0).color(theme::DIM),
                            ),
                        );
                        ui.label(RichText::new(what).size(12.0).color(theme::TEXT));
                    });
                }
            });
        if !open {
            self.state.help_open = false;
        }
    }

    /// Takes the requested screenshot, once the interface has settled.
    ///
    /// The frame the renderer hands back is the window's own pixels rather than
    /// a copy of the screen, so a test sees exactly what a person would — with
    /// no window manager in the way, and no display needed to ask for it.
    ///
    /// @param ctx the egui context
    fn capture(&mut self, ctx: &egui::Context) {
        let Some(shot) = &self.shot else { return };
        if !self.asked {
            // Keep frames coming while the wait runs, and keep the window above
            // whatever is on screen: an occluded window paints nothing, so a
            // pending shot has to be visible to fire at all. Both of these are
            // only ever done when a shot was asked for on the command line, and
            // neither takes the keyboard: a capture must not swallow what the
            // person at the machine is typing.
            ctx.request_repaint_after(Duration::from_millis(50));
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::WindowLevel::AlwaysOnTop,
            ));
            if self.opened.elapsed() < shot.delay {
                return;
            }
            self.asked = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            ctx.request_repaint();
            return;
        }
        let frame = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let Some(frame) = frame else { return };
        match write_ppm(&shot.path, &frame) {
            Ok(()) => log::info!("screenshot: wrote {}", shot.path.display()),
            Err(err) => log::error!("screenshot: {}", err),
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// Writes an image as a binary PPM.
///
/// PPM needs no encoder and no dependency: three bytes a pixel behind a short
/// header is enough of a file for a test to read, and every image tool on the
/// planet can convert it.
///
/// @param path where to write
/// @param image the frame
/// @returns whether it was written
fn write_ppm(path: &std::path::Path, image: &egui::ColorImage) -> std::io::Result<()> {
    let [width, height] = image.size;
    let mut bytes = Vec::with_capacity(width * height * 3 + 32);
    bytes.extend_from_slice(format!("P6\n{width} {height}\n255\n").as_bytes());
    for pixel in &image.pixels {
        let [red, green, blue, _] = pixel.to_array();
        bytes.extend_from_slice(&[red, green, blue]);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)
}

impl eframe::App for HarnessApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.logic();
        // A window opened from a terminal can come up behind it, and a person
        // who just launched the application expects to see it: for the first
        // moments of a normal launch, keep asking to be front. A capture never
        // asks — `--shot` must not take the keyboard from whoever is driving.
        if !self.fronted && self.shot.is_none() {
            let focused = ctx.input(|input| input.viewport().focused) == Some(true);
            if focused || self.opened.elapsed() >= Duration::from_millis(1_500) {
                self.fronted = true;
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        // A pending shot has to keep the event loop awake. While the window is
        // hidden eframe runs no egui pass at all, and a timer alone is not enough
        // to bring it back: only a repaint request is. Nothing else here runs
        // logic this hot, because nothing else is on a deadline.
        if self.shot.is_some() {
            ctx.request_repaint();
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.keys(&ctx);

        // The window names the folder it is working in, so a folder switch has to
        // rename it — otherwise the title bar disagrees with the footer.
        if self.titled != self.state.root {
            self.titled = self.state.root.clone();
            let title = format!("AI Harness — {}", self.titled.display());
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }

        egui::Panel::left("harness-sidebar")
            .exact_size(theme::SIDEBAR)
            .frame(sidebar_frame())
            .show(ui, |ui| shell::sidebar(&mut self.state, ui));

        // The tree is a column of the workspace rather than part of the chrome,
        // so it appears only when it is asked for and never moves the cards.
        if self.state.tree_open {
            egui::Panel::left("harness-tree")
                .default_size(246.0)
                .size_range(160.0..=460.0)
                .frame(tree_frame())
                .show(ui, |ui| panes::tree::show(&mut self.state, ui));
        }

        egui::CentralPanel::default()
            .frame(workspace_frame())
            .show(ui, |ui| shell::workspace(&mut self.state, ui));

        palette::show(&mut self.state, &ctx);
        dialogs::show(&mut self.state, &ctx);
        self.help(&ctx);

        self.capture(&ctx);

        // The interfaces that can change without a click — a shell's output, the
        // agent's stream — are the reason for a repaint at all; the cadence is
        // chosen so an idle harness costs nothing visible.
        let busy = self.state.agent.conversation.state.busy()
            || self.state.shells.iter().any(|shell| {
                shell
                    .terminal
                    .as_ref()
                    .is_some_and(harness_core::term::Terminal::blocks_running)
            });
        if busy {
            ctx.request_repaint_after(Duration::from_millis(16));
        } else if self.state.current_toast().is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        } else {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
}

/// The sidebar's frame: flat chrome, no divider — the cards carry the edges.
fn sidebar_frame() -> Frame {
    Frame::new()
        .fill(theme::BG)
        .inner_margin(egui::Margin::ZERO)
}

/// The file tree's frame: a raised panel between the chrome and the cards.
fn tree_frame() -> Frame {
    Frame::new()
        .fill(theme::PANEL)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
}

/// The workspace's frame: nothing but the gray the cards float on.
fn workspace_frame() -> Frame {
    Frame::new()
        .fill(theme::BG)
        .inner_margin(egui::Margin::ZERO)
}

/// Shortens a string for a fixed-width slot, keeping its ends.
///
/// @param text the text
/// @param limit the most characters to show
/// @returns the text, elided in the middle when it is too long
pub fn shorten(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.to_string();
    }
    let keep = limit.saturating_sub(1) / 2;
    let head: String = text.chars().take(keep).collect();
    let tail: String = text.chars().skip(count - keep).collect();
    format!("{head}…{tail}")
}

/// A token count in the shortest honest form.
///
/// @param tokens the count
/// @returns e.g. `12.4k`
pub fn compact(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 1_000_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        format!("{:.2}M", tokens as f64 / 1_000_000.0)
    }
}
