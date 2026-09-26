//! AI Harness — native application entry point.
//!
//! Owns the window, the command line, and the headless self-test. Everything the
//! interface reads and writes lives in [`state`]; everything it draws lives in
//! [`app`] and [`panes`].
//!
//! Two ways in:
//!
//! - no arguments: the harness opens a window on the current directory;
//! - `--check`: it verifies the pieces a window would need — the workspace
//!   documents, the engine checkout, the shell integration, the file tree, the
//!   repository, and (with `--engine`) a live engine handshake — and exits with a
//!   status. That is how this app is tested without a display.

mod app;
mod code;
mod dialogs;
mod icons;
mod palette;
mod panes;
mod shell;
mod state;
mod theme;

use std::path::PathBuf;

use eframe::egui;

use crate::state::Pane;

/// Command-line options.
#[derive(Debug)]
struct Cli {
    /// The workspace to open.
    workspace: PathBuf,
    /// Harness home, when the caller named one.
    dsh_home: Option<PathBuf>,
    /// Run the headless self-test instead of opening a window.
    check: bool,
    /// Include the live engine handshake in the self-test.
    engine: bool,
    /// A prompt to open the harness on.
    ask: Option<String>,
    /// Write one frame here and exit, for tests that need to see the window.
    shot: Option<PathBuf>,
    /// How long to let the interface settle before that capture.
    shot_delay: f32,
    /// The view to open on, when the caller named one.
    pane: Option<Pane>,
    /// A dialog to open on, when the caller named one. Like `--pane`, this
    /// exists so a test can photograph a card without a person clicking to it.
    dialog: Option<String>,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            workspace: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            dsh_home: None,
            check: false,
            engine: false,
            ask: None,
            shot: None,
            shot_delay: 3.0,
            pane: None,
            dialog: None,
        }
    }
}

impl Cli {
    /// Parses the argument list.
    ///
    /// @returns the options, and any unknown argument that was ignored
    fn parse() -> (Self, Vec<String>) {
        let mut cli = Self::default();
        let mut ignored = Vec::new();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--workspace" | "-w" => match args.next() {
                    Some(value) => cli.workspace = PathBuf::from(value),
                    None => ignored.push("--workspace without a path".to_string()),
                },
                "--dsh-home" => match args.next() {
                    Some(value) => cli.dsh_home = Some(PathBuf::from(value)),
                    None => ignored.push("--dsh-home without a path".to_string()),
                },
                "--shot" => match args.next() {
                    Some(value) => cli.shot = Some(PathBuf::from(value)),
                    None => ignored.push("--shot without a path".to_string()),
                },
                "--shot-delay" => match args.next().map(|value| value.parse::<f32>()) {
                    Some(Ok(value)) => cli.shot_delay = value.max(0.0),
                    _ => ignored.push("--shot-delay without a number".to_string()),
                },
                "--pane" => match args.next().as_deref().and_then(Pane::from_name) {
                    Some(pane) => cli.pane = Some(pane),
                    None => ignored.push("--pane without a known view".to_string()),
                },
                "--dialog" => match args.next() {
                    Some(value) => cli.dialog = Some(value),
                    None => ignored.push("--dialog without a name".to_string()),
                },
                "--check" => cli.check = true,
                "--engine" => cli.engine = true,
                "--ask" => match args.next() {
                    Some(value) => cli.ask = Some(value),
                    None => ignored.push("--ask without a prompt".to_string()),
                },
                "--help" | "-h" => {
                    println!("{USAGE}");
                    std::process::exit(0);
                }
                other => ignored.push(other.to_string()),
            }
        }
        (cli, ignored)
    }
}

/// What `--help` prints.
const USAGE: &str = "\
AI Harness — a terminal-first coding harness with an agent that works in it.

usage: ai-harness [--workspace <dir>] [--dsh-home <dir>] [--ask <prompt>]
                  [--check [--engine]]
                  [--shot <file.ppm> [--shot-delay <seconds>]] [--pane <view>]
                  [--dialog <models|folder>]

  --workspace <dir>  open this directory instead of the current one
  --dsh-home <dir>   harness home holding the engine profile and its state
  --ask <prompt>     open on a task: the prompt is sent as soon as the engine
                     has a session, so no model is contacted until then
  --check            verify the workspace and exit, without opening a window
  --engine           with --check, also start the engine and open a session
  --shot <file>      draw for a moment, write the frame as a binary PPM, exit
  --pane <view>      open on a view: conversation, terminal, editor, git,
                     workflows, or observability (also term, code, flow, meter)
  --dialog <name>    open on a dialog: models, or folder

keys
  ⌘K / Ctrl+K        command palette          ⌘P / Ctrl+P    go to file
  ⌘⇧F / Ctrl+Shift+F search file contents    ⌘S / Ctrl+S    save the open file
  ⌘B / Ctrl+B        file tree                ⌘J / Ctrl+J    session panel
  ⌘1…⌘6 / Ctrl+1…6   switch view              ⌘T / Ctrl+T    new shell
  ⌘O / Ctrl+O        open a different folder  ⌘, / Ctrl+,    models & routes
";

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let (cli, ignored) = Cli::parse();
    for arg in &ignored {
        log::warn!("ignoring unknown argument {arg}");
    }

    // A workspace's `.env` is read before anything else, so the credential a
    // route names is present by the time the engine starts.
    let loaded = harness_core::config::load_dotenv(&cli.workspace);
    if !loaded.is_empty() {
        log::info!("env: {} variable(s) from .env", loaded.len());
    }
    if !cli.workspace.is_dir() {
        eprintln!("ai-harness: {} is not a directory", cli.workspace.display());
        std::process::exit(2);
    }

    if cli.check {
        std::process::exit(check::run(&cli));
    }

    let workspace = cli.workspace.clone();
    let title = format!("AI Harness — {}", workspace.display());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1480.0, 940.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title(title),
        ..Default::default()
    };
    let shot = cli.shot.clone().map(|path| app::Shot {
        path,
        delay: std::time::Duration::from_secs_f32(cli.shot_delay),
    });
    let pane = cli.pane;
    let dialog = cli.dialog.clone();
    eframe::run_native(
        "AI Harness",
        options,
        Box::new(move |cc| {
            let mut app = app::HarnessApp::new(
                cc,
                cli.workspace.clone(),
                cli.dsh_home.clone(),
                cli.ask.clone(),
                shot,
            );
            if let Some(pane) = pane {
                app.state.pane = pane;
            }
            match dialog.as_deref() {
                Some("models") => app.state.open_model_dialog(),
                Some("folder") => app.state.open_folder_dialog(),
                Some(other) => log::warn!("ignoring unknown dialog {other}"),
                None => {}
            }
            Ok(Box::new(app))
        }),
    )
}

/// The headless self-test.
///
/// A window is not something a test can look at, so this checks the layers the
/// window is made of: the documents the workspace owns, the engine checkout, the
/// shell integration, the file tree and its search, the repository, the saved
/// workflows, a live PTY, and — with `--engine` — a full ACP handshake.
mod check {
    use std::path::Path;

    use harness_core::config::{EnginePaths, HarnessConfig, ModelsDocument, Prices};
    use harness_core::fsops;
    use harness_core::git::GitRepo;
    use harness_core::term::{write_shell_integration, ScreenSize, Terminal, TerminalOptions};
    use harness_core::workflow::WorkflowStore;

    use crate::Cli;

    /// One check's outcome.
    struct Report {
        passed: usize,
        failed: usize,
        warned: usize,
    }

    impl Report {
        /// Records a pass.
        ///
        /// @param what what was checked
        /// @param detail what was found
        fn pass(&mut self, what: &str, detail: impl std::fmt::Display) {
            self.passed += 1;
            println!("  ok    {what}: {detail}");
        }

        /// Records a failure.
        ///
        /// @param what what was checked
        /// @param detail why it failed
        fn fail(&mut self, what: &str, detail: impl std::fmt::Display) {
            self.failed += 1;
            println!("  FAIL  {what}: {detail}");
        }

        /// Records something that is missing rather than broken.
        ///
        /// @param what what was checked
        /// @param detail what is missing
        fn warn(&mut self, what: &str, detail: impl std::fmt::Display) {
            self.warned += 1;
            println!("  --    {what}: {detail}");
        }
    }

    /// Runs every check.
    ///
    /// @param cli the parsed command line
    /// @returns the process exit status
    pub fn run(cli: &Cli) -> i32 {
        let root = &cli.workspace;
        println!(
            "AI Harness {} — checking {}",
            harness_core::VERSION,
            root.display()
        );
        let mut report = Report {
            passed: 0,
            failed: 0,
            warned: 0,
        };

        let config = stage_documents(root, &mut report);
        let paths = stage_engine(root, cli, &mut report);
        stage_workspace(root, &mut report);
        stage_shell(root, &mut report);
        if let Some(paths) = paths {
            stage_engine_session(&paths, root, &mut report, cli.engine);
        }

        // The settings document is written back so the check leaves the
        // workspace as the app would: a `.harness/` that is ready to run in.
        if config.save(root).is_ok() {
            report.pass(
                "config",
                format!("{} saved", HarnessConfig::path_for(root).display()),
            );
        }

        println!(
            "\n{} passed, {} failed, {} not available",
            report.passed, report.failed, report.warned
        );
        if report.failed == 0 {
            0
        } else {
            1
        }
    }

    /// Checks the workspace's own documents.
    ///
    /// @returns the settings, so the caller can write them back
    fn stage_documents(root: &Path, report: &mut Report) -> HarnessConfig {
        let config = HarnessConfig::load(root);
        report.pass(
            "config",
            format!(
                "font {:.1}pt, terminal {:.1}pt",
                config.font_size, config.terminal_font_size
            ),
        );

        let models_path = HarnessConfig::models_path_for(root);
        match ModelsDocument::load(&models_path) {
            Ok(models) => {
                let routes = models.routes();
                if routes.is_empty() {
                    report.warn(
                        "models",
                        format!(
                            "{} has no route; the agent panel stays empty until one is added",
                            models_path.display()
                        ),
                    );
                } else {
                    let active = models
                        .active
                        .as_ref()
                        .map(|route| format!("{}/{}", route.provider, route.model))
                        .unwrap_or_else(|| "none selected".to_string());
                    report.pass(
                        "models",
                        format!("{} route(s), active {active}", models.providers.len()),
                    );
                    // A credential's value never appears in a document, so the
                    // most a check can confirm is that the variable the active
                    // route names is present — the classic first failure.
                    let key_var = models
                        .active
                        .as_ref()
                        .and_then(|active| models.providers.get(&active.provider))
                        .and_then(|profile| profile.api_key_env.clone())
                        .unwrap_or_else(|| "AI_API_KEY".to_string());
                    match std::env::var(&key_var) {
                        Ok(value) if value.trim() == "mock-key" => report.warn(
                            "credential",
                            format!("{key_var} is still the probe placeholder (mock-key): add a real key in the model dialog, or in the workspace .env"),
                        ),
                        Ok(_) => report.pass("credential", format!("{key_var} is set")),
                        Err(_) => report.warn(
                            "credential",
                            format!("{key_var} is not set: paste a key in the model dialog, or add one to the workspace .env, or every turn will fail"),
                        ),
                    }
                }
            }
            Err(err) => report.fail("models", err),
        }

        match Prices::load(&HarnessConfig::prices_path_for(root)) {
            Ok(prices) => report.pass("prices", format!("{} model(s) priced", prices.models.len())),
            Err(err) => report.fail("prices", err),
        }

        let workflows = WorkflowStore::load(&HarnessConfig::workflows_dir_for(root));
        report.pass(
            "workflows",
            format!(
                "{} saved in {}",
                workflows.all().len(),
                workflows.dir().display()
            ),
        );
        config
    }

    /// Resolves the engine checkout.
    ///
    /// @returns the paths, when they resolved
    fn stage_engine(root: &Path, cli: &Cli, report: &mut Report) -> Option<EnginePaths> {
        match EnginePaths::resolve(root, cli.dsh_home.clone()) {
            Ok(paths) => {
                report.pass(
                    "engine",
                    format!(
                        "{} (node {})",
                        paths.launcher.display(),
                        paths.node.display()
                    ),
                );
                if paths.dsh_repo.is_dir() {
                    report.pass("engine checkout", paths.dsh_repo.display());
                } else {
                    report.warn(
                        "engine checkout",
                        format!("{} is missing; run make setup", paths.dsh_repo.display()),
                    );
                }
                Some(paths)
            }
            Err(err) => {
                report.warn("engine", err);
                None
            }
        }
    }

    /// Checks the file tree, a read, and a search.
    fn stage_workspace(root: &Path, report: &mut Report) {
        match fsops::list_dir(root, root, false) {
            Ok(listing) => report.pass(
                "tree",
                format!("{} entries in the workspace root", listing.entries.len()),
            ),
            Err(err) => report.fail("tree", err),
        }

        let files = fsops::list_files(root, 5_000, true);
        if files.is_empty() {
            report.warn("files", "no files found to search");
        } else {
            report.pass("files", format!("{} files walked", files.len()));
            let sample = &files[0];
            match fsops::read_text(root, &root.join(sample), fsops::TEXT_LOAD_LIMIT) {
                Ok(text) if text.binary => report.pass("read", format!("{sample} is binary")),
                Ok(text) => report.pass(
                    "read",
                    format!(
                        "{sample}: {} bytes{}",
                        text.bytes,
                        if text.truncated { " (truncated)" } else { "" }
                    ),
                ),
                Err(err) => report.fail("read", err),
            }
            let needle = "fn";
            let options = fsops::SearchOptions {
                max_results: 20,
                ..Default::default()
            };
            let outcome = fsops::search(root, needle, &options);
            match outcome.error {
                Some(err) => report.fail("search", err),
                None => report.pass(
                    "search",
                    format!(
                        "{needle:?}: {} match(es) in {} file(s)",
                        outcome.matches.len(),
                        outcome.files_searched
                    ),
                ),
            }
        }

        match GitRepo::discover(root) {
            Some(git) => match git.status() {
                Ok(status) => report.pass(
                    "git",
                    format!(
                        "{} on {} with {} change(s)",
                        git.root().display(),
                        status.branch,
                        status.change_count()
                    ),
                ),
                Err(err) => report.fail("git status", err),
            },
            None => report.warn("git", "the workspace is not in a repository"),
        }
    }

    /// Checks that the shell integration writes, and that a PTY runs a command.
    fn stage_shell(root: &Path, report: &mut Report) {
        let dir = root.join(".harness").join("shell");
        let shell_name = std::env::var("SHELL")
            .ok()
            .as_deref()
            .and_then(|shell| Path::new(shell).file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("sh")
            .to_string();
        let integration = match write_shell_integration(&dir, &shell_name) {
            Ok(Some(integration)) => {
                report.pass(
                    "shell integration",
                    format!("{} → {}", shell_name, integration.script.display()),
                );
                Some(integration)
            }
            Ok(None) => {
                report.warn(
                    "shell integration",
                    format!("no integration for {shell_name}; blocks stay empty"),
                );
                None
            }
            Err(err) => {
                report.fail("shell integration", err);
                None
            }
        };

        let shell = std::env::var("SHELL")
            .map(Into::into)
            .unwrap_or_else(|_| std::path::PathBuf::from("/bin/sh"));
        let (env, args) = match integration {
            Some(integration) => (integration.env, integration.args),
            None => (Vec::new(), Vec::new()),
        };
        let options = TerminalOptions {
            shell,
            args,
            cwd: root.to_path_buf(),
            size: ScreenSize { cols: 80, rows: 24 },
            env,
            scrollback: 1_000,
        };
        let (sink, events) = crossbeam_channel::unbounded();
        match Terminal::spawn(harness_core::event::TerminalId(1), options, sink) {
            Ok(terminal) => {
                terminal.run("echo harness-check");
                let found = wait_for(
                    &terminal,
                    &events,
                    "harness-check",
                    std::time::Duration::from_secs(10),
                );
                match found {
                    Ok(()) => report.pass("pty", "a shell started and echoed a command back"),
                    Err(err) => report.fail("pty", err),
                }
                // A block closes when the shell paints its next prompt, which is
                // a moment after the command's output reaches the screen. Give
                // it that moment rather than reporting the race as a lost marker.
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while terminal.block_count() == 0 && std::time::Instant::now() < deadline {
                    let _ = events.recv_timeout(std::time::Duration::from_millis(50));
                }
                let blocks = terminal.recent_blocks(4);
                if blocks.is_empty() {
                    report.warn("blocks", "the shell produced no block markers");
                } else {
                    report.pass(
                        "blocks",
                        format!("{} block(s), last {}", blocks.len(), blocks[0].command),
                    );
                }
            }
            Err(err) => report.fail("pty", err),
        }
    }

    /// Waits for text to appear on a terminal's screen.
    ///
    /// @param terminal the terminal to watch
    /// @param events its event stream, used to wake up rather than spin
    /// @param needle the text expected
    /// @param timeout how long to wait
    /// @returns `Ok` when the text appeared
    fn wait_for(
        terminal: &Terminal,
        events: &crossbeam_channel::Receiver<harness_core::event::CoreEvent>,
        needle: &str,
        timeout: std::time::Duration,
    ) -> Result<(), String> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if terminal.screen().text().contains(needle) {
                return Ok(());
            }
            if std::time::Instant::now() > deadline {
                return Err(format!("{needle:?} did not appear within {timeout:?}"));
            }
            let _ = events.recv_timeout(std::time::Duration::from_millis(50));
        }
    }

    /// Starts the engine and opens a session, when asked to.
    ///
    /// @param paths the engine layout
    /// @param root the workspace
    /// @param report where to record the outcome
    /// @param live whether to actually start it
    fn stage_engine_session(paths: &EnginePaths, root: &Path, report: &mut Report, live: bool) {
        if !live {
            report.warn("engine session", "skipped; pass --engine to start it");
            return;
        }
        let (sender, events) = std::sync::mpsc::channel();
        let client = match harness_core::acp::AcpClient::spawn(paths, sender, root) {
            Ok(client) => client,
            Err(err) => {
                report.fail("engine start", err);
                return;
            }
        };
        client.initialize();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        let mut session: Option<String> = None;
        let mut opened_at: Option<std::time::Instant> = None;
        let mut models = Vec::new();
        while std::time::Instant::now() < deadline {
            match events.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(harness_core::acp::AgentEvent::Initialized { name, version }) => {
                    report.pass("engine handshake", format!("{name} {version}"));
                    client.new_session(&root.to_string_lossy());
                }
                // The model list rides along with the session; asking the engine
                // for it again would only be a second round trip.
                Ok(harness_core::acp::AgentEvent::SessionReady {
                    session_id,
                    config_options,
                }) => {
                    session = Some(session_id);
                    models = model_labels(&config_options);
                    opened_at = Some(std::time::Instant::now());
                }
                Ok(harness_core::acp::AgentEvent::ConfigOptions { config_options, .. }) => {
                    models = model_labels(&config_options);
                }
                Ok(harness_core::acp::AgentEvent::RequestFailed { method, message }) => {
                    report.fail("engine request", format!("{method}: {message}"));
                }
                Ok(harness_core::acp::AgentEvent::EngineExited { code }) => {
                    report.fail("engine", format!("exited with {code:?}"));
                    return;
                }
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            // A session that opened is the result. Its model list is a bonus, so
            // an engine with no model option is not worth waiting ninety seconds
            // for: a short grace settles whether one is coming.
            let settled =
                opened_at.is_some_and(|at| at.elapsed() > std::time::Duration::from_secs(2));
            if session.is_some() && (!models.is_empty() || settled) {
                break;
            }
        }
        match session {
            Some(_) if models.is_empty() => report.pass(
                "engine session",
                "opened; the engine advertises no model option",
            ),
            Some(_) => report.pass(
                "engine session",
                format!("opened with {} model choice(s)", models.len()),
            ),
            None => report.fail("engine session", "no session was opened within 90s"),
        }
        for model in models.iter().take(8) {
            println!("        · {model}");
        }
    }

    /// The labels of the model choices an engine advertised.
    ///
    /// @param options the session's configuration options
    /// @returns the labels, in the engine's order; empty when it offers no model
    fn model_labels(options: &[harness_core::acp::ConfigOption]) -> Vec<String> {
        options
            .iter()
            .find(|option| option.id == "model")
            .map(|option| {
                option
                    .choices
                    .iter()
                    .map(|choice| choice.label.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}
