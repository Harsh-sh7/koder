//! Live proof of the terminal: a real PTY, a real shell, real blocks.
//!
//! The unit tests in `term.rs` drive the reducer with synthetic markers. These
//! tests run an actual shell in an actual pty, because that is where the parts
//! that can be wrong on a real machine live: whether the pty starts, whether the
//! reader sees the bytes, and whether our shell integration really emits the
//! markers the block model is built on.
//!
//! `cargo test -p harness-core --test terminal_smoke` runs the deterministic
//! `/bin/sh` case. The interactive-shell case is opt-in:
//! `cargo test -p harness-core --test terminal_smoke -- --ignored --nocapture`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use harness_core::event::CoreEvent;
use harness_core::term::{write_shell_integration, ScreenSize, Terminal, TerminalOptions};

/// How long to wait for the shell to say something.
const WAIT: Duration = Duration::from_secs(20);

/// Spawns a terminal and waits until `check` is satisfied.
///
/// @param options how to start the shell
/// @param check called repeatedly with the terminal; returns true when done
/// @returns the terminal, and the events seen while waiting
/// @throws `Err` when the wait runs out or the shell dies first
fn spawn_and_wait(
    options: TerminalOptions,
    mut check: impl FnMut(&Terminal) -> bool,
) -> Result<(Terminal, Vec<CoreEvent>), String> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let terminal = Terminal::spawn(harness_core::event::TerminalId(1), options, tx)?;
    let mut seen = Vec::new();
    let deadline = Instant::now() + WAIT;
    loop {
        while let Ok(event) = rx.try_recv() {
            seen.push(event);
        }
        if check(&terminal) {
            return Ok((terminal, seen));
        }
        if !terminal.is_alive() {
            return Err(format!(
                "the shell exited before the check passed; screen was:\n{}",
                terminal.screen().text()
            ));
        }
        if Instant::now() > deadline {
            return Err(format!(
                "timed out waiting on the shell; screen was:\n{}",
                terminal.screen().text()
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// A shell that only prints a scripted stream, so the test is about the pty and
/// the marker scanner rather than about interactive shell behaviour.
#[test]
fn a_real_pty_turns_a_scripted_stream_into_a_block() {
    let script = r"printf '\033]133;A\007$ '; printf '\033]633;E;echo hello from the pty\007'; printf '\033]133;C\007'; printf '\033[32mhello from the pty\033[0m\n'; printf '\033]133;D;3\007'";
    let options = TerminalOptions {
        shell: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_string(), script.to_string()],
        cwd: std::env::temp_dir(),
        size: ScreenSize { cols: 80, rows: 24 },
        env: Vec::new(),
        scrollback: 1_000,
    };
    let (terminal, events) = spawn_and_wait(options, |terminal| {
        terminal.block_count() == 1 && !terminal.blocks_running()
    })
    .expect("the scripted shell produced a finished block");

    assert_eq!(terminal.block_count(), 1, "one command, one block");
    let blocks = terminal.recent_blocks(4);
    let block = &blocks[0];
    assert_eq!(block.command, "echo hello from the pty");
    assert_eq!(
        block.exit_code,
        Some(3),
        "the reported status is the block's"
    );
    assert_eq!(
        block.output.trim_end(),
        "hello from the pty",
        "output only, no prompt or markers"
    );

    let screen = terminal.screen().text();
    assert!(
        screen.contains("$"),
        "the live screen still shows the shell's prompt"
    );
    assert!(screen.contains("hello from the pty"));

    assert!(
        events
            .iter()
            .any(|event| matches!(event, CoreEvent::BlockCompleted { exit_code: 3, .. })),
        "the app is told when a command finishes: {events:?}"
    );
}

/// The interactive case: the user's own shell, our integration, a command
/// submitted the way the pane submits one.
#[test]
#[ignore = "starts an interactive login shell; run with --ignored"]
fn the_shell_integration_drives_blocks_interactively() {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let name = PathBuf::from(&shell)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join("harness-terminal-smoke");
    let integration = write_shell_integration(&dir, &name)
        .expect("integration writes")
        .unwrap_or_else(|| {
            eprintln!("{name} has no integration; nothing to prove here");
            std::process::exit(0);
        });
    eprintln!(
        "integration for {name}: {} {integration:?}",
        integration.script.display()
    );

    let mut env = integration.env;
    env.push(("PS1".to_string(), "$ ".to_string()));
    let options = TerminalOptions {
        shell: PathBuf::from(&shell),
        args: integration.args,
        cwd: std::env::temp_dir(),
        size: ScreenSize {
            cols: 100,
            rows: 30,
        },
        env,
        scrollback: 2_000,
    };

    // The prompt must arrive before the command is typed, or the shell would
    // swallow it while still starting up.
    let (terminal, _) = spawn_and_wait(options, |terminal| terminal.at_prompt())
        .expect("the shell reached its prompt");
    assert!(terminal.is_integrated(), "the shell emitted our markers");

    terminal.run("echo block-proof");
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if terminal.block_count() >= 1 && !terminal.blocks_running() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let blocks = terminal.recent_blocks(4);
    assert!(
        !blocks.is_empty(),
        "typing a command in an interactive shell produces a block"
    );
    eprintln!("blocks: {blocks:#?}");
    let block = blocks
        .iter()
        .find(|block| block.command.contains("echo block-proof"))
        .expect("the submitted command is a block");
    assert_eq!(block.exit_code, Some(0));
    assert!(
        block.output.contains("block-proof"),
        "the block captured the output"
    );
}
